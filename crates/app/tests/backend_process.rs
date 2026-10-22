//! 后台在独立限时进程运行，UI 用合成点击驱动；不创建可见窗口、不注册输入法。
#![cfg(windows)]
use std::{
    io::{BufRead, BufReader, Write},
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command as ProcessCommand, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use tigerrime_app::{Result, backend::Server};
use tigerrime_contracts::{
    Key, Options,
    candidate::{CandidateChoice, ScreenRect},
    protocol::{Command, Message, Rejection, Reply},
};
use tigerrime_engine::{Data, Dictionary, Entry};
use tigerrime_ipc::{pipe::Runtime, service::Client};
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    System::Threading::{CREATE_NO_WINDOW, WaitForSingleObject},
};
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(3)
}
fn dictionary() -> Dictionary {
    Dictionary::new(Data {
        entries: vec![Entry {
            character: '的',
            code: "u".into(),
            weight: 1,
        }],
        common: vec!['的'],
        ..Default::default()
    })
    .unwrap()
}
struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
#[test]
fn real_backend_singleton_authenticated_hosts_notification_and_one_commit() -> Result<()> {
    let namespace = format!("service-test-{}", std::process::id());
    let image = std::env::current_exe()?;
    let mut worker = Worker(
        ProcessCommand::new(&image)
            .args(["--ignored", "--exact", "backend_worker", "--nocapture"])
            .env("TIGERRIME_BACKEND_TEST", &namespace)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?,
    );
    let stdout = worker.0.stdout.take().unwrap();
    let (ready, observed) = mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout)
            .lines()
            .map_while(std::result::Result::ok)
        {
            if line == "BACKEND_READY" {
                let _ = ready.try_send(());
            }
        }
    });
    observed.recv_timeout(Duration::from_secs(5))?;
    let data = dictionary();
    assert!(Server::new(&data, &namespace, Options::default()).is_err());
    let runtime = Runtime::new()?;
    assert!(
        Client::connect(
            &runtime,
            &image.with_file_name("not-the-server.exe"),
            &namespace,
            deadline()
        )
        .is_err()
    );
    let mut a = Client::connect(&runtime, &image, &namespace, deadline())?;
    let mut b = Client::connect(&runtime, &image, &namespace, deadline())?;
    assert_ne!(a.host, b.host);
    assert_eq!(a.generation, b.generation);
    for client in [&mut a, &mut b] {
        assert!(matches!(
            client.request(
                1,
                Command::Open {
                    options: Options::default()
                },
                deadline()
            )?,
            Reply::Opened(_)
        ));
    }
    a.request(
        1,
        Command::Focus {
            epoch: 1,
            caret: Some(ScreenRect {
                left: 10,
                top: 10,
                right: 11,
                bottom: 30,
            }),
        },
        deadline(),
    )?;
    let Reply::Key(state) = a.request(
        1,
        Command::Key {
            key: Key::Character('u'),
            epoch: 1,
        },
        deadline(),
    )?
    else {
        panic!("需要按键回复");
    };
    assert!(state.commit.is_none());
    assert_eq!(
        b.request(
            1,
            Command::Key {
                key: Key::Space,
                epoch: 1
            },
            deadline()
        )?,
        Reply::Rejected(Rejection::NotFocused)
    );
    assert!(b.poll_notification()?.is_none());
    let Message::Notification {
        token,
        reset: false,
    } = a.notification(deadline())?
    else {
        panic!("需要候选通知");
    };
    assert_eq!(token.host, a.host);
    assert_eq!(token.revision, state.state.revision);
    let Reply::Key(outcome) =
        a.request(1, Command::TakeSelection { epoch: token.focus }, deadline())?
    else {
        panic!("需要提交结果");
    };
    assert_eq!(outcome.commit.as_deref(), Some("的"));
    // 单次提交已验证；关闭通道不会重连或重放，固定工作线程由测试 UI 结束通知停止。
    a.stop_backend(deadline())?;
    b.cancel();
    drop(a);
    drop(b);
    runtime.shutdown(deadline())?;
    assert_eq!(
        // SAFETY: 等待自己创建的测试进程，失败时 Worker 析构负责终止与回收。
        unsafe { WaitForSingleObject(worker.0.as_raw_handle(), 5000) },
        WAIT_OBJECT_0
    );
    assert!(worker.0.wait()?.success());
    reader.join().unwrap();
    Ok(())
}
#[test]
#[ignore = "只由限时父测试启动，使用合成词表和合成 UI 点击"]
fn backend_worker() -> Result<()> {
    let namespace = std::env::var("TIGERRIME_BACKEND_TEST")?;
    let data = dictionary();
    let server = Server::new(&data, &namespace, Options::default())?;
    let (frames, received) = tigerrime_ui::candidate::mailbox()?;
    let (choices, clicks) = tigerrime_ui::mailbox()?;
    let ui = std::thread::spawn(move || {
        let mut clicked = false;
        while let Ok(frame) = received.receive() {
            if let Some(frame) = frame
                && !clicked
                && !frame.state.candidates.is_empty()
            {
                choices
                    .send(CandidateChoice {
                        token: frame.token,
                        index: 0,
                    })
                    .unwrap();
                clicked = true;
            }
        }
    });
    println!("BACKEND_READY");
    std::io::stdout().flush()?;
    let result = server.run(frames, clicks);
    ui.join().unwrap();
    result
}
