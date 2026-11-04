//! 独立安装入口不依赖 GTK 初始化；缺少界面库时仍能由包管理器补齐依赖。
fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    let result = tigertype_linux_app::setup::main();
    #[cfg(not(target_os = "linux"))]
    let result: Result<(), String> = Err("安装程序只支持 Linux".into());
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
