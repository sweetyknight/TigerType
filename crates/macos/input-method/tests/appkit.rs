//! 复用生产源文件检查输入边界、隐藏面板与原生事件，不启用 IMKServer 或修改系统输入法。
#[cfg(target_os = "macos")]
pub use tigertype_macos_input_method::{
    Action, KeyAction, Keyboard, Location, MenuAction, NativeRect, Presentation, Screen, Selection,
    marked_selection,
};
#[cfg(target_os = "macos")]
#[path = "../src/appkit.rs"]
mod appkit;
fn main() {
    #[cfg(target_os = "macos")]
    appkit::tests::run();
    #[cfg(not(target_os = "macos"))]
    eprintln!("Mac 输入/面板原生检查未运行（非 macOS 宿主），不计为 passed。");
}
