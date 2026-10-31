//! 本机、单对端命名管道；正常输入直接等待内核事件，Runtime 共用至多一个取消回收线程。
//! 默认入口拒绝 DLL；显式驻留入口锁定模块至宿主退出，关闭排空不宣称允许热卸载。

mod io;
#[cfg(test)]
mod sandbox_tests;
mod security;
#[cfg(test)]
mod tests;

pub use io::Cancellation;
pub use io::Pending;
pub use security::grant_app_container_query;

use self::{
    io::{Io, Kind},
    security::{CLIENT_ACCESS, Identity, open_server_process, verify_peer_image},
};
use crate::{ClientState, Error, FrameAssembler, Result, decode};
use std::{
    io as std_io,
    os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
    path::Path,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tigertype_contracts::protocol::{Request, Response};
use windows_sys::Win32::{
    Foundation::{INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
        PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    },
    System::{
        Pipes::{
            CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_TYPE_BYTE, PIPE_WAIT, PeekNamedPipe,
        },
        Threading::{
            CloseThreadpool, CreateEventW, CreateThreadpool, PTP_POOL, SetEvent,
            SetThreadpoolThreadMaximum, WaitForSingleObject,
        },
    },
};

const MAX_PIPES: usize = 64;
const CHUNK: usize = 16 * 1024;
const CLOSING: usize = 1 << (usize::BITS - 1);

struct Pool {
    pool: PTP_POOL,
    identity: Identity,
    // 关闭位和数量用同一次 CAS 准入，输入路径不等待生命周期互斥锁。
    lifecycle: AtomicUsize,
    drained: OwnedHandle,
    wake: OwnedHandle,
}
impl Drop for Pool {
    fn drop(&mut self) {
        // SAFETY: 所有取消等待均已释放；Windows 在最后回调退出后回收线程池。
        unsafe {
            CloseThreadpool(self.pool);
        }
    }
}

struct Slot(Arc<Pool>);
impl Drop for Slot {
    fn drop(&mut self) {
        let previous = self.0.lifecycle.fetch_sub(1, Ordering::AcqRel);
        if previous == (CLOSING | 1) {
            // SAFETY: 只有关闭后的最后一个持有者通知；手动事件不重置，不会漏唤醒重复等候者。
            unsafe {
                SetEvent(self.0.drained.as_raw_handle());
            }
        }
    }
}

pub struct Runtime(Arc<Pool>);
impl Runtime {
    /// 每个产品进程在启动阶段创建一次，不能放在逐键或逐连接路径中。
    pub fn new() -> Result<Self> {
        io::require_executable()?;
        Self::create()
    }

    /// 仅在 DLL 激活阶段调用：明确接受模块直到宿主进程退出都不可卸载。
    /// 不允许从 DllMain 或逐键回调调用；shutdown 只排空资源，不解除系统 PIN。
    pub fn new_process_resident() -> Result<Self> {
        io::pin_callback_module()?;
        Self::create()
    }

    fn create() -> Result<Self> {
        let identity = Identity::current()?;
        // SAFETY: 非继承、未命名的手动复位事件，只有最后一个关闭引用释放时才置位。
        let drained = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if drained.is_null() {
            return Err(std_io::Error::last_os_error().into());
        }
        // SAFETY: 成功创建的事件由唯一 RAII 所有者回收，覆盖线程池创建失败路径。
        let drained = unsafe { OwnedHandle::from_raw_handle(drained) };
        // SAFETY: 自动复位事件合并取消尾部回收通知，反应器据此补建失效监听。
        let wake = unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) };
        if wake.is_null() {
            return Err(std_io::Error::last_os_error().into());
        }
        // SAFETY: 成功创建的事件由 Runtime 唯一持有。
        let wake = unsafe { OwnedHandle::from_raw_handle(wake) };
        // SAFETY: 无保留参数；池由 Pool 的 Drop 独占回收。
        let pool = unsafe { CreateThreadpool(ptr::null()) };
        if pool == 0 {
            return Err(std_io::Error::last_os_error().into());
        }
        // SAFETY: 固定最多一个取消回收线程，回调只释放已完成操作，不阻塞或调用引擎。
        unsafe {
            SetThreadpoolThreadMaximum(pool, 1);
        }
        Ok(Self(Arc::new(Pool {
            pool,
            identity,
            lifecycle: AtomicUsize::new(0),
            drained,
            wake,
        })))
    }

    fn slot(&self) -> Result<Slot> {
        self.0
            .lifecycle
            .try_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                (state < MAX_PIPES).then_some(state + 1)
            })
            .map_err(|state| {
                if state & CLOSING != 0 {
                    Error::Closed
                } else {
                    Error::Busy
                }
            })?;
        Ok(Slot(self.0.clone()))
    }
    /// 取消尾部回收事件；固定反应器另等各 Pipe 的内核事件和设置事件，最多 18 个等待对象。
    pub fn completion_event(&self) -> BorrowedHandle<'_> {
        self.0.wake.as_handle()
    }

    /// 拒绝后续连接，等待所有管道/取消令牌/在途操作释放；仅用于显式停止，不用于输入回调。
    /// 调用者先取消并释放持有者。超时仍保持关闭状态，可继续等候；不会强杀或假装线程已退出。
    pub fn shutdown(&self, deadline: Instant) -> Result<()> {
        let previous = self.0.lifecycle.fetch_or(CLOSING, Ordering::AcqRel);
        if previous & !CLOSING != 0 {
            let budget = remaining(deadline)?;
            let millis = budget
                .as_nanos()
                .div_ceil(1_000_000)
                .min((u32::MAX - 1) as u128) as u32;
            // SAFETY: Runtime 持有事件直到等候返回；仅停机路径等待，管道取消独立唤醒它。
            match unsafe { WaitForSingleObject(self.0.drained.as_raw_handle(), millis) } {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => return Err(Error::TimedOut),
                _ => return Err(std_io::Error::last_os_error().into()),
            }
        }
        // 成功只代表资源所有权已排空；取消回调仍可能处于返回尾部，由 EXE 或 DLL PIN 保护。
        Ok(())
    }

    pub fn bind(&self, label: &str) -> Result<Listener> {
        let name = self.0.identity.name(label)?;
        let descriptor = self.0.identity.descriptor()?;
        let slot = self.slot()?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        // SAFETY: 名称、ACL 在调用期间有效；首实例和远程拒绝均由内核强制执行。
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                CHUNK as u32,
                CHUNK as u32,
                0,
                &attributes,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std_io::Error::last_os_error().into());
        }
        // SAFETY: 新建管道句柄的唯一所有权转交给 RAII。
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        Ok(Listener(Pipe {
            io: Io::new(handle, slot)?,
        }))
    }

    /// 宿主发现后台：只尝试已创建的本机实例，忙/不存在立即失败，不在输入路径启动进程或轮询。
    /// 核对管道所有者、服务端会话及显式预期映像，不读取后台令牌或发现文件。
    pub fn connect(&self, label: &str, image: &Path, deadline: Instant) -> Result<Pipe> {
        remaining(deadline)?;
        let name = self.0.identity.name(label)?;
        let slot = self.slot()?;
        // SAFETY: 受限本机名称；IDENTIFICATION 防止服务端获得客户端的可模拟令牌。
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                CLIENT_ACCESS,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std_io::Error::last_os_error().into());
        }
        // SAFETY: CreateFileW 成功的句柄由 OwnedHandle 负责所有失败路径回收。
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        self.0
            .identity
            .verify_server(handle.as_raw_handle(), image)?;
        remaining(deadline)?;
        let pipe = Pipe {
            io: Io::new(handle, slot)?,
        };
        pipe.check_open(deadline)?;
        Ok(pipe)
    }
}

pub struct Listener(Pipe);
impl Listener {
    /// 挂起可取消的内核监听，不为连接创建线程。
    pub fn start_accept(mut self) -> Result<(Pipe, Pending)> {
        let pending = self.0.submit(Kind::Connect, Vec::new())?;
        Ok((self.0, pending))
    }
    pub fn cancellation(&self) -> Cancellation {
        self.0.cancellation()
    }

    pub fn accept(mut self, deadline: Instant) -> Result<Pipe> {
        self.0.operate(Kind::Connect, Vec::new(), deadline)?;
        self.0.verify_client()?;
        self.0.check_open(deadline)?;
        Ok(self.0)
    }
}

pub struct Pipe {
    io: Arc<Io>,
}
impl Drop for Pipe {
    fn drop(&mut self) {
        self.io.cancel();
    }
}

impl Pipe {
    /// 每连接的内核完成事件；反应器直接等待它，不必等完成线程先被调度。
    pub fn completion_event(&self) -> BorrowedHandle<'_> {
        self.io.ready.as_handle()
    }
    /// 服务端接纳：只核对同会话；不打开宿主进程，宿主断开由常驻读失败发现。
    pub fn verify_client(&self) -> Result<()> {
        self.io
            ._slot
            .0
            .identity
            .verify_client(self.io.handle.as_raw_handle())
    }
    /// 仅供停止命令：受限查询打开调用进程并核对映像，普通宿主请求不走此路径。
    pub fn verify_client_image(&self, image: &Path) -> Result<()> {
        verify_peer_image(self.io.handle.as_raw_handle(), false, image)
    }
    /// 仅供停止命令：停机前取得服务端进程的等待句柄；打开后管道仍连通才说明句柄属于对端。
    /// 后台在本进程时为 None。
    pub fn server_process(&self) -> Result<Option<OwnedHandle>> {
        let process = open_server_process(self.io.handle.as_raw_handle())?;
        self.available()?;
        Ok(process)
    }
    /// 非阻塞查询可读字节数；失败表示已断开，宿主据此决定是否在同一期限内重连。
    pub fn available(&self) -> Result<u32> {
        let mut available = 0;
        // SAFETY: 只查询连接状态与字节数，不读取或消费数据。
        if unsafe {
            PeekNamedPipe(
                self.io.handle.as_raw_handle(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                &mut available,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(std_io::Error::last_os_error().into());
        }
        Ok(available)
    }
    /// 服务端立即断开对端，使被回收宿主发送前就能发现；随后释放仍由 Drop 取消在途操作。
    pub fn disconnect(&self) {
        self.io.cancel();
        // SAFETY: Arc 保持句柄存活；对未连接实例失败无副作用，结果由后续 I/O 错误体现。
        unsafe {
            DisconnectNamedPipe(self.io.handle.as_raw_handle());
        }
    }
    fn check_open(&self, deadline: Instant) -> Result<()> {
        if self.io.closed.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        remaining(deadline)?;
        Ok(())
    }

    pub fn cancellation(&self) -> Cancellation {
        Cancellation(self.io.clone())
    }

    pub fn send_frame(&mut self, frame: &[u8], deadline: Instant) -> Result<()> {
        let result = (|| {
            remaining(deadline)?;
            decode(frame)?;
            for chunk in frame.chunks(CHUNK) {
                let mut offset = 0;
                while offset < chunk.len() {
                    let (_, count) =
                        self.operate(Kind::Write, chunk[offset..].to_vec(), deadline)?;
                    if count == 0 {
                        return Err(std_io::Error::from(std_io::ErrorKind::WriteZero).into());
                    }
                    offset += count;
                }
            }
            self.check_open(deadline)?;
            Ok(())
        })();
        if result.is_err() {
            self.io.cancel();
        }
        result
    }

    pub fn receive_frame(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        let mut frame = FrameAssembler::default();
        let result = (|| loop {
            let (bytes, count) = self.read(deadline)?;
            if let Some(bytes) = frame.push(&bytes[..count])? {
                decode(&bytes)?;
                self.check_open(deadline)?;
                return Ok(bytes);
            }
        })();
        if result.is_err() {
            self.io.cancel();
        }
        result
    }

    /// 仅供常驻服务端空闲接收，不能用于输入回调；首段之后组帧/处理/发送共用返回的期限。
    /// 空闲只保留一个可取消的读取，断连或 Cancellation 会唤醒；半帧不能无限占用连接。
    pub fn receive_frame_idle(&mut self, frame_budget: Duration) -> Result<(Vec<u8>, Instant)> {
        if frame_budget.is_zero() || frame_budget > Duration::from_secs(5) {
            return Err(Error::Invalid("服务端组帧预算必须大于零且不超过五秒"));
        }
        let result = (|| {
            let (mut bytes, mut count) = self.idle_read(CHUNK)?;
            let deadline = Instant::now() + frame_budget;
            let mut frame = FrameAssembler::default();
            loop {
                if count == 0 {
                    return Err(std_io::Error::from(std_io::ErrorKind::UnexpectedEof).into());
                }
                if let Some(bytes) = frame.push(&bytes[..count])? {
                    decode(&bytes)?;
                    self.check_open(deadline)?;
                    return Ok((bytes, deadline));
                }
                (bytes, count) = self.read(deadline)?;
            }
        })();
        if result.is_err() {
            self.io.cancel();
        }
        result
    }

    pub fn transact(
        &mut self,
        client: &mut ClientState,
        request: Request,
        deadline: Instant,
    ) -> Result<Response> {
        let frame = match client.begin(request, deadline) {
            Ok(frame) => frame,
            Err(error) => {
                if matches!(error, Error::TimedOut | Error::Closed) {
                    self.io.cancel();
                }
                return Err(error);
            }
        };
        let result = (|| {
            self.send_frame(&frame, deadline)?;
            loop {
                let (bytes, count) = self.read(deadline)?;
                if let Some(response) = client.receive(&bytes[..count])? {
                    // 取消若发生在内核完成与协议解码之间，仍不能交付该提交。
                    self.check_open(deadline)?;
                    return Ok(response);
                }
            }
        })();
        if result.is_err() {
            client.cancel();
            self.io.cancel();
        }
        result
    }

    fn read(&mut self, deadline: Instant) -> Result<(Vec<u8>, usize)> {
        let result = self.operate(Kind::Read, vec![0; CHUNK], deadline)?;
        if result.1 == 0 {
            return Err(std_io::Error::from(std_io::ErrorKind::UnexpectedEof).into());
        }
        Ok(result)
    }
}

/// 等进程退出至绝对期限；内核等待，不忙轮询。
pub(crate) fn wait_exit(process: &OwnedHandle, deadline: Instant) -> Result<()> {
    let millis = remaining(deadline)?
        .as_millis()
        .min(u128::from(u32::MAX - 1)) as u32;
    // SAFETY: 句柄由调用方持有且带 SYNCHRONIZE；有限等待。
    match unsafe { WaitForSingleObject(process.as_raw_handle(), millis) } {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err(Error::TimedOut),
        _ => Err(std_io::Error::last_os_error().into()),
    }
}

fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or(Error::TimedOut)
}
