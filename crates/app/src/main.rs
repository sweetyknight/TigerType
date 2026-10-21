//! 开发期诊断命令；未实现的安装/常驻模式不暴露空壳入口。

use tigerrime_app::{Result, probe, project_root, resources};

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() || args == ["--help"] {
        println!(
            "TigerRime 开发工具\n  prepare-resources <虎码原目录>\n  verify-resources\n  probe-deploy <隔离测试目录>\n  probe-engine <隔离测试目录>\n不注册输入法，不探测 APPDATA，不需要 Python。"
        );
        return Ok(());
    }
    let root = project_root()?;
    match (args[0].to_str(), args.len()) {
        (Some("prepare-resources"), 2) => {
            resources::prepare(&root, std::path::Path::new(&args[1]))?
        }
        (Some("verify-resources"), 1) => resources::verify(&root)?,
        (Some("probe-deploy"), 2) => probe::run(&root, std::path::Path::new(&args[1]), true)?,
        (Some("probe-engine"), 2) => probe::run(&root, std::path::Path::new(&args[1]), false)?,
        #[cfg(windows)]
        (Some("probe-idle-worker"), 3) => {
            // SAFETY: 专用子进程入口只执行一次，启动句柄尚未被任何 Rust 所有者接管。
            unsafe {
                tigerrime_app::job::probe_idle(
                    args[1].to_str().ok_or("父进程句柄格式不符")?.parse()?,
                    args[2].to_str().ok_or("事件句柄格式不符")?.parse()?,
                )?;
            }
        }
        #[cfg(windows)]
        (Some("probe-engine-pipe"), 7) => {
            // SAFETY: 同上；子模式在创建事件/进程所有者之前由 Bootstrap 校验并接管句柄。
            unsafe {
                tigerrime_app::pipe_engine::run(
                    &root,
                    std::path::Path::new(&args[1]),
                    args[2].to_str().ok_or("管道标签格式不符")?,
                    args[3].to_str().ok_or("代次格式不符")?.parse()?,
                    tigerrime_app::pipe_engine::Fault::parse(
                        args[4].to_str().ok_or("故障模式格式不符")?,
                    )?,
                    args[5].to_str().ok_or("父进程句柄格式不符")?.parse()?,
                    args[6].to_str().ok_or("事件句柄格式不符")?.parse()?,
                )?;
            }
        }
        _ => return Err("无效参数；请运行 --help".into()),
    }
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
