//! Windows 产品入口；开发工具与其他平台不通过本程序启动。
//! GUI 子系统：随登录启动的后台与开始菜单唤起的查字窗都不弹控制台。
#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
fn main() -> std::process::ExitCode {
    tigertype_windows_app::main()
}

// Mac 的整工作区检查也包含此入口；误运行时明确失败，不调用已排除的 Windows 库。
#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("Windows 输入法只能在 Windows 上运行");
    std::process::ExitCode::FAILURE
}
