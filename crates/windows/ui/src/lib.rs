//! Windows 查字/设置窗、托盘和事件邮箱；不包含宿主候选窗，不调用引擎。
//! 只消费共享 UI 模型与 contracts，由 Windows app 组装固定 UI/工作线程。
#![cfg(windows)]
mod mailbox;
mod win32;
pub use mailbox::{Inbox, Latest, ReplySender, mailbox};
pub use win32::{ico_file as icon_file, run};
