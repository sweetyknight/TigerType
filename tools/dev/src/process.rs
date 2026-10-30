//! 开发期工具及测试共享的限时子进程等待；不是生产 IPC 或引擎监督器。

use crate::Result;
#[cfg(windows)]
use std::os::windows::{io::AsRawHandle, process::CommandExt};
#[cfg(windows)]
use std::process::{Child, Stdio};
use std::{
    process::{Command, ExitStatus},
    time::Duration,
};
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::WaitForSingleObject,
};

#[cfg(windows)]
struct ChildGuard(Child);
#[cfg(windows)]
impl Drop for ChildGuard {
    fn drop(&mut self) {
        // panic、超时和等待失败均回收自己创建的进程，不触碰其他进程。
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[cfg(windows)]
pub fn run(command: &mut Command, timeout: Duration) -> Result<ExitStatus> {
    let millis = u32::try_from(timeout.as_millis())?;
    if millis == u32::MAX {
        return Err("不允许 INFINITE 等待".into());
    }
    command.creation_flags(0x08000000).stdin(Stdio::null());
    let mut child = ChildGuard(command.spawn()?);
    // SAFETY: guard 持有有效进程句柄；到期后由 guard 终止和回收，不忙轮询。
    let result = unsafe { WaitForSingleObject(child.0.as_raw_handle(), millis) };
    match result {
        WAIT_OBJECT_0 => Ok(child.0.wait()?),
        WAIT_TIMEOUT => Err("子进程超时；已安排终止并回收".into()),
        _ => Err(std::io::Error::last_os_error().into()),
    }
}

#[cfg(not(windows))]
pub fn run(_: &mut Command, _: Duration) -> Result<ExitStatus> {
    Err("开发期子进程工具仅支持 Windows".into())
}
