//! 一次本地验收入口：固定命令、失败即停，不下载或隐式放宽测试。
use std::{process::Command, time::Duration};
use tigertype_dev::{Result, check, process, project_root};
fn run() -> Result<()> {
    let root = project_root()?;
    for args in check::commands(std::env::consts::OS) {
        println!("\n== cargo {} ==", args.join(" "));
        let status = process::run(
            Command::new("cargo").args(&args).current_dir(&root),
            Duration::from_secs(600),
        )?;
        if !status.success() {
            return Err(format!("检查失败：cargo {} ({status})", args.join(" ")).into());
        }
    }
    println!(
        "本机常规门禁通过；异平台仅执行命令矩阵所列的类型检查，不计原生测试。真实应用、完整资源与长期验收另列。"
    );
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
