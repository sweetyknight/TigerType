//! 组装资源工具、引擎会话调度与诊断；尚不提供安装、TSF 或后台常驻模式。

pub mod dispatch;
#[cfg(windows)]
pub mod job;
#[cfg(windows)]
pub mod pipe_engine;
pub mod probe;
pub mod process;
pub mod resources;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn project_root() -> Result<std::path::PathBuf> {
    // 诊断工具绑定源码工作区；正式安装的数据定位将在部署阶段单独实现。
    Ok(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}
