//! 检查输入边界、隐藏面板与隔离 IMK 服务/客户端，不注册、启用或切换系统输入法。
#[cfg(target_os = "macos")]
#[path = "../src/lib.rs"]
// 主线程 harness 只编译一份原生类；普通单元测试及其辅助项仍由库测试执行。
#[allow(dead_code, unused_imports)]
mod input_method;
#[cfg(target_os = "macos")]
pub use input_method::*;
fn main() {
    #[cfg(target_os = "macos")]
    appkit::tests::run();
    #[cfg(not(target_os = "macos"))]
    eprintln!("Mac 输入/面板原生检查未运行（非 macOS 宿主），不计为 passed。");
}
