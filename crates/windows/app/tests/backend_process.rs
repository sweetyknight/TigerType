//! 后台在独立限时进程运行；合成词表和合成宿主，不创建窗口、不注册输入法。
#![cfg(windows)]
use std::{
    io::{BufRead, BufReader, Write},
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command as ProcessCommand, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use tigertype_backend::{Data, Dictionary, Entry};
use tigertype_contracts::{
    Key, Options,
    protocol::{Command, Panel, Prepare, Rejection, Reply},
};
use tigertype_windows_app::{Result, server::Server};
use tigertype_windows_ipc::{
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
/// 单 CPU 竞争验收子进程的标记；只有它按生产 50ms 计每键期限。
const PRESSURE_WORKER: &str = "TIGERTYPE_CPU_PRESSURE_WORKER";
/// 常规门禁与其他测试并行，单次调度抖动不代表输入路径退化，只用握手期限并断言 P95。
fn input_deadline() -> Instant {
    if std::env::var_os(PRESSURE_WORKER).is_some() {
        Instant::now() + Duration::from_millis(50)
    } else {
        deadline()
    }
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
fn input(client: &mut Client, epoch: u64, prepare: Prepare, key: Key) -> Result<Reply> {
    Ok(client.request(
        1,
        Command::Key {
            key,
            epoch,
            prepare,
        },
        deadline(),
    )?)
}
fn key(client: &mut Client, epoch: u64, key: Key) -> Result<Reply> {
    input(client, epoch, Prepare::Keep, key)
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

    // 宿主懒连接：首次 ensure 才连接，后续复用同一连接；首键合并开会话与取得焦点。
    let mut a = Link::new(image.clone(), &namespace);
    let (client, fresh) = a.ensure(&runtime, deadline())?;
    assert!(fresh);
    assert_eq!(
        commit(input(client, 1, Prepare::Open, Key::Character('u'))?),
        None
    );
    // B 只握手、不开会话：无焦点、无组合，是可回收的空闲宿主。
    let mut b = Link::new(image.clone(), &namespace);
    let (client, fresh) = b.ensure(&runtime, deadline())?;
    assert!(fresh);
    let old_b = client.host;
    assert_eq!(
        key(client, 1, Key::Space)?,
        Reply::Rejected(Rejection::UnknownSession)
    );

    // 再连 14 个宿主占满 16 槽：后台立即回收最久未用的空闲宿主 B；A 持有焦点和组合，不被回收。
    let mut others = Vec::new();
    for _ in 0..14 {
        let mut link = Link::new(image.clone(), &namespace);
        assert!(link.ensure(&runtime, deadline())?.1);
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
        key(client, 1, Key::Space)?,
        Reply::Rejected(Rejection::UnknownSession)
    );
    assert_eq!(
        commit(input(client, 1, Prepare::Open, Key::Character('u'))?),
        None
    );
    assert_eq!(commit(key(client, 1, Key::Space)?).as_deref(), Some("的"));
    // B 重连再次占满槽位，最久未用的 others[0] 被回收；第 17 个宿主仍能连接并触发下一次回收。
    let mut extra = Link::new(image.clone(), &namespace);
    assert!(extra.ensure(&runtime, deadline())?.1);
    for link in &mut others[..2] {
        assert!(link.ensure(&runtime, deadline())?.1);
    }
    // 未被回收的连接照常开会话输入。
    let (client, fresh) = others[13].ensure(&runtime, deadline())?;
    assert!(!fresh);
    assert_eq!(
        commit(input(client, 1, Prepare::Open, Key::Character('u'))?),
        None
    );

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
    let first = Instant::now();
    assert!(
        matches!(client.request(1, Command::Key { key: Key::Character('u'), epoch: 1, prepare: tigertype_contracts::protocol::Prepare::Open }, input_deadline())?, Reply::Key(value) if value.handled && value.state.composition.text == "u" && value.commit.is_none())
    );
    eprintln!("first input including open/focus: {:?}", first.elapsed());
    assert!(
        matches!(client.request(1, Command::Key { key: Key::Backspace, epoch: 1, prepare: tigertype_contracts::protocol::Prepare::Keep }, input_deadline())?, Reply::Key(value) if value.handled && !value.state.composing && value.commit.is_none())
    );

    // 合成输入反复进入/退出组合；竞争验收按生产 50ms 绝对期限，常规门禁只约束 P95。
    let mut samples = Vec::with_capacity(2_000);
    for round in 0..1_000 {
        let started = Instant::now();
        assert!(matches!(
            client.request(
                1,
                Command::Key {
                    key: Key::Character('u'),
                    epoch: 1,
                    prepare: tigertype_contracts::protocol::Prepare::Keep
                },
                input_deadline()
            ).map_err(|error| format!("pressure round={round} phase=1 elapsed={:?}: {error}", started.elapsed()))?,
            Reply::Key(value) if value.handled && value.state.composition.text == "u" && value.commit.is_none()
        ));
        assert!(matches!(
            client.request(
                1,
                Command::Key {
                    key: Key::Backspace,
                    epoch: 1,
                    prepare: tigertype_contracts::protocol::Prepare::Keep
                },
                input_deadline()
            ).map_err(|error| format!("pressure round={round} phase=2 elapsed={:?}: {error}", started.elapsed()))?,
            Reply::Key(value) if value.handled && !value.state.composing && value.commit.is_none()
        ));
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
#[ignore = "单列 CPU 竞争验收；只约束独立测试子进程及其后台，不改用户进程优先级或亲和性"]
fn backend_input_under_single_cpu_contention() -> Result<()> {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessAffinityMask, SetProcessAffinityMask,
    };
    if std::env::var_os(PRESSURE_WORKER).is_some() {
        let (mut allowed, mut system) = (0, 0);
        // SAFETY: 当前进程是本测试独立子进程；后台子进程继承同一 CPU 亲和性，不影响现用后台。
        unsafe {
            if GetProcessAffinityMask(GetCurrentProcess(), &mut allowed, &mut system) == 0
                || allowed == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            let one = 1usize << allowed.trailing_zeros();
            if SetProcessAffinityMask(GetCurrentProcess(), one) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        let stop = std::sync::atomic::AtomicBool::new(false);
        return std::thread::scope(|scope| {
            let (started, running) = mpsc::sync_channel(1);
            let stopping = &stop;
            let load = scope.spawn(move || {
                let end = Instant::now() + Duration::from_secs(3);
                let mut value = 1u64;
                started.send(()).unwrap();
                // 只在合成压测中制造有界 CPU 竞争；正式输入路径不轮询。
                while Instant::now() < end && !stopping.load(std::sync::atomic::Ordering::Relaxed) {
                    for _ in 0..1_000 {
                        value = std::hint::black_box(
                            value.wrapping_mul(6364136223846793005).wrapping_add(1),
                        );
                    }
                }
            });
            running.recv_timeout(Duration::from_secs(2)).unwrap();
            let result = backend_key_round_trip_pressure_stays_within_input_budget();
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            load.join().unwrap();
            result
        });
    }
    let mut child = Worker(
        ProcessCommand::new(std::env::current_exe()?)
            .args([
                "--ignored",
                "--exact",
                "backend_input_under_single_cpu_contention",
                "--nocapture",
            ])
            .env(PRESSURE_WORKER, "1")
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?,
    );
    // SAFETY: 仅等待自己启动的测试进程；超时由 Worker 终止并回收。
    if unsafe { WaitForSingleObject(child.0.as_raw_handle(), 15_000) } != WAIT_OBJECT_0 {
        return Err("CPU 竞争验收超时".into());
    }
    assert!(child.0.wait()?.success());
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
