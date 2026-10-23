//! 有界资源读取、hash 与安全新目录，离线导入不包含下载或脚本执行。
use crate::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Deserialize)]
pub struct Group {
    pub files: Vec<Entry>,
}
pub fn relative(name: &str) -> Result<PathBuf> {
    let normalized = name.replace('\\', "/");
    let path = Path::new(&normalized);
    if name.is_empty()
        || name.contains(':')
        || name.contains('\0')
        || path.components().next().is_none()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("不安全的相对路径: {name}").into());
    }
    Ok(path.to_owned())
}

pub fn reject_links(path: &Path) -> Result<()> {
    for part in path.ancestors() {
        let metadata = match fs::symlink_metadata(part) {
            Ok(value) => value,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        #[cfg(windows)]
        let reparse = metadata.file_attributes() & 0x400 != 0;
        #[cfg(not(windows))]
        let reparse = false;
        if reparse || metadata.is_symlink() {
            return Err(format!("拒绝链接或目录联接: {}", part.display()).into());
        }
    }
    Ok(())
}

pub fn sha256(path: &Path) -> Result<String> {
    Ok(sha256_bytes(&crate::data::read_file(
        path,
        128 * 1024 * 1024,
    )?))
}
pub fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn new_directory(parent: &Path, prefix: &str) -> Result<PathBuf> {
    if prefix.is_empty()
        || !prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("目录前缀必须为字母、数字或连字符".into());
    }
    // 系统时钟并不保证每次读取都不同；只保护名称生成，不持锁执行文件 I/O。
    static LAST_NONCE: Mutex<u128> = Mutex::new(0);
    reject_links(parent)?;
    fs::create_dir_all(parent)?;
    let name = format!(
        "{prefix}-{}-{}",
        std::process::id(),
        directory_nonce(
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
            &LAST_NONCE
        )?
    );
    let path = parent.join(name);
    fs::create_dir(&path)?;
    Ok(path)
}

fn directory_nonce(clock: u128, previous: &Mutex<u128>) -> Result<u128> {
    let mut previous = previous.lock().map_err(|_| "目录编号锁已损坏")?;
    let next = clock.max(previous.checked_add(1).ok_or("目录编号耗尽")?);
    *previous = next;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn directory_nonces_are_unique_with_frozen_or_rewound_clock() {
        let previous = Mutex::new(0);
        let values = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        (0..32)
                            .map(|_| directory_nonce(1_000, &previous).unwrap())
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().unwrap())
                .collect::<BTreeSet<_>>()
        });
        assert_eq!(values.len(), 256);
        assert_eq!(values.first(), Some(&1_000));
        assert_eq!(directory_nonce(900, &previous).unwrap(), 1_256);
        assert_eq!(directory_nonce(2_000, &previous).unwrap(), 2_000);
        assert!(directory_nonce(1, &Mutex::new(u128::MAX)).is_err());
    }

    #[test]
    fn rejects_escape_alternate_data_streams_and_bad_prefixes() {
        for name in ["", ".", "..", "../a", r"a\..\b", "/a", "C:/a", "a:stream"] {
            assert!(relative(name).is_err(), "{name}");
        }
        assert!(new_directory(&std::env::temp_dir(), "../outside").is_err());
    }
}
