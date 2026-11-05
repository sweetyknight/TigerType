//! Linux 主线程组装：GApplication 只转发窗口动作，真实查询和配置事务留在固定工作线程。
use crate::runtime::Runtime;
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
    /// startup 为桌面启动器转来的激活令牌（可空）；框架菜单没有令牌，前置与否由合成器决定。
    fn show(&mut self, panel: Panel, startup: &str) -> Result<(), String> {
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
        self.views.as_ref().unwrap().show(panel, startup);
        Ok(())
    }
}

pub fn main() -> Result<(), String> {
    // SAFETY: 只查询身份；不让提权安装器身份启动个人输入后台。
    if unsafe { libc::geteuid() } == 0 {
        return Err("后台必须以普通用户身份运行".into());
    }
    let mut args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string().map_err(|_| "参数不是 UTF-8"))
        .collect::<Result<_, _>>()?;
    if args.first().is_some_and(|arg| arg == "integration") {
        return crate::integration::command(&args[1..]).map_err(|e| e.to_string());
    }
    // 只有 Fcitx5 待登记的自启动走 login：启动后台并在框架出现后登记一次。
    let login = args.first().is_some_and(|arg| arg == "login");
    if login {
        if args.len() != 2 || args[1] != "fcitx" {
            return Err("登录参数不符".into());
        }
        args = vec!["fcitx".into()];
    }
    let command = args.first().map(String::as_str).unwrap_or("settings");
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
    // 名字只取数据目录：同一用户的会话总线与数据租约已限定一个后台；不掺显示变量，
    // 免得 IBus 拉起的进程与桌面启动器/SSH 环境不同而另起后台、停机找不到现用后台。
    // 隔离验证使用新建数据目录与私有总线，动作不会发给正式后台。
    let identity = resources::sha256_bytes(data.display().to_string().as_bytes());
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
        // 窗口动作携带启动器的激活令牌（可为空串），让已在后台的窗口在 Wayland 下合法前置。
        let action = gio::SimpleAction::new(name, panel.map(|_| glib::VariantTy::STRING));
        let state = running.clone();
        let quit = main.clone();
        action.connect_activate(move |_, parameter| {
            let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
                match panel {
                    Some(panel) => {
                        let token = parameter.and_then(|p| p.str()).unwrap_or_default();
                        if token.len() > 4096 {
                            return Err("激活令牌过长".into());
                        }
                        state
                            .borrow_mut()
                            .as_mut()
                            .ok_or("后台未就绪")?
                            .show(panel, token)
                    }
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
        match command {
            "stop" => app.activate_action(command, None),
            "lookup" | "settings" => {
                app.activate_action(command, Some(&launch_token().to_variant()));
            }
            _ => {}
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
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        return Err("缺少图形登录会话".into());
    }
    // GTK4 不从 argv 取程序名，未设置时 Wayland app_id 退为“GTK Application”；与启动项 tigertype.desktop 对齐。
    glib::set_prgname(Some("tigertype"));
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
        let _watch = login.then(crate::integration::watch);
        let _fcitx = if service_mode == "fcitx" {
            let state = running.clone();
            let connector = connector.clone();
            Some(tigertype_linux_input_method::fcitx::Server::new(
                app.dbus_connection().ok_or("缺少会话总线")?,
                move || Box::new(connector.link()),
                move |panel| {
                    if let Some(running) = state.borrow_mut().as_mut()
                        && let Err(error) = running.show(panel, "")
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
                        && let Err(error) = running.show(panel, "")
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
            // 本进程首个窗口由 GTK 自取启动器令牌，这里不重复使用同一令牌。
            app.activate_action(command, Some(&"".to_variant()));
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
/// libgtk-4 的库构造函数在 main 之前暂存并删除启动器令牌变量，std::env 已读不到；
/// 内核保留的初始环境块不受影响，转发给已有后台前从这里取回。
fn launch_token() -> String {
    token_from_environ(&std::fs::read("/proc/self/environ").unwrap_or_default())
}
fn token_from_environ(environ: &[u8]) -> String {
    let find = |key: &[u8]| {
        environ
            .split(|&byte| byte == 0)
            .find_map(|entry| entry.strip_prefix(key))
            .filter(|value| !value.is_empty())
    };
    find(b"XDG_ACTIVATION_TOKEN=")
        .or_else(|| find(b"DESKTOP_STARTUP_ID="))
        .and_then(|value| std::str::from_utf8(value).ok())
        .unwrap_or_default()
        .to_owned()
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
    let selected = crate::integration::selected().map_err(|e| e.to_string())?;
    println!("现有框架优先选择：{selected:?}。未更改系统设置。");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::token_from_environ;

    #[test]
    fn launch_token_prefers_xdg_activation() {
        assert_eq!(
            token_from_environ(b"DESKTOP_STARTUP_ID=x11\0XDG_ACTIVATION_TOKEN=wl\0"),
            "wl"
        );
        // 空令牌视同缺失，退用 X11 启动标识；前缀须整段匹配，不误取相似变量。
        assert_eq!(
            token_from_environ(b"XDG_ACTIVATION_TOKEN=\0DESKTOP_STARTUP_ID=x11\0"),
            "x11"
        );
        assert_eq!(token_from_environ(b"MY_XDG_ACTIVATION_TOKEN=no\0"), "");
        assert_eq!(token_from_environ(b""), "");
    }
}
