//! 一次本地验收入口：固定命令、失败即停，不隐式下载资源或放宽测试。
use std::{process::Command, time::Duration};
use tigertype_dev::{Result, process, project_root};

fn run() -> Result<()> {
    let root = project_root()?;
    let commands: &[&[&str]] = &[
        &[
            "test",
            "--locked",
            "--offline",
            "-p",
            "tigertype-dev",
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
        // Windows 不能重链接正在运行的 check.exe；显式测试目标避免 Cargo 顺带重建此示例。
        // 示例仍由 all-targets Clippy 检查，DLL 探针另由集成测试显式构建和执行。
        &[
            "test",
            "--workspace",
            "--locked",
            "--offline",
            "--lib",
            "--bins",
            "--tests",
        ],
        &["test", "--workspace", "--locked", "--offline", "--doc"],
        &[
            "test",
            "--workspace",
            "--locked",
            "--offline",
            "--lib",
            "--bins",
            "--tests",
            "--target",
            "i686-pc-windows-msvc",
        ],
        &[
            "test",
            "--workspace",
            "--locked",
            "--offline",
            "--doc",
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
    // M1 暂缓：只检查已有 Mac 视图和合成预览，不把尚不存在的 Mac 后台当作可构建产品。
    for target in ["aarch64-apple-darwin", "x86_64-apple-darwin"] {
        for selection in [
            vec![
                "-p",
                "tigertype-macos-input-method",
                "-p",
                "tigertype-macos-ui",
                "--all-targets",
            ],
            vec!["-p", "tigertype-dev", "--example", "macos_views"],
        ] {
            let mut args = vec!["clippy", "--locked", "--offline"];
            args.extend(selection);
            args.extend(["--target", target, "--", "-D", "warnings"]);
            println!("\n== cargo {} ==", args.join(" "));
            let status = process::run(
                Command::new("cargo").args(&args).current_dir(&root),
                Duration::from_secs(600),
            )?;
            if !status.success() {
                return Err(
                    format!("Mac 视图类型检查失败：cargo {} ({status})", args.join(" ")).into(),
                );
            }
        }
    }
    println!(
        "本地常规门禁和 Apple 视图类型检查通过；Mac 原生测试未在 Windows 执行。完整资源、真实应用与长期验收另列。"
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
