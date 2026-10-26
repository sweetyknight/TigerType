//! 后台在独立限时进程运行；合成词表和合成宿主，不创建窗口、不注册输入法。
#![cfg(windows)]
use std::{
    io::{BufRead, BufReader, Write},
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command as ProcessCommand, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use tigertype_app::{Result, backend::Server};
use tigertype_contracts::{
    Key, Options,
    protocol::{Command, Panel, Rejection, Reply},
};
use tigertype_engine::{Data, Dictionary, Entry};
use tigertype_ipc::{
    pipe::Runtime,
    service::{Client, Link},
};
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
fn open(client: &mut Client) -> Result<()> {
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
    Ok(())
}
fn key(client: &mut Client, epoch: u64, key: Key) -> Result<Reply> {
    Ok(client.request(1, Command::Key { key, epoch }, deadline())?)
}
fn focus(client: &mut Client, epoch: u64) -> Result<()> {
    assert!(matches!(
        client.request(
            1,
            Command::Focus {
                epoch,
                focused: true
            },
            deadline()
        )?,
        Reply::Focused(_)
    ));
    Ok(())
}
fn commit(reply: Reply) -> Option<String> {
    let Reply::Key(outcome) = reply else {
        panic!("需要按键回复：{reply:?}");
    };
    outcome.commit
}

#[test]
fn real_backend_singleton_image_check_reclaims_idle_hosts_and_reconnects_without_losing_keys()
-> Result<()> {
    let namespace = format!("service-test-{}", std::process::id());
    let image = std::env::current_exe()?;
    let mut worker = Worker(
        ProcessCommand::new(&image)
            .args(["--ignored", "--exact", "backend_worker", "--nocapture"])
            .env("TIGERTYPE_BACKEND_TEST", &namespace)
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

    // 宿主懒连接：首次 ensure 才连接，后续复用同一连接。
    let mut a = Link::new(image.clone(), &namespace);
    let (client, fresh) = a.ensure(&runtime, deadline())?;
    assert!(fresh);
    open(client)?;
    focus(client, 1)?;
    assert_eq!(commit(key(client, 1, Key::Character('u'))?), None);
    let mut b = Link::new(image.clone(), &namespace);
    let (client, fresh) = b.ensure(&runtime, deadline())?;
    assert!(fresh);
    let old_b = client.host;
    open(client)?;
    assert_eq!(
        key(client, 1, Key::Space)?,
        Reply::Rejected(Rejection::NotFocused)
    );

    // 再连 14 个宿主占满 16 槽：后台立即回收最久未用的空闲宿主 B；A 持有焦点和组合，不被回收。
    let mut others = Vec::new();
    for _ in 0..14 {
        let mut link = Link::new(image.clone(), &namespace);
        let (client, fresh) = link.ensure(&runtime, deadline())?;
        assert!(fresh);
        open(client)?;
        others.push(link);
    }
    let (client, fresh) = a.ensure(&runtime, deadline())?;
    assert!(!fresh);
    assert_eq!(commit(key(client, 1, Key::Space)?).as_deref(), Some("的"));

    // B 发送前发现已断开，同一期限内只重连一次；新宿主须重新打开会话，按键不丢、旧请求不重放。
    let (client, fresh) = b.ensure(&runtime, deadline())?;
    assert!(fresh);
    assert_ne!(client.host, old_b);
    assert_eq!(
        client.request(1, Command::Clear, deadline())?,
        Reply::Rejected(Rejection::UnknownSession)
    );
    open(client)?;
    focus(client, 1)?;
    assert_eq!(commit(key(client, 1, Key::Character('u'))?), None);
    assert_eq!(commit(key(client, 1, Key::Space)?).as_deref(), Some("的"));
    // B 重连再次占满槽位，最久未用的 others[0] 被回收；第 17 个宿主仍能连接并触发下一次回收。
    let mut extra = Link::new(image.clone(), &namespace);
    assert!(extra.ensure(&runtime, deadline())?.1);
    for link in &mut others[..2] {
        let (client, fresh) = link.ensure(&runtime, deadline())?;
        assert!(fresh);
        open(client)?;
    }
    let (client, fresh) = others[13].ensure(&runtime, deadline())?;
    assert!(!fresh);
    assert!(matches!(
        client.request(1, Command::Clear, deadline())?,
        Reply::Cleared(_)
    ));

    // 反例：未接查字 UI 的后台拒绝唤起请求，该连接作废，后台继续服务。
    assert!(
        Client::connect(&runtime, &image, &namespace, deadline())?
            .show(Panel::Settings, deadline())
            .is_err()
    );
    Client::connect(&runtime, &image, &namespace, deadline())?.stop_backend(deadline())?;
    // stop 返回即后台已退出：安装程序紧接着检测文件占用并启动新后台（旧实现一收到确认就返回）。
    assert!(worker.0.try_wait()?.is_some(), "stop 返回时后台须已退出");
    drop((a, b, others, extra));
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
fn backend_key_round_trip_pressure_stays_within_input_budget() -> Result<()> {
    let namespace = format!("service-pressure-{}", std::process::id());
    let image = std::env::current_exe()?;
    let mut worker = Worker(
        ProcessCommand::new(&image)
            .args(["--ignored", "--exact", "backend_worker", "--nocapture"])
            .env("TIGERTYPE_BACKEND_TEST", &namespace)
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
    let runtime = Runtime::new()?;
    let mut link = Link::new(image.clone(), &namespace);
    let (client, fresh) = link.ensure(&runtime, deadline())?;
    assert!(fresh);
    open(client)?;
    focus(client, 1)?;

    // 合成输入反复进入/退出组合，覆盖候选状态创建、清空与 JSON/管道往返，不记录按键内容。
    let mut samples = Vec::with_capacity(2_000);
    for _ in 0..1_000 {
        let started = Instant::now();
        assert!(matches!(
            key(client, 1, Key::Character('u'))?,
            Reply::Key(_)
        ));
        assert!(matches!(key(client, 1, Key::Backspace)?, Reply::Key(_)));
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let p95 = samples[samples.len() * 95 / 100];
    eprintln!(
        "backend pressure: n={} p95={p95:?} max={:?}",
        samples.len(),
        samples.last()
    );
    assert!(
        p95 <= Duration::from_millis(10),
        "输入往返 P95 超过 10ms: {p95:?}"
    );

    Client::connect(&runtime, &image, &namespace, deadline())?.stop_backend(deadline())?;
    drop(link);
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
#[ignore = "只由限时父测试启动，使用合成词表"]
fn backend_worker() -> Result<()> {
    let namespace = std::env::var("TIGERTYPE_BACKEND_TEST")?;
    let data = dictionary();
    let server = Server::new(&data, &namespace, Options::default())?;
    println!("BACKEND_READY");
    std::io::stdout().flush()?;
    server.run()
}
