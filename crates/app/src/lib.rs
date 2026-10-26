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
pub mod settings;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn project_root() -> Result<std::path::PathBuf> {
    // 诊断工具绑定源码工作区；安装版的数据定位见 installed。
    Ok(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}

/// 码表目录名：源码中为 resources 下的同名目录，安装版在程序目录下；打包与安装版后台共用。
pub const TABLES: &str = "tiger";

/// 安装版路径：码表目录在程序旁（Program Files，只读），用户数据独立在 %APPDATA%\TigerType。
pub fn installed() -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let tables = std::env::current_exe()?.with_file_name(TABLES);
    let appdata = std::env::var_os("APPDATA").ok_or("缺少 APPDATA")?;
    Ok((tables, std::path::PathBuf::from(appdata).join("TigerType")))
}

/// canonicalize 给出的 `\\?\` 本地盘符路径去掉前缀即为同一路径：用户看到的、交给编辑器或 ISCC 的
/// 都用常见写法（ISCC 不认该前缀）。UNC 等其他形式原样返回。
pub fn plain(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::{Component, Path, Prefix};
    let mut parts = path.components();
    if let Some(Component::Prefix(prefix)) = parts.next()
        && let Prefix::VerbatimDisk(drive) = prefix.kind()
    {
        return Path::new(&format!("{}:", drive as char)).join(parts.as_path());
    }
    path.to_owned()
}

#[cfg(all(test, windows))]
mod tests {
    use std::path::Path;
    #[test]
    fn verbatim_drive_prefix_is_removed_but_unc_is_kept() {
        assert_eq!(
            super::plain(Path::new(r"\\?\D:\a\b.iss")),
            Path::new(r"D:\a\b.iss")
        );
        assert_eq!(
            super::plain(Path::new(r"\\?\UNC\srv\s\a")),
            Path::new(r"\\?\UNC\srv\s\a")
        );
        assert_eq!(super::plain(Path::new(r"D:\a")), Path::new(r"D:\a"));
    }
}
