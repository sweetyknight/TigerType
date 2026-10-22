//! OVERLAPPED 与缓冲区由操作对象持有；超时返回不释放内核仍在使用的内存。

use super::{Pipe, Slot, remaining};
use crate::{Error, Result};
use std::{
    cell::UnsafeCell,
    ffi::c_void,
    io,
    os::windows::io::{AsRawHandle, OwnedHandle},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{ERROR_IO_PENDING, ERROR_PIPE_CONNECTED},
    Storage::FileSystem::{ReadFile, WriteFile},
    System::{
        IO::{CancelIoEx, OVERLAPPED},
        LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            GetModuleHandleExW, GetModuleHandleW,
        },
        Pipes::ConnectNamedPipe,
        Threading::{
            CancelThreadpoolIo, CloseThreadpoolIo, CreateThreadpoolIo, PTP_CALLBACK_INSTANCE,
            PTP_IO, StartThreadpoolIo, TP_CALLBACK_ENVIRON_V3, TP_CALLBACK_PRIORITY_NORMAL,
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
    // shortcut: 尚无可卸载 DLL 的回调驻留保护，接入 TSF DLL 前须先实现受控卸载。
    // SAFETY: NULL 查询进程主模块；EXE 在进程退出前不会卸载，取消回调可以安全收尾。
    if module != unsafe { GetModuleHandleW(ptr::null()) } {
        return Err(Error::Invalid(
            "管道完成线程目前只允许在 EXE 中运行，尚不支持可卸载 TSF DLL",
        ));
    }
    Ok(())
}

pub(super) struct Io {
    pub handle: OwnedHandle,
    notification: PTP_IO,
    pub closed: AtomicBool,
    // 完成回调和管道必须先于自定义线程池释放。
    pub(super) _slot: Slot,
    #[cfg(test)]
    pub on_drop: Option<SyncSender<()>>,
}

impl Io {
    pub fn new(handle: OwnedHandle, slot: Slot) -> Result<Arc<Self>> {
        let environment = TP_CALLBACK_ENVIRON_V3 {
            Version: 3,
            Pool: slot.0.pool,
            CallbackPriority: TP_CALLBACK_PRIORITY_NORMAL,
            Size: size_of::<TP_CALLBACK_ENVIRON_V3>() as u32,
            ..Default::default()
        };
        // SAFETY: handle 为 OVERLAPPED 管道；环境仅在创建时读取，Pool 由 slot 持有。
        let notification = unsafe {
            CreateThreadpoolIo(
                handle.as_raw_handle(),
                Some(completed),
                ptr::null_mut(),
                &environment,
            )
        };
        if notification == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Arc::new(Self {
            handle,
            notification,
            closed: AtomicBool::new(false),
            _slot: slot,
            #[cfg(test)]
            on_drop: None,
        }))
    }

    pub fn cancel(&self) {
        self.closed.store(true, Ordering::Release);
        // SAFETY: Arc 保持句柄存活；仅请求取消，不在输入线程等待内核取消完成。
        unsafe {
            CancelIoEx(self.handle.as_raw_handle(), ptr::null());
        }
    }
}

impl Drop for Io {
    fn drop(&mut self) {
        // SAFETY: 每个已提交操作持有 Arc<Io>，最后一次释放必在内核完成之后。
        // CloseThreadpoolIo 允许当前回调结束后异步释放注册，不等待或取消回调本身。
        unsafe {
            CloseThreadpoolIo(self.notification);
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
    /// 线程安全且幂等；取消后整个连接失效，避免半帧写入后继续复用。
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

// repr(C) 保证首字段地址可由完成回调恢复为 Operation；地址由 Arc 分配保持稳定。
#[repr(C)]
struct Operation {
    overlapped: UnsafeCell<OVERLAPPED>,
    buffer: UnsafeCell<Vec<u8>>,
    done: SyncSender<io::Result<usize>>,
    io: Arc<Io>,
}

// SAFETY: 内核只在完成前访问缓冲区；回调不访问它，调用者仅在完成通知后取走它。
unsafe impl Send for Operation {}
// SAFETY: OVERLAPPED/缓冲区不提供共享引用；只有提交者和内核访问，取消不读写它们。
unsafe impl Sync for Operation {}

unsafe extern "system" fn completed(
    _: PTP_CALLBACK_INSTANCE,
    _: *mut c_void,
    overlapped: *mut c_void,
    result: u32,
    transferred: usize,
    _: PTP_IO,
) {
    // SAFETY: 每次 StartThreadpoolIo 前恰好交出一个 Arc 引用；首字段偏移为 0。
    let operation = unsafe { Arc::from_raw(overlapped.cast::<Operation>()) };
    // 系统回调不执行协议/业务代码；异常也不得穿过 Windows ABI。
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let result = if result == 0 {
            Ok(transferred)
        } else {
            Err(io::Error::from_raw_os_error(result as i32))
        };
        // 每操作只有一个完成通知，容量为 1；超时接收端已丢弃时直接释放资源。
        let _ = operation.done.try_send(result);
    }));
}

pub(super) enum Kind {
    Connect,
    Read,
    Write,
}

impl Pipe {
    pub(super) fn operate(
        &mut self,
        kind: Kind,
        buffer: Vec<u8>,
        deadline: Instant,
    ) -> Result<(Vec<u8>, usize)> {
        self.operate_until(kind, buffer, Some(deadline))
    }

    /// 仅服务端等首字节可空闲；保留同一个 OVERLAPPED，不超时撤销再重读而丢失竞态数据。
    pub(super) fn idle_read(&mut self, size: usize) -> Result<(Vec<u8>, usize)> {
        self.operate_until(Kind::Read, vec![0; size], None)
    }

    fn operate_until(
        &mut self,
        kind: Kind,
        buffer: Vec<u8>,
        deadline: Option<Instant>,
    ) -> Result<(Vec<u8>, usize)> {
        if self.io.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        check_deadline(deadline)?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let operation = Arc::new(Operation {
            overlapped: UnsafeCell::new(OVERLAPPED::default()),
            buffer: UnsafeCell::new(buffer),
            done: sender,
            io: self.io.clone(),
        });
        let callback_ref = Arc::into_raw(operation.clone());
        // SAFETY: 提交前独占缓冲区元数据；内核只访问其分配，不改变 Vec 的指针/长度。
        let buffer = unsafe { &mut *operation.buffer.get() };
        // SAFETY: 回调引用已建立；所有地址保持到回调完成，即使调用方提前超时返回。
        let success = unsafe {
            StartThreadpoolIo(self.io.notification);
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
        if success == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                // SAFETY: 未提交异步 I/O，不会收到完成回调；撤销通知并收回它的 Arc。
                unsafe {
                    CancelThreadpoolIo(self.io.notification);
                    drop(Arc::from_raw(callback_ref));
                }
                if !matches!(kind, Kind::Connect)
                    || error.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32)
                {
                    self.io.cancel();
                    return Err(error.into());
                }
                // 客户端先于 ConnectNamedPipe 连上时没有完成包，按已连接处理。
                check_deadline(deadline)?;
                if self.io.closed.load(Ordering::Acquire) {
                    return Err(Error::Cancelled);
                }
                return Ok((Vec::new(), 0));
            }
        }
        // 覆盖 cancel 在检查 closed 与提交 I/O 之间发生的竞态。
        if self.io.closed.load(Ordering::Acquire) {
            self.io.cancel();
        }
        let result = loop {
            let budget = match deadline.map(remaining).transpose() {
                Ok(budget) => budget.unwrap_or(Duration::from_secs(60)),
                Err(error) => break Err(error),
            };
            match receiver.recv_timeout(budget) {
                Ok(result) => break result.map_err(Error::Io),
                // 空闲等待可被 CancelIoEx/断连唤醒；有限等待到期不重新提交 I/O 或分配缓冲区。
                Err(mpsc::RecvTimeoutError::Timeout)
                    if deadline.is_none() && !self.io.closed.load(Ordering::Acquire) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => break Err(Error::TimedOut),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Err(Error::Invalid("管道完成回调未返回结果"));
                }
            }
        };
        let result = result.and_then(|count| {
            check_deadline(deadline)?;
            Ok(count)
        });
        let count = match result {
            Ok(_) if self.io.closed.load(Ordering::Acquire) => return Err(Error::Cancelled),
            Ok(count) => count,
            Err(error) => {
                if self.io.closed.load(Ordering::Acquire) {
                    return Err(Error::Cancelled);
                }
                self.io.cancel();
                return Err(error);
            }
        };
        // SAFETY: 完成通知已通过 channel 同步；内核不再访问，回调始终不访问 buffer。
        let buffer = unsafe { std::mem::take(&mut *operation.buffer.get()) };
        if count > buffer.len() {
            self.io.cancel();
            return Err(Error::Invalid("内核完成长度越界"));
        }
        Ok((buffer, count))
    }
}

fn check_deadline(deadline: Option<Instant>) -> Result<()> {
    deadline.map(remaining).transpose().map(|_| ())
}
