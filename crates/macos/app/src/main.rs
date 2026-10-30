//! 仅 Mac 产品入口；Windows 执行失败，不伪装为已能运行的输入法。
fn main() -> std::process::ExitCode {
    #[cfg(target_os = "macos")]
    let result = tigertype_macos_app::native::main();
    #[cfg(not(target_os = "macos"))]
    let result: Result<(), String> = Err("Mac 输入法只能在 macOS 上运行".into());
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
