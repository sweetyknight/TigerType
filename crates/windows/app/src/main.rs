//! Windows 产品入口；开发工具与其他平台不通过本程序启动。
//! GUI 子系统：随登录启动的后台与开始菜单唤起的查字窗都不弹控制台。
#![windows_subsystem = "windows"]
fn main() -> std::process::ExitCode {
    tigertype_windows_app::main()
}
