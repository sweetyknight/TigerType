//! macOS 查字/设置视图及草稿交互，不持有词表或受管目录。
//! 由 macOS app 在主线程创建并接固定工作线程；开发预览另用内存合成数据。
#[cfg(target_os = "macos")]
pub mod appkit;
#[cfg(any(target_os = "macos", test))]
mod editor;
