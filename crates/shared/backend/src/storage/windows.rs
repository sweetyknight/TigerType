//! Windows 安全存储边界：固定目录对象、排他租约、链接拒绝与同卷事务替换。
//! 共享业务不直接调用 Windows API；这里不导入旧输入法，不在按键回调执行文件操作。

use crate::{Result, resources};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Component, Path, PathBuf, Prefix},
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, MOVEFILE_REPLACE_EXISTING,
    MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

// 所有权绑定当前产品；改名后不接管旧产品目录，也不改写旧标记。
const MARKER: &[u8] = b"TigerType native data v2\n";
const MAX_CONFIG: u64 = 1024 * 1024;
const ERROR_SHARING_VIOLATION: i32 = 32;

/// 固定每一级目录对象，防止检查后被换成目录联接或重命名到工作区之外。
struct Directories(Vec<File>);
impl Directories {
    fn pin(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err("数据路径必须为绝对路径".into());
        }
        let mut result = Self(Vec::new());
        for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
            let file = OpenOptions::new()
                .access_mode(0)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(ancestor)?;
            let info = information(&file)?;
            if info.dwFileAttributes & 0x400 != 0 || info.dwFileAttributes & 0x10 == 0 {
                return Err("数据目录不得包含链接或非目录组件".into());
            }
            result.0.push(file);
        }
        Ok(result)
    }
}

fn information(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: 只查询本对象持有的句柄；输出完整结构由栈分配。
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(info)
}

/// 有界读取单个文件：父目录逐级固定，拒绝链接、硬链接与目录；码表、设置与随包字体共用。
pub fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let _parent = Directories::pin(path.parent().ok_or("文件缺少父目录")?)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let info = information(&file)?;
    if info.dwFileAttributes & (0x400 | 0x10) != 0 || info.nNumberOfLinks != 1 {
        return Err("配置文件不得为链接、硬链接或目录".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("配置文件长度超限".into());
    }
    Ok(bytes)
}

/// 持有者必须覆盖部署/引擎使用期；排他租约禁止两套管理进程同时接管同一目录。
pub struct Data {
    root: PathBuf,
    _directories: Directories,
    _lease: File,
}

impl Data {
    /// 只创建显式指定的新目录；已有目录即使为空也不自动接管。测试只能传合成临时路径。
    pub fn create(root: &Path) -> Result<Self> {
        let _parent = Directories::pin(root.parent().ok_or("数据目录缺少父目录")?)?;
        fs::create_dir(root)?;
        let directories = Directories::pin(root)?;
        for name in ["user", "config", "backups"] {
            fs::create_dir(root.join(name))?;
        }
        let mut marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join("ownership"))?;
        marker.write_all(MARKER)?;
        marker.sync_all()?;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(root.join("lease"))?;
        Ok(Self {
            root: root.canonicalize()?,
            _directories: directories,
            _lease: lease,
        })
    }

    pub fn open(root: &Path) -> Result<Self> {
        let directories = Directories::pin(root)?;
        if read_file(&root.join("ownership"), 128)? != MARKER {
            return Err("目录所有权标记不符，不接管旧数据".into());
        }
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(root.join("lease"))
            // 租约只有后台持有；后台还在加载、管道未建时也走到这里，比探测管道更早更准。
            .map_err(|error| -> Box<dyn std::error::Error> {
                if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION) {
                    "后台已在运行，无需重复启动".into()
                } else {
                    error.into()
                }
            })?;
        let info = information(&lease)?;
        if info.dwFileAttributes & (0x400 | 0x10) != 0 || info.nNumberOfLinks != 1 {
            return Err("数据租约文件不安全".into());
        }
        for name in ["user", "config", "backups"] {
            Directories::pin(&root.join(name))?;
        }
        Ok(Self {
            root: root.canonicalize()?,
            _directories: directories,
            _lease: lease,
        })
    }

    /// 设置仅写本程序模板生成的 TOML；先核对读取时摘要，拒绝覆盖外部修改或旧格式。
    pub fn replace_settings(&self, expected: Option<&str>, contents: &[u8]) -> Result<PathBuf> {
        if contents.len() as u64 > MAX_CONFIG || contents.contains(&0) {
            return Err("设置补丁长度或内容不符".into());
        }
        // 写入边界再读回校验：结构不符、范围外、偏蓝或不可读的配色都不落盘。
        crate::settings::parse(contents)?;
        let _config = Directories::pin(&self.root.join("config"))?;
        let _backups = Directories::pin(&self.root.join("backups"))?;
        let target = self.settings_path();
        let old = if target.try_exists()? {
            Some(read_file(&target, MAX_CONFIG)?)
        } else {
            None
        };
        let hash = old
            .as_ref()
            .map(|bytes| format!("{:x}", Sha256::digest(bytes)));
        if hash.as_deref() != expected {
            return Err("settings.toml 已在外部修改，未覆盖；请到“高级”点“重新载入”后再改".into());
        }
        let transaction = resources::new_directory(&self.root.join("backups"), "settings")?;
        let _transaction = Directories::pin(&transaction)?;
        if let Some(old) = old {
            write_new(&transaction.join("previous.toml"), &old)?;
        }
        let staged = transaction.join("next.toml");
        write_new(&staged, contents)?;
        // 同卷替换，失败保留旧配置和事务证据；不先删除目标，也不清理失败目录。
        let wide = |path: &Path| {
            use std::os::windows::ffi::OsStrExt;
            path.as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>()
        };
        let source = wide(&staged);
        let destination = wide(&target);
        // SAFETY: 两个路径均位于已固定的受管目录，缓冲区 NUL 终止且调用期间存活。
        if unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(transaction)
    }

    pub fn settings(&self) -> Result<Option<Vec<u8>>> {
        let _config = Directories::pin(&self.root.join("config"))?;
        let path = self.settings_path();
        if path.try_exists()? {
            Ok(Some(read_file(&path, MAX_CONFIG)?))
        } else {
            Ok(None)
        }
    }

    /// 返回已规范化且受目录租约保护的设置文件路径，供设置页交给系统编辑器。
    pub(crate) fn settings_path(&self) -> PathBuf {
        self.root.join("config/settings.toml")
    }
}

pub(crate) fn write_new(path: &Path, contents: &[u8]) -> Result<()> {
    let _parent = Directories::pin(path.parent().ok_or("文件缺少父目录")?)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> Result<PathBuf> {
        Ok(resources::new_directory(&std::env::temp_dir(), "tigertype-data")?.join("TigerType"))
    }

    #[test]
    fn owned_data_is_exclusive_and_never_adopts_existing_directories() -> Result<()> {
        let root = root()?;
        let data = Data::create(&root)?;
        assert!(Data::create(&root).is_err());
        let held = Data::open(&root).err().unwrap().to_string();
        assert_eq!(held, "后台已在运行，无需重复启动");
        drop(data);
        assert_eq!(
            fs::read(root.join("ownership"))?,
            b"TigerType native data v2\n"
        );
        drop(Data::open(&root)?);
        // 反例：租约文件缺失是数据损坏，不能说成“已在运行”，否则用户查不到真实原因。
        fs::remove_file(root.join("lease"))?;
        let missing = Data::open(&root).err().unwrap().to_string();
        assert!(!missing.contains("已在运行"), "{missing}");
        fs::write(root.join("ownership"), b"another application")?;
        assert!(Data::open(&root).is_err());
        Ok(())
    }

    #[test]
    fn settings_preserve_user_data_and_reject_conflicts_links_and_locked_replacement() -> Result<()>
    {
        let root = root()?;
        let data = Data::create(&root)?;
        fs::write(root.join("user/personal.yaml"), b"untouched")?;
        let first = crate::settings::render(&tigertype_contracts::Options::default())?.into_bytes();
        let second = crate::settings::render(&tigertype_contracts::Options {
            auto_commit_four: true,
            ..Default::default()
        })?
        .into_bytes();
        data.replace_settings(None, &first)?;
        let old = data.settings()?.unwrap();
        let digest = format!("{:x}", Sha256::digest(&old));
        assert!(data.replace_settings(None, b"bad").is_err());
        // 偏蓝的自定义配色在写入边界被拒绝，旧设置保持不变。
        let blue = crate::settings::render(&tigertype_contracts::Options {
            theme: tigertype_contracts::Theme::Custom,
            custom: tigertype_contracts::CustomColors {
                accent: 0x0078D4,
                ..Default::default()
            },
            ..Default::default()
        })?
        .into_bytes();
        assert!(data.replace_settings(Some(&digest), &blue).is_err());
        assert_eq!(data.settings()?, Some(old.clone()));
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(root.join("config/settings.toml"))?;
        assert!(data.replace_settings(Some(&digest), &second).is_err());
        assert_eq!(data.settings()?, Some(old.clone()));
        drop(lock);
        let backup = data.replace_settings(Some(&digest), &second)?;
        assert_eq!(fs::read(backup.join("previous.toml"))?, old);
        assert_eq!(fs::read(root.join("user/personal.yaml"))?, b"untouched");
        fs::hard_link(root.join("config/settings.toml"), root.join("linked.yaml"))?;
        assert!(data.settings().is_err());
        assert!(data.replace_settings(None, b"bad").is_err());
        Ok(())
    }
}

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

pub fn reject_links(path: &Path) -> Result<()> {
    for part in path.ancestors() {
        let metadata = match fs::symlink_metadata(part) {
            Ok(value) => value,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        let reparse = metadata.file_attributes() & 0x400 != 0;
        if reparse || metadata.is_symlink() {
            return Err(format!("拒绝链接或目录联接: {}", part.display()).into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod path_tests {
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
