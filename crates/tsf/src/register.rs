//! 注册/注销入口：由用户在单独授权后以管理员身份运行 regsvr32 调用，激活与输入路径从不调用，也不切换当前输入法。
//! COM 类写入 HKLM\SOFTWARE\Classes（32 位 DLL 经 WOW64 写入 32 位视图，混合位数宿主须两种位数各注册一次），
//! TSF 语言配置与类别经系统接口登记；任一步失败回滚已写内容。
use crate::service::{BACKEND, CLSID_TEXT_SERVICE};
use std::{ffi::OsString, os::windows::ffi::OsStrExt, panic::catch_unwind, path::Path};
use windows::{
    Win32::{
        Foundation::{E_INVALIDARG, E_UNEXPECTED, ERROR_FILE_NOT_FOUND, RPC_E_CHANGED_MODE, S_OK},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            Registry::{
                HKEY, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
                RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
            },
        },
        UI::{Input::KeyboardAndMouse::HKL, TextServices::*},
    },
    core::{GUID, HRESULT, HSTRING, PCWSTR, Result},
};

/// 简体中文语言配置标识；类工厂、注册与注销共用，开发期不兼容旧值。
pub const PROFILE: GUID = GUID::from_u128(0x2097c544_b7d6_4f5c_9f4d_6db1be3ee319);
const LANGUAGE: u16 = 0x0804;
const NAME: &str = "TigerType 虎码";
/// 键盘 TIP、沉浸式（UWP）与桌面托盘支持；未实现 UILess 与显示属性，不登记相应类别。
const CATEGORIES: [GUID; 3] = [
    GUID_TFCAT_TIP_KEYBOARD,
    GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
    GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
];

fn class_key() -> String {
    format!("SOFTWARE\\Classes\\CLSID\\{{{CLSID_TEXT_SERVICE:?}}}")
}
/// COM 类的注册表内容（相对根键）：子键、值名（None 为默认值）、字符串。
fn class_values(dll: &Path) -> [(String, Option<&'static str>, OsString); 3] {
    let key = class_key();
    let server = format!("{key}\\InprocServer32");
    [
        (key, None, NAME.into()),
        (server.clone(), None, dll.as_os_str().to_owned()),
        // 与 TSF 线程模型一致：只在宿主 STA 内创建与回调。
        (server, Some("ThreadingModel"), "Apartment".into()),
    ]
}
/// 宿主只连接与 DLL 同目录的后台；注册前确认路径为绝对路径且后台已在位，不在此复制文件或改权限。
fn validate(dll: &Path) -> Result<()> {
    if dll.is_absolute() && dll.is_file() && dll.with_file_name(BACKEND).is_file() {
        Ok(())
    } else {
        Err(E_INVALIDARG.into())
    }
}
fn write(root: HKEY, values: &[(String, Option<&'static str>, OsString)]) -> Result<()> {
    for (path, name, data) in values {
        let mut key = HKEY::default();
        // SAFETY: 路径与输出句柄在调用期间有效；成功打开的句柄在本次循环内关闭。
        unsafe {
            RegCreateKeyExW(
                root,
                &HSTRING::from(path.as_str()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut key,
                None,
            )
            .ok()?;
        }
        let bytes: Vec<u8> = data
            .encode_wide()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        let name = name.map(HSTRING::from);
        let name = name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
        // SAFETY: 值名与数据在调用期间有效，数据为含结尾空字符的 UTF-16。
        let result = unsafe { RegSetValueExW(key, name, None, REG_SZ, Some(&bytes)) };
        // SAFETY: 关闭本次打开的句柄。
        let _ = unsafe { RegCloseKey(key) };
        result.ok()?;
    }
    Ok(())
}
/// 删除整个子树；本就不存在视为已删除，使注销可重复执行。
fn remove(root: HKEY, path: &str) -> Result<()> {
    // SAFETY: 只删除本服务 CLSID 下的子树，路径由常量生成。
    match unsafe { RegDeleteTreeW(root, &HSTRING::from(path)) } {
        ERROR_FILE_NOT_FOUND => Ok(()),
        code => code.ok(),
    }
}

/// 在当前线程进入单线程套间；调用方已初始化（含不同模式）时不重复反初始化。
struct Apartment(bool);
impl Apartment {
    fn enter() -> Result<Self> {
        // SAFETY: 与 Drop 中的 CoUninitialize 配对。
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if result == RPC_E_CHANGED_MODE {
            return Ok(Self(false));
        }
        result.ok()?;
        Ok(Self(true))
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: 只撤销本对象成功进行的初始化。
            unsafe { CoUninitialize() };
        }
    }
}
/// 取得 TSF 配置与类别管理器；两者先于套间释放。
fn tsf<T>(op: impl FnOnce(&ITfInputProcessorProfileMgr, &ITfCategoryMgr) -> T) -> Result<T> {
    let _apartment = Apartment::enter()?;
    // SAFETY: 系统进程内组件，引用随作用域释放。
    let profiles: ITfInputProcessorProfileMgr =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)? };
    // SAFETY: 同上。
    let categories: ITfCategoryMgr =
        unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)? };
    Ok(op(&profiles, &categories))
}

fn register(dll: &Path) -> Result<()> {
    validate(dll)?;
    let result = write(HKEY_LOCAL_MACHINE, &class_values(dll)).and_then(|()| {
        tsf(|profiles, categories| -> Result<()> {
            let name: Vec<u16> = NAME.encode_utf16().collect();
            // SAFETY: GUID 与名称在调用期间有效；无图标资源，传空图标文件。
            unsafe {
                profiles.RegisterProfile(
                    &CLSID_TEXT_SERVICE,
                    LANGUAGE,
                    &PROFILE,
                    &name,
                    &[],
                    0,
                    HKL::default(),
                    0,
                    true,
                    0,
                )?;
            }
            for category in &CATEGORIES {
                // SAFETY: 同上。
                unsafe {
                    categories.RegisterCategory(
                        &CLSID_TEXT_SERVICE,
                        category,
                        &CLSID_TEXT_SERVICE,
                    )?
                };
            }
            Ok(())
        })?
    });
    if result.is_err() {
        // 回滚已写入的部分；回滚自身失败不掩盖原始错误。
        let _ = unregister();
    }
    result
}
/// 尽力注销全部内容，单项失败不阻止其余步骤，返回第一个错误。
fn unregister() -> Result<()> {
    let mut first = None;
    let mut note = |result: Result<()>| {
        if let Err(error) = result {
            first.get_or_insert(error);
        }
    };
    match tsf(|profiles, categories| {
        // SAFETY: 以注册时的同一组标识注销。
        let mut results =
            vec![unsafe { profiles.UnregisterProfile(&CLSID_TEXT_SERVICE, LANGUAGE, &PROFILE, 0) }];
        for category in &CATEGORIES {
            // SAFETY: 同上。
            results.push(unsafe {
                categories.UnregisterCategory(&CLSID_TEXT_SERVICE, category, &CLSID_TEXT_SERVICE)
            });
        }
        results
    }) {
        Ok(results) => results.into_iter().for_each(&mut note),
        Err(error) => note(Err(error)),
    }
    note(remove(HKEY_LOCAL_MACHINE, &class_key()));
    first.map_or(Ok(()), Err)
}
fn report(result: Result<()>) -> HRESULT {
    result.map_or_else(|error| error.code(), |()| S_OK)
}

/// regsvr32 注册入口：须管理员权限，只由用户在单独授权后手动执行。
#[unsafe(no_mangle)]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    catch_unwind(|| report(crate::module_path().and_then(|dll| register(&dll))))
        .unwrap_or(E_UNEXPECTED)
}
/// regsvr32 /u 注销入口：尽力移除全部注册内容；重复注销时系统对未登记项的报错照实返回。
#[unsafe(no_mangle)]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    catch_unwind(|| report(unregister())).unwrap_or(E_UNEXPECTED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::windows::ffi::OsStringExt, path::PathBuf};
    use windows::Win32::System::Registry::{
        KEY_ALL_ACCESS, RRF_RT_REG_SZ, RegGetValueW, RegLoadAppKeyW,
    };

    /// 新建的私有临时目录；删除前核对仍位于系统临时目录内。
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "tigertype-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let temp = std::env::temp_dir().canonicalize().unwrap();
            if let Ok(path) = self.0.canonicalize()
                && path.starts_with(&temp)
                && path != temp
            {
                let _ = fs::remove_dir_all(path);
            }
        }
    }
    /// 私有应用配置单元：只存在于本测试的临时文件中，不触碰系统或用户注册表。
    struct Hive(HKEY);
    impl Hive {
        fn load(file: &Path) -> Self {
            let mut root = HKEY::default();
            // SAFETY: 文件路径在调用期间有效；不存在时由系统新建私有配置单元。
            unsafe {
                RegLoadAppKeyW(
                    &HSTRING::from(file.as_os_str()),
                    &mut root,
                    KEY_ALL_ACCESS.0,
                    0,
                    None,
                )
                .ok()
                .unwrap();
            }
            Self(root)
        }
        fn read(&self, path: &str, name: Option<&str>) -> Option<OsString> {
            let name = name.map(HSTRING::from);
            let name = name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
            let mut buffer = vec![0u16; 1024];
            let mut size = (buffer.len() * 2) as u32;
            // SAFETY: 输出缓冲区与字节数匹配，只读本测试配置单元。
            let code = unsafe {
                RegGetValueW(
                    self.0,
                    &HSTRING::from(path),
                    name,
                    RRF_RT_REG_SZ,
                    None,
                    Some(buffer.as_mut_ptr().cast()),
                    Some(&mut size),
                )
            };
            code.is_ok()
                .then(|| OsString::from_wide(&buffer[..size as usize / 2 - 1]))
        }
    }
    impl Drop for Hive {
        fn drop(&mut self) {
            // SAFETY: 关闭最后一个句柄后系统卸载私有配置单元。
            let _ = unsafe { RegCloseKey(self.0) };
        }
    }

    #[test]
    fn registration_content_targets_this_clsid_with_an_apartment_inproc_server() {
        let dll = Path::new(r"C:\Staging\TigerType\tigertype_tsf.dll");
        let key = r"SOFTWARE\Classes\CLSID\{79A65161-8689-43AC-A57C-574C6A74922A}";
        let server = format!(r"{key}\InprocServer32");
        assert_eq!(
            class_values(dll),
            [
                (key.to_owned(), None, "TigerType 虎码".into()),
                (server.clone(), None, dll.as_os_str().to_owned()),
                (server, Some("ThreadingModel"), "Apartment".into()),
            ]
        );
        assert_ne!(PROFILE, CLSID_TEXT_SERVICE);
        assert_eq!(LANGUAGE, 0x0804);
        assert_eq!(
            CATEGORIES,
            [
                GUID_TFCAT_TIP_KEYBOARD,
                GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
                GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT
            ]
        );
    }

    #[test]
    fn registration_refuses_relative_paths_and_a_missing_backend_before_touching_the_registry() {
        let scratch = Scratch::new("register-validate");
        let dll = scratch.0.join("tigertype_tsf.dll");
        assert!(validate(Path::new("tigertype_tsf.dll")).is_err());
        assert!(validate(&dll).is_err());
        fs::write(&dll, b"").unwrap();
        assert!(validate(&dll).is_err());
        fs::write(scratch.0.join(BACKEND), b"").unwrap();
        assert!(validate(&dll).is_ok());
    }

    #[test]
    fn writer_round_trips_non_ascii_paths_and_removal_is_repeatable_in_a_private_hive() {
        let scratch = Scratch::new("register-hive");
        let hive = Hive::load(&scratch.0.join("classes.dat"));
        let dll = scratch.0.join("暂存 目录").join("tigertype_tsf.dll");
        let values = class_values(&dll);
        write(hive.0, &values).unwrap();
        for (path, name, data) in &values {
            assert_eq!(hive.read(path, *name).as_ref(), Some(data));
        }
        remove(hive.0, &class_key()).unwrap();
        assert!(hive.read(&values[1].0, None).is_none());
        remove(hive.0, &class_key()).unwrap();
    }
}
