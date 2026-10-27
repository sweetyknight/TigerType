//! 查字/设置视图：`model` 为平台共用的界面模型，`win32` 为 Windows 视图（macOS 的 appkit 视图随 M3 加入）；
//! 只消费 contracts 数据，词表与磁盘操作由 backend 控制器在 app 组装的固定工作线程上负责。
#[cfg(windows)]
mod mailbox;
mod model;
#[cfg(windows)]
mod win32;
#[cfg(windows)]
pub use mailbox::{Inbox, Latest, ReplySender, mailbox};
#[cfg(windows)]
pub use win32::{ico_file as icon_file, run};
