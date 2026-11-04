//! 完整锁定资源的 Linux 启动验收；私有会话总线、临时数据与字体配置，不安装或切换输入法。
#![cfg(target_os = "linux")]
use std::{
    io::{BufRead, BufReader},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, Stdio},
};
struct Process(Child);

#[test]
#[ignore = "单列 Debian 包安装生命周期，TIGERTYPE_BUNDLE 指向本机生成并已校验的交付目录"]
fn debian_package_install_upgrade_remove_preserves_fonts_and_personal_data() {
    let bundle = std::path::PathBuf::from(
        std::env::var_os("TIGERTYPE_BUNDLE").expect("需要 TIGERTYPE_BUNDLE"),
    );
    tigertype_linux_app::package::inspect(&bundle).unwrap();
    let root = tigertype_backend::resources::new_directory(
        &std::env::temp_dir().canonicalize().unwrap(),
        "tigertype-dpkg-root",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("var/lib/dpkg")).unwrap();
    std::fs::write(root.join("var/lib/dpkg/status"), "").unwrap();
    std::fs::create_dir_all(root.join("home/user/.local/share/TigerType")).unwrap();
    let personal = root.join("home/user/.local/share/TigerType/personal.txt");
    std::fs::write(&personal, b"keep user data").unwrap();
    let dpkg = |args: &[&std::ffi::OsStr]| {
        // 只允许本测试新建绝对根；测试时依赖检查另列，不在宿主安装任何包或运行系统触发器。
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("tigertype-dpkg-root-")
        );
        assert_eq!(root.canonicalize().unwrap(), root);
        let status = Command::new("/usr/bin/dpkg")
            .arg(format!("--root={}", root.display()))
            .arg(format!("--log={}", root.join("dpkg.log").display()))
            .args(["--force-not-root", "--force-depends", "--no-triggers"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    };
    for _ in 0..2 {
        // 只解包和替换，不执行 postinst；字体缓存留到获准的真实系统安装中验证。
        dpkg(&[
            "--unpack".as_ref(),
            bundle.join("tigertype-fonts.deb").as_os_str(),
            bundle.join("tigertype.deb").as_os_str(),
        ]);
        for (path, entry) in tigertype_linux_app::package::resources().unwrap() {
            let bytes =
                tigertype_backend::storage::read_file(&root.join(path), entry.size).unwrap();
            assert_eq!(
                tigertype_backend::resources::sha256_bytes(&bytes),
                entry.sha256
            );
        }
    }
    dpkg(&["--remove".as_ref(), "tigertype".as_ref()]);
    assert!(!root.join("opt/tigertype/bin/tigertype-linux-app").exists());
    assert!(!root.join("usr/share/ibus/component/tigertype.xml").exists());
    for font in tigertype_linux_app::package::fonts().unwrap() {
        assert!(
            root.join("usr/share/fonts/truetype/tigertype")
                .join(font.path)
                .is_file()
        );
    }
    assert_eq!(std::fs::read(personal).unwrap(), b"keep user data");
}
impl Drop for Process {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
#[test]
#[ignore = "完整资源与 GTK 启动验收：须在 Linux 图形会话单列执行"]
fn locked_resources_start_one_backend_forward_windows_and_stop() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap();
    let root = tigertype_backend::resources::new_directory(
        &std::env::temp_dir().canonicalize().unwrap(),
        "tigertype-linux-native",
    )
    .unwrap();
    let config = root.join("fonts.conf");
    // 此目录刚由测试创建，收紧权限并禁用系统服务激活，避免拉起现用桌面的 portal/GVfs。
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let bus_config = root.join("bus.conf");
    std::fs::write(&bus_config, format!("<busconfig><type>session</type><listen>unix:tmpdir={}</listen><auth>EXTERNAL</auth><policy context=\"default\"><allow own=\"*\"/><allow send_destination=\"*\"/><allow receive_sender=\"*\"/></policy></busconfig>", root.display())).unwrap();
    let xml = |path: &Path| {
        path.to_str()
            .unwrap()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
    };
    std::fs::write(&config, format!("<fontconfig><cachedir>{}</cachedir><include ignore_missing=\"yes\">/etc/fonts/fonts.conf</include><dir>{}</dir></fontconfig>", xml(&root.join("font-cache")), xml(&source.join("resources/fonts")))).unwrap();
    let mut bus = Process(
        Command::new("dbus-daemon")
            .arg(format!("--config-file={}", bus_config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut address = String::new();
    BufReader::new(bus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let command = |action| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tigertype-linux-app"));
        command
            .arg(action)
            .arg(source.join("resources/tiger"))
            .arg(root.join("data"))
            .env("DBUS_SESSION_BUS_ADDRESS", address.trim())
            .env("FONTCONFIG_FILE", &config)
            // 只隔离本测试的无障碍桥接，不影响正式桌面；读屏验收仍另列。
            .env("NO_AT_BRIDGE", "1")
            .env("GIO_USE_VFS", "local")
            .env("GTK_USE_PORTAL", "0")
            .stdin(Stdio::null());
        command
    };
    let mut backend = Process(command("serve").stdout(Stdio::piped()).spawn().unwrap());
    let mut ready = String::new();
    BufReader::new(backend.0.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(ready.contains("后台就绪"), "后台未就绪：{ready}");
    assert!(
        command("serve").status().unwrap().success(),
        "重复启动须转到既有后台"
    );
    assert!(backend.0.try_wait().unwrap().is_none());
    assert!(
        !command("ibus").status().unwrap().success(),
        "不能把未登记 IBus 的既有后台说成已就绪"
    );
    let settings = root.join("data/config/settings.toml");
    let before = std::fs::read(&settings).unwrap();
    assert!(command("lookup").status().unwrap().success());
    assert!(command("settings").status().unwrap().success());
    assert!(command("stop").status().unwrap().success());
    assert!(backend.0.wait().unwrap().success());
    assert_eq!(
        std::fs::read(&settings).unwrap(),
        before,
        "只打开窗口不应改写设置"
    );
    assert!(
        tigertype_backend::storage::Data::open(&root.join("data")).is_ok(),
        "退出须释放数据租约"
    );

    // 由真实 daemon 冷启动产品，而非测试先导出工厂，覆盖名字刚出现即 CreateEngine 的竞态。
    use gio::glib::{self, variant::ToVariant};
    let components = root.join("components");
    std::fs::create_dir(&components).unwrap();
    let exec = format!(
        "{} ibus {} {}",
        glib::shell_quote(env!("CARGO_BIN_EXE_tigertype-linux-app")).to_string_lossy(),
        glib::shell_quote(source.join("resources/tiger").to_str().unwrap()).to_string_lossy(),
        glib::shell_quote(root.join("data").to_str().unwrap()).to_string_lossy()
    );
    std::fs::write(
        components.join("tigertype.xml"),
        tigertype_linux_app::package::IBUS_XML.replace(
            "/opt/tigertype/bin/tigertype-linux-app ibus",
            &exec.replace('&', "&amp;").replace('<', "&lt;"),
        ),
    )
    .unwrap();
    let ibus_address = format!("unix:path={}", root.join("ibus.sock").display());
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
            .arg(&ibus_address)
            .env("DBUS_SESSION_BUS_ADDRESS", address.trim())
            .env("IBUS_ADDRESS", &ibus_address)
            .env("IBUS_COMPONENT_PATH", &components)
            .env("HOME", &root)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("FONTCONFIG_FILE", &config)
            .env("GSETTINGS_BACKEND", "memory")
            .env("NO_AT_BRIDGE", "1")
            .env("GIO_USE_VFS", "local")
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(root.join("ibus.log")).unwrap())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let client = loop {
        if let Ok(client) = gio::DBusConnection::for_address_sync(
            &ibus_address,
            gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
                | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
            None,
            gio::Cancellable::NONE,
        ) {
            break client;
        }
        assert!(std::time::Instant::now() < deadline && daemon.0.try_wait().unwrap().is_none());
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
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
                5000,
                gio::Cancellable::NONE,
            )
            .unwrap_or_else(|error| {
                panic!(
                    "{method}: {error}\n{}",
                    std::fs::read_to_string(root.join("ibus.log")).unwrap()
                )
            })
    };
    let path = call(
        "/org/freedesktop/IBus",
        "org.freedesktop.IBus",
        "CreateInputContext",
        ("tigertype-cold-test",).to_variant(),
    )
    .get::<(glib::variant::ObjectPath,)>()
    .unwrap()
    .0;
    let iface = "org.freedesktop.IBus.InputContext";
    call(&path, iface, "SetCapabilities", (31u32,).to_variant());
    call(&path, iface, "FocusIn", ().to_variant());
    call(
        "/org/freedesktop/IBus",
        "org.freedesktop.IBus",
        "SetGlobalEngine",
        ("tigertype",).to_variant(),
    );
    let commits = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let captured = commits.clone();
    let filter = client.add_filter(move |_, message, incoming| {
        if incoming
            && message.member().as_deref() == Some("CommitText")
            && let Some(body) = message.body()
            && let Some(text) = body.child_value(0).as_variant()
        {
            captured
                .lock()
                .unwrap()
                .push(text.child_value(2).get::<String>().unwrap());
        }
        Some(message.clone())
    });
    for key in [u32::from(b's'), u32::from(b'd'), 0x20] {
        assert_eq!(
            call(
                &path,
                iface,
                "ProcessKeyEvent",
                (key, 0u32, 0u32).to_variant()
            )
            .get::<(bool,)>(),
            Some((true,))
        );
    }
    assert_eq!(*commits.lock().unwrap(), vec!["加"]);
    call(&path, iface, "FocusOut", ().to_variant());
    client.remove_filter(filter);
    // 通过产品停机入口等待退出并释放租约，不让测试后台遗留到下一轮。
    assert!(command("stop").status().unwrap().success());
}
