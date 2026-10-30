//! TSF TIP DLL：类工厂、按键/焦点 sink、组合编辑组件、宿主内候选窗、任务栏输入法图标与注册入口；
//! 只依赖 contracts/input-client/windows-ipc，不加载词表或引擎；会话、按键规则与候选窗视图模型取自 input-client。
#![cfg(windows)]
mod candidate;
mod edit;
mod indicator;
mod key;
mod register;
mod service;
mod uia;
pub use edit::{EditReport, Editor, Verdict};
pub use register::{DllRegisterServer, DllUnregisterServer, PROFILE};
pub use service::{CLSID_TEXT_SERVICE, DllCanUnloadNow, DllGetClassObject};

use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};
use windows::{
    Win32::{
        Foundation::{E_FAIL, HMODULE},
        System::LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            GetModuleFileNameW, GetModuleHandleExW,
        },
    },
    core::{PCWSTR, Result},
};

/// 本 TIP 所在模块（DLL，测试时为测试程序）；用于注册窗口类与定位同目录后台。
fn module() -> Result<HMODULE> {
    let mut handle = HMODULE::default();
    // SAFETY: FROM_ADDRESS 以本函数地址定位模块，不增加引用计数；
    // 激活后模块由运行时 PIN 驻留，注册期间由 regsvr32 的加载引用保持。
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(module as *const () as *const u16),
            &mut handle,
        )?;
    }
    Ok(handle)
}
/// 本模块文件的绝对路径。
fn module_path() -> Result<PathBuf> {
    let module = module()?;
    let mut path = vec![0u16; 32768];
    // SAFETY: 缓冲区长度随切片传入，返回值为实际写入长度。
    let length = unsafe { GetModuleFileNameW(Some(module), &mut path) } as usize;
    if length == 0 || length >= path.len() {
        return Err(E_FAIL.into());
    }
    Ok(PathBuf::from(OsString::from_wide(&path[..length])))
}
