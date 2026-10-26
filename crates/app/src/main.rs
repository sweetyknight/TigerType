//! tigertype 程序入口；各平台的组装见 tigertype_app 中由 cfg(target_os) 选入的模块。
//! GUI 子系统：随登录启动的后台与开始菜单唤起的查字窗都不弹控制台。
#![windows_subsystem = "windows"]
fn main() -> std::process::ExitCode {
    tigertype_app::windows::main()
}
