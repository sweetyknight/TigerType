//! Linux 输入法边界：XKB/IBus 键值转共享逻辑键；框架面板与上屏由系统协议处理。
#![forbid(unsafe_code)]
mod keyboard;
pub mod menu;
pub use keyboard::Keyboard;
#[cfg(target_os = "linux")]
pub mod fcitx;
#[cfg(target_os = "linux")]
pub mod ibus;
