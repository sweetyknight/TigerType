//! Mac 安全存储：逐级 openat 固定目录、O_NOFOLLOW、所有者/硬链接检查、flock 与原子替换。
use crate::{Result, resources};
use std::{
    ffi::CString,
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path, PathBuf},
};
const MARKER: &[u8] = b"TigerType native data v2\n";
const MAX_CONFIG: u64 = 1024 * 1024;
struct Directory(File);
fn name(value: &std::ffi::OsStr) -> Result<CString> {
    Ok(CString::new(value.as_bytes())?)
}
fn fd(value: i32) -> Result<File> {
    if value < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: 只接收本模块 open/openat 新建且非负的描述符，转移给 File 唯一关闭。
    Ok(unsafe { File::from_raw_fd(value) })
}
impl Directory {
    fn pin(path: &Path) -> Result<Self> {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        {
            return Err("目录须为无上级跳转的绝对路径".into());
        }
        // SAFETY: 根路径常量 NUL 终止；只打开目录且不跟随链接。
        let mut directory = Self(fd(unsafe {
            libc::open(
                c"/".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        })?);
        for component in path.components() {
            if let Component::Normal(part) = component {
                directory = directory.child(part)?;
            }
        }
        Ok(directory)
    }
    fn child(&self, part: &std::ffi::OsStr) -> Result<Self> {
        let part = name(part)?;
        // SAFETY: 父描述符存活，单段名字 NUL 终止；禁止链接和非目录。
        Ok(Self(fd(unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                part.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })?))
    }
    fn create(&self, part: &str) -> Result<Self> {
        let value = CString::new(part)?;
        // SAFETY: 名称为内部生成的单段，父描述符固定；已有名字拒绝，不接管旧目录。
        if unsafe { libc::mkdirat(self.0.as_raw_fd(), value.as_ptr(), 0o700) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        self.child(part.as_ref())
    }
    fn owned(&self) -> Result<()> {
        let meta = self.0.metadata()?;
        // SAFETY: geteuid 无参数、无指针，不改变身份。
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
            return Err("受管目录属主或写权限不安全".into());
        }
        Ok(())
    }
    fn file(&self, part: &std::ffi::OsStr, flags: i32) -> Result<File> {
        let part = name(part)?;
        // SAFETY: 父描述符与名字有效；NONBLOCK 防止恶意 FIFO 在类型核对前阻塞。
        let file = fd(unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                part.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                0o600,
            )
        })?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.nlink() != 1 {
            return Err("拒绝非普通文件或硬链接".into());
        }
        Ok(file)
    }
    fn read(&self, part: &str, limit: u64) -> Result<Option<Vec<u8>>> {
        match self.file(part.as_ref(), libc::O_RDONLY) {
            Ok(file) => Ok(Some(read(file, limit)?)),
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
    fn write(&self, part: &str, contents: &[u8]) -> Result<()> {
        let mut file = self.file(part.as_ref(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
        file.write_all(contents)?;
        // SAFETY: 对本函数持有的普通文件请求落盘，失败保留事务文件，不伪装保存成功。
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}
fn read(file: File, limit: u64) -> Result<Vec<u8>> {
    // 先按实际大小留足容量：码表文件大，边读边翻倍会多占近一倍内存。
    let mut bytes = Vec::with_capacity(file.metadata()?.len().min(limit) as usize + 1);
    file.take(limit.checked_add(1).ok_or("读取上限溢出")?)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("文件长度超限".into());
    }
    Ok(bytes)
}
pub fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let parent = Directory::pin(path.parent().ok_or("文件缺少父目录")?)?;
    read(
        parent.file(path.file_name().ok_or("文件缺少名称")?, libc::O_RDONLY)?,
        limit,
    )
}
pub fn plain(path: &Path) -> PathBuf {
    path.to_owned()
}
pub fn reject_links(path: &Path) -> Result<()> {
    // 只允许末尾尚不存在；已存在的每一级仍用无跟随打开验证。
    let mut existing = path;
    while let Err(e) = std::fs::symlink_metadata(existing) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e.into());
        }
        existing = existing.parent().ok_or("缺少已存在的父目录")?;
    }
    Directory::pin(existing)?;
    Ok(())
}
pub struct Data {
    root: PathBuf,
    _root: Directory,
    config: Directory,
    backups: Directory,
    _lease: File,
}
impl Data {
    pub fn create(root: &Path) -> Result<Self> {
        let parent = Directory::pin(root.parent().ok_or("数据目录缺少父目录")?)?;
        let directory = parent.create(
            root.file_name()
                .and_then(|s| s.to_str())
                .ok_or("数据目录名称非法")?,
        )?;
        for child in ["user", "config", "backups"] {
            directory.create(child)?;
        }
        directory.write("ownership", MARKER)?;
        directory.0.sync_all()?;
        Self::open(root)
    }
    pub fn open(root: &Path) -> Result<Self> {
        let directory = Directory::pin(root)?;
        directory.owned()?;
        let lease = directory.file("ownership".as_ref(), libc::O_RDWR)?;
        let marker = lease.metadata()?;
        // SAFETY: 仅查询有效用户 ID；租约标记也不能由其他用户替换或写入。
        if marker.uid() != unsafe { libc::geteuid() } || marker.mode() & 0o022 != 0 {
            return Err("所有权标记权限不安全".into());
        }
        // SAFETY: 本进程持有的标记文件；非阻塞排他锁，失败不等待其他实例。
        if unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if read(lease.try_clone()?, 128)? != MARKER {
            return Err("目录所有权标记不符，不接管旧数据".into());
        }
        let config = directory.child("config".as_ref())?;
        config.owned()?;
        let backups = directory.child("backups".as_ref())?;
        backups.owned()?;
        directory.child("user".as_ref())?.owned()?;
        Ok(Self {
            root: root.to_owned(),
            _root: directory,
            config,
            backups,
            _lease: lease,
        })
    }
    pub fn settings(&self) -> Result<Option<Vec<u8>>> {
        self.verify()?;
        self.config.read("settings.toml", MAX_CONFIG)
    }
    fn verify(&self) -> Result<()> {
        // 持有描述符不等于路径还指向同一对象；目录被移动/替换后不继续写入旧位置。
        let root = Directory::pin(&self.root)?;
        for (expected, actual) in [
            (&self._root, root),
            (&self.config, self._root.child("config".as_ref())?),
            (&self.backups, self._root.child("backups".as_ref())?),
        ] {
            let expected = expected.0.metadata()?;
            let actual = actual.0.metadata()?;
            if (expected.dev(), expected.ino()) != (actual.dev(), actual.ino()) {
                return Err("受管目录已被替换，拒绝继续访问".into());
            }
        }
        Ok(())
    }
    pub(crate) fn settings_path(&self) -> PathBuf {
        self.root.join("config/settings.toml")
    }
    pub fn replace_settings(&self, expected: Option<&str>, contents: &[u8]) -> Result<PathBuf> {
        if contents.len() as u64 > MAX_CONFIG || contents.contains(&0) {
            return Err("设置补丁长度或内容不符".into());
        }
        crate::settings::parse(contents)?;
        let old = self.settings()?;
        if old.as_ref().map(|b| resources::sha256_bytes(b)).as_deref() != expected {
            return Err("settings.toml 已在外部修改，未覆盖；请重新载入".into());
        }
        // 纳秒只用于唯一目录名；冲突时拒绝，不截断已有事务文件。
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let name = format!("settings-{}-{nonce}", std::process::id());
        let transaction = self.backups.create(&name)?;
        if let Some(old) = &old {
            transaction.write("previous.toml", old)?;
        }
        transaction.write("next.toml", contents)?;
        transaction.0.sync_all()?;
        if self.settings()? != old {
            return Err("settings.toml 在事务期间被修改，未覆盖".into());
        }
        // SAFETY: 源和目标都相对于固定目录；原子改名失败时旧配置仍在，成功后同步目录。
        if unsafe {
            libc::renameat(
                transaction.0.as_raw_fd(),
                c"next.toml".as_ptr(),
                self.config.0.as_raw_fd(),
                c"settings.toml".as_ptr(),
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        self.config.0.sync_all()?;
        transaction.0.sync_all()?;
        Ok(self.root.join("backups").join(name))
    }
}
#[cfg(test)]
pub(crate) fn write_new(path: &Path, contents: &[u8]) -> Result<()> {
    Directory::pin(path.parent().ok_or("文件缺少父目录")?)?.write(
        path.file_name()
            .and_then(|s| s.to_str())
            .ok_or("文件名非法")?,
        contents,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lease_links_conflicts_and_atomic_backup() -> Result<()> {
        let parent = resources::new_directory(
            &std::env::temp_dir().canonicalize()?,
            "tigertype-mac-storage",
        )?;
        let root = parent.join("data");
        let data = Data::create(&root)?;
        assert!(Data::open(&root).is_err());
        assert!(Data::create(&root).is_err());
        let first = crate::settings::render(&Default::default())?.into_bytes();
        data.replace_settings(None, &first)?;
        let second = crate::settings::render(&tigertype_contracts::Options {
            page_size: 7,
            ..Default::default()
        })?
        .into_bytes();
        assert!(data.replace_settings(None, &second).is_err());
        let backup = data.replace_settings(Some(&resources::sha256_bytes(&first)), &second)?;
        assert_eq!(read_file(&backup.join("previous.toml"), MAX_CONFIG)?, first);
        std::fs::hard_link(root.join("config/settings.toml"), parent.join("hard"))?;
        assert!(data.settings().is_err());
        std::os::unix::fs::symlink(&root, parent.join("alias"))?;
        assert!(Data::open(&parent.join("alias")).is_err());
        drop(data);
        Ok(())
    }
}
