//! 输入直接等待 OVERLAPPED 的内核事件；只有取消后仍在途的操作交线程池回收。
//! 地址与缓冲区由 Arc 固定，超时不释放内核仍在使用的内存，不用完成线程中转正常输入。
//! 每连接的事件为自动复位，同一时刻只能有一个等待者：提交者/反应器只等未丢弃的操作，
//! 丢弃后只由回收等待等；反应器必须先把连接移出等待数组再丢弃其 Pending，否则信号被吞、操作永不回收。

use super::{Pipe, Slot, remaining};
use crate::{Error, Result};
#[cfg(test)]
use std::sync::mpsc::SyncSender;
use std::{
    cell::UnsafeCell,
    ffi::c_void,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Storage::FileSystem::{ReadFile, WriteFile},
    System::{
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN,
            GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleHandleExW, GetModuleHandleW,
        },
        Pipes::ConnectNamedPipe,
        Threading::{
            CloseThreadpoolWait, CreateEventW, CreateThreadpoolWait, INFINITE,
            PTP_CALLBACK_INSTANCE, PTP_WAIT, ResetEvent, SetEvent, SetThreadpoolWait,
            TP_CALLBACK_ENVIRON_V3, TP_CALLBACK_PRIORITY_NORMAL, WaitForSingleObject,
        },
    },
};

pub(super) fn require_executable() -> Result<()> {
    let mut module = ptr::null_mut();
    // SAFETY: FROM_ADDRESS 将参数视为回调地址，不解引用为字符串；仅借用已加载模块句柄。
    if unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            completed as *const () as *const u16,
            &mut module,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    // 默认入口继续拒绝 DLL；DLL 必须显式选择不可热卸载的进程期驻留策略。
    // SAFETY: NULL 查询进程主模块；EXE 在进程退出前不会卸载，取消回调可以安全收尾。
    if module != unsafe { GetModuleHandleW(ptr::null()) } {
        return Err(Error::Invalid(
            "管道完成线程目前只允许在 EXE 中运行，尚不支持可卸载 TSF DLL",
        ));
    }
    Ok(())
}

pub(super) fn pin_callback_module() -> Result<()> {
    let mut module = ptr::null_mut();
    // SAFETY: FROM_ADDRESS 使用实际完成回调地址定位本模块；PIN 让取消回调的尾部也保持可执行。
    // PIN 由系统保持至宿主退出，不能用 FreeLibrary 抵消；本函数不得在 DllMain/逐键路径调用。
    if unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
            completed as *const () as *const u16,
            &mut module,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

pub(super) struct Io {
    pub handle: OwnedHandle,
    pub closed: AtomicBool,
    in_flight: AtomicBool,
    /// 每连接一个自动复位事件，由内核完成直接置位；每次只有一个在途操作。
    pub(super) ready: OwnedHandle,
    /// 建连时预建一次、正常完成不启用；取消后仍在途才启用，逐次读写不再创建等待对象。
    cleanup: PTP_WAIT,
    /// 取消后内核仍在使用的操作；回调确认内核完成才取出释放。取消是永久的，至多登记一次。
    orphan: Mutex<Option<Arc<Operation>>>,
    pub(super) _slot: Slot,
    #[cfg(test)]
    pub on_drop: Option<SyncSender<()>>,
}
impl Io {
    pub fn new(handle: OwnedHandle, slot: Slot) -> Result<Arc<Self>> {
        // SAFETY: 非继承、未命名的自动复位事件，随最后一个操作释放。
        let ready = unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) };
        if ready.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: 新句柄的唯一所有权交给 RAII。
        let ready = unsafe { OwnedHandle::from_raw_handle(ready) };
        let environment = TP_CALLBACK_ENVIRON_V3 {
            Version: 3,
            Pool: slot.0.pool,
            CallbackPriority: TP_CALLBACK_PRIORITY_NORMAL,
            Size: size_of::<TP_CALLBACK_ENVIRON_V3>() as u32,
            ..Default::default()
        };
        let io = Arc::new_cyclic(|this: &std::sync::Weak<Self>| Self {
            // SAFETY: 上下文是本 Io 的稳定地址；等待只在登记 orphan（其持有 Arc<Io>）后启用，回调时必然存活。
            cleanup: unsafe {
                CreateThreadpoolWait(
                    Some(completed),
                    this.as_ptr().cast_mut().cast(),
                    &environment,
                )
            },
            handle,
            ready,
            orphan: Mutex::new(None),
            _slot: slot,
            closed: AtomicBool::new(false),
            in_flight: AtomicBool::new(false),
            #[cfg(test)]
            on_drop: None,
        });
        if io.cleanup == 0 {
            // 构造 Arc 只分配内存、不调用系统 API，最近错误仍是创建等待的结果。
            return Err(io::Error::last_os_error().into());
        }
        Ok(io)
    }
    pub fn cancel(&self) {
        self.closed.store(true, Ordering::Release);
        // SAFETY: Arc 保持句柄存活；仅请求取消，输入线程不等待取消回调。
        unsafe {
            CancelIoEx(self.handle.as_raw_handle(), ptr::null());
        }
    }
}
impl Drop for Io {
    fn drop(&mut self) {
        if self.cleanup != 0 {
            // SAFETY: 等待启用期间 orphan 持有本 Io，最后释放时等待已触发或从未启用；
            // 在本等待自己的回调中关闭时，Windows 待回调返回后才异步回收。
            unsafe {
                CloseThreadpoolWait(self.cleanup);
            }
        }
        #[cfg(test)]
        if let Some(sender) = &self.on_drop {
            let _ = sender.try_send(());
        }
    }
}
#[derive(Clone)]
pub struct Cancellation(pub(super) Arc<Io>);
impl Cancellation {
    /// 取消后整条连接失效，不继续复用可能只写出半帧的连接。
    pub fn cancel(&self) {
        self.0.cancel();
    }
}
struct Operation {
    overlapped: UnsafeCell<OVERLAPPED>,
    buffer: UnsafeCell<Vec<u8>>,
    io: Arc<Io>,
    /// ConnectNamedPipe 的传输长度未定义，不能把它当作读写长度校验。
    connecting: bool,
}
// SAFETY: 缓冲区仅由内核在完成前访问；领取结果在 GetOverlappedResult 确认完成后进行。
unsafe impl Send for Operation {}
// SAFETY: 不暴露共享缓冲区/OVERLAPPED 引用；回收回调只经 GetOverlappedResult 查询完成状态，不读写缓冲区。
unsafe impl Sync for Operation {}
impl Operation {
    fn result(&self) -> Option<io::Result<usize>> {
        let mut count = 0;
        // SAFETY: 操作地址与句柄稳定，非阻塞查询完成；返回成功/终态错误后内核不再访问缓冲区。
        if unsafe {
            GetOverlappedResult(
                self.io.handle.as_raw_handle(),
                self.overlapped.get(),
                &mut count,
                0,
            )
        } != 0
        {
            Some(Ok(if self.connecting { 0 } else { count as usize }))
        } else {
            let error = io::Error::last_os_error();
            (error.raw_os_error() != Some(ERROR_IO_INCOMPLETE as i32)).then_some(Err(error))
        }
    }
}
unsafe extern "system" fn completed(
    _: PTP_CALLBACK_INSTANCE,
    context: *mut c_void,
    _: PTP_WAIT,
    _: u32,
) {
    // 回调不执行协议/引擎；即便资源析构意外 panic，也不得穿过 Windows ABI。
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: 等待只在 orphan 登记后启用，orphan 持有 Arc<Io>，取出前本 Io 必然存活。
        let io = unsafe { &*context.cast::<Io>() };
        let mut orphan = io.orphan.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(operation) = orphan.take() else {
            return;
        };
        if operation.result().is_none() {
            // 信号若非内核完成（不变式被破坏），宁可继续等也不释放内核仍在写的缓冲区。
            *orphan = Some(operation);
            // SAFETY: 等待与事件都属于仍被 orphan 保活的本 Io。
            unsafe {
                SetThreadpoolWait(io.cleanup, io.ready.as_raw_handle(), ptr::null());
            }
            return;
        }
        drop(orphan);
        let pool = io._slot.0.clone();
        // 释放后本 Io 可能随之析构，此后不得再访问 io。
        drop(operation);
        // SAFETY: 先释放取消操作再通知补建监听，Pool 单独保持事件存活到置位结束。
        unsafe {
            SetEvent(pool.wake.as_raw_handle());
        }
    }));
}

pub(super) enum Kind {
    Connect,
    Read,
    Write,
}
/// 单个内核操作；同步等待和后台反应器都直接检查内核结果，禁止忙轮询。
pub struct Pending {
    operation: Arc<Operation>,
    /// 即时失败或 ERROR_PIPE_CONNECTED 没有可供查询的内核操作，保存提交时的结果。
    immediate: Option<io::Result<usize>>,
    finished: bool,
}
impl Pending {
    fn finish(&mut self, result: io::Result<usize>) -> Result<(Vec<u8>, usize)> {
        self.finished = true;
        self.operation.io.in_flight.store(false, Ordering::Release);
        if self.operation.io.closed.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let count = match result {
            Ok(count) => count,
            Err(error) => {
                self.operation.io.cancel();
                return Err(error.into());
            }
        };
        // SAFETY: 已确认内核停止访问，本 Pending 独占缓冲区的领取权；回收回调不访问它。
        let buffer = unsafe { std::mem::take(&mut *self.operation.buffer.get()) };
        if count > buffer.len() {
            self.operation.io.cancel();
            return Err(Error::Invalid("内核完成长度越界"));
        }
        Ok((buffer, count))
    }
    pub fn try_complete(&mut self) -> Result<Option<(Vec<u8>, usize)>> {
        if self.finished {
            return Err(Error::Closed);
        }
        match self.immediate.take().or_else(|| self.operation.result()) {
            Some(result) => self.finish(result).map(Some),
            None => Ok(None),
        }
    }
    fn wait(mut self, deadline: Option<Instant>) -> Result<(Vec<u8>, usize)> {
        loop {
            check_deadline(deadline)?;
            if let Some(result) = self.try_complete()? {
                // 领取途中发生调度延迟也不得交付迟到回复；调用方据此作废连接。
                if let Err(error) = check_deadline(deadline) {
                    self.operation.io.cancel();
                    return Err(error);
                }
                return Ok(result);
            }
            let millis = match deadline {
                Some(end) => remaining(end)?
                    .as_nanos()
                    .div_ceil(1_000_000)
                    .min((u32::MAX - 1) as u128) as u32,
                None => INFINITE,
            };
            // SAFETY: 操作持有内核事件；只在未完成时阻塞，不因分片/唤醒重置期限。
            match unsafe { WaitForSingleObject(self.operation.io.ready.as_raw_handle(), millis) } {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => return Err(Error::TimedOut),
                _ => return Err(io::Error::last_os_error().into()),
            }
        }
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        if !self.finished {
            self.operation.io.cancel();
            if self.immediate.is_none() && self.operation.result().is_none() {
                let io = &self.operation.io;
                // 先登记再启用等待；内核若已抢先置位，启用后立即回调，仍只回收一次。
                let previous = io
                    .orphan
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .replace(self.operation.clone());
                debug_assert!(previous.is_none(), "取消后不应再有第二个在途操作");
                // SAFETY: 等待与事件随 orphan 保活的 Io 存活；无超时，必须等内核实际完成。
                unsafe {
                    SetThreadpoolWait(io.cleanup, io.ready.as_raw_handle(), ptr::null());
                }
            }
        }
    }
}
impl Pipe {
    pub(super) fn operate(
        &mut self,
        kind: Kind,
        buffer: Vec<u8>,
        deadline: Instant,
    ) -> Result<(Vec<u8>, usize)> {
        check_deadline(Some(deadline))?;
        self.submit(kind, buffer)?.wait(Some(deadline))
    }
    pub(super) fn idle_read(&mut self, size: usize) -> Result<(Vec<u8>, usize)> {
        self.submit(Kind::Read, vec![0; size])?.wait(None)
    }
    pub fn start_read(&mut self) -> Result<Pending> {
        self.submit(Kind::Read, vec![0; super::CHUNK])
    }
    pub fn start_write(&mut self, bytes: Vec<u8>) -> Result<Pending> {
        if bytes.is_empty() || bytes.len() > crate::MAX_BODY_LEN + crate::HEADER_LEN {
            return Err(Error::Invalid("异步写入长度超限或为空"));
        }
        self.submit(Kind::Write, bytes)
    }
    pub(super) fn submit(&mut self, kind: Kind, buffer: Vec<u8>) -> Result<Pending> {
        if self.io.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        if self.io.in_flight.swap(true, Ordering::AcqRel) {
            return Err(Error::Busy);
        }
        let operation = Arc::new(Operation {
            overlapped: UnsafeCell::new(OVERLAPPED {
                hEvent: self.io.ready.as_raw_handle(),
                ..Default::default()
            }),
            buffer: UnsafeCell::new(buffer),
            io: self.io.clone(),
            connecting: matches!(kind, Kind::Connect),
        });
        // SAFETY: 旧操作已完成且无正常完成回调，新操作尚未提交，不会抹掉本次完成信号。
        unsafe {
            ResetEvent(self.io.ready.as_raw_handle());
        }
        // SAFETY: 提交期间独占缓冲区元数据；内核通过 OVERLAPPED/事件交还访问权。
        let buffer = unsafe { &mut *operation.buffer.get() };
        // SAFETY: 地址与缓冲区在整个内核操作期间由 Arc 固定；取消失败不影响回收引用存活。
        let success = unsafe {
            match kind {
                Kind::Connect => {
                    ConnectNamedPipe(self.io.handle.as_raw_handle(), operation.overlapped.get())
                }
                Kind::Read => ReadFile(
                    self.io.handle.as_raw_handle(),
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    ptr::null_mut(),
                    operation.overlapped.get(),
                ),
                Kind::Write => WriteFile(
                    self.io.handle.as_raw_handle(),
                    buffer.as_ptr(),
                    buffer.len() as u32,
                    ptr::null_mut(),
                    operation.overlapped.get(),
                ),
            }
        };
        let immediate = if success == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                None
            } else if matches!(kind, Kind::Connect)
                && error.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32)
            {
                Some(Ok(0))
            } else {
                Some(Err(error))
            }
        } else {
            None
        };
        if immediate.is_some() {
            // SAFETY: 即时失败/先连接成功没有内核完成通知，显式唤醒反应器领取终态。
            unsafe {
                SetEvent(self.io.ready.as_raw_handle());
            }
        }
        let pending = Pending {
            operation,
            immediate,
            finished: false,
        };
        // 取消可能发生在最初 closed 检查与提交之间，提交后再取消一次覆盖这个窗口。
        if self.io.closed.load(Ordering::Acquire) {
            self.io.cancel();
        }
        Ok(pending)
    }
}
fn check_deadline(deadline: Option<Instant>) -> Result<()> {
    deadline.map(remaining).transpose().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipe::Runtime;
    use std::time::Duration;

    #[test]
    fn completed_connect_ignores_undefined_transfer_length() -> Result<()> {
        let runtime = Runtime::new()?;
        let name = format!("connect-length-{}", std::process::id());
        let listener = runtime.bind(&name)?;
        let (server, mut pending) = listener.start_accept()?;
        let client = runtime.connect(
            &name,
            &std::env::current_exe()?,
            Instant::now() + Duration::from_secs(2),
        )?;
        // SAFETY: 只等待本测试连接完成，随后确认内核已经不再写 OVERLAPPED。
        let waited =
            unsafe { WaitForSingleObject(server.completion_event().as_raw_handle(), 2_000) };
        assert_eq!(waited, WAIT_OBJECT_0);
        assert!(pending.operation.result().unwrap().is_ok());
        // SAFETY: 内核已停止访问；模拟 ConnectNamedPipe 未定义的长度，不能误判为缓冲区越界。
        unsafe {
            (*pending.operation.overlapped.get()).InternalHigh = 42;
        }
        let (bytes, count) = pending.try_complete()?.unwrap();
        assert!(bytes.is_empty() && count == 0);
        drop(pending);
        drop(client);
        drop(server);
        runtime.shutdown(Instant::now() + Duration::from_secs(2))
    }

    #[test]
    fn spurious_cleanup_signal_never_frees_a_buffer_the_kernel_still_owns() -> Result<()> {
        let runtime = Runtime::new()?;
        let name = format!("spurious-cleanup-{}", std::process::id());
        let (mut server, accept) = runtime.bind(&name)?.start_accept()?;
        let client = runtime.connect(
            &name,
            &std::env::current_exe()?,
            Instant::now() + Duration::from_secs(2),
        )?;
        accept.wait(Some(Instant::now() + Duration::from_secs(2)))?;
        // 对端不写，读取保持在内核中；测试接管回收，不走 Pending::drop 的取消。
        let mut pending = server.start_read()?;
        assert!(pending.try_complete()?.is_none());
        let watch = Arc::downgrade(&pending.operation);
        pending.finished = true;
        *server.io.orphan.lock().unwrap() = Some(pending.operation.clone());
        drop(pending);
        // SAFETY: 在测试线程直接调用回收回调模拟非内核来源的信号；上下文为仍存活的本 Io。
        unsafe {
            completed(0, Arc::as_ptr(&server.io).cast_mut().cast(), 0, 0);
        }
        assert!(server.io.orphan.lock().unwrap().is_some());
        assert!(watch.upgrade().is_some(), "内核仍在读时不得释放缓冲区");
        // 回调已重新启用等待；真正的取消完成后才释放并通知补建监听。
        server.io.cancel();
        // SAFETY: Runtime 持有该事件，只做有限等待。
        let waited =
            unsafe { WaitForSingleObject(runtime.completion_event().as_raw_handle(), 2_000) };
        assert_eq!(waited, WAIT_OBJECT_0);
        assert!(watch.upgrade().is_none());
        drop(client);
        drop(server);
        runtime.shutdown(Instant::now() + Duration::from_secs(2))
    }
}
