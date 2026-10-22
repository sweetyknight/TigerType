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
        mpsc::{self, Receiver, SyncSender},
    },
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::{ERROR_IO_PENDING, ERROR_PIPE_CONNECTED},
    Storage::FileSystem::{ReadFile, WriteFile},
    System::{
        IO::{CancelIoEx, OVERLAPPED},
        LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN,
            GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleHandleExW, GetModuleHandleW,
        },
        Pipes::ConnectNamedPipe,
        Threading::{
            CancelThreadpoolIo, CloseThreadpoolIo, CreateThreadpoolIo, PTP_CALLBACK_INSTANCE,
            PTP_IO, SetEvent, StartThreadpoolIo, TP_CALLBACK_ENVIRON_V3,
            TP_CALLBACK_PRIORITY_NORMAL,
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
    notification: PTP_IO,
    pub closed: AtomicBool,
    in_flight: AtomicBool,
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
            in_flight: AtomicBool::new(false),
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
    let pool = operation.io._slot.0.clone();
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
    drop(operation);
    // SAFETY: 取消操作先释放回调持有的管道，再唤醒重建监听；Pool 单独保持事件存活。
    unsafe {
        SetEvent(pool.wake.as_raw_handle());
    }
}

pub(super) enum Kind {
    Connect,
    Read,
    Write,
}

/// 一个内核在途操作；只可在完成事件唤醒后非阻塞领取，提前丢弃会取消连接。
pub struct Pending {
    operation: Arc<Operation>,
    receiver: Receiver<io::Result<usize>>,
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
        // SAFETY: channel 完成通知建立同步；内核已停止访问，回调从不读写 buffer。
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
        match self.receiver.try_recv() {
            Ok(result) => self.finish(result).map(Some),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.operation.io.cancel();
                Err(Error::Closed)
            }
        }
    }
    fn wait(mut self, deadline: Option<Instant>) -> Result<(Vec<u8>, usize)> {
        let result =
            match deadline {
                Some(deadline) => self.receiver.recv_timeout(remaining(deadline)?).map_err(
                    |error| match error {
                        mpsc::RecvTimeoutError::Timeout => Error::TimedOut,
                        mpsc::RecvTimeoutError::Disconnected => Error::Closed,
                    },
                )?,
                None => self.receiver.recv().map_err(|_| Error::Closed)?,
            };
        // 超时边界仍不交付刚到达的迟到结果；Drop 保留内核所需内存直到取消完成。
        check_deadline(deadline)?;
        self.finish(result)
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        if !self.finished {
            self.operation.io.cancel();
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
    /// 服务端反应器每条连接仅保留一个操作；不能把 try_complete 放进忙轮询。
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
        let (sender, receiver) = mpsc::sync_channel(1);
        let operation = Arc::new(Operation {
            overlapped: UnsafeCell::new(OVERLAPPED::default()),
            buffer: UnsafeCell::new(buffer),
            done: sender,
            io: self.io.clone(),
        });
        let callback_ref = Arc::into_raw(operation.clone());
        let pending = Pending {
            operation: operation.clone(),
            receiver,
            finished: false,
        };
        // SAFETY: 提交者此时独占缓冲区元数据，Arc 保持内核访问的分配和 OVERLAPPED 稳定。
        let buffer = unsafe { &mut *operation.buffer.get() };
        // SAFETY: 每次 StartThreadpoolIo 已交出一份回调引用，所有退出路径配对收回。
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
                // SAFETY: 没有提交异步操作，不会有完成包，撤销注册并收回回调引用。
                unsafe {
                    CancelThreadpoolIo(self.io.notification);
                    drop(Arc::from_raw(callback_ref));
                }
                if !matches!(kind, Kind::Connect)
                    || error.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32)
                {
                    return Err(error.into());
                }
                // 先连上的客户端没有完成包，显式通知同一个完成通道和反应器。
                let _ = operation.done.try_send(Ok(0));
                // SAFETY: Pool 在操作存活期持有事件。
                unsafe {
                    SetEvent(self.io._slot.0.wake.as_raw_handle());
                }
            }
        }
        // 覆盖检查 closed 后、提交前发生取消的竞态。
        if self.io.closed.load(Ordering::Acquire) {
            self.io.cancel();
        }
        Ok(pending)
    }
}
fn check_deadline(deadline: Option<Instant>) -> Result<()> {
    deadline.map(remaining).transpose().map(|_| ())
}
