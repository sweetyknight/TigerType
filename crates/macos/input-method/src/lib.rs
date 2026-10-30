//! macOS 前端的展示组件：只消费快照、输出带身份的用户动作，不持有引擎或读写设置。
//! IMK 控制器、文本上屏和进程组装属于暂缓的 M1；这里不提供假控制器或备用输入通道。
mod presentation;
pub use presentation::{Action, Location, MenuAction, NativeRect, Presentation, Screen, Selection};
#[cfg(target_os = "macos")]
pub mod appkit;
