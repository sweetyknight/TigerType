//! 受管目录与安全文件访问边界；目录租约、链接拒绝和事务替换不可用普通 fs 调用代替。
//! Windows 使用句柄，macOS/Linux 使用目录描述符；不提供绕过安全约束的普通 fs 回退。
#[cfg(windows)]
mod windows;
#[cfg(all(test, windows))]
pub(crate) use windows::write_new;
#[cfg(windows)]
pub use windows::{Data, plain, read_file, reject_links};
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod unix;
#[cfg(target_os = "linux")]
pub use unix::user_data_root;
#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
pub(crate) use unix::write_new;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub use unix::{Data, plain, read_file, reject_links};
