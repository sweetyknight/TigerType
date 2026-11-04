//! Linux 主入口不安装或自动切换输入源；原生初始化失败须给出非零退出码。
fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    match tigertype_linux_app::native::main() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("此程序只在 Linux 运行");
        std::process::ExitCode::FAILURE
    }
}
