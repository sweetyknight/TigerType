//! 真实 AppKit 检查使用同一源文件，独立测试进程主线程创建隐藏窗口；Windows 明确不计为通过。
#[cfg(target_os = "macos")]
#[path = "../src/appkit.rs"]
mod appkit;
#[cfg(target_os = "macos")]
#[path = "../src/editor.rs"]
// 自定义主线程 harness 不生成 #[test] 函数，只允许其共用导入未使用；产品编译仍严格检查。
#[allow(unused_imports)]
mod editor;
#[cfg(target_os = "macos")]
#[path = "../src/layout.rs"]
mod layout;
#[cfg(target_os = "macos")]
#[path = "../src/model.rs"]
#[allow(unused_imports)]
mod model;
fn main() {
    #[cfg(target_os = "macos")]
    appkit::tests::run();
    #[cfg(not(target_os = "macos"))]
    eprintln!("AppKit 原生检查未运行（非 macOS 宿主），不计为 passed。");
}
