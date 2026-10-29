//! 复用生产源文件的隐藏面板与原生事件检查，不启用 IMKServer 或修改系统输入法。
#[cfg(target_os = "macos")]
pub use tigertype_imk::{
    Action, Location, MenuAction, NativeRect, Presentation, Screen, Selection,
};
#[cfg(target_os = "macos")]
#[path = "../src/appkit.rs"]
mod appkit;
fn main() {
    #[cfg(target_os = "macos")]
    appkit::tests::run();
    #[cfg(not(target_os = "macos"))]
    eprintln!("IMK 面板原生检查未运行（非 macOS 宿主），不计为 passed。");
}
