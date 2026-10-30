//! 受管目录与安全文件访问边界；目录租约、链接拒绝和事务替换不可用普通 fs 调用代替。
//! 仅支持 Windows/macOS；不提供绕过安全约束的普通 fs 回退。
#[cfg(windows)]
mod windows;
#[cfg(all(test, windows))]
pub(crate) use windows::write_new;
#[cfg(windows)]
pub use windows::{Data, plain, read_file, reject_links};
#[cfg(target_os = "macos")]
mod macos;
#[cfg(all(test, target_os = "macos"))]
pub(crate) use macos::write_new;
#[cfg(target_os = "macos")]
pub use macos::{Data, plain, read_file, reject_links};
