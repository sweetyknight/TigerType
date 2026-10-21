//! 无 librime 的真实进程生命周期测试，x64/x86 均运行。
#![cfg(windows)]

use std::{
    os::windows::io::{AsHandle, AsRawHandle},
    path::Path,
    time::{Duration, Instant},
};
use tigerrime_app::{Result, job::JobChild, resources};
use windows_sys::Win32::{Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject};

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

#[test]
fn explicit_inheritance_reaches_ready_and_job_termination_returns_nonzero() -> Result<()> {
    let work = resources::new_directory(&std::env::temp_dir(), "tigerrime-job")?;
    let child = JobChild::spawn(
        Path::new(env!("CARGO_BIN_EXE_tigerrime")),
        &["probe-idle-worker".into()],
        &work,
        &work.join("stdout.log"),
        &work.join("stderr.log"),
    )?;
    child.wait_signal(deadline())?;
    assert!(
        child
            .wait_signal(Instant::now() + Duration::from_millis(10))
            .is_err(),
        "就绪事件必须自动复位"
    );
    assert!(
        child
            .wait(Instant::now() + Duration::from_millis(10))
            .is_err()
    );
    child.terminate()?;
    assert_eq!(child.wait(deadline())?, 1);
    Ok(())
}

#[test]
fn dropping_owner_reaps_its_live_child_and_existing_logs_are_not_overwritten() -> Result<()> {
    let work = resources::new_directory(&std::env::temp_dir(), "tigerrime-job-drop")?;
    let binary = Path::new(env!("CARGO_BIN_EXE_tigerrime"));
    let child = JobChild::spawn(
        binary,
        &["probe-idle-worker".into()],
        &work,
        &work.join("stdout.log"),
        &work.join("stderr.log"),
    )?;
    child.wait_signal(deadline())?;
    let process = child.as_handle().try_clone_to_owned()?;
    drop(child);
    assert_eq!(
        // SAFETY: 保留同一进程对象的句柄，检查已结束；不按可能复用的 PID 查找其他进程。
        unsafe { WaitForSingleObject(process.as_raw_handle(), 1_000) },
        WAIT_OBJECT_0
    );
    assert!(
        JobChild::spawn(
            binary,
            &["probe-idle-worker".into()],
            &work,
            &work.join("stdout.log"),
            &work.join("stderr.log")
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn invalid_executable_fails_without_a_running_child() -> Result<()> {
    let work = resources::new_directory(&std::env::temp_dir(), "tigerrime-job-failure")?;
    assert!(
        JobChild::spawn(
            &work.join("missing.exe"),
            &[],
            &work,
            &work.join("stdout.log"),
            &work.join("stderr.log")
        )
        .is_err()
    );
    assert!(std::fs::read(work.join("stdout.log"))?.is_empty());
    Ok(())
}

#[test]
fn ready_wait_reports_child_exit_instead_of_waiting_for_a_missing_signal() -> Result<()> {
    let work = resources::new_directory(&std::env::temp_dir(), "tigerrime-job-early-exit")?;
    let child = JobChild::spawn(
        Path::new(env!("CARGO_BIN_EXE_tigerrime")),
        &["invalid-command".into()],
        &work,
        &work.join("stdout.log"),
        &work.join("stderr.log"),
    )?;
    let error = child.wait_signal(deadline()).unwrap_err();
    assert!(error.to_string().contains("代码 1"), "{error}");
    assert_eq!(child.wait(deadline())?, 1);
    Ok(())
}
