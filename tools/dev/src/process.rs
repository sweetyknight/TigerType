//! 开发期工具及测试共享的限时子进程等待；不是生产 IPC 或引擎监督器。

use crate::Result;
#[cfg(windows)]
use std::os::windows::{io::AsRawHandle, process::CommandExt};
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
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

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
struct ChildGuard(Child);
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
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

#[cfg(target_os = "macos")]
pub fn run(command: &mut Command, timeout: Duration) -> Result<ExitStatus> {
    use std::{
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
        time::Instant,
    };
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or("子进程期限溢出")?;
    let mut child = ChildGuard(command.stdin(Stdio::null()).spawn()?);
    // SAFETY: kqueue 新建描述符，失败时不构造 OwnedFd。
    let raw = unsafe { libc::kqueue() };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: raw 已核对非负且尚未由其他对象持有。
    let queue = unsafe { OwnedFd::from_raw_fd(raw) };
    let change = libc::kevent {
        ident: child.0.id() as usize,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    loop {
        if let Some(status) = child.0.try_wait()? {
            return Ok(status);
        }
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or("子进程超时；将终止并回收")?;
        let wait = libc::timespec {
            tv_sec: left.as_secs().try_into()?,
            tv_nsec: left.subsec_nanos().into(),
        };
        let mut event = std::mem::MaybeUninit::<libc::kevent>::uninit();
        // SAFETY: 只监听本命令的 PID；所有缓冲区在调用期间有效，无忙轮询。
        let count =
            unsafe { libc::kevent(queue.as_raw_fd(), &change, 1, event.as_mut_ptr(), 1, &wait) };
        if count < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if count == 0 {
            return Err("子进程超时；将终止并回收".into());
        }
        if let Some(status) = child.0.try_wait()? {
            return Ok(status);
        }
        // SAFETY: count 为 1 时内核已完整写入事件。
        let event = unsafe { event.assume_init() };
        if event.flags & libc::EV_ERROR != 0 {
            return Err(std::io::Error::from_raw_os_error(event.data as i32).into());
        }
        return Ok(child.0.wait()?);
    }
}

#[cfg(target_os = "linux")]
pub fn run(command: &mut Command, timeout: Duration) -> Result<ExitStatus> {
    use std::{
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
        time::Instant,
    };
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or("子进程期限溢出")?;
    let mut child = ChildGuard(command.stdin(Stdio::null()).spawn()?);
    // SAFETY: 仅为尚未回收的本次子进程取得 pidfd，flags 为零；支持范围的内核均提供此接口。
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, child.0.id(), 0) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: pidfd_open 返回的新描述符已核对非负，唯一交给 OwnedFd 关闭。
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw.try_into()?) };
    let mut event = libc::pollfd {
        fd: descriptor.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        if let Some(status) = child.0.try_wait()? {
            return Ok(status);
        }
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or("子进程超时；将终止并回收")?;
        let millis = left.as_millis().saturating_add(1).min(i32::MAX as u128) as i32;
        // SAFETY: poll 只读取一个有效 pidfd，等待退出事件或绝对期限；EINTR 不重置预算。
        let count = unsafe { libc::poll(&mut event, 1, millis) };
        if count < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if event.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err("子进程 pidfd 等待失败".into());
        }
        // 很长的期限可能分段等待；普通检查只等待一次，不周期唤醒。
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub fn run(_: &mut Command, _: Duration) -> Result<ExitStatus> {
    Err("开发期子进程工具仅支持 Windows/macOS/Linux".into())
}

#[cfg(all(test, any(windows, target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    #[test]
    #[ignore = "仅由父测试作为独立限时子进程调用，不直接运行"]
    fn child() {
        if std::env::var("TIGERTYPE_PROCESS_TEST").as_deref() == Ok("wait") {
            // park 允许伪唤醒；子进程只由父测试的超时回收结束。
            loop {
                std::thread::park();
            }
        }
    }
    #[test]
    fn child_exit_and_timeout_use_the_platform_wait_and_are_reaped() -> Result<()> {
        let command = |wait| {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "process::tests::child", "--ignored"]);
            if wait {
                command.env("TIGERTYPE_PROCESS_TEST", "wait");
            }
            command
        };
        assert!(run(&mut command(false), Duration::from_secs(5))?.success());
        let error = run(&mut command(true), Duration::from_millis(30)).unwrap_err();
        assert!(error.to_string().contains("超时"), "{error}");
        assert!(run(&mut command(false), Duration::from_secs(5))?.success());
        Ok(())
    }
}
