//! 受管目录与安全文件访问边界；目录租约、链接拒绝和事务替换不可用普通 fs 调用代替。
//! 目前仅实现 Windows，Unix 原语随 M1 实施，不提供绕过安全约束的回退。
mod windows;
#[cfg(test)]
pub(crate) use windows::write_new;
pub use windows::{Data, plain, read_file, reject_links};
