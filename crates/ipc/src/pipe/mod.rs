//! 本机、单对端命名管道；一个 Runtime 共用最多一个完成线程，不按连接创建线程。
//! 当前运行时显式拒绝可卸载 DLL；TSF 接入前必须先补齐完成回调的模块驻留保护。

mod io;
mod security;
#[cfg(test)]
mod tests;

pub use io::Cancellation;
pub use security::Peer;

use self::{
    io::{Io, Kind},
    security::{CLIENT_ACCESS, Identity},
};
use crate::{ClientState, Error, FrameAssembler, Result, decode};
use std::{
    io as std_io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tigerrime_contracts::protocol::{Request, Response};
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
        PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    },
    System::{
        Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
            PIPE_WAIT,
        },
        Threading::{
            CloseThreadpool, CreateThreadpool, GetCurrentProcess, PTP_POOL,
            SetThreadpoolThreadMaximum,
        },
    },
};

const MAX_PIPES: usize = 64;
const CHUNK: usize = 16 * 1024;

struct Pool {
    pool: PTP_POOL,
    identity: Identity,
    pipes: AtomicUsize,
}
impl Drop for Pool {
    fn drop(&mut self) {
        // SAFETY: 所有 I/O 注册均已释放；Windows 在最后回调退出后回收线程池。
        unsafe {
            CloseThreadpool(self.pool);
        }
    }
}

struct Slot(Arc<Pool>);
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.pipes.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct Runtime(Arc<Pool>);
impl Runtime {
    /// 每个产品进程在启动阶段创建一次，不能放在逐键或逐连接路径中。
    pub fn new() -> Result<Self> {
        io::require_executable()?;
        // SAFETY: 当前进程伪句柄只借用，不关闭。
        let identity = Identity::from_process(unsafe { GetCurrentProcess() })?;
        // SAFETY: 无保留参数；池由 Pool 的 Drop 独占回收。
        let pool = unsafe { CreateThreadpool(ptr::null()) };
        if pool == 0 {
            return Err(std_io::Error::last_os_error().into());
        }
        // SAFETY: 固定最多一个完成线程，所有回调只投递结果，不阻塞或调用引擎。
        unsafe {
            SetThreadpoolThreadMaximum(pool, 1);
        }
        Ok(Self(Arc::new(Pool {
            pool,
            identity,
            pipes: AtomicUsize::new(0),
        })))
    }

    fn slot(&self) -> Result<Slot> {
        self.0
            .pipes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_PIPES).then_some(count + 1)
            })
            .map_err(|_| Error::Busy)?;
        Ok(Slot(self.0.clone()))
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
            _peer: None,
        }))
    }

    /// 只尝试连接已创建的本机实例；忙/不存在立即失败，不在输入路径启动进程或轮询。
    pub fn connect(&self, label: &str, peer: Peer, deadline: Instant) -> Result<Pipe> {
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
        peer.verify(handle.as_raw_handle(), &self.0.identity, false)?;
        remaining(deadline)?;
        let pipe = Pipe {
            io: Io::new(handle, slot)?,
            _peer: Some(peer),
        };
        pipe.check_open(deadline)?;
        Ok(pipe)
    }
}

pub struct Listener(Pipe);
impl Listener {
    pub fn cancellation(&self) -> Cancellation {
        self.0.cancellation()
    }

    pub fn accept(mut self, peer: Peer, deadline: Instant) -> Result<Pipe> {
        self.0.operate(Kind::Connect, Vec::new(), deadline)?;
        peer.verify(
            self.0.io.handle.as_raw_handle(),
            &self.0.io._slot.0.identity,
            true,
        )?;
        self.0.check_open(deadline)?;
        self.0._peer = Some(peer);
        Ok(self.0)
    }
}

pub struct Pipe {
    io: Arc<Io>,
    // 保持预期进程句柄存活，不把后续 PID 复用当作同一对端。
    _peer: Option<Peer>,
}
impl Drop for Pipe {
    fn drop(&mut self) {
        self.io.cancel();
    }
}

impl Pipe {
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

fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or(Error::TimedOut)
}
