//! 查字/设置视图：`model` 为平台共用的界面模型，`win32` 与 `appkit` 分别负责原生视图；
//! 只消费 contracts 数据，词表与磁盘操作由 backend 控制器在 app 组装的固定工作线程上负责。
#[cfg(target_os = "macos")]
pub mod appkit;
#[cfg(any(target_os = "macos", test))]
mod editor;
#[cfg(any(windows, target_os = "macos", test))]
mod layout;
#[cfg(windows)]
mod mailbox;
mod model;
#[cfg(windows)]
mod win32;
#[cfg(windows)]
pub use mailbox::{Inbox, Latest, ReplySender, mailbox};
#[cfg(windows)]
pub use win32::{ico_file as icon_file, run};
