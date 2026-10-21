//! 锁定资源的校验、下载和副本管理；不会自动探测个人词典路径。

use crate::{Result, process};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Component, Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Deserialize, Serialize)]
pub struct Entry {
    #[serde(alias = "name")]
    pub path: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Deserialize)]
pub struct Group {
    pub files: Vec<Entry>,
}
#[derive(Deserialize)]
pub struct Librime {
    pub artifacts: Vec<Entry>,
}
#[derive(Deserialize)]
pub struct Lock {
    pub librime: Librime,
    pub prelude: Group,
    pub essay: Group,
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
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
    let mut hash = Sha256::new();
    io::copy(&mut fs::File::open(path)?, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

pub fn verify_entry(root: &Path, entry: &Entry) -> Result<()> {
    let path = root.join(relative(&entry.path)?);
    reject_links(&path)?;
    let metadata = fs::metadata(&path)?;
    if !metadata.is_file() || metadata.len() != entry.size || sha256(&path)? != entry.sha256 {
        return Err(format!("资源校验失败: {}", path.display()).into());
    }
    Ok(())
}

fn file_names(root: &Path, directory: &Path, names: &mut BTreeSet<String>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        reject_links(&path)?;
        if path.is_dir() {
            file_names(root, &path, names)?;
        } else {
            names.insert(
                path.strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

pub fn verify_files(root: &Path, entries: &[Entry], exact: bool) -> Result<()> {
    let mut unique = BTreeSet::new();
    for entry in entries {
        if !unique.insert(entry.path.to_lowercase()) {
            return Err("清单包含重复路径".into());
        }
        verify_entry(root, entry)?;
    }
    if exact {
        // 额外文件仅枚举名称，不读取可能混入的个人数据内容。
        let mut names = BTreeSet::new();
        file_names(root, root, &mut names)?;
        if names != entries.iter().map(|e| e.path.clone()).collect() {
            return Err("目录含清单外文件".into());
        }
    }
    Ok(())
}

pub fn new_directory(parent: &Path, prefix: &str) -> Result<PathBuf> {
    reject_links(parent)?;
    fs::create_dir_all(parent)?;
    let name = format!(
        "{prefix}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let path = parent.join(name);
    fs::create_dir(&path)?;
    Ok(path)
}

pub fn copy_entries(source: &Path, target: &Path, entries: &[Entry]) -> Result<()> {
    for entry in entries {
        verify_entry(source, entry)?;
        let path = target.join(relative(&entry.path)?);
        reject_links(&path)?;
        fs::create_dir_all(path.parent().ok_or("缺少父目录")?)?;
        // create_new 防止覆盖已修改文件；复制后复验以发现源文件在复制时发生变化。
        let mut output = fs::File::options()
            .write(true)
            .create_new(true)
            .open(&path)?;
        io::copy(&mut fs::File::open(source.join(&entry.path))?, &mut output)?;
        drop(output);
        verify_entry(target, entry)?;
    }
    Ok(())
}

pub fn import_tiger(source: &Path, target: &Path, entries: &[Entry]) -> Result<()> {
    reject_links(target)?;
    verify_files(source, entries, false)?;
    if target.exists() {
        return verify_files(target, entries, true);
    }
    let staging = new_directory(target.parent().ok_or("缺少目标父目录")?, "tiger-staging")?;
    copy_entries(source, &staging, entries)?;
    verify_files(&staging, entries, true)?;
    // 失败保留隔离暂存目录供诊断，不删除或覆盖旧副本。
    fs::rename(staging, target)?;
    Ok(())
}

fn download(entry: &Entry, cache: &Path) -> Result<PathBuf> {
    let path = cache.join(relative(&entry.path)?);
    reject_links(&path)?;
    if path.exists() {
        verify_entry(cache, entry)?;
        return Ok(path);
    }
    let url = entry.url.as_deref().ok_or("缺少官方下载地址")?;
    if !url.starts_with("https://github.com/rime/")
        && !url.starts_with("https://raw.githubusercontent.com/rime/")
    {
        return Err("仅允许锁定的官方 Rime HTTPS 地址".into());
    }
    let staging = new_directory(cache, "download")?;
    let partial = staging.join(relative(&entry.path)?);
    fs::create_dir_all(partial.parent().ok_or("缺少父目录")?)?;
    // 使用系统目录中的 curl，避免 Windows 优先执行当前目录的同名程序。
    let system_root = PathBuf::from(std::env::var_os("SystemRoot").ok_or("缺少 SystemRoot")?);
    if !system_root.is_absolute() {
        return Err("SystemRoot 必须是绝对路径".into());
    }
    let status = process::run(
        Command::new(system_root.join("System32/curl.exe"))
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--connect-timeout",
                "15",
                "--max-time",
                "180",
                "--max-filesize",
                &entry.size.to_string(),
                "--output",
            ])
            .arg(&partial)
            .arg(url),
        Duration::from_secs(190),
    )?;
    if !status.success() {
        return Err(format!("curl 下载失败: {status}").into());
    }
    verify_entry(&staging, entry)?;
    fs::create_dir_all(path.parent().ok_or("缺少父目录")?)?;
    fs::rename(partial, &path)?;
    Ok(path)
}

fn extract(archive: &Path, target: &Path) -> Result<()> {
    let seven_zip = Path::new(r"C:\Program Files\7-Zip\7z.exe");
    let listing_path = target.with_extension("archive-list.txt");
    // 输出写入自己的临时文件，避免等待子进程时 stdout 管道塞满造成死锁。
    let listing = fs::File::options()
        .write(true)
        .create_new(true)
        .open(&listing_path)?;
    let status = process::run(
        Command::new(seven_zip)
            .args(["l", "-slt", "-ba", "-sccUTF-8"])
            .arg(archive)
            .stdout(listing),
        Duration::from_secs(60),
    )?;
    if !status.success() {
        return Err("7-Zip 无法读取已校验发行包".into());
    }
    validate_archive_listing(&fs::read_to_string(&listing_path)?)?;
    fs::remove_file(listing_path)?;
    let status = process::run(
        Command::new(seven_zip)
            .args(["x", "-y"])
            .arg(format!("-o{}", target.display()))
            .arg(archive),
        Duration::from_secs(120),
    )?;
    if !status.success() {
        return Err("7-Zip 解压失败".into());
    }
    Ok(())
}

fn validate_archive_listing(listing: &str) -> Result<()> {
    for line in listing.lines() {
        if let Some(name) = line.strip_prefix("Path = ") {
            relative(name)?;
        }
        if line.starts_with("Symbolic Link =") || line.starts_with("Hard Link =") {
            return Err("发行包不允许链接".into());
        }
    }
    Ok(())
}

pub fn verify(root: &Path) -> Result<()> {
    let tiger: Group = read_json(&root.join("resources/tiger.lock.json"))?;
    let lock: Lock = read_json(&root.join("dependencies.lock.json"))?;
    verify_files(&root.join("vendor/tiger"), &tiger.files, true)?;
    for (name, group) in [("prelude", lock.prelude), ("essay", lock.essay)] {
        verify_files(&root.join("vendor").join(name), &group.files, true)?;
    }
    let files: Vec<Entry> = read_json(&root.join("vendor/librime/files.json"))?;
    verify_files(&root.join("vendor/librime"), &files, false)?;
    if sha256(&root.join("vendor/librime/dist/lib/rime.dll"))? != tigerrime_engine::DLL_SHA256 {
        return Err("DLL 与编译锁不符".into());
    }
    Ok(())
}

pub fn prepare(root: &Path, tiger_source: &Path) -> Result<()> {
    let tiger: Group = read_json(&root.join("resources/tiger.lock.json"))?;
    let lock: Lock = read_json(&root.join("dependencies.lock.json"))?;
    import_tiger(tiger_source, &root.join("vendor/tiger"), &tiger.files)?;
    let mut archives = Vec::new();
    for entry in &lock.librime.artifacts {
        archives.push(download(entry, &root.join(".cache/downloads"))?);
    }
    if archives.len() != 2 {
        return Err("引擎锁必须包含主包和依赖包".into());
    }
    let target = root.join("vendor/librime");
    reject_links(&target)?;
    if !target.exists() {
        let staging = new_directory(&root.join("vendor"), "librime-staging")?;
        extract(&archives[0], &staging)?;
        extract(&archives[1], &staging.join("deps"))?;
        let mut names = BTreeSet::new();
        file_names(&staging, &staging, &mut names)?;
        let files: Vec<Entry> = names
            .into_iter()
            .map(|name| {
                let path = staging.join(&name);
                Ok(Entry {
                    size: fs::metadata(&path)?.len(),
                    sha256: sha256(&path)?,
                    path: name,
                    url: None,
                })
            })
            .collect::<Result<_>>()?;
        fs::write(
            staging.join("files.json"),
            serde_json::to_vec_pretty(&files)?,
        )?;
        fs::rename(staging, target)?;
    }
    for (name, group) in [("prelude", lock.prelude), ("essay", lock.essay)] {
        let cache = root.join(".cache/downloads").join(name);
        for entry in &group.files {
            download(entry, &cache)?;
        }
        let target = root.join("vendor").join(name);
        import_tiger(&cache, &target, &group.files)?;
    }
    verify(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PathBuf, Vec<Entry>) {
        let root = new_directory(&std::env::temp_dir(), "tigerrime-resources").unwrap();
        fs::write(root.join("schema.yaml"), b"test").unwrap();
        let entry = Entry {
            path: "schema.yaml".into(),
            size: 4,
            sha256: sha256(&root.join("schema.yaml")).unwrap(),
            url: None,
        };
        (root, vec![entry])
    }

    #[test]
    fn rejects_escape_and_alternate_data_streams() {
        for name in ["", ".", "..", "../a", r"a\..\b", "/a", "C:/a", "a:stream"] {
            assert!(relative(name).is_err(), "{name}");
        }
    }

    #[test]
    fn import_is_exact_idempotent_and_excludes_user_data() {
        let (root, entries) = fixture();
        fs::write(root.join("personal.userdb.txt"), "不能导入").unwrap();
        let target = root.join("copy");
        import_tiger(&root, &target, &entries).unwrap();
        import_tiger(&root, &target, &entries).unwrap();
        assert!(!target.join("personal.userdb.txt").exists());
        assert_eq!(fs::read(target.join("schema.yaml")).unwrap(), b"test");
    }

    #[test]
    fn modified_source_is_not_published_and_modified_copy_is_not_overwritten() {
        let (root, entries) = fixture();
        let target = root.join("copy");
        import_tiger(&root, &target, &entries).unwrap();
        fs::write(target.join("schema.yaml"), "manual").unwrap();
        assert!(import_tiger(&root, &target, &entries).is_err());
        assert_eq!(fs::read(target.join("schema.yaml")).unwrap(), b"manual");
        fs::write(root.join("schema.yaml"), "evil").unwrap();
        let new_target = root.join("other");
        assert!(import_tiger(&root, &new_target, &entries).is_err());
        assert!(!new_target.exists());
    }

    #[test]
    fn extra_files_and_duplicate_entries_are_rejected() {
        let (root, entries) = fixture();
        fs::write(root.join("extra.txt"), "extra").unwrap();
        assert!(verify_files(&root, &entries, true).is_err());
        let duplicated: Vec<Entry> =
            serde_json::from_value(serde_json::json!([entries[0], entries[0]])).unwrap();
        assert!(verify_files(&root, &duplicated, false).is_err());
    }

    #[test]
    fn corrupted_cache_is_never_replaced_by_a_download() {
        let (root, entries) = fixture();
        fs::write(root.join("schema.yaml"), "evil").unwrap();
        assert!(download(&entries[0], &root).is_err());
        assert_eq!(fs::read(root.join("schema.yaml")).unwrap(), b"evil");
    }

    #[test]
    fn rejects_archive_escape_and_links_before_extraction() {
        for listing in [
            "Path = ../outside.txt",
            "Path = a\nSymbolic Link = outside",
            "Path = a\nHard Link = outside",
        ] {
            assert!(validate_archive_listing(listing).is_err());
        }
        assert!(validate_archive_listing("Path = dist/include/rime_api.h").is_ok());
    }

    #[test]
    fn unsafe_download_origin_is_rejected_without_starting_curl() {
        let (root, _) = fixture();
        let entry = Entry {
            path: "missing.txt".into(),
            size: 1,
            sha256: String::new(),
            url: Some("http://example.com/file".into()),
        };
        assert!(download(&entry, &root).is_err());
        assert!(!root.join("missing.txt").exists());
    }

    #[test]
    fn compiled_dll_pin_matches_dependency_lock() {
        let lock: serde_json::Value =
            serde_json::from_str(include_str!("../../../dependencies.lock.json")).unwrap();
        assert_eq!(
            lock["librime"]["runtime"]["dll_sha256"],
            tigerrime_engine::DLL_SHA256
        );
    }
}
