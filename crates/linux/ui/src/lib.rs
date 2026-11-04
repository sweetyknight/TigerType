//! Linux 查字/设置视图（GTK4）；草稿与状态规则取自共享 ui-model，不持有词表或受管目录。
//! 样式表生成与平台无关、各宿主都测；GTK 控件只在 Linux 编译，由组装层在主线程创建并接工作线程。
#[cfg(target_os = "linux")]
pub mod gtk;
pub mod style;
