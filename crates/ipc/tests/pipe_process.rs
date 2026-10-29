//! 跨进程管道验收不加载 librime；辅助子进程由父测试限时终止和回收。
#![cfg(windows)]

use std::{
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command as ProcessCommand, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
use tigertype_contracts::{
    InputState, KeyOutcome,
    protocol::{Command, Reply, Request, Response, Route},
};
use tigertype_ipc::{ClientState, Error, Message, decode, encode, pipe::Runtime};
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    System::Threading::{CREATE_NO_WINDOW, WaitForSingleObject},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Worker(Child);
impl Worker {
    fn start(label: &str, mode: &str) -> Result<Self> {
        Ok(Self(
            ProcessCommand::new(std::env::current_exe()?)
                .args(["--ignored", "--exact", "pipe_worker", "--nocapture"])
                .env("TIGERTYPE_PIPE_TEST_LABEL", label)
                .env("TIGERTYPE_PIPE_TEST_MODE", mode)
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()?,
        ))
    }

    fn finish(mut self) -> Result<()> {
        // SAFETY: 持有自己创建的进程，最多等待 15 秒；所有错误/超时由 Drop 回收。
        if unsafe { WaitForSingleObject(self.0.as_raw_handle(), 15_000) } != WAIT_OBJECT_0 {
            return Err("管道辅助进程超时或等待失败".into());
        }
        assert!(self.0.wait()?.success(), "管道辅助进程断言失败");
        Ok(())
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn label() -> String {
    // 并行用例不依赖系统时钟精度，避免同名首实例冲突被误判为权限失败。
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    format!(
        "process-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

fn request() -> Request {
    Request {
        route: Route {
            generation: 3,
            request_id: 1,
            session_id: 1,
        },
        command: Command::Key {
            epoch: 1,
            key: tigertype_contracts::Key::Space,
            prepare: tigertype_contracts::protocol::Prepare::Keep,
        },
    }
}

#[test]
fn authenticated_child_round_trip_uses_real_process_boundaries() -> Result<()> {
    let runtime = Runtime::new().map_err(|error| format!("runtime: {error}"))?;
    let label = label();
    let listener = runtime
        .bind(&label)
        .map_err(|error| format!("bind: {error}"))?;
    let worker = Worker::start(&label, "echo").map_err(|error| format!("spawn: {error}"))?;
    let mut pipe = listener
        .accept(deadline())
        .map_err(|error| format!("accept: {error}"))?;
    assert_eq!(
        decode(
            &pipe
                .receive_frame(deadline())
                .map_err(|error| format!("receive: {error}"))?
        )?,
        Message::Request(request())
    );
    pipe.send_frame(
        &encode(&Message::Response(Response {
            route: request().route,
            reply: Reply::Key(KeyOutcome {
                handled: true,
                commit: Some("𠀀😀".into()),
                state: InputState::default(),
            }),
        }))?,
        deadline(),
    )?;
    worker.finish()
}

#[test]
fn same_logon_server_with_unexpected_image_is_rejected() -> Result<()> {
    let runtime = Runtime::new()?;
    let label = label();
    let _listener = runtime.bind(&label)?;
    Worker::start(&label, "wrong-server")?.finish()
}

#[test]
#[ignore = "只由有期限的父测试调用；手工运行不创建后台进程"]
fn pipe_worker() -> Result<()> {
    let Ok(label) = std::env::var("TIGERTYPE_PIPE_TEST_LABEL") else {
        return Ok(());
    };
    let mode = std::env::var("TIGERTYPE_PIPE_TEST_MODE")?;
    // 父子是同一测试程序；映像来自本进程而非环境变量，错误映像用于覆盖拒绝分支。
    let image = std::env::current_exe()?;
    let runtime = Runtime::new()?;
    if mode == "wrong-server" {
        assert!(matches!(
            runtime.connect(
                &label,
                &image.with_file_name("not-the-server.exe"),
                deadline()
            ),
            Err(Error::Invalid(_))
        ));
        return Ok(());
    }
    let mut pipe = runtime.connect(&label, &image, deadline())?;
    assert_eq!(mode, "echo");
    let reply = pipe.transact(&mut ClientState::new(3)?, request(), deadline())?;
    assert!(
        matches!(reply.reply, Reply::Key(outcome) if outcome.commit.as_deref() == Some("𠀀😀"))
    );
    Ok(())
}
