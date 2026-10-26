//! 平台无关的后台：码表核对与加载、settings.toml、受管目录、类型化调度与宿主回收策略、查字/设置控制器。
//! 不依赖 ipc、前端或界面；跨进程传输与进程组装在 app，受管目录的文件原语按平台实现（现为 Windows）。
#[cfg(windows)]
pub mod data;
pub mod dispatch;
pub mod import;
#[cfg(windows)]
pub mod lookup;
pub mod resources;
pub mod settings;

use std::path::{Component, Path, PathBuf, Prefix};
use tigertype_contracts::{Key, KeyOutcome, Options};
use tigertype_engine::Session;
/// 词表类型属于本 crate 的公开接口：调度器与控制器借用 Dictionary，Data/Entry 供组装层构造合成词表；
/// 组装层不直接依赖 engine。
pub use tigertype_engine::{Data, Dictionary, Entry};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// canonicalize 给出的 `\\?\` 本地盘符路径去掉前缀即为同一路径：用户看到的、交给编辑器或 ISCC 的
/// 都用常见写法（ISCC 不认该前缀）。UNC 等其他形式原样返回。
pub fn plain(path: &Path) -> PathBuf {
    let mut parts = path.components();
    if let Some(Component::Prefix(prefix)) = parts.next()
        && let Prefix::VerbatimDisk(drive) = prefix.kind()
    {
        return Path::new(&format!("{}:", drive as char)).join(parts.as_path());
    }
    path.to_owned()
}

/// 诊断命令 demo-input：默认设置的新会话逐字处理（空格为选字键），返回每键结果；不经调度、不上屏。
pub fn demo(dictionary: &Dictionary, input: &str) -> Result<Vec<KeyOutcome>> {
    let mut session = Session::new(dictionary, Options::default());
    input
        .chars()
        .map(|c| {
            let key = if c == ' ' {
                Key::Space
            } else {
                Key::Character(c)
            };
            Ok(session.process(key)?)
        })
        .collect()
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
