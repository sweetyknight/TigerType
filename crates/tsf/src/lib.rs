//! TSF COM 编辑边界：只接受 contracts 结果和已经绑定的宿主上下文，不依赖引擎或 UI。
#![cfg(windows)]
mod edit;
pub use edit::{EditReport, Editor};
