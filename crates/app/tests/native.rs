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
fn native_engine_outputs_and_known_lua_failure() -> Result<()> {
    let root = project_root()?;
    let work = probe::new_workspace(&root)?;
    assert!(
        // SAFETY: 仅查询已加载模块，不加载 DLL；资源准备必须保持父进程无引擎。
        unsafe {
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"rime.dll".as_ptr().cast())
        }
        .is_null()
    );
    eprintln!("Rust 原生测试目录: {}", work.display());
    let binary = env!("CARGO_BIN_EXE_tigerrime");
    let deploy = run_child(
        Command::new(binary)
            .arg("probe-deploy")
            .arg(&work)
            .current_dir(work.join("user")),
        &work,
        "deploy",
        Duration::from_secs(300),
    )?;
    assert!(deploy.success(), "部署失败，请检查 {}", work.display());
    let status = run_child(
        Command::new(binary)
            .arg("probe-engine")
            .arg(&work)
            .current_dir(work.join("user")),
        &work,
        "engine",
        Duration::from_secs(120),
    )?;
    let report: serde_json::Value = resources::read_json(&work.join("engine.json"))?;
    assert_eq!(report["cases"].as_array().unwrap().len(), 20);
    assert_eq!(report["failed_cases"], serde_json::json!([]));
    assert_eq!(report["session_cycles"], 100);
    assert_eq!(report["commit_once"], true);
    assert_eq!(report["ascii_passthrough"], true);
    // 这是已知缺陷回归，不是干净诊断验收：必须准确复现，不能隐藏或忽略其他错误。
    assert_eq!(status.code(), Some(1), "原计算器应产生诊断失败");
    let errors = report["native_errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "出现新的原生错误: {errors:?}");
    let error = errors[0].as_str().unwrap();
    assert!(
        error.contains("LuaTranslation::Next error(2)")
            && error.contains("calculator_translator.lua:400: attempt to call a nil value")
    );
    eprintln!(
        "20 个输出断言及会话/提交测试通过；原计算器错误仍存在。{}",
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
