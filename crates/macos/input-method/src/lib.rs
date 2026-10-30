//! macOS 前端边界：按键、组合位置与展示组件；不持有引擎或读写设置。
//! IMK 控制器由 macOS app 注入进程内通道；纯逻辑在 Windows 测试，原生执行须在 Mac 验收。
mod keyboard;
mod presentation;
mod text;
pub use keyboard::{KeyAction, Keyboard};
pub use presentation::{Action, Location, MenuAction, NativeRect, Presentation, Screen, Selection};
pub use text::marked_selection;
#[cfg(target_os = "macos")]
pub mod appkit;
