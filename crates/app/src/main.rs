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
