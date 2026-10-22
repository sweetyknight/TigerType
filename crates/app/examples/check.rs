//! 一次本地验收入口：固定命令、失败即停，不隐式下载资源或放宽测试。
use std::{process::Command, time::Duration};
use tigerrime_app::{Result, process, project_root};

fn run() -> Result<()> {
    let root = project_root()?;
    let commands: &[&[&str]] = &[
        &[
            "test",
            "--locked",
            "--offline",
            "-p",
            "tigerrime-app",
            "--test",
            "governance",
        ],
        &["fmt", "--all", "--", "--check"],
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--offline",
            "--",
            "-D",
            "warnings",
        ],
        &["test", "--workspace", "--locked", "--offline"],
        &[
            "test",
            "--workspace",
            "--locked",
            "--offline",
            "--target",
            "i686-pc-windows-msvc",
        ],
    ];
    for args in commands {
        println!("\n== cargo {} ==", args.join(" "));
        let status = process::run(
            Command::new("cargo").args(*args).current_dir(&root),
            Duration::from_secs(600),
        )?;
        if !status.success() {
            return Err(format!("检查失败：cargo {} ({status})", args.join(" ")).into());
        }
    }
    println!("本地常规门禁通过；完整资源、GUI/TSF、真实应用与长期验收不包含在本命令中。");
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
