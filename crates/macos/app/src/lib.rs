//! Mac 产品组装：进程内传输与固定引擎线程；纯 Rust 部分在 Windows 执行反例测试。
pub mod bundle;
pub mod channel;
#[cfg(target_os = "macos")]
pub mod fonts;
#[cfg(target_os = "macos")]
pub mod icons;
#[cfg(target_os = "macos")]
pub mod install;
#[cfg(target_os = "macos")]
pub mod native;
pub mod runtime;
