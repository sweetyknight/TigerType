//! Windows 后台的唯一组装点：管道反应器、引擎线程、查字/设置工作线程与 UI 线程。
//! 不承载 Mac 预览或开发工具；共享后台不反向引用本 crate。
#![cfg(windows)]
mod cli;
pub mod fonts;
pub mod server;
pub use cli::main;

use std::path::PathBuf;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// 安装版只从程序旁加载锁定码表，正式用户数据独立于安装目录。
pub fn installed() -> Result<(PathBuf, PathBuf)> {
    let tables = std::env::current_exe()?.with_file_name(tigertype_backend::TABLES);
    let appdata = std::env::var_os("APPDATA").ok_or("缺少 APPDATA")?;
    Ok((tables, PathBuf::from(appdata).join("TigerType")))
}
