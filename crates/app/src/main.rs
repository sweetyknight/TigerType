//! 离线资源、输入后台与查字窗入口；不注册输入法或探测个人词库。
//! GUI 子系统：随登录启动的后台与开始菜单唤起的查字窗都不弹控制台；从终端运行时借用父控制台输出。
#![windows_subsystem = "windows"]
use std::{
    path::Path,
    time::{Duration, Instant},
};
use tigertype_app::{Result, import, installed, project_root};
use tigertype_contracts::{Key, Options};
use tigertype_engine::{Dictionary, Session};
use tigertype_ipc::{pipe::Runtime, service::Client};
use windows_sys::Win32::{
    System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE},
    UI::WindowsAndMessaging::{
        ASFW_ANY, AllowSetForegroundWindow, MB_ICONERROR, MB_OK, MessageBoxW,
    },
};

/// 连接本登录会话中与本程序同映像的后台；管理请求同样要求映像一致。
fn backend(deadline: Instant) -> std::result::Result<Client, tigertype_ipc::Error> {
    let runtime = Runtime::new()?;
    Client::connect(&runtime, &std::env::current_exe()?, "backend", deadline)
}

fn run(args: &[std::ffi::OsString]) -> Result<()> {
    if args.is_empty() || args == ["--help"] {
        println!(
            "TigerType Rust 核心工具\n  serve [<数据包绝对路径> <独立受管数据目录绝对路径>]\n  lookup-window\n  stop\n  import-resources <新数据包绝对路径>\n  verify-package <数据包绝对路径>\n  lookup <数据包绝对路径> <拼音或汉字>\n  demo-input <数据包绝对路径> <编码>\n  install-fonts <随包字体目录绝对路径>\nserve 省略路径时使用安装版位置（程序目录的数据包与 %APPDATA%\\TigerType）；lookup-window 请本会话后台显示查字窗，后台未运行时以安装版位置启动后台并显示。"
        );
        return Ok(());
    }
    match (args[0].to_str(), args.len()) {
        (Some("stop"), 1) => {
            let deadline = Instant::now() + Duration::from_millis(50);
            backend(deadline)?.stop_backend(deadline)?;
        }
        (Some("serve"), 1) => {
            let (package, directory) = installed()?;
            tigertype_app::backend::run(&package, &directory, false)?;
        }
        (Some("serve"), 3) => {
            tigertype_app::backend::run(Path::new(&args[1]), Path::new(&args[2]), false)?;
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
                    client.show_lookup(deadline)?;
                }
                // 本会话没有后台：本进程即成为后台并立即显示查字窗。
                Err(tigertype_ipc::Error::Busy) => {
                    let (package, directory) = installed()?;
                    tigertype_app::backend::run(&package, &directory, true)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        (Some("install-fonts"), 2) => {
            for (name, outcome) in tigertype_app::fonts::install(Path::new(&args[1]))? {
                let state = match outcome {
                    tigertype_app::fonts::Outcome::Copied => "已安装",
                    tigertype_app::fonts::Outcome::Registered => "已登记",
                    tigertype_app::fonts::Outcome::Unchanged => "已存在，未改动",
                };
                println!("{name}：{state}");
            }
        }
        (Some("import-resources"), 2) => {
            let package = import::build(&project_root()?.join("vendor/tiger"))?;
            import::save(&package, Path::new(&args[1]))?;
            println!(
                "已校验并写入 {} 条单字编码；未修改原资源。",
                package.data.entries.len()
            );
        }
        (Some("verify-package"), 2) => {
            let package = import::load(Path::new(&args[1]))?;
            println!(
                "数据包校验通过：{} 条编码，{} 条单字拼音，{} 条学习资料。",
                package.data.entries.len(),
                package.data.pinyin.len(),
                package.data.details.len()
            );
        }
        (Some("lookup" | "demo-input"), 3) => {
            let package = import::load(Path::new(&args[1]))?;
            let dictionary = Dictionary::new(package.data)?;
            let query = args[2].to_str().ok_or("查询不是有效 Unicode")?;
            if args[0] == "lookup" {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&dictionary.lookup(query, 0)?)?
                );
            } else {
                let mut session = Session::new(&dictionary, Options::default());
                for c in query.chars() {
                    let outcome = session.process(if c == ' ' {
                        Key::Space
                    } else {
                        Key::Character(c)
                    })?;
                    println!("{}", serde_json::to_string(&outcome)?);
                }
            }
        }
        _ => return Err("无效参数；请运行 --help".into()),
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
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
