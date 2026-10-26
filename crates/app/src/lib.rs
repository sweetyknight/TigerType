//! 唯一组装点：各平台模块只由 cfg(target_os) 选入、互不引用；另含打包、检查与测试共用的开发期小工具。

pub mod process;
#[cfg(target_os = "windows")]
pub mod windows;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn project_root() -> Result<std::path::PathBuf> {
    // 诊断工具绑定源码工作区；安装版的数据定位见各平台模块。
    Ok(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}

/// 码表目录名：源码中为 resources 下的同名目录，安装版在程序目录下；打包与安装版后台共用。
pub const TABLES: &str = "tiger";
