//! 查字/设置共享模型：请求匹配、界面状态、控件内容和纯布局；不创建窗口，不读盘或连接后台。
//! 原生视图直接消费本接口，平台 app 负责与工作线程接线。
#![forbid(unsafe_code)]
pub mod editor;
pub mod layout;
pub mod model;
pub mod view;
