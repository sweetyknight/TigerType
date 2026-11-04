//! Fcitx D-Bus 与真实 Rust 引擎的隔离回归，不启用现用框架，也不读用户配置。
#![cfg(target_os = "linux")]
use gio::{
    DBusConnection,
    glib::{self, variant::ToVariant},
};
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
};
use tigertype_backend::{Data, Dictionary, Entry};
use tigertype_contracts::Options;
use tigertype_linux_app::channel;
use tigertype_linux_input_method::fcitx::{self, Server};
struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Quit(glib::MainLoop);

#[test]
fn installer_session_registration_preserves_existing_fcitx_entries() {
    use std::{
        cell::RefCell,
        panic::{AssertUnwindSafe, catch_unwind},
        rc::Rc,
    };
    let root = tigertype_backend::resources::new_directory(
        &std::env::temp_dir().canonicalize().unwrap(),
        "tigertype-session-registration",
    )
    .unwrap();
    let mut daemon = Daemon(
        Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut address = String::new();
    BufReader::new(daemon.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let context = glib::MainContext::new();
    context.with_thread_default(||{
        let bus=connect(address.trim());
        bus.call_sync(Some("org.freedesktop.DBus"),"/org/freedesktop/DBus","org.freedesktop.DBus","RequestName",Some(&("org.fcitx.Fcitx5",4u32).to_variant()),None,gio::DBusCallFlags::NONE,1000,gio::Cancellable::NONE).unwrap();
        let original=vec![("keyboard-us".to_owned(),String::new()),("pinyin".into(),String::new())];
        let entries=Rc::new(RefCell::new(original.clone()));let data=entries.clone();
        let history=Rc::new(RefCell::new(Vec::new()));let writes=history.clone();
        let info=gio::DBusNodeInfo::for_xml(r#"<node><interface name="org.fcitx.Fcitx.Controller1"><method name="InputMethodGroups"><arg type="as" direction="out"/></method><method name="InputMethodGroupInfo"><arg type="s" direction="in"/><arg type="s" direction="out"/><arg type="a(ss)" direction="out"/></method><method name="SetInputMethodGroupInfo"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="a(ss)" direction="in"/></method></interface></node>"#).unwrap().lookup_interface("org.fcitx.Fcitx.Controller1").unwrap();
        let id=bus.register_object("/controller",&info).method_call(move|_,_,_,_,method,args,reply|{
            let result=catch_unwind(AssertUnwindSafe(||->Result<glib::Variant,&str>{
                match method {
                    "InputMethodGroups"=>Ok((vec!["Default"],).to_variant()),
                    "InputMethodGroupInfo"=>Ok(("us",data.borrow().clone()).to_variant()),
                    "SetInputMethodGroupInfo"=>{let (name,layout,entries)=args.get::<(String,String,Vec<(String,String)>)>().ok_or("参数不符")?;if name!="Default"||layout!="us"{return Err("不应修改分组或键盘布局");}writes.borrow_mut().push(entries.clone());*data.borrow_mut()=entries;Ok(().to_variant())},
                    _=>Err("未知方法"),
                }
            })).unwrap_or(Err("测试服务失败"));
            match result {Ok(value)=>reply.return_value(Some(&value)),Err(error)=>reply.return_dbus_error("local.tigertype.TestError",error)}
        }).build().unwrap();
        let main=glib::MainLoop::new(Some(&context),false);let quit=main.clone();
        let worker=std::thread::spawn(move||{
            let _quit=Quit(quit);
            let run=|action|{
                // 此用例只验控制器登记；固定会话类型，不能继承宿主 Wayland 后误走真实 KWin 配置。
                let output=Command::new(env!("CARGO_BIN_EXE_tigertype-linux-app")).args(["integration","fcitx",action])
                    .env("DBUS_SESSION_BUS_ADDRESS",address.trim()).env("HOME",&root).env("XDG_CONFIG_HOME",root.join("config")).env("XDG_CURRENT_DESKTOP","KDE").env("XDG_SESSION_TYPE","x11").output().unwrap();
                assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
            };
            run("install");run("install");
            assert!(root.join("config/autostart/tigertype.desktop").is_file());
            run("remove");
            assert!(!root.join("config/autostart/tigertype.desktop").exists());
        });
        main.run();let result=worker.join();assert_eq!(*entries.borrow(),original);bus.unregister_object(id).unwrap();result.unwrap();
        let history=history.borrow();assert_eq!(history.len(),3);
        for installed in &history[..2] {assert_eq!(&installed[..2],original.as_slice());assert_eq!(installed.len(),3);assert_eq!(installed[2].0,"tigertype");}
    }).unwrap();
}

#[test]
#[ignore = "单列实际 Fcitx5 插件加载；TIGERTYPE_PLUGIN 指向本机已编译插件，仅使用私有总线和合成配置"]
fn actual_fcitx_plugin_loads_and_commits_through_its_native_frontend() {
    use std::{
        fs,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    let plugin = std::path::PathBuf::from(
        std::env::var_os("TIGERTYPE_PLUGIN").expect("需要 TIGERTYPE_PLUGIN"),
    );
    let root = tigertype_backend::resources::new_directory(
        &std::env::temp_dir().canonicalize().unwrap(),
        "tigertype-fcitx-native",
    )
    .unwrap();
    let data = root.join("data");
    let config = root.join("config");
    let libs = root.join("addons");
    fs::create_dir_all(data.join("fcitx5/addon")).unwrap();
    fs::create_dir_all(data.join("fcitx5/inputmethod")).unwrap();
    fs::create_dir_all(config.join("fcitx5")).unwrap();
    fs::create_dir(&libs).unwrap();
    fs::write(
        data.join("fcitx5/addon/tigertype.conf"),
        tigertype_linux_app::package::ADDON,
    )
    .unwrap();
    fs::write(
        data.join("fcitx5/inputmethod/tigertype.conf"),
        tigertype_linux_app::package::INPUT_METHOD,
    )
    .unwrap();
    fs::copy(&plugin, libs.join("tigertype.so")).unwrap();
    fs::write(config.join("fcitx5/profile"),"[Groups/0]\nName=Default\nDefault Layout=us\nDefaultIM=tigertype\n[Groups/0/Items/0]\nName=keyboard-us\nLayout=\n[Groups/0/Items/1]\nName=tigertype\nLayout=\n[GroupOrder]\n0=Default\n").unwrap();
    let mut daemon = Daemon(
        Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut address = String::new();
    BufReader::new(daemon.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim().to_owned();
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| {
            let (connector, inbox) = channel::mailbox();
            let stopped = connector.clone();
            let engine = std::thread::spawn(move || {
                let dictionary = Dictionary::new(Data {
                    entries: vec![Entry {
                        character: '虎',
                        code: "ab",
                        weight: 1,
                    }],
                    common: vec!['虎'],
                    ..Default::default()
                })
                .unwrap();
                inbox.run(&dictionary, 1, Options::default()).unwrap();
            });
            let server = Server::new(
                connect(&address),
                move || Box::new(connector.link()),
                |_| {},
            )
            .unwrap();
            let main = glib::MainLoop::new(Some(&context), false);
            let quit = main.clone();
            let worker = std::thread::spawn(move || {
                let _quit = Quit(quit);
                // 禁用所有显示前端，不能接管现用桌面；只有测试总线客户端能送键。
                let mut fcitx = Daemon(
                    Command::new("/usr/bin/fcitx5")
                        .args([
                            "-D",
                            "-k",
                            "--disable",
                            "all",
                            "--enable",
                            "dbus,dbusfrontend,keyboard,tigertype",
                        ])
                        .env("DBUS_SESSION_BUS_ADDRESS", &address)
                        .env("XDG_CONFIG_HOME", &config)
                        .env("XDG_DATA_HOME", &data)
                        .env("XDG_CACHE_HOME", root.join("cache"))
                        .env(
                            "FCITX_ADDON_DIRS",
                            format!("{}:/usr/lib/x86_64-linux-gnu/fcitx5", libs.display()),
                        )
                        .env("XDG_DATA_DIRS", "/usr/share")
                        .env_remove("DISPLAY")
                        .env_remove("WAYLAND_DISPLAY")
                        .stdout(Stdio::null())
                        .stderr(fs::File::create(root.join("framework.log")).unwrap())
                        .spawn()
                        .unwrap(),
                );
                let client = connect(&address);
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    let owned = client
                        .call_sync(
                            Some("org.freedesktop.DBus"),
                            "/org/freedesktop/DBus",
                            "org.freedesktop.DBus",
                            "NameHasOwner",
                            Some(&("org.fcitx.Fcitx5",).to_variant()),
                            None,
                            gio::DBusCallFlags::NONE,
                            500,
                            gio::Cancellable::NONE,
                        )
                        .unwrap()
                        .get::<(bool,)>()
                        .unwrap()
                        .0;
                    if owned {
                        break;
                    }
                    assert!(
                        Instant::now() < deadline && fcitx.0.try_wait().unwrap().is_none(),
                        "{}",
                        fs::read_to_string(root.join("framework.log")).unwrap()
                    );
                    // 仅测试启动期有界等待，产品输入路径没有轮询。
                    std::thread::sleep(Duration::from_millis(10));
                }
                let call = |path: &str, interface: &str, method: &str, args: glib::Variant| {
                    client
                        .call_sync(
                            Some("org.fcitx.Fcitx5"),
                            path,
                            interface,
                            method,
                            Some(&args),
                            None,
                            gio::DBusCallFlags::NO_AUTO_START,
                            2000,
                            gio::Cancellable::NONE,
                        )
                        .unwrap()
                };
                let created = call(
                    "/org/freedesktop/portal/inputmethod",
                    "org.fcitx.Fcitx.InputMethod1",
                    "CreateInputContext",
                    (vec![("program", "tigertype-native-test")],).to_variant(),
                );
                let (path, _) = created
                    .get::<(glib::variant::ObjectPath, Vec<u8>)>()
                    .unwrap();
                let iface = "org.fcitx.Fcitx.InputContext1";
                let commits = Arc::new(Mutex::new(Vec::<String>::new()));
                let collected = commits.clone();
                let filter = client.add_filter(move |_, message, incoming| {
                    if incoming
                        && message.member().as_deref() == Some("CommitString")
                        && let Some(body) = message.body()
                        && let Some((text,)) = body.get::<(String,)>()
                        && let Ok(mut list) = collected.lock()
                    {
                        list.push(text);
                    }
                    Some(message.clone())
                });
                call(
                    path.as_str(),
                    iface,
                    "SetCapability",
                    (1u64 << 4,).to_variant(),
                );
                call(path.as_str(), iface, "FocusIn", ().to_variant());
                call(
                    "/controller",
                    "org.fcitx.Fcitx.Controller1",
                    "SetCurrentIM",
                    ("tigertype",).to_variant(),
                );
                for symbol in [u32::from(b'a'), u32::from(b'b'), 0x20] {
                    assert_eq!(
                        call(
                            path.as_str(),
                            iface,
                            "ProcessKeyEvent",
                            (symbol, 0u32, 0u32, false, 0u32).to_variant()
                        )
                        .get::<(bool,)>(),
                        Some((true,)),
                        "{}",
                        fs::read_to_string(root.join("framework.log")).unwrap()
                    );
                }
                assert_eq!(*commits.lock().unwrap(), vec!["虎"]);
                call(
                    path.as_str(),
                    iface,
                    "SetCapability",
                    (1u64 << 3,).to_variant(),
                );
                assert_eq!(
                    call(
                        path.as_str(),
                        iface,
                        "ProcessKeyEvent",
                        (u32::from(b'a'), 0u32, 0u32, false, 0u32).to_variant()
                    )
                    .get::<(bool,)>(),
                    Some((false,)),
                    "密码输入必须放行"
                );
                assert_eq!(*commits.lock().unwrap(), vec!["虎"]);
                call(path.as_str(), iface, "FocusOut", ().to_variant());
                call(path.as_str(), iface, "DestroyIC", ().to_variant());
                client.remove_filter(filter);
            });
            main.run();
            let result = worker.join();
            drop(server);
            stopped.stop();
            engine.join().unwrap();
            result.unwrap();
        })
        .unwrap();
}
impl Drop for Quit {
    fn drop(&mut self) {
        self.0.quit();
    }
}
fn connect(address: &str) -> DBusConnection {
    DBusConnection::for_address_sync(
        address,
        gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
            | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
        None,
        gio::Cancellable::NONE,
    )
    .unwrap()
}
#[test]
fn real_engine_menu_snapshots_and_absolute_deadlines_cross_private_bus() {
    let mut daemon = Daemon(
        Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut address = String::new();
    BufReader::new(daemon.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| {
            let (connector, inbox) = channel::mailbox();
            let stopped = connector.clone();
            let engine = std::thread::spawn(move || {
                let dictionary = Dictionary::new(Data {
                    entries: vec![Entry {
                        character: '虎',
                        code: "ab",
                        weight: 1,
                    }],
                    common: vec!['虎'],
                    ..Default::default()
                })
                .unwrap();
                inbox.run(&dictionary, 1, Options::default()).unwrap();
            });
            let bus = connect(address.trim());
            let caller = connect(address.trim());
            let stranger = connect(address.trim());
            caller
                .call_sync(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "RequestName",
                    Some(&("org.fcitx.Fcitx5", 4u32).to_variant()),
                    None,
                    gio::DBusCallFlags::NONE,
                    1000,
                    gio::Cancellable::NONE,
                )
                .unwrap();
            let server = Server::new(bus, move || Box::new(connector.link()), |_| {}).unwrap();
            let main = glib::MainLoop::new(Some(&context), false);
            let quit = main.clone();
            let child_address = address.clone();
            let worker = std::thread::spawn(move || {
                let _quit = Quit(quit);
                // 独立进程不能冒用框架名字；总线提供的 PID 而不是请求正文决定身份。
                let foreign = Command::new("/usr/bin/gdbus")
                    .args([
                        "call",
                        "--address",
                        child_address.trim(),
                        "--dest",
                        fcitx::NAME,
                        "--object-path",
                        fcitx::PATH,
                        "--method",
                        "local.tigertype.Fcitx1.Event",
                        "1",
                        "1",
                        "1",
                    ])
                    .arg((glib::monotonic_time() + 50_000).to_string())
                    .args(["focus", "0", "0", "''"])
                    .output()
                    .unwrap();
                assert!(!foreign.status.success());
                assert!(
                    String::from_utf8_lossy(&foreign.stderr).contains("非 Fcitx5"),
                    "{}",
                    String::from_utf8_lossy(&foreign.stderr)
                );
                let call = |client: &DBusConnection,
                            sequence: u64,
                            command: &str,
                            key: u32,
                            token: &str,
                            expires: i64,
                            version: u32| {
                    client.call_sync(
                        Some(fcitx::NAME),
                        fcitx::PATH,
                        fcitx::INTERFACE,
                        "Event",
                        Some(
                            &(version, 1u64, sequence, expires, command, key, 0u32, token)
                                .to_variant(),
                        ),
                        None,
                        gio::DBusCallFlags::NONE,
                        2000,
                        gio::Cancellable::NONE,
                    )
                };
                let invoke = |sequence, command, key, token| {
                    call(
                        &caller,
                        sequence,
                        command,
                        key,
                        token,
                        glib::monotonic_time() + 50_000,
                        1,
                    )
                };
                // 两条连接属于同一测试进程，模拟 Fcitx 自有总线与 GIO 胶水的合法分离。
                assert!(
                    call(
                        &stranger,
                        1,
                        "focus",
                        0,
                        "",
                        glib::monotonic_time() + 50_000,
                        1
                    )
                    .is_ok()
                );
                assert!(call(&caller, 1, "focus", 0, "", glib::monotonic_time() - 1, 1).is_err());
                assert!(
                    call(
                        &caller,
                        1,
                        "focus",
                        0,
                        "",
                        glib::monotonic_time() + 50_000,
                        2
                    )
                    .is_err()
                );
                invoke(1, "focus", 0, "").unwrap();
                invoke(2, "key", u32::from(b'a'), "").unwrap();
                let first = invoke(3, "key", u32::from(b'b'), "").unwrap();
                assert_eq!(first.type_().as_str(), "(utbssa(ss)isa(ss))");
                assert_eq!(first.child_value(3).get::<String>().unwrap(), "");
                let menus = first.child_value(8).get::<Vec<(String, String)>>().unwrap();
                let old = menus
                    .iter()
                    .find(|(_, label)| label == "选字 1：虎")
                    .unwrap()
                    .0
                    .clone();
                let confirm = invoke(4, "confirm", 0, "").unwrap();
                assert_eq!(confirm.child_value(3).get::<String>().unwrap(), "");
                assert!(invoke(5, "menu", 0, &old).is_err());
                let menus = confirm
                    .child_value(8)
                    .get::<Vec<(String, String)>>()
                    .unwrap();
                let token = menus
                    .iter()
                    .find(|(_, label)| label == "确认上屏：虎")
                    .unwrap()
                    .0
                    .clone();
                let commit = invoke(6, "menu", 0, &token).unwrap();
                assert_eq!(commit.child_value(3).get::<String>().unwrap(), "虎");
                assert!(invoke(7, "menu", 0, &token).is_err());
                assert!(invoke(6, "key", u32::from(b'a'), "").is_err());
                invoke(8, "close", 0, "").unwrap();
                assert!(invoke(9, "key", u32::from(b'a'), "").is_err());
            });
            main.run();
            let result = worker.join();
            drop(server);
            stopped.stop();
            engine.join().unwrap();
            result.unwrap();
        })
        .unwrap();
}
