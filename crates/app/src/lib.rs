//! 纯 Rust 核心与安全数据工具的应用组装；不初始化或加载旧引擎。

#[cfg(windows)]
pub mod backend;
#[cfg(windows)]
pub mod data;
pub mod dispatch;
#[cfg(windows)]
pub mod fonts;
pub mod import;
#[cfg(windows)]
pub mod lookup;
pub mod process;
pub mod resources;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn project_root() -> Result<std::path::PathBuf> {
    // 诊断工具绑定源码工作区；安装版的数据定位见 installed。
    Ok(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}

/// 数据包在安装包里的文件名；打包与安装版后台共用。
pub const PACKAGE: &str = "tiger-v1.json";

/// 安装版路径：数据包与程序同目录（Program Files，只读），用户数据独立在 %APPDATA%\TigerType。
pub fn installed() -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let package = std::env::current_exe()?.with_file_name(PACKAGE);
    let appdata = std::env::var_os("APPDATA").ok_or("缺少 APPDATA")?;
    Ok((package, std::path::PathBuf::from(appdata).join("TigerType")))
}
