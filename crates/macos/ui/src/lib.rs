//! macOS 查字/设置视图及草稿交互，不持有词表或受管目录。
//! 由组装层在主线程创建；当前只接合成预览，真实工作线程接线留给 M1。
#[cfg(target_os = "macos")]
pub mod appkit;
#[cfg(any(target_os = "macos", test))]
mod editor;
