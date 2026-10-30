//! 本地开发工具：检查、打包与合成预览；产品不得反向依赖本 crate（包括测试依赖）。
pub mod process;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// 仅开发工具绑定源码工作区；产品路径由各平台安装约定决定。
pub fn project_root() -> Result<std::path::PathBuf> {
    Ok(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}
