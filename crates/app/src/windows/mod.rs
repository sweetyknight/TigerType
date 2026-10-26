//! Windows 组装：每登录会话一个后台进程（管道反应器、查字/设置 UI 与工作线程、通知区图标）、命令行与字体安装。
mod cli;
pub mod fonts;
pub mod server;
pub use cli::main;

use crate::{Result, TABLES};
use std::path::PathBuf;

/// 安装版路径：码表目录在程序旁（Program Files，只读），用户数据独立在 %APPDATA%\TigerType。
pub fn installed() -> Result<(PathBuf, PathBuf)> {
    let tables = std::env::current_exe()?.with_file_name(TABLES);
    let appdata = std::env::var_os("APPDATA").ok_or("缺少 APPDATA")?;
    Ok((tables, PathBuf::from(appdata).join("TigerType")))
}
