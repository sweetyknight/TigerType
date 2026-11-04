//! 真实 AppKit 检查使用同一源文件，独立测试进程主线程创建隐藏窗口；Windows 明确不计为通过。
#[cfg(target_os = "macos")]
#[path = "../src/appkit.rs"]
mod appkit;
fn main() {
    #[cfg(target_os = "macos")]
    appkit::tests::run();
    #[cfg(not(target_os = "macos"))]
    eprintln!("AppKit 原生检查未运行（非 macOS 宿主），不计为 passed。");
}
