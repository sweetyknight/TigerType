//! Windows 命令行：离线资源、输入后台与查字窗入口；不注册输入法或探测个人词库。
//! 程序为 GUI 子系统：从终端运行时借用父控制台输出，无控制台时启动失败弹错误框。
use super::{fonts, installed, server};
use crate::Result;
use std::{
    ffi::c_void,
    path::Path,
    time::{Duration, Instant},
};
use tigertype_backend::{Dictionary, import};
use tigertype_contracts::protocol::Panel;
use tigertype_windows_ipc::{pipe::Runtime, service::Client};
use windows_sys::Win32::{
    System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE},
    System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
    UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    UI::WindowsAndMessaging::{
        ASFW_ANY, AllowSetForegroundWindow, MB_ICONERROR, MB_OK, MessageBoxW,
    },
};

/// 仅在系统提供时设置进程 DPI；通过动态解析避免旧版 Windows 10 因缺少导入而无法启动。
fn set_process_dpi_awareness() {
    // SAFETY: 只查询已加载的 user32 模块句柄，不取得新所有权。
    let module = unsafe { GetModuleHandleA(c"user32.dll".as_ptr().cast()) };
    if module.is_null() {
        return;
    }
    // SAFETY: 模块句柄来自上面的系统查询，名称为固定 NUL 结尾字符串。
    let Some(address) =
        (unsafe { GetProcAddress(module, c"SetProcessDpiAwarenessContext".as_ptr().cast()) })
    else {
        return;
    };
    // SAFETY: 名称来自 user32，签名与 Windows SDK 的 SetProcessDpiAwarenessContext 一致。
    let set: unsafe extern "system" fn(*mut c_void) -> i32 =
        unsafe { std::mem::transmute(address) };
    // SAFETY: 地址由固定 API 名称解析，参数为 SDK 定义的 PMv2 上下文值。
    unsafe {
        let _ = set(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// 连接本登录会话中与本程序同映像的后台；停止请求另要求调用方映像一致。
fn backend(deadline: Instant) -> std::result::Result<Client, tigertype_windows_ipc::Error> {
    let runtime = Runtime::new()?;
    Client::connect(&runtime, &std::env::current_exe()?, "backend", deadline)
}

fn run(args: &[std::ffi::OsString]) -> Result<()> {
    if args.is_empty() || args == ["--help"] {
        println!(
            "TigerType Rust 核心工具\n  serve [<码表目录绝对路径> <独立受管数据目录绝对路径>]\n  lookup-window\n  stop\n  lookup <码表目录绝对路径> <拼音、虎码或汉字/词>\n  demo-input <码表目录绝对路径> <编码>\n  install-fonts <随包字体目录绝对路径>\nserve 省略路径时使用安装版位置（程序目录下的 tiger 码表目录与 %APPDATA%\\TigerType）；lookup-window 请本会话后台显示查字窗，后台未运行时以安装版位置启动后台并显示。"
        );
        return Ok(());
    }
    match (args[0].to_str(), args.len()) {
        (Some("stop"), 1) => {
            // 含等后台退出（其关闭管道最多等 2s）；安装程序在此之后才检测占用、替换文件。
            let deadline = Instant::now() + Duration::from_secs(5);
            backend(deadline)?.stop_backend(deadline)?;
        }
        (Some("serve"), 1) => {
            let (package, directory) = installed()?;
            server::run(&package, &directory, false)?;
        }
        (Some("serve"), 3) => {
            server::run(Path::new(&args[1]), Path::new(&args[2]), false)?;
        }
        (Some("lookup-window"), 1) => {
            let deadline = Instant::now() + Duration::from_secs(1);
            match backend(deadline) {
                Ok(mut client) => {
                    // 本进程由用户点击启动、持有前台权；放行后台把查字窗切到前台。
                    // SAFETY: 只放宽下一次前台切换限制，不涉及指针或句柄。
                    unsafe {
                        AllowSetForegroundWindow(ASFW_ANY);
                    }
                    client.show(Panel::Lookup, deadline)?;
                }
                // 本会话没有后台：本进程即成为后台并立即显示查字窗。
                Err(tigertype_windows_ipc::Error::Busy) => {
                    let (package, directory) = installed()?;
                    server::run(&package, &directory, true)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        (Some("install-fonts"), 2) => {
            for (name, outcome) in fonts::install(Path::new(&args[1]))? {
                let state = match outcome {
                    fonts::Outcome::Copied => "已安装",
                    fonts::Outcome::Registered => "已登记",
                    fonts::Outcome::Unchanged => "已存在，未改动",
                };
                println!("{name}：{state}");
            }
        }
        (Some("lookup" | "demo-input"), 3) => {
            let dictionary = Dictionary::new(import::load(Path::new(&args[1]))?)?;
            let query = args[2].to_str().ok_or("查询不是有效 Unicode")?;
            if args[0] == "lookup" {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&dictionary.lookup(query, 0)?)?
                );
            } else {
                for outcome in tigertype_backend::demo(&dictionary, query)? {
                    println!("{}", serde_json::to_string(&outcome)?);
                }
            }
        }
        _ => return Err("无效参数；请运行 --help".into()),
    }
    Ok(())
}
pub fn main() -> std::process::ExitCode {
    // 缺少该 API 时安全回退到系统默认 DPI，不影响命令行入口启动。
    set_process_dpi_awareness();
    // SAFETY: 只查询本进程标准句柄或附加父进程已有的控制台，不新建窗口；重定向时句柄已由启动信息给出。
    let console = unsafe {
        !GetStdHandle(STD_ERROR_HANDLE).is_null() || AttachConsole(ATTACH_PARENT_PROCESS) != 0
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match run(&args) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            // 开机自启与开始菜单唤起没有控制台，失败须让用户看见；stop 等由安装程序隐藏调用，不弹窗。
            if !console
                && matches!(
                    args.first().and_then(|a| a.to_str()),
                    Some("serve" | "lookup-window")
                )
            {
                let text: Vec<u16> = format!("TigerType 未能启动：{error}")
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                let title: Vec<u16> = "TigerType".encode_utf16().chain(Some(0)).collect();
                // SAFETY: 两个 UTF-16 字符串 NUL 结尾且在调用期间有效；无所有者窗口。
                unsafe {
                    MessageBoxW(
                        std::ptr::null_mut(),
                        text.as_ptr(),
                        title.as_ptr(),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
            std::process::ExitCode::FAILURE
        }
    }
}
