//! 真实 DLL 只能进入有期限的子进程，测试崩溃/卡死不会拖住 Cargo。

#![cfg(all(windows, target_arch = "x86_64"))]

use std::{
    fs,
    path::Path,
    process::{Command, ExitStatus},
    time::Duration,
};
use tigerrime_app::{Result, probe, process, project_root, resources};

fn run_child(
    command: &mut Command,
    work: &Path,
    label: &str,
    timeout: Duration,
) -> Result<ExitStatus> {
    command
        .stdout(fs::File::create(work.join(format!("{label}.stdout.log")))?)
        .stderr(fs::File::create(work.join(format!("{label}.stderr.log")))?);
    process::run(command, timeout)
}

#[test]
#[ignore = "需要锁定的 vendor 资源；命令见 README"]
fn native_engine_outputs_without_errors() -> Result<()> {
    let root = project_root()?;
    let work = probe::new_workspace(&root)?;
    let paths = probe::runtime_paths(&root, &work)?;
    assert!(
        !paths.user.starts_with(&root),
        "活动数据库不能在源码工作区内"
    );
    assert!(
        // SAFETY: 仅查询已加载模块，不加载 DLL；资源准备必须保持父进程无引擎。
        unsafe {
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"rime.dll".as_ptr().cast())
        }
        .is_null()
    );
    eprintln!("Rust 原生测试目录: {}", work.display());
    eprintln!("隔离运行数据: {}", paths.user.display());
    let binary = env!("CARGO_BIN_EXE_tigerrime");
    let deploy = run_child(
        Command::new(binary)
            .arg("probe-deploy")
            .arg(&work)
            .current_dir(&paths.user),
        &work,
        "deploy",
        Duration::from_secs(300),
    )?;
    assert!(deploy.success(), "部署失败，请检查 {}", work.display());
    let status = run_child(
        Command::new(binary)
            .arg("probe-engine")
            .arg(&work)
            .current_dir(&paths.user),
        &work,
        "engine",
        Duration::from_secs(120),
    )?;
    let report: serde_json::Value = resources::read_json(&work.join("engine.json"))?;
    assert_eq!(report["cases"].as_array().unwrap().len(), 40);
    assert_eq!(report["failed_cases"], serde_json::json!([]));
    assert_eq!(report["session_cycles"], 1000);
    assert_eq!(report["commit_once"], true);
    assert_eq!(report["ascii_passthrough"], true);
    assert_eq!(report["calculator_recovery"], true);
    assert_eq!(report["protocol_dispatch"], true);
    assert_eq!(report["protocol_client_delivery"], true);
    // 已知缺陷修复后不再保留白名单；任何原生错误都必须使验收失败。
    assert!(status.success(), "引擎子进程失败: {report}");
    assert_eq!(report["native_errors"], serde_json::json!([]));
    eprintln!(
        "40 个输出断言及会话/提交/计算器恢复/协议调度/分片交付测试通过，原生诊断为零。{}",
        report["latency_ms"]
    );
    Ok(())
}

#[test]
fn timeout_kills_and_reaps_child() -> Result<()> {
    let work = resources::new_directory(&std::env::temp_dir(), "tigerrime-timeout")?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["--ignored", "--exact", "timeout_worker"])
        .env("TIGERRIME_TIMEOUT_WORKER", "1");
    assert!(run_child(&mut command, &work, "timeout", Duration::from_millis(50)).is_err());
    Ok(())
}

#[test]
fn child_failure_does_not_hang_parent() -> Result<()> {
    let work = resources::new_directory(&std::env::temp_dir(), "tigerrime-failure")?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_tigerrime"));
    command.arg("invalid-command");
    assert!(!run_child(&mut command, &work, "failure", Duration::from_secs(10))?.success());
    Ok(())
}

#[test]
#[ignore = "只供超时父测试调用"]
fn timeout_worker() {
    // 手工运行所有 ignored 测试时不意外等待一分钟，只有父测试能启用故障。
    if std::env::var_os("TIGERRIME_TIMEOUT_WORKER").is_some() {
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[test]
#[ignore = "需要已锁定下载缓存与 7-Zip；不访问网络"]
fn cold_resource_preparation_from_locked_cache() -> Result<()> {
    let root = project_root()?;
    let work = resources::new_directory(&root.join("artifacts"), "rust-resources")?;
    fs::create_dir(work.join("resources"))?;
    fs::copy(
        root.join("resources/tiger.lock.json"),
        work.join("resources/tiger.lock.json"),
    )?;
    fs::copy(
        root.join("dependencies.lock.json"),
        work.join("dependencies.lock.json"),
    )?;
    let lock: resources::Lock = resources::read_json(&root.join("dependencies.lock.json"))?;
    resources::copy_entries(
        &root.join(".cache/downloads"),
        &work.join(".cache/downloads"),
        &lock.librime.artifacts,
    )?;
    for (name, group) in [("prelude", lock.prelude), ("essay", lock.essay)] {
        resources::copy_entries(
            &root.join(".cache/downloads").join(name),
            &work.join(".cache/downloads").join(name),
            &group.files,
        )?;
    }
    resources::prepare(&work, &root.join("vendor/tiger"))?;
    resources::verify(&work)?;
    eprintln!("Rust 冷启动资源准备通过: {}", work.display());
    Ok(())
}

#[test]
#[ignore = "需要锁定资源；真实引擎管道及受控故障测试"]
fn native_engine_pipe_faults_and_recovery() -> Result<()> {
    use std::time::Instant;
    use tigerrime_app::pipe_engine::{EngineProcess, Fault};
    use tigerrime_contracts::protocol::{Command as Input, Reply};
    use tigerrime_ipc::{Error as IpcError, pipe::Runtime};

    let deadline = || Instant::now() + Duration::from_secs(5);
    let key = |key: char| Input::Key {
        keycode: key as i32,
        modifiers: 0,
    };
    let root = project_root()?;
    let work = probe::new_workspace(&root)?;
    let paths = probe::runtime_paths(&root, &work)?;
    let binary = Path::new(env!("CARGO_BIN_EXE_tigerrime"));
    eprintln!("真实管道引擎报告: {}", work.display());
    assert!(
        run_child(
            Command::new(binary)
                .arg("probe-deploy")
                .arg(&work)
                .current_dir(&paths.user),
            &work,
            "deploy",
            Duration::from_secs(300)
        )?
        .success()
    );
    let runtime = Runtime::new()?;
    let mut summary = serde_json::json!({"scope": "actual isolated librime over named pipe; no TSF/UI",
        "normal": false, "faults": [], "recovery": false});
    let report = work.join("pipe-regression.json");
    fs::write(&report, serde_json::to_vec_pretty(&summary)?)?;

    let mut worker = EngineProcess::start(&runtime, binary, &root, &work, 1, Fault::None)?;
    for (id, schema) in [(1, "tiger"), (2, "tigress")] {
        assert!(
            matches!(worker.request(id, Input::Open { schema: schema.into() }, deadline())?.reply,
            Reply::Opened(state) if state.schema_id == schema)
        );
    }
    assert!(matches!(worker.request(1, key('u'), deadline())?.reply,
        Reply::Key(outcome) if outcome.commit.is_none() && outcome.state.candidates.first().is_some_and(|candidate| candidate.text == "的")));
    assert!(matches!(worker.request(2, key('d'), deadline())?.reply,
        Reply::Key(outcome) if outcome.handled && outcome.commit.is_none() && !outcome.state.composition.text.is_empty()));
    assert!(
        matches!(worker.request(1, key(' '), deadline())?.reply, Reply::Key(outcome) if outcome.commit.as_deref() == Some("的"))
    );
    assert!(
        matches!(worker.request(1, key(' '), deadline())?.reply, Reply::Key(outcome) if outcome.commit.is_none())
    );
    assert!(
        matches!(worker.request(2, Input::Clear, deadline())?.reply, Reply::Cleared(state) if state.composition.text.is_empty())
    );
    for id in [1, 2] {
        assert_eq!(
            worker.request(id, Input::Close, deadline())?.reply,
            Reply::Closed
        );
    }
    for (id, schema) in [(3, "tiger"), (4, "tigress"), (5, "PY_c")] {
        worker.request(
            id,
            Input::Open {
                schema: schema.into(),
            },
            deadline(),
        )?;
        for input in "=sin()".chars() {
            assert!(
                matches!(worker.request(id, key(input), deadline())?.reply, Reply::Key(outcome) if outcome.commit.is_none())
            );
        }
        worker.request(id, Input::Clear, deadline())?;
        for input in "=1+".chars() {
            worker.request(id, key(input), deadline())?;
        }
        assert!(matches!(worker.request(id, key('2'), deadline())?.reply,
            Reply::Key(outcome) if outcome.state.candidates.iter().any(|candidate| candidate.text == "1+2=3")));
        worker.request(id, Input::Close, deadline())?;
    }
    assert_eq!(worker.finish(deadline())?, 0);
    let normal: serde_json::Value = resources::read_json(&work.join("pipe-1.json"))?;
    assert_eq!(normal["native_errors"], serde_json::json!([]));
    assert!(normal["error"].is_null());
    summary["normal"] = true.into();
    fs::write(&report, serde_json::to_vec_pretty(&summary)?)?;

    for (generation, fault) in [(2, Fault::StallAfterCommit), (3, Fault::ExitAfterCommit)] {
        let mut worker = EngineProcess::start(&runtime, binary, &root, &work, generation, fault)?;
        worker.request(
            1,
            Input::Open {
                schema: "tiger".into(),
            },
            deadline(),
        )?;
        worker.request(1, key('u'), deadline())?;
        let error = worker
            .request(1, key(' '), Instant::now() + Duration::from_millis(100))
            .unwrap_err();
        // 事件证明引擎已产生提交；没有命中注入点的普通故障不能冒充预期回归成功。
        worker.wait_fault_point(deadline())?;
        if fault == Fault::StallAfterCommit {
            assert!(matches!(
                error.downcast_ref::<IpcError>(),
                Some(IpcError::TimedOut)
            ));
        }
        if fault == Fault::ExitAfterCommit {
            assert_eq!(worker.wait_exit(deadline())?, 23);
        }
        let closed = worker.request(1, key('u'), deadline()).unwrap_err();
        assert!(matches!(
            closed.downcast_ref::<IpcError>(),
            Some(IpcError::Closed)
        ));
        let code = worker.terminate(deadline())?;
        assert_eq!(
            code,
            if fault == Fault::ExitAfterCommit {
                23
            } else {
                1
            }
        );
        summary["faults"].as_array_mut().unwrap().push(serde_json::json!({"generation": generation,
            "mode": format!("{fault:?}"), "fault_point_observed": true, "request_error": error.to_string(),
            "exit_code": code, "reply_delivered": false, "follow_up_rejected": true}));
        fs::write(&report, serde_json::to_vec_pretty(&summary)?)?;
    }

    // 使用新的进程和代次重开同一份合成数据库，不重放上一代状态不确定的请求。
    let mut worker = EngineProcess::start(&runtime, binary, &root, &work, 4, Fault::None)?;
    worker.request(
        1,
        Input::Open {
            schema: "tiger".into(),
        },
        deadline(),
    )?;
    worker.request(1, key('u'), deadline())?;
    assert!(
        matches!(worker.request(1, key(' '), deadline())?.reply, Reply::Key(outcome) if outcome.commit.as_deref() == Some("的"))
    );
    worker.request(1, Input::Close, deadline())?;
    assert_eq!(worker.finish(deadline())?, 0);
    let recovery: serde_json::Value = resources::read_json(&work.join("pipe-4.json"))?;
    assert_eq!(recovery["native_errors"], serde_json::json!([]));
    assert!(recovery["error"].is_null());
    summary["recovery"] = true.into();
    fs::write(&report, serde_json::to_vec_pretty(&summary)?)?;
    // 父进程只持有管道和 Job，任何阶段都不得把 librime 加载到父测试中。
    assert!(
        // SAFETY: 仅查询模块是否已经存在，不加载引擎。
        unsafe {
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"rime.dll".as_ptr().cast())
        }
        .is_null()
    );
    eprintln!(
        "真实管道、双会话、Lua 恢复、提交后卡死/退出及新代次恢复通过: {}",
        report.display()
    );
    Ok(())
}
