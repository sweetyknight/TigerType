//! Windows 动态加载边界：校验后才执行 DLL，且禁止从当前目录搜索依赖。

use crate::{Error, Result, raw};
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt};
use std::{fs::File, io, path::Path};
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HMODULE},
    System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
        LoadLibraryExW,
    },
};

pub const DLL_SHA256: &str = "86b4c7357d4c6d293ce5589b234d8859ca2ac30923a03bedfa3926eeaf97fb0b";

pub struct Library {
    #[cfg(windows)]
    module: HMODULE,
    // Windows 文件共享只允许读取，防止校验到加载之间被覆盖或重命名。
    _file: File,
}

impl Library {
    #[cfg(windows)]
    pub fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() || !cfg!(target_arch = "x86_64") {
            return Err(Error::Invalid("引擎只支持 x64，DLL 必须使用绝对路径"));
        }
        let path = path.canonicalize()?;
        let mut file = File::options().read(true).share_mode(1).open(&path)?;
        let mut hash = Sha256::new();
        io::copy(&mut file, &mut hash)?;
        if format!("{:x}", hash.finalize()) != DLL_SHA256 {
            return Err(Error::Invalid("rime.dll SHA256 不匹配"));
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: 路径以 NUL 结尾，已验证文件保持只读锁直到 DLL 释放。
        let module = unsafe {
            LoadLibraryExW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if module.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self {
            module,
            _file: file,
        })
    }

    #[cfg(not(windows))]
    pub fn open(_: &Path) -> Result<Self> {
        Err(Error::Invalid("引擎仅支持 Windows x64"))
    }

    #[cfg(windows)]
    pub fn api(&self) -> Result<raw::Api> {
        // SAFETY: 句柄仍有效，导出名称来自固定版本官方 API。
        let symbol = unsafe { GetProcAddress(self.module, c"rime_get_api".as_ptr().cast()) }
            .ok_or_else(io::Error::last_os_error)?;
        // SAFETY: 固定 DLL 的 rime_get_api 使用 C ABI，返回自描述 API 表。
        let get_api: unsafe extern "C" fn() -> *const raw::Api =
            unsafe { std::mem::transmute(symbol) };
        // SAFETY: DLL 的返回表在 Library 整个生命周期内有效。
        unsafe { checked_api(get_api()) }
    }

    #[cfg(not(windows))]
    pub fn api(&self) -> Result<raw::Api> {
        Err(Error::Invalid("引擎仅支持 Windows x64"))
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: Engine 先 finalize，再释放 Library；不会留下仍借用的会话。
        unsafe {
            FreeLibrary(self.module);
        }
    }
}

/// 调用者保证非空地址至少有一个 int，且声明长度内的表有效。
unsafe fn checked_api(pointer: *const raw::Api) -> Result<raw::Api> {
    if pointer.is_null() {
        return Err(Error::Invalid("rime_get_api 返回空指针"));
    }
    // SAFETY: 先只读头部，确认长度后才读取整个所需前缀。
    let bytes = unsafe { pointer.cast::<i32>().read_unaligned() };
    if bytes < raw::data_size::<raw::Api>() {
        return Err(Error::Invalid("RimeApi 表长度不足"));
    }
    // SAFETY: 调用者提供的表覆盖上述已检查前缀；所有槽位均为可空指针。
    let api = unsafe { pointer.read_unaligned() };
    macro_rules! require {
        ($($field:ident),+ $(,)?) => { $(
            if api.$field.is_none() { return Err(Error::MissingApi(stringify!($field))); }
        )+ };
    }
    require!(
        setup,
        initialize,
        finalize,
        deployer_initialize,
        deploy,
        create_session,
        find_session,
        destroy_session,
        process_key,
        clear_composition,
        get_commit,
        free_commit,
        get_context,
        free_context,
        get_status,
        free_status,
        set_option,
        get_option,
        select_schema,
        schema_open,
        config_close,
        config_get_bool,
        config_get_int,
        find_module,
        get_version
    );
    Ok(api)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(all(windows, target_arch = "x86_64"))]
    fn rejects_unverified_dll_before_loading_code() {
        use std::io::Write;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "tigerrime-invalid-{}-{nonce}.dll",
            std::process::id()
        ));
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(b"not executable code").unwrap();
        drop(file);
        assert!(matches!(
            Library::open(&path),
            Err(Error::Invalid("rime.dll SHA256 不匹配"))
        ));
        // 仅删除此测试刚创建的单个文件，不递归清理临时目录。
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_null_short_and_missing_api_without_calling_it() {
        // SAFETY: 空指针由函数显式拒绝；短表测试只暴露一个有效 int。
        assert!(unsafe { checked_api(std::ptr::null()) }.is_err());
        let short = 0_i32;
        // SAFETY: 长度为零，函数不得读取 int 后的任何数据。
        assert!(unsafe { checked_api((&short as *const i32).cast()) }.is_err());
        let table = raw::Api {
            data_size: raw::data_size::<raw::Api>(),
            ..Default::default()
        };
        assert!(matches!(
            // SAFETY: 测试表完整初始化，但所有函数槽为空，必须报错。
            unsafe { checked_api(&table) },
            Err(Error::MissingApi("setup"))
        ));
    }
}
