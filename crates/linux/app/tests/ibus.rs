//! 真实 IBus daemon + Rust 引擎的隔离验收；组件路径、HOME、缓存与总线均指向新建临时目录。
#![cfg(target_os = "linux")]
use gio::{
    DBusConnection,
    glib::{self, variant::ToVariant},
};
use std::{
    fs,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tigertype_backend::{Data, Dictionary, Entry, resources};
use tigertype_contracts::Options;
use tigertype_linux_app::{channel, package};
use tigertype_linux_input_method::ibus::Server;
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Quit(glib::MainLoop);
impl Drop for Quit {
    fn drop(&mut self) {
        self.0.quit();
    }
}
fn connect(address: &str) -> Result<DBusConnection, glib::Error> {
    DBusConnection::for_address_sync(
        address,
        gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
            | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
        None,
        gio::Cancellable::NONE,
    )
}
#[test]
#[ignore = "单列真实 IBus daemon 接入；仅隔离组件和总线，不登记现用桌面输入源"]
fn daemon_discovers_factory_and_routes_keys_to_real_engine() {
    let root = resources::new_directory(
        &std::env::temp_dir().canonicalize().unwrap(),
        "tigertype-ibus-native",
    )
    .unwrap();
    let components = root.join("components");
    fs::create_dir(&components).unwrap();
    // 工厂由本测试主动登记，daemon 无需启动产品程序或接触安装目录。
    fs::write(
        components.join("tigertype.xml"),
        package::IBUS_XML.replace(
            "/opt/tigertype/bin/tigertype-linux-app ibus",
            "/usr/bin/false",
        ),
    )
    .unwrap();
    let config = root.join("bus.conf");
    fs::write(&config,format!("<busconfig><type>session</type><listen>unix:tmpdir={}</listen><auth>EXTERNAL</auth><policy context=\"default\"><allow own=\"*\"/><allow send_destination=\"*\"/><allow receive_sender=\"*\"/></policy></busconfig>",root.display())).unwrap();
    let mut session = Process(
        Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut session_address = String::new();
    BufReader::new(session.0.stdout.take().unwrap())
        .read_line(&mut session_address)
        .unwrap();
    let address = format!("unix:path={}", root.join("ibus.sock").display());
    let mut daemon = Process(
        Command::new("/usr/bin/ibus-daemon")
            .args([
                "--single",
                "--panel=disable",
                "--config=disable",
                "--emoji-extension=disable",
                "--cache=none",
                "--address",
            ])
            .arg(&address)
            .env("DBUS_SESSION_BUS_ADDRESS", session_address.trim())
            .env("HOME", &root)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("IBUS_COMPONENT_PATH", &components)
            .env("GSETTINGS_BACKEND", "memory")
            .env("GIO_USE_VFS", "local")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .stdout(Stdio::null())
            .stderr(fs::File::create(root.join("daemon.log")).unwrap())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let bus = loop {
        if let Ok(bus) = connect(&address) {
            break bus;
        }
        assert!(
            Instant::now() < deadline && daemon.0.try_wait().unwrap().is_none(),
            "{}",
            fs::read_to_string(root.join("daemon.log")).unwrap()
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| {
            let (connector, inbox) = channel::mailbox();
            let stop = connector.clone();
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
            let owner = bus
                .call_sync(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "GetNameOwner",
                    Some(&("org.freedesktop.IBus",).to_variant()),
                    None,
                    gio::DBusCallFlags::NONE,
                    1000,
                    gio::Cancellable::NONE,
                )
                .unwrap()
                .get::<(String,)>()
                .unwrap()
                .0;
            let server = Server::new(
                bus.clone(),
                owner,
                move || Box::new(connector.link()),
                |_| {},
            )
            .unwrap();
            bus.call_sync(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "RequestName",
                Some(&("org.freedesktop.IBus.TigerType", 4u32).to_variant()),
                None,
                gio::DBusCallFlags::NONE,
                1000,
                gio::Cancellable::NONE,
            )
            .unwrap();
            let main = glib::MainLoop::new(Some(&context), false);
            let quit = main.clone();
            let worker = std::thread::spawn(move || {
                let _quit = Quit(quit);
                let client = connect(&address).unwrap();
                let call = |path: &str, interface: &str, method: &str, args: glib::Variant| {
                    client
                        .call_sync(
                            Some("org.freedesktop.IBus"),
                            path,
                            interface,
                            method,
                            Some(&args),
                            None,
                            gio::DBusCallFlags::NONE,
                            3000,
                            gio::Cancellable::NONE,
                        )
                        .unwrap()
                };
                let created = call(
                    "/org/freedesktop/IBus",
                    "org.freedesktop.IBus",
                    "CreateInputContext",
                    ("tigertype-native-test",).to_variant(),
                );
                let path = created.get::<(glib::variant::ObjectPath,)>().unwrap().0;
                let commits = Arc::new(Mutex::new(Vec::<String>::new()));
                let collected = commits.clone();
                let filter = client.add_filter(move |_, message, incoming| {
                    if incoming
                        && message.member().as_deref() == Some("CommitText")
                        && let Some(body) = message.body()
                        && let Some(text) = body.child_value(0).as_variant()
                        && let Some(text) = text.child_value(2).get::<String>()
                        && let Ok(mut list) = collected.lock()
                    {
                        list.push(text);
                    }
                    Some(message.clone())
                });
                let iface = "org.freedesktop.IBus.InputContext";
                call(
                    path.as_str(),
                    iface,
                    "SetCapabilities",
                    (31u32,).to_variant(),
                );
                call(path.as_str(), iface, "FocusIn", ().to_variant());
                call(
                    "/org/freedesktop/IBus",
                    "org.freedesktop.IBus",
                    "SetGlobalEngine",
                    ("tigertype",).to_variant(),
                );
                // 固定的模式属性跨 Reset 仍可用；切换两次还原默认值，不产生提交。
                for property in [
                    "tigertype-ascii",
                    "tigertype-shape",
                    "tigertype-punctuation",
                ] {
                    for _ in 0..2 {
                        call(path.as_str(), iface, "Reset", ().to_variant());
                        call(
                            path.as_str(),
                            iface,
                            "PropertyActivate",
                            (property, 0u32).to_variant(),
                        );
                    }
                }
                assert!(commits.lock().unwrap().is_empty());
                for key in [u32::from(b'a'), u32::from(b'b'), 0x20] {
                    assert_eq!(
                        call(
                            path.as_str(),
                            iface,
                            "ProcessKeyEvent",
                            (key, 0u32, 0u32).to_variant()
                        )
                        .get::<(bool,)>(),
                        Some((true,))
                    );
                }
                assert_eq!(*commits.lock().unwrap(), vec!["虎"]);
                call(path.as_str(), iface, "FocusOut", ().to_variant());
                client.remove_filter(filter);
            });
            main.run();
            let result = worker.join();
            drop(server);
            stop.stop();
            engine.join().unwrap();
            result.unwrap();
        })
        .unwrap();
}
