//! 可在 Windows 验证的应用包契约：固定文件集合、锁定资源、ARM64 Mach-O 与逐文件摘要。
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, path::Path};
use tigertype_backend::{
    Result, import,
    resources::{self, Group},
    storage,
};
pub const INFO: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>local.inputmethod.tigertype</string>
<key>CFBundleName</key><string>TigerType 虎码</string>
<key>CFBundleDisplayName</key><string>TigerType 虎码</string>
<key>CFBundleExecutable</key><string>tigertype</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleIconFile</key><string>TigerType.icns</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>LSUIElement</key><true/><key>LSBackgroundOnly</key><false/>
<key>InputMethodConnectionName</key><string>TigerType_Connection</string>
<key>InputMethodServerControllerClass</key><string>TigerTypeInputController</string>
<key>TISInputSourceID</key><string>local.inputmethod.tigertype</string>
<key>TISIntendedLanguage</key><string>zh-Hans</string>
<key>tsInputMethodCharacterRepertoireKey</key><array><string>Hans</string><string>Hant</string></array>
<key>tsInputMethodIconFileKey</key><string>TigerTypeTemplate.png</string>
<key>TISIconIsTemplate</key><true/>
</dict></plist>
"#;
const MANIFEST: &str = "Contents/Resources/bundle.json";
const EXECUTABLE: &str = "Contents/MacOS/tigertype";
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub files: Vec<File>,
}
#[derive(Deserialize)]
struct Fonts {
    fonts: Vec<resources::Entry>,
}
/// 锁定的码表/字体沿用仓库原始字节，不引入另一个可随意删项的清单。
pub fn locked() -> Result<Vec<File>> {
    let tables: Group =
        serde_json::from_str(include_str!("../../../../resources/tiger.lock.json"))?;
    let fonts: Fonts = serde_json::from_str(include_str!("../../../../resources/fonts.lock.json"))?;
    let mut files = Vec::new();
    for name in import::SOURCES {
        let entry = tables
            .files
            .iter()
            .find(|e| &e.path == name)
            .ok_or("码表清单缺失")?;
        files.push(File {
            path: format!("Contents/Resources/tiger/{name}"),
            size: entry.size,
            sha256: entry.sha256.clone(),
        });
    }
    for font in fonts.fonts {
        files.push(File {
            path: format!("Contents/Resources/Fonts/{}", font.path),
            size: font.size,
            sha256: font.sha256,
        });
    }
    Ok(files)
}
fn expected() -> Result<BTreeSet<String>> {
    let mut names: BTreeSet<_> = locked()?.into_iter().map(|e| e.path).collect();
    names.extend(
        [
            EXECUTABLE,
            "Contents/Info.plist",
            "Contents/Resources/TigerType.icns",
            "Contents/Resources/TigerTypeTemplate.png",
        ]
        .map(str::to_owned),
    );
    Ok(names)
}
pub fn entry(path: &str, bytes: &[u8]) -> File {
    File {
        path: path.into(),
        size: bytes.len() as u64,
        sha256: resources::sha256_bytes(bytes),
    }
}
/// 只接受本机 ARM64 可执行文件，且保留链接器代码签名命令；不修改 Mach-O 原始字节。
pub fn executable(bytes: &[u8]) -> Result<()> {
    let word = |at: usize| -> Result<u32> {
        Ok(u32::from_le_bytes(
            bytes.get(at..at + 4).ok_or("截断的 Mach-O")?.try_into()?,
        ))
    };
    if bytes.len() < 32 || word(0)? != 0xfeedfacf || word(4)? != 0x0100000c || word(12)? != 2 {
        return Err("需要 Apple 芯片本机 Mach-O 可执行文件".into());
    }
    let commands = word(16)?;
    let end = 32usize
        .checked_add(word(20)? as usize)
        .ok_or("Mach-O 长度溢出")?;
    if end > bytes.len() || commands > 4096 {
        return Err("Mach-O 命令长度不符".into());
    }
    let mut at = 32;
    let mut signed = false;
    for _ in 0..commands {
        let command = word(at)?;
        let size = word(at + 4)? as usize;
        if size < 8 || !size.is_multiple_of(8) || at.checked_add(size).is_none_or(|next| next > end)
        {
            return Err("Mach-O 命令越界".into());
        }
        if command == 0x1d {
            if size != 16 {
                return Err("代码签名命令长度不符".into());
            }
            let offset = word(at + 8)? as usize;
            let length = word(at + 12)? as usize;
            if length == 0
                || offset < end
                || offset.checked_add(length).is_none_or(|n| n > bytes.len())
            {
                return Err("代码签名范围不符".into());
            }
            signed = true;
        }
        at += size;
    }
    if at != end || !signed {
        return Err("缺少完整链接器签名；禁止链接后修改可执行文件".into());
    }
    Ok(())
}
pub fn validate_manifest(manifest: &Manifest) -> Result<()> {
    if manifest.files.len() != expected()?.len() {
        return Err("应用包文件数量不符".into());
    }
    let mut names = BTreeSet::new();
    for file in &manifest.files {
        resources::relative(&file.path)?;
        if !names.insert(file.path.clone())
            || file.size > 128 * 1024 * 1024
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err("应用包摘要、大小或重复路径不符".into());
        }
    }
    if names != expected()? {
        return Err("应用包文件集合不符".into());
    }
    for locked in locked()? {
        if !manifest
            .files
            .iter()
            .any(|f| f.path == locked.path && f.size == locked.size && f.sha256 == locked.sha256)
        {
            return Err(format!("锁定资源不符：{}", locked.path).into());
        }
    }
    Ok(())
}
/// 校验与复制共用同一份读取字节；安装器不能先检查再用 fs::copy 重新打开来源。
pub fn read(root: &Path, file: &File) -> Result<Vec<u8>> {
    let bytes = storage::read_file(&root.join(resources::relative(&file.path)?), file.size)?;
    if entry(&file.path, &bytes).sha256 != file.sha256 || bytes.len() as u64 != file.size {
        return Err(format!("包文件校验失败：{}", file.path).into());
    }
    if file.path == EXECUTABLE {
        executable(&bytes)?;
    }
    Ok(bytes)
}
pub fn inspect(root: &Path) -> Result<Manifest> {
    let manifest = inspect_existing(root)?;
    let info = manifest
        .files
        .iter()
        .find(|file| file.path == "Contents/Info.plist")
        .unwrap();
    if read(root, info)? != INFO.as_bytes() {
        return Err("Info.plist 与产品契约不符".into());
    }
    Ok(manifest)
}
/// 旧包按自身完整摘要校验；升级不能要求旧声明已等于新声明，来源包仍用 inspect 严格检查。
pub fn inspect_existing(root: &Path) -> Result<Manifest> {
    let manifest: Manifest =
        serde_json::from_slice(&storage::read_file(&root.join(MANIFEST), 64 * 1024)?)?;
    validate_manifest(&manifest)?;
    for file in &manifest.files {
        read(root, file)?;
    }
    Ok(manifest)
}
/// 组装只写全新目标目录；编译产物字节由打包驱动传入，图标由 Mac 原生绘制器传入。
pub fn assemble(
    repository: &Path,
    program: &[u8],
    target: &Path,
    icon: &[u8],
    menu: &[u8],
) -> Result<()> {
    if program.len() > 128 * 1024 * 1024 {
        return Err("可执行文件长度超限".into());
    }
    executable(program)?;
    storage::reject_links(target.parent().ok_or("包缺少父目录")?)?;
    fs::create_dir(target)?;
    let mut files = Vec::new();
    let mut write = |path: &str, bytes: &[u8]| -> Result<()> {
        let pathbuf = target.join(resources::relative(path)?);
        fs::create_dir_all(pathbuf.parent().unwrap())?;
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(pathbuf)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        files.push(entry(path, bytes));
        Ok(())
    };
    write(EXECUTABLE, program)?;
    write("Contents/Info.plist", INFO.as_bytes())?;
    write("Contents/Resources/TigerType.icns", icon)?;
    write("Contents/Resources/TigerTypeTemplate.png", menu)?;
    for locked in locked()? {
        let relative = locked
            .path
            .strip_prefix("Contents/Resources/")
            .ok_or("资源目录不符")?;
        let relative = relative
            .strip_prefix("Fonts/")
            .map(|f| format!("fonts/{f}"))
            .unwrap_or_else(|| relative.to_owned());
        let bytes = storage::read_file(&repository.join("resources").join(relative), locked.size)?;
        if entry(&locked.path, &bytes).sha256 != locked.sha256 || bytes.len() as u64 != locked.size
        {
            return Err(format!("资源校验失败：{}", locked.path).into());
        }
        write(&locked.path, &bytes)?;
    }
    let manifest = Manifest { files };
    validate_manifest(&manifest)?;
    fs::write(target.join(MANIFEST), serde_json::to_vec_pretty(&manifest)?)?;
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(target.join(EXECUTABLE), fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}
/// 只读检查用户级冲突副本，连悬空链接也拒绝；不替用户删除或迁移数据。
pub fn check_user_copy(home: &Path) -> Result<()> {
    if !home.is_absolute() {
        return Err("用户目录必须为绝对路径".into());
    }
    let conflict = home.join("Library/Input Methods/TigerType.app");
    match fs::symlink_metadata(&conflict) {
        Ok(_) => Err(format!("用户级副本冲突，请先自行移除：{}", conflict.display()).into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
/// 同父目录原子切换；第二步失败尽力恢复旧包，不先删除现用版本。
pub fn replace(stage: &Path, target: &Path, backup: &Path) -> Result<()> {
    replace_with(stage, target, backup, |from, to| fs::rename(from, to))
}
fn replace_with(
    stage: &Path,
    target: &Path,
    backup: &Path,
    mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    if !stage.is_absolute()
        || stage.parent() != target.parent()
        || stage.parent() != backup.parent()
        || stage == target
        || stage == backup
        || target == backup
    {
        return Err("包替换路径必须是三个不同的同目录绝对路径".into());
    }
    storage::reject_links(stage)?;
    storage::reject_links(target)?;
    storage::reject_links(backup)?;
    if backup.try_exists()? {
        return Err("备份路径已存在，拒绝覆盖恢复证据".into());
    }
    let previous = target.try_exists()?;
    if previous {
        rename(target, backup)?;
    }
    if let Err(error) = rename(stage, target) {
        if previous && let Err(restore) = rename(backup, target) {
            return Err(format!(
                "新包就位失败：{error}；旧包恢复失败：{restore}；保留 {}",
                backup.display()
            )
            .into());
        }
        return Err(error.into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_method_identifier_is_a_middle_segment_and_matches_the_server() {
        let value = |key| {
            INFO.split_once(&format!("<key>{key}</key><string>"))
                .unwrap()
                .1
                .split_once("</string>")
                .unwrap()
                .0
        };
        let identifier = value("CFBundleIdentifier");
        assert!(
            identifier.contains(".inputmethod."),
            "系统输入源识别要求 inputmethod 是中间独立段"
        );
        assert!(!identifier.ends_with('.'));
        assert_eq!(value("TISInputSourceID"), identifier);
        #[cfg(target_os = "macos")]
        assert_eq!(identifier, tigertype_macos_input_method::appkit::IDENTIFIER);
    }
    #[test]
    fn user_copy_conflict_is_rejected_without_removing_it() -> Result<()> {
        let home = resources::new_directory(
            &std::env::temp_dir().canonicalize()?,
            "tigertype-mac-conflict",
        )?;
        check_user_copy(&home)?;
        let existing = home.join("Library/Input Methods/TigerType.app");
        fs::create_dir_all(&existing)?;
        assert!(check_user_copy(&home).is_err());
        assert!(existing.is_dir());
        assert!(check_user_copy(Path::new("relative-home")).is_err());
        Ok(())
    }
    #[test]
    fn replacement_rolls_back_and_never_overwrites_an_existing_backup() -> Result<()> {
        let root = resources::new_directory(
            &std::env::temp_dir().canonicalize()?,
            "tigertype-bundle-swap",
        )?;
        let stage = root.join("new");
        let target = root.join("target");
        let backup = root.join("old");
        fs::create_dir(&stage)?;
        fs::create_dir(&target)?;
        fs::write(target.join("identity"), b"old")?;
        assert!(
            replace_with(&stage, &target, &backup, |from, to| {
                if from == stage {
                    Err(std::io::Error::other("合成第二步失败"))
                } else {
                    fs::rename(from, to)
                }
            })
            .is_err()
        );
        assert_eq!(fs::read(target.join("identity"))?, b"old");
        assert!(!backup.exists());
        replace(&stage, &target, &backup)?;
        assert_eq!(fs::read(backup.join("identity"))?, b"old");
        assert!(replace(&target, &stage, &backup).is_err());
        assert!(replace(&target, &target, &backup).is_err());
        Ok(())
    }
    /// 只有结构的合成 Mach-O，不含可执行代码或真实签名，绝不能用于安装/加载验收。
    fn synthetic_binary() -> Vec<u8> {
        let mut bytes = vec![0; 56];
        for (at, value) in [
            (0, 0xfeedfacfu32),
            (4, 0x0100000c),
            (12, 2),
            (16, 1),
            (20, 16),
            (32, 0x1d),
            (36, 16),
            (40, 48),
            (44, 8),
        ] {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }
    #[test]
    fn arm_binary_needs_bounded_code_signature_and_exact_architecture() {
        let bytes = synthetic_binary();
        executable(&bytes).unwrap();
        for at in [0, 4, 12, 16, 20, 32, 36, 40, 44] {
            let mut bad = bytes.clone();
            bad[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(executable(&bad).is_err());
        }
        for length in 0..bytes.len() {
            assert!(executable(&bytes[..length]).is_err());
        }
    }
    #[test]
    #[ignore = "完整锁定资源复制/核对单列，须显式执行；生成的是不可安装的合成测试包"]
    fn complete_locked_resources_assemble_and_detect_corruption() -> Result<()> {
        use std::io::Write;
        let root = resources::new_directory(
            &std::env::temp_dir().canonicalize()?,
            "tigertype-mac-package-test",
        )?;
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .canonicalize()?;
        let binary = synthetic_binary();
        let target = root.join("synthetic-not-installable.app");
        assemble(
            &repository,
            &binary,
            &target,
            b"synthetic-icon",
            b"synthetic-menu",
        )?;
        assert_eq!(inspect(&target)?.files.len(), expected()?.len());
        assert!(assemble(&repository, &binary, &target, b"x", b"x").is_err());
        // 旧版声明保持自身摘要一致即可被替换，但不能作为新版来源安装。
        let previous_info = INFO.replace("<string>0.1.0</string>", "<string>0.0.1</string>");
        fs::write(target.join("Contents/Info.plist"), &previous_info)?;
        assert!(
            inspect_existing(&target).is_err(),
            "未更新摘要的旧包必须拒绝"
        );
        let mut manifest: Manifest = serde_json::from_slice(&fs::read(target.join(MANIFEST))?)?;
        *manifest
            .files
            .iter_mut()
            .find(|f| f.path == "Contents/Info.plist")
            .unwrap() = entry("Contents/Info.plist", previous_info.as_bytes());
        fs::write(target.join(MANIFEST), serde_json::to_vec_pretty(&manifest)?)?;
        inspect_existing(&target)?;
        assert!(inspect(&target).is_err());
        // 只破坏刚复制的合成包，仓库原始码表始终只读。
        fs::OpenOptions::new()
            .append(true)
            .open(target.join("Contents/Resources/tiger/tiger.dict.yaml"))?
            .write_all(b"corrupt")?;
        assert!(inspect(&target).is_err());
        assert!(inspect_existing(&target).is_err());
        println!(
            "完整锁定资源复制与损坏拒绝通过；非可安装包：{}",
            target.display()
        );
        Ok(())
    }
    #[test]
    fn package_contract_rejects_missing_duplicate_escape_and_changed_locked_resource() {
        let mut files = locked().unwrap();
        for name in expected().unwrap() {
            if !files.iter().any(|f| f.path == name) {
                files.push(entry(&name, b"synthetic"));
            }
        }
        let manifest = Manifest { files };
        validate_manifest(&manifest).unwrap();
        for path in ["../escape", "Contents/Info.plist", "Contents/Other"] {
            let mut files = manifest.files.clone();
            files[0].path = path.into();
            assert!(validate_manifest(&Manifest { files }).is_err());
        }
        let mut files = manifest.files.clone();
        files[0].sha256 = "0".repeat(64);
        assert!(validate_manifest(&Manifest { files }).is_err());
        let mut files = manifest.files;
        files.pop();
        assert!(validate_manifest(&Manifest { files }).is_err());
    }
}
