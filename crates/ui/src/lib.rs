//! 原生查字/设置视图，只消费 contracts 数据；引擎和磁盘操作由 app 的固定线程负责。
#[cfg(windows)]
mod mailbox;
mod model;
#[cfg(windows)]
mod native;
#[cfg(windows)]
pub use mailbox::{Inbox, Latest, ReplySender, mailbox};
#[cfg(windows)]
pub use native::run;
