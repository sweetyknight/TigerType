//! 按锁定清单为当前用户安装随包字体：先校验全部来源的大小与 SHA256，再复制到用户字体目录并登记。
//! 不需要管理员；已有同名而内容不同的文件一律不覆盖。字体装进系统后候选窗在任何宿主里按字体名取用，
//! DLL 仍不读盘。
use crate::Result;
use serde::Deserialize;
use std::{
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, Write},
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use tigertype_backend::{resources, storage};
use windows_sys::Win32::{
    Foundation::ERROR_SUCCESS,
    Graphics::Gdi::AddFontResourceW,
    System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegEnumValueW, RegSetValueExW,
    },
    UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_FONTCHANGE,
    },
};

/// 字体文件 128MB 上限只防异常输入。
const LIMIT: u64 = 128 * 1024 * 1024;
const USER_FONTS: &str = r"Software\Microsoft\Windows NT\CurrentVersion\Fonts";

#[derive(Debug, Clone, Deserialize)]
pub struct Font {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// 登记名用的字体族名；界面按族名取字体。
    pub family: String,
}
#[derive(Deserialize)]
struct Lock {
    fonts: Vec<Font>,
}
pub fn locked() -> Result<Vec<Font>> {
    Ok(serde_json::from_str::<Lock>(include_str!("../../../../resources/fonts.lock.json"))?.fonts)
}

/// 当前用户字体登记键；Drop 时关闭。
pub(crate) struct Key(HKEY);
impl Key {
    fn user() -> Result<Self> {
        let mut key = null_mut();
        // SAFETY: 路径 NUL 结尾，输出句柄为本地值；只请求查询与写值权限。
        let code = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(OsStr::new(USER_FONTS)).as_ptr(),
                0,
                null(),
                0,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                null(),
                &mut key,
                null_mut(),
            )
        };
        if code != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(code as i32).into());
        }
        Ok(Self(key))
    }
    /// 任一登记项已指向该文件即视为已登记，不按名称重复登记（用户可能以本地化名称登记过）。
    fn registered(&self, file: &Path) -> Result<bool> {
        let target = file.as_os_str().to_string_lossy().to_lowercase();
        for index in 0.. {
            let mut name = [0u16; 512];
            let mut name_length = name.len() as u32;
            let mut data = [0u16; 1024];
            let mut data_size = (data.len() * 2) as u32;
            let mut kind = 0;
            // SAFETY: 名称与数据缓冲区长度如实传入，只读本键的值。
            let code = unsafe {
                RegEnumValueW(
                    self.0,
                    index,
                    name.as_mut_ptr(),
                    &mut name_length,
                    null(),
                    &mut kind,
                    data.as_mut_ptr().cast(),
                    &mut data_size,
                )
            };
            match code {
                ERROR_SUCCESS => {}
                // ERROR_NO_MORE_ITEMS
                259 => return Ok(false),
                // ERROR_MORE_DATA：超长值不可能是本工具的路径，跳过。
                234 => continue,
                _ => return Err(io::Error::from_raw_os_error(code as i32).into()),
            }
            let units = (data_size as usize / 2).min(data.len());
            let value = String::from_utf16_lossy(&data[..units]);
            if kind == REG_SZ && value.trim_end_matches('\0').to_lowercase() == target {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn set(&self, name: &str, file: &Path) -> Result<()> {
        let data = wide(file.as_os_str());
        // SAFETY: 名称与数据均 NUL 结尾，字节数含结尾 NUL。
        let code = unsafe {
            RegSetValueExW(
                self.0,
                wide(OsStr::new(name)).as_ptr(),
                0,
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        if code != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(code as i32).into());
        }
        Ok(())
    }
}
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: 句柄由本结构打开，只关闭一次。
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Copied,
    Registered,
    Unchanged,
}

/// 正式入口：装到 %LOCALAPPDATA%\Microsoft\Windows\Fonts 并登记到当前用户，随即通知已运行的程序。
pub fn install(source: &Path) -> Result<Vec<(String, Outcome)>> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("缺少 LOCALAPPDATA")?;
    let target = PathBuf::from(local).join(r"Microsoft\Windows\Fonts");
    let report = install_into(&locked()?, source, &target, &Key::user()?)?;
    let mut changed = false;
    for (name, outcome) in &report {
        if *outcome != Outcome::Unchanged {
            changed = true;
            // SAFETY: 路径 NUL 结尾；把新字体加入本会话字体表，下次登录由登记项加载。
            unsafe {
                AddFontResourceW(wide(target.join(name).as_os_str()).as_ptr());
            }
        }
    }
    if changed {
        // SAFETY: 广播值类型通知；挂起的窗口最多等 1 秒，不阻塞安装结果。
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_FONTCHANGE,
                0,
                0,
                SMTO_ABORTIFHUNG,
                1000,
                null_mut(),
            );
        }
    }
    Ok(report)
}

pub(crate) fn install_into(
    fonts: &[Font],
    source: &Path,
    target: &Path,
    key: &Key,
) -> Result<Vec<(String, Outcome)>> {
    // 先校验全部来源，任何一个不符都不开始复制，避免装一半。
    let mut verified = Vec::with_capacity(fonts.len());
    for font in fonts {
        let name = resources::relative(&font.path)?;
        if name.components().count() != 1 {
            return Err(format!("字体清单只能是文件名: {}", font.path).into());
        }
        let from = source.join(&name);
        storage::reject_links(&from)?;
        let bytes = storage::read_file(&from, LIMIT)?;
        if bytes.len() as u64 != font.size || resources::sha256_bytes(&bytes) != font.sha256 {
            return Err(format!("字体与锁定清单不符: {}", from.display()).into());
        }
        verified.push((name, bytes, font));
    }
    fs::create_dir_all(target)?;
    storage::reject_links(target)?;
    let mut report = Vec::with_capacity(verified.len());
    for (name, bytes, font) in verified {
        let to = target.join(&name);
        let copied = match fs::symlink_metadata(&to) {
            Ok(_) => {
                storage::reject_links(&to)?;
                if resources::sha256(&to)? != font.sha256 {
                    return Err(format!("已有不同内容的同名字体，未覆盖: {}", to.display()).into());
                }
                false
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create(&to, &bytes)?;
                true
            }
            Err(error) => return Err(error.into()),
        };
        let registered = !key.registered(&to)?;
        if registered {
            key.set(&format!("{} (TrueType)", font.family), &to)?;
        }
        let outcome = match (copied, registered) {
            (true, _) => Outcome::Copied,
            (false, true) => Outcome::Registered,
            (false, false) => Outcome::Unchanged,
        };
        report.push((name.to_string_lossy().into_owned(), outcome));
    }
    Ok(report)
}

/// 新建并写满字体文件；写入失败只删除本次新建的文件。新建失败（如另一安装进程刚建了同名文件）不删任何东西。
fn create(to: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(to)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        // 写入失败不留下残缺字体文件。
        let _ = fs::remove_file(to);
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Registry::{KEY_ALL_ACCESS, RegLoadAppKeyW};

    #[test]
    fn a_failed_create_never_deletes_a_file_another_installer_just_made() -> Result<()> {
        let root = resources::new_directory(&std::env::temp_dir(), "tigertype-font-race")?;
        let to = root.join("甲.ttf");
        // 检查时不存在、新建前被另一进程抢先写入：本进程新建失败，对方的文件原样保留。
        fs::write(&to, b"other installer")?;
        assert!(create(&to, b"alpha").is_err());
        assert_eq!(fs::read(&to)?, b"other installer");
        fs::remove_file(&to)?;
        create(&to, b"alpha")?;
        assert_eq!(fs::read(&to)?, b"alpha");
        fs::remove_dir_all(&root)?;
        Ok(())
    }

    /// 私有应用配置单元：只存在于本测试的临时文件中，不触碰用户注册表。
    fn hive(file: &Path) -> Key {
        let mut key = null_mut();
        // SAFETY: 文件路径 NUL 结尾；不存在时由系统新建私有配置单元。
        let code = unsafe {
            RegLoadAppKeyW(
                wide(file.as_os_str()).as_ptr(),
                &mut key,
                KEY_ALL_ACCESS,
                0,
                0,
            )
        };
        assert_eq!(code, ERROR_SUCCESS);
        Key(key)
    }

    #[test]
    fn verified_fonts_install_once_never_overwrite_and_reject_mismatched_sources() -> Result<()> {
        let root = resources::new_directory(&std::env::temp_dir(), "tigertype-fonts")?;
        let (source, target) = (root.join("source"), root.join("target"));
        fs::create_dir(&source)?;
        let font = |name: &str, bytes: &[u8]| {
            fs::write(source.join(name), bytes).unwrap();
            Font {
                path: name.into(),
                size: bytes.len() as u64,
                sha256: resources::sha256_bytes(bytes),
                family: format!("合成 {name}"),
            }
        };
        let fonts = vec![font("甲.ttf", b"alpha"), font("Beta.ttf", b"beta")];
        let key = hive(&root.join("fonts.dat"));
        let first = install_into(&fonts, &source, &target, &key)?;
        assert!(first.iter().all(|(_, outcome)| *outcome == Outcome::Copied));
        assert_eq!(fs::read(target.join("甲.ttf"))?, b"alpha");
        assert!(
            key.registered(&target.join("甲.ttf"))? && key.registered(&target.join("Beta.ttf"))?
        );
        // 重复安装不复制、不重复登记。
        let again = install_into(&fonts, &source, &target, &key)?;
        assert!(
            again
                .iter()
                .all(|(_, outcome)| *outcome == Outcome::Unchanged)
        );
        // 已有同名而内容不同的文件不覆盖；来源被改动则一个都不装。
        fs::write(target.join("Beta.ttf"), b"user's own")?;
        assert!(install_into(&fonts, &source, &target, &key).is_err());
        assert_eq!(fs::read(target.join("Beta.ttf"))?, b"user's own");
        fs::write(source.join("甲.ttf"), b"tampered")?;
        let fresh = root.join("fresh");
        assert!(install_into(&fonts, &source, &fresh, &key).is_err());
        assert!(!fresh.exists());
        // 清单路径不得逃出来源目录。
        let escape = Font {
            path: "../甲.ttf".into(),
            ..fonts[0].clone()
        };
        assert!(install_into(&[escape], &source, &fresh, &key).is_err());
        drop(key);
        fs::remove_dir_all(&root)?;
        Ok(())
    }
    /// 完整资源验收：核对仓库内 resources/fonts 的真实字体，单列运行，不计入普通测试。
    #[test]
    #[ignore = "完整字体验收单列；必须显式执行"]
    fn bundled_fonts_match_the_lock() -> Result<()> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../resources/fonts");
        for font in locked()? {
            let path = dir.join(&font.path);
            assert_eq!(fs::metadata(&path)?.len(), font.size, "{}", font.path);
            assert_eq!(resources::sha256(&path)?, font.sha256, "{}", font.path);
        }
        Ok(())
    }
    #[test]
    fn lock_includes_the_bundled_fonts_and_rare_character_fallbacks() -> Result<()> {
        let fonts = locked()?;
        let names: Vec<_> = fonts.iter().map(|f| f.family.as_str()).collect();
        assert_eq!(
            names,
            [
                "LXGW WenKai GB Screen",
                "Cascadia Mono",
                "TumanPUA",
                "TH-Tshyn-P0",
                "TH-Tshyn-P1",
                "TH-Tshyn-P2",
            ]
        );
        assert!(fonts.iter().all(|f| f.size > 0 && f.sha256.len() == 64));
        Ok(())
    }
}
