//! TSF TIP DLL：类工厂、按键/焦点 sink、组合编辑组件与宿主内候选窗；只依赖 contracts/ipc，不加载词表或引擎。
#![cfg(windows)]
mod candidate;
mod edit;
mod host;
mod key;
mod service;
mod uia;
pub use edit::{EditReport, Editor, Verdict};
pub use service::{CLSID_TEXT_SERVICE, DllCanUnloadNow, DllGetClassObject};

use windows::{
    Win32::{
        Foundation::HMODULE,
        System::LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            GetModuleHandleExW,
        },
    },
    core::{PCWSTR, Result},
};

/// 本 TIP 所在模块（DLL，测试时为测试程序）；用于注册窗口类与定位同目录后台。
fn module() -> Result<HMODULE> {
    let mut handle = HMODULE::default();
    // SAFETY: FROM_ADDRESS 以本函数地址定位模块，不增加引用计数；模块由运行时 PIN 驻留至进程退出。
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(module as *const () as *const u16),
            &mut handle,
        )?;
    }
    Ok(handle)
}
