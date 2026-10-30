//! 全机安装边界；只允许 root 操作固定目录，复制已校验字节，不自动启用输入法或触碰用户数据。
use crate::bundle;
use std::{
    ffi::{CStr, OsStr},
    fs,
    io::Write,
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tigertype_backend::{Result, resources, storage};
const PARENT: &str = "/Library/Input Methods";
const NAME: &str = "TigerType.app";
struct Mask(libc::mode_t);
impl Drop for Mask {
    fn drop(&mut self) {
        // SAFETY: 安装命令尚未创建产品线程，只恢复本进程先前的文件创建掩码。
        unsafe {
            libc::umask(self.0);
        }
    }
}
fn privileged() -> Result<PathBuf> {
    // SAFETY: 仅查询身份，不改变用户权限。
    if unsafe { libc::geteuid() } != 0 {
        return Err("安装/卸载须由 sudo 运行".into());
    }
    let parent = PathBuf::from(PARENT);
    storage::reject_links(&parent)?;
    let metadata = fs::metadata(&parent)?;
    if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err("系统输入法目录属主或权限不安全".into());
    }
    Ok(parent)
}
fn caller_home() -> Result<PathBuf> {
    let uid: u32 = std::env::var("SUDO_UID")?.parse()?;
    if uid == 0 {
        return Err("请从普通用户会话通过 sudo 安装".into());
    }
    let mut buffer = vec![0u8; 64 * 1024];
    let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    // SAFETY: passwd 输出与缓冲区均有效；成功且非空后才读取 pw_dir。
    let code = unsafe {
        libc::getpwuid_r(
            uid,
            record.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if code != 0 || result.is_null() {
        return Err("无法确定 sudo 调用者目录".into());
    }
    // SAFETY: getpwuid_r 成功返回的 pw_dir 在 buffer 中、以 NUL 结尾，复制后不保留借用。
    let home = unsafe { CStr::from_ptr((*result).pw_dir) };
    Ok(PathBuf::from(OsStr::from_bytes(home.to_bytes())))
}
fn process_path(pid: i32) -> Option<PathBuf> {
    let mut buffer = [0u8; 4096];
    // SAFETY: 只查询 PID 的映像路径，缓冲区固定且长度如实传入。
    let size = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if size <= 0 {
        return None;
    }
    let end = buffer.iter().position(|&b| b == 0)?;
    Some(PathBuf::from(OsStr::from_bytes(&buffer[..end])))
}
fn stop(target: &Path) -> Result<()> {
    let binary = target.join("Contents/MacOS/tigertype");
    let mut pids = vec![0i32; 65_536];
    // SAFETY: PID 数组预分配并有界；返回值为进程数，满数组时拒绝截断枚举。
    let count =
        unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), (pids.len() * 4) as i32) };
    if count <= 0 || count as usize >= pids.len() {
        return Err("无法完整枚举输入法进程".into());
    }
    pids.truncate(count as usize);
    pids.retain(|pid| {
        *pid > 0
            && *pid != std::process::id() as i32
            && process_path(*pid).as_deref() == Some(&binary)
    });
    for pid in &pids {
        if process_path(*pid).as_deref() != Some(&binary) {
            continue;
        }
        // SAFETY: 再次核对 PID 映像属于固定安装路径后才发送 SIGTERM，不按进程名误杀。
        if unsafe { libc::kill(*pid, libc::SIGTERM) } != 0
            && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while pids.iter().any(|pid| match process_path(*pid) {
        Some(path) => path == binary,
        None => {
            // SAFETY: 信号 0 仅查询此前已匹配的进程是否还存在；查不到路径不等于已退出。
            let present = unsafe { libc::kill(*pid, 0) };
            present == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
        }
    }) {
        if Instant::now() >= deadline {
            return Err("旧输入法尚未退出，停止本次操作；请核对安装状态和保留的旧包".into());
        }
        // 安装命令的有界等待，不用于常驻输入进程或逐键路径。
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}
fn safe_tree(path: &Path, count: &mut usize) -> Result<()> {
    *count += 1;
    if *count > 256 {
        return Err("安装目录含异常数量文件，拒绝清理".into());
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_symlink()
        || metadata.uid() != 0
        || metadata.mode() & 0o022 != 0
        || (!metadata.is_file() && !metadata.is_dir())
    {
        return Err(format!("拒绝不安全的安装项：{}", path.display()).into());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            safe_tree(&entry?.path(), count)?;
        }
    }
    Ok(())
}
fn remove(parent: &Path, path: &Path) -> Result<()> {
    // 递归删除只接收本函数校验的系统目录直接子目录；不触碰 /Users 或用户数据。
    if !path.is_absolute()
        || path.parent() != Some(parent)
        || parent != Path::new(PARENT)
        || !path
            .file_name()
            .is_some_and(|s| s.to_string_lossy().starts_with("TigerType"))
    {
        return Err("清理目标越界".into());
    }
    storage::reject_links(path)?;
    safe_tree(path, &mut 0)?;
    fs::remove_dir_all(path)?;
    Ok(())
}
fn fix_directories(path: &Path) -> Result<()> {
    // 仅用于刚创建且 root 独占的 staging，固定目录的 root:wheel/755，不继承调用者 umask。
    let directory = fs::File::open(path)?;
    // SAFETY: 已打开的新建目录描述符有效，只设置当前安装树的固定属主。
    if unsafe { libc::fchown(directory.as_raw_fd(), 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    directory.set_permissions(fs::Permissions::from_mode(0o755))?;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            fix_directories(&entry.path())?;
        }
    }
    directory.sync_all()?;
    Ok(())
}
pub fn install(source: &Path) -> Result<()> {
    let parent = privileged()?;
    // SAFETY: 安装是独立单线程命令，先收紧 staging 创建权限，结束后恢复；不改全局系统设置。
    let _mask = Mask(unsafe { libc::umask(0o077) });
    let target = parent.join(NAME);
    if source == target {
        return Err("请从新构建包运行 install，不在现用包内自替换".into());
    }
    bundle::check_user_copy(&caller_home()?)?;
    let manifest = bundle::inspect(source)?;
    let stage = resources::new_directory(&parent, "TigerType-new")?;
    fs::set_permissions(&stage, fs::Permissions::from_mode(0o755))?;
    for entry in &manifest.files {
        let bytes = bundle::read(source, entry)?;
        let path = stage.join(resources::relative(&entry.path)?);
        fs::create_dir_all(path.parent().unwrap())?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(&bytes)?;
        // SAFETY: 文件由 root 在新 staging 内独占创建，固定属主 root:wheel，不复制扩展属性。
        if unsafe { libc::fchown(file.as_raw_fd(), 0, 0) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        file.set_permissions(fs::Permissions::from_mode(
            if entry.path == "Contents/MacOS/tigertype" {
                0o755
            } else {
                0o644
            },
        ))?;
        file.sync_all()?;
    }
    fs::write(
        stage.join("Contents/Resources/bundle.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::set_permissions(
        stage.join("Contents/Resources/bundle.json"),
        fs::Permissions::from_mode(0o644),
    )?;
    fix_directories(&stage)?;
    safe_tree(&stage, &mut 0)?;
    bundle::inspect(&stage)?;
    // 旧包必须仍是安全的受管布局；失败时保留旧包和 staging 证据。
    if target.try_exists()? {
        safe_tree(&target, &mut 0)?;
        bundle::inspect(&target)?;
    }
    let backup = parent.join(format!("TigerType-old-{}", std::process::id()));
    if backup.try_exists()? {
        return Err("旧恢复目录已存在，请先核对安装状态；未停止现用输入法".into());
    }
    stop(&target)?;
    bundle::replace(&stage, &target, &backup)?;
    if backup.try_exists()? {
        // 系统可能在替换前再次拉起旧输入法；改名后的旧映像也确认退出后才清理。
        stop(&backup)?;
        remove(&parent, &backup)?;
    }
    println!(
        "安装完成。首次安装请注销重登，再在系统设置中手动添加 TigerType 虎码；未自动启用或切换。"
    );
    Ok(())
}
pub fn uninstall() -> Result<()> {
    let parent = privileged()?;
    let target = parent.join(NAME);
    bundle::inspect(&target)?;
    stop(&target)?;
    remove(&parent, &target)?;
    println!("已卸载程序，保留全部用户数据；请手动移除系统设置中的输入法条目。");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_refuses_non_system_or_nested_targets_before_touching_files() {
        assert!(remove(Path::new("/tmp"), Path::new("/tmp/TigerType.app")).is_err());
        assert!(
            remove(
                Path::new(PARENT),
                Path::new("/Library/Input Methods/TigerType.app/Contents")
            )
            .is_err()
        );
        assert!(
            remove(
                Path::new(PARENT),
                Path::new("/Library/Input Methods/Other.app")
            )
            .is_err()
        );
    }
    #[test]
    fn process_identity_is_read_from_the_executable_not_its_display_name() {
        assert_eq!(
            process_path(std::process::id() as i32),
            Some(std::env::current_exe().unwrap())
        );
    }
}
