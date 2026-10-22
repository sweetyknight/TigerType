//! 每种位数在独立限时进程测试 DLL；PIN 不污染测试调度器，也不进入用户应用。
#![cfg(windows)]
use std::{
    os::windows::{ffi::OsStrExt, io::AsRawHandle, process::CommandExt},
    path::PathBuf,
    process::{Command, Stdio},
};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HMODULE, WAIT_OBJECT_0},
    System::{
        LibraryLoader::{
            GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
            LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
        },
        Threading::{CREATE_NO_WINDOW, WaitForSingleObject},
    },
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Library(HMODULE);
impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: 失败/断言路径仍归还宿主加载引用；成功手工归还后会忘记此守卫。
        unsafe {
            FreeLibrary(self.0);
        }
    }
}

#[test]
fn dll_residence_survives_host_release_and_drains_repeated_io() -> Result<()> {
    let target = if cfg!(target_arch = "x86") {
        "i686-pc-windows-msvc"
    } else {
        "x86_64-pc-windows-msvc"
    };
    // 从 Cargo 的本次产物清单取绝对路径，兼容自定义 target-dir，不猜 DLL 搜索目录。
    let output = Command::new("cargo")
        .args([
            "build",
            "--locked",
            "--offline",
            "-p",
            "tigertype-ipc",
            "--example",
            "resident_probe",
            "--target",
            target,
            "--message-format=json",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut paths = Vec::new();
    for line in output.stdout.split(|byte| *byte == b'\n') {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if value["reason"] == "compiler-artifact"
            && value["target"]["name"] == "resident_probe"
            && let Some(files) = value["filenames"].as_array()
        {
            paths.extend(
                files
                    .iter()
                    .filter_map(|file| file.as_str())
                    .filter(|file| file.ends_with(".dll"))
                    .map(PathBuf::from),
            );
        }
    }
    assert_eq!(paths.len(), 1, "必须只找到本次构建的一个探针 DLL");
    let path = paths.remove(0).canonicalize()?;
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--ignored", "--exact", "dll_probe_host", "--nocapture"])
        .env("TIGERTYPE_PROBE_DLL", &path)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    // SAFETY: 只等待本测试创建并持有的子进程；超时回收，不以超时充当验证成功。
    if unsafe { WaitForSingleObject(child.as_raw_handle(), 15_000) } != WAIT_OBJECT_0 {
        let _ = child.kill();
        let _ = child.wait();
        return Err("DLL 驻留探针超时或等待失败".into());
    }
    assert!(child.wait()?.success(), "DLL 驻留探针失败");
    Ok(())
}

#[test]
#[ignore = "仅由限时父测试调用，不直接加载 DLL 到用户应用"]
fn dll_probe_host() -> Result<()> {
    let path = PathBuf::from(std::env::var_os("TIGERTYPE_PROBE_DLL").ok_or("缺少探针路径")?);
    assert!(path.is_absolute());
    let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: 仅加载父测试刚构建的绝对路径，依赖搜索限定到该目录和 System32。
    let module = unsafe {
        LoadLibraryExW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if module.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    let library = Library(module);
    // SAFETY: 导出签名与本仓库探针一致，宿主持有加载引用。
    let address = unsafe { GetProcAddress(module, c"TigerTypeResidentProbe".as_ptr().cast()) }
        .ok_or("探针导出缺失")?;
    // SAFETY: 已核对导出名称，对应无参数 u32 system ABI，不能用于任意第三方 DLL。
    let probe: unsafe extern "system" fn() -> u32 = unsafe { std::mem::transmute(address) };
    // SAFETY: 模块当前仍有宿主引用；探针内明确 PIN 后才提交异步 I/O。
    assert_eq!(unsafe { probe() }, 0);
    // SAFETY: 释放宿主自己的唯一加载引用，PIN 必须继续保护回调代码。
    assert_ne!(unsafe { FreeLibrary(module) }, 0);
    std::mem::forget(library);
    // SAFETY: 仅查询现有映像，不重新加载；若 PIN 不存在则必须在调用函数前失败。
    assert_eq!(unsafe { GetModuleHandleW(wide.as_ptr()) }, module);
    for _ in 0..4 {
        // SAFETY: 上面已确认显式进程期驻留生效，引用释放后仍能创建/排空真实管道回调。
        assert_eq!(unsafe { probe() }, 0);
    }
    println!(
        "DLL PIN、EXE 默认拒绝、80 次超时 accept 和关闭排空通过；不表示可热卸载或 TSF 已接入。"
    );
    Ok(())
}
