//! Linux 主线程组装：GApplication 只转发窗口动作，真实查询和配置事务留在固定工作线程。
use crate::{platform, runtime::Runtime};
use gio::{glib, prelude::*};
use glib::variant::ToVariant;
use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    process::Command,
    rc::Rc,
    sync::mpsc,
    time::Duration,
};
use tigertype_backend::{resources, storage};
use tigertype_contracts::{
    Options,
    protocol::Panel,
    ui::{Reply, Request},
};
use tigertype_linux_input_method::ibus::Server;
use tigertype_linux_ui::gtk::{self, Views};

struct Running {
    runtime: Runtime,
    options: Options,
    config: PathBuf,
    views: Option<Views>,
}
impl Running {
    fn show(&mut self, panel: Panel) -> Result<(), String> {
        if self.views.is_none() {
            let worker = self.runtime.worker.clone();
            self.views = Some(Views::new(
                self.options,
                self.config.clone(),
                move |request, completion| {
                    worker.submit(request, move |reply| completion.finish(reply));
                },
            )?);
        }
        self.views.as_ref().unwrap().show(panel);
        Ok(())
    }
}

pub fn main() -> Result<(), String> {
    // SAFETY: 只查询身份；不让提权安装器身份启动个人输入后台。
    if unsafe { libc::geteuid() } == 0 {
        return Err("后台必须以普通用户身份运行".into());
    }
    let mut args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "integration") {
        if args.len() != 3 {
            return Err("登记参数不符".into());
        }
        let framework = crate::integration::parse(args[1].to_str().ok_or("框架名非法")?)
            .map_err(|e| e.to_string())?;
        return match args[2].to_str() {
            Some("remove") => {
                crate::integration::shell_appearance(true).map_err(|e| e.to_string())?;
                crate::integration::configure(framework, true).map_err(|e| e.to_string())
            }
            Some("install") => {
                crate::integration::prepare_login(framework).map_err(|e| e.to_string())?;
                if framework == platform::Framework::IBus {
                    crate::integration::shell_appearance(false).map_err(|e| e.to_string())?;
                    // IBus 不热重读引擎注册表；仅刷新输入框架，不注销桌面或关闭用户应用。
                    crate::integration::refresh_ibus().map_err(|e| e.to_string())?;
                    return crate::integration::configure(framework, false)
                        .map_err(|e| e.to_string());
                }
                // Fcitx5 原生插件仍由框架加载，不能把仅复制文件当成插件已经生效。
                if let Err(error) = crate::integration::configure(framework, false) {
                    eprintln!("文件已安装，注销重登后自动完成框架登记：{error}");
                }
                Ok(())
            }
            _ => Err("未知登记动作".into()),
        };
    }
    let login = if args.first().is_some_and(|arg| arg == "login") {
        if args.len() != 2 {
            return Err("登录参数不符".into());
        }
        let framework = crate::integration::parse(args[1].to_str().ok_or("框架名非法")?)
            .map_err(|e| e.to_string())?;
        if framework == platform::Framework::IBus {
            return crate::integration::configure(framework, false).map_err(|e| e.to_string());
        }
        args = vec!["fcitx".into()];
        Some(framework)
    } else {
        None
    };
    let command = args.first().and_then(|s| s.to_str()).unwrap_or("settings");
    if command == "diagnose" && args.len() == 1 {
        return diagnose();
    }
    if !["lookup", "settings", "serve", "ibus", "fcitx", "stop"].contains(&command)
        || !(args.len() <= 1 || args.len() == 3)
    {
        return Err("用法：tigertype-linux-app [lookup|settings|serve|ibus|fcitx|stop] [码表绝对目录 数据绝对目录]，或 diagnose".into());
    }
    // 已安装程序首次从设置入口启动也登记输入边界，不能留下只会显示窗口却挡住输入法的后台。
    let service_mode = if args.len() <= 1 && matches!(command, "lookup" | "settings") {
        crate::integration::name(crate::integration::selected().map_err(|e| e.to_string())?)
    } else {
        command
    };
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("缺少 HOME")?);
    let (tables, data) = if args.len() == 3 {
        (PathBuf::from(&args[1]), PathBuf::from(&args[2]))
    } else {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let tables = exe
            .parent()
            .and_then(|p| p.parent())
            .ok_or("安装布局不符")?
            .join("share/tigertype/tiger");
        let xdg = std::env::var_os("XDG_DATA_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        (
            tables,
            storage::user_data_root(&home, xdg.as_deref()).map_err(|e| e.to_string())?,
        )
    };
    if !tables.is_absolute() || !data.is_absolute() {
        return Err("码表和数据路径必须为绝对路径".into());
    }
    storage::reject_links(data.parent().ok_or("数据目录没有父级")?).map_err(|e| e.to_string())?;
    // 会话与隔离数据目录分别进入名字，开发验证不会把动作发给正式后台。
    let session = std::env::var("WAYLAND_DISPLAY")
        .or_else(|_| std::env::var("DISPLAY"))
        .or_else(|_| std::env::var("XDG_SESSION_ID"))
        .map_err(|_| "缺少图形登录会话")?;
    let identity = resources::sha256_bytes(format!("{}\n{session}", data.display()).as_bytes());
    let app = gio::Application::new(
        Some(&format!("local.tigertype.Session.s{}", &identity[..24])),
        gio::ApplicationFlags::FLAGS_NONE,
    );
    let main = glib::MainLoop::new(None, false);
    let running = Rc::new(RefCell::new(None::<Running>));
    for (name, panel) in [
        ("lookup", Some(Panel::Lookup)),
        ("settings", Some(Panel::Settings)),
        ("stop", None),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let state = running.clone();
        let quit = main.clone();
        action.connect_activate(move |_, _| {
            let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
                match panel {
                    Some(panel) => state.borrow_mut().as_mut().ok_or("后台未就绪")?.show(panel),
                    None => {
                        quit.quit();
                        Ok(())
                    }
                }
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("{error}"),
                Err(_) => {
                    eprintln!("窗口动作失败");
                    quit.quit();
                }
            }
        });
        app.add_action(&action);
    }
    app.register(gio::Cancellable::NONE)
        .map_err(|e| e.to_string())?;
    if app.is_remote() {
        if matches!(command, "ibus" | "fcitx") {
            return Err(
                "当前数据目录已有后台；请先停止再以 ibus 启动，不能假称已登记输入法".into(),
            );
        }
        if !matches!(command, "serve" | "ibus") {
            app.activate_action(command, None);
        }
        if let Some(bus) = app.dbus_connection() {
            bus.flush_sync(gio::Cancellable::NONE)
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    if command == "stop" {
        return Err("当前会话没有运行的后台".into());
    }
    gtk::initialize()?;
    let config = data.join("config/settings.toml");
    let runtime = Runtime::start(tables, data).map_err(|e| e.to_string())?;
    let (send, receive) = mpsc::sync_channel(1);
    // 启动期用一次性回复等待加载，不用计时器轮询 ready；输入回调只在就绪后登记。
    runtime
        .worker
        .submit(Request::ReloadSettings, move |reply| {
            let _ = send.send(reply);
        });
    let options = match receive.recv_timeout(Duration::from_secs(30)) {
        Ok(Reply::Saved(Ok(options))) => options,
        result => {
            runtime.shutdown();
            return Err(format!("后台初始化失败：{result:?}"));
        }
    };
    let connector = runtime.connector.clone();
    *running.borrow_mut() = Some(Running {
        runtime,
        options,
        config,
        views: None,
    });
    let result = (|| {
        let _watch = login.map(crate::integration::watch);
        let _fcitx = if service_mode == "fcitx" {
            let state = running.clone();
            let connector = connector.clone();
            Some(tigertype_linux_input_method::fcitx::Server::new(
                app.dbus_connection().ok_or("缺少会话总线")?,
                move || Box::new(connector.link()),
                move |panel| {
                    if let Some(running) = state.borrow_mut().as_mut()
                        && let Err(error) = running.show(panel)
                    {
                        eprintln!("{error}");
                    }
                },
            )?)
        } else {
            None
        };
        let _server = if service_mode == "ibus" {
            let state = running.clone();
            let connection = ibus_connection()?;
            let quit = main.clone();
            connection.connect_closed(move |_, _, _| quit.quit());
            let daemon = bus_call(
                &connection,
                "GetNameOwner",
                ("org.freedesktop.IBus",).to_variant(),
            )?
            .get::<(String,)>()
            .ok_or("IBus daemon 回复不符")?
            .0;
            // 名字出现即可能收到 CreateEngine；必须先导出工厂，不能让 daemon 抢先调用空路径。
            let server = Server::new(
                connection.clone(),
                daemon,
                move || Box::new(connector.link()),
                move |panel| {
                    if let Some(running) = state.borrow_mut().as_mut()
                        && let Err(error) = running.show(panel)
                    {
                        eprintln!("{error}");
                    }
                },
            )?;
            let reply = bus_call(
                &connection,
                "RequestName",
                ("org.freedesktop.IBus.TigerType", 4_u32).to_variant(),
            )?;
            if reply.get::<(u32,)>() != Some((1,)) {
                return Err("TigerType IBus 服务已被占用".into());
            }
            Some(server)
        } else {
            None
        };
        if matches!(command, "lookup" | "settings") {
            app.activate_action(command, None);
        }
        println!("TigerType Linux 后台就绪（{command}）");
        main.run();
        Ok(())
    })();
    if let Some(mut state) = running.borrow_mut().take() {
        drop(state.views.take());
        state.runtime.shutdown();
    }
    result
}
fn bus_call(
    bus: &gio::DBusConnection,
    method: &str,
    parameters: glib::Variant,
) -> Result<glib::Variant, String> {
    bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        method,
        Some(&parameters),
        None,
        gio::DBusCallFlags::NONE,
        2000,
        gio::Cancellable::NONE,
    )
    .map_err(|e| e.to_string())
}
pub(crate) fn ibus_connection() -> Result<gio::DBusConnection, String> {
    // 只在启动期查询地址；按键路径绝不启动子进程或读取 IBus 地址文件。
    let output = Command::new("/usr/bin/ibus")
        .arg("address")
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("无法取得 IBus 地址，框架尚未启动".into());
    }
    let address = std::str::from_utf8(&output.stdout)
        .map_err(|e| e.to_string())?
        .trim();
    if !address.starts_with("unix:") || address.contains(';') || address.len() > 4096 {
        return Err("只允许本地 IBus Unix 地址".into());
    }
    gio::DBusConnection::for_address_sync(
        address,
        gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
            | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
        None,
        gio::Cancellable::NONE,
    )
    .map_err(|e| e.to_string())
}
fn diagnose() -> Result<(), String> {
    let desktop =
        platform::desktop(&std::env::var("XDG_CURRENT_DESKTOP").map_err(|_| "缺少桌面标识")?)?;
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .map_err(|e| e.to_string())?;
    let owner = |name| {
        bus_call(&bus, "GetNameOwner", (name,).to_variant())
            .ok()
            .and_then(|reply| reply.get::<(String,)>())
            .map(|tuple| tuple.0)
    };
    let ibus = owner("org.freedesktop.IBus");
    let fcitx = owner("org.fcitx.Fcitx5");
    // Fcitx5 的 IBus 兼容前端可能持有两个总线名；同一所有者不算两个活动框架。
    let selected = platform::framework(desktop, ibus.is_some() && ibus != fcitx, fcitx.is_some())?;
    println!("桌面：{desktop:?}；现有框架优先选择：{selected:?}。未更改系统设置。");
    Ok(())
}
