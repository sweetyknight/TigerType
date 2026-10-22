//! 纯 Rust 核心与安全数据工具的应用组装；不初始化或加载旧引擎。

#[cfg(windows)]
pub mod data;
pub mod dispatch;
pub mod import;
pub mod process;
pub mod resources;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn project_root() -> Result<std::path::PathBuf> {
    // 诊断工具绑定源码工作区；正式安装的数据定位将在部署阶段单独实现。
    Ok(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}
