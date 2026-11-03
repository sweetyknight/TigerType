//! Apple 芯片本机打包：不调用签名/通用二进制工具，链接后原样复制可执行文件。
use std::{
    fs, io::Read, os::unix::fs::OpenOptionsExt, path::Path, process::Command, time::Duration,
};
use tigertype_dev::{Result, process, project_root};
use tigertype_macos_app::{bundle, icons};
/// Cargo 可用硬链接发布自身产物；只对此固定构建输出允许硬链接，不放宽数据/字体/码表读取规则。
fn read_executable(path: &Path) -> Result<Vec<u8>> {
    tigertype_backend::storage::reject_links(path.parent().ok_or("产物缺少父目录")?)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err("编译产物须为普通文件".into());
    }
    let mut bytes = Vec::new();
    file.take(128 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 128 * 1024 * 1024 {
        return Err("编译产物长度超限".into());
    }
    Ok(bytes)
}
fn run() -> Result<()> {
    if !cfg!(target_arch = "aarch64") {
        return Err("当前个人使用目标为 Apple 芯片，须在该 Mac 本机构建".into());
    }
    let root = project_root()?;
    let status = process::run(
        Command::new("cargo")
            .current_dir(&root)
            .env("MACOSX_DEPLOYMENT_TARGET", "13.0")
            .args([
                "build",
                "--locked",
                "--offline",
                "--release",
                "--target",
                "aarch64-apple-darwin",
                "-p",
                "tigertype-macos-app",
                "--bin",
                "tigertype-macos",
            ]),
        Duration::from_secs(1800),
    )?;
    if !status.success() {
        return Err("Mac Release 构建失败".into());
    }
    let parent = root.join("target/package");
    fs::create_dir_all(&parent)?;
    let parent = parent.canonicalize()?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let stage = parent.join(format!("TigerType-new-{nonce}.app"));
    let target = parent.join("TigerType.app");
    let backup = parent.join(format!("TigerType-old-{nonce}.app"));
    let (icon, menu) = icons::render(&root)?;
    let program =
        read_executable(&root.join("target/aarch64-apple-darwin/release/tigertype-macos"))?;
    bundle::assemble(&root, &program, &stage, &icon, &menu)?;
    let manifest = bundle::inspect(&stage)?;
    if target.try_exists()? {
        bundle::inspect(&target)?;
    }
    bundle::replace(&stage, &target, &backup)?;
    if backup.try_exists()? {
        // 递归清理前再次核对绝对目标只在本次包目录下；已有包此前已完整核对，不接管其他目录。
        let resolved = backup.canonicalize()?;
        if resolved.parent() != Some(parent.as_path()) || resolved != backup {
            return Err("旧包清理路径越界".into());
        }
        fs::remove_dir_all(&resolved)?;
    }
    for file in manifest.files {
        println!("{}  {}", file.sha256, file.path);
    }
    println!("应用包：{}（未安装）", target.display());
    Ok(())
}
pub fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compiler_hardlink_is_read_only_but_symlinks_and_directories_are_rejected() -> Result<()> {
        let root = tigertype_backend::resources::new_directory(
            &std::env::temp_dir().canonicalize()?,
            "tigertype-build-artifact",
        )?;
        let original = root.join("original");
        let linked = root.join("linked");
        let alias = root.join("alias");
        fs::write(&original, b"synthetic compiler output")?;
        fs::hard_link(&original, &linked)?;
        assert_eq!(read_executable(&linked)?, b"synthetic compiler output");
        assert!(tigertype_backend::storage::read_file(&linked, 100).is_err());
        std::os::unix::fs::symlink(&original, &alias)?;
        assert!(read_executable(&alias).is_err());
        assert!(read_executable(&root).is_err());
        assert_eq!(fs::read(original)?, b"synthetic compiler output");
        Ok(())
    }
}
