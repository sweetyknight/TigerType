//! 用户会话登记：只增删 TigerType 项；IBus 安装时刷新发现，Fcitx5 下次登录完成插件加载。
use crate::{
    package,
    platform::{self, Framework},
};
use gio::{
    glib::{self, variant::ToVariant},
    prelude::*,
};
use std::{
    cell::Cell,
    fs,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    time::{Duration, Instant},
};
use tigertype_backend::{Result, storage};

/// 只登记/移除本产品的 GNOME 扩展，不接管用户覆盖件或改变全局扩展开关。
pub fn shell_appearance(remove: bool) -> Result<()> {
    if platform::desktop(&std::env::var("XDG_CURRENT_DESKTOP")?)? != platform::Desktop::Gnome {
        return Ok(());
    }
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("缺少 HOME")?);
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    if !data.is_absolute() {
        return Err("用户数据根不是绝对路径".into());
    }
    if !remove
        && fs::symlink_metadata(
            data.join("gnome-shell/extensions")
                .join(package::GNOME_UUID),
        )
        .is_ok()
    {
        return Err("存在同名个人 GNOME 扩展，未覆盖或激活；请先确认该目录归属".into());
    }
    let settings = gio::Settings::new("org.gnome.shell");
    if !remove && settings.boolean("disable-user-extensions") {
        return Err("用户已禁用 GNOME 扩展；未擅自打开全局开关，候选适配尚未启用".into());
    }
    for key in ["enabled-extensions", "disabled-extensions"] {
        let old = settings.value(key);
        let mut names = old.get::<Vec<String>>().ok_or("GNOME 扩展列表类型不符")?;
        extension_list(&mut names, remove || key == "disabled-extensions");
        if settings.value(key) != old {
            return Err("扩展列表已被修改，未覆盖，请重试".into());
        }
        settings.set_value(key, &names.to_variant())?;
    }
    gio::Settings::sync();
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    let call = |method: &str| {
        bus.call_sync(
            Some("org.gnome.Shell.Extensions"),
            "/org/gnome/Shell/Extensions",
            "org.gnome.Shell.Extensions",
            method,
            Some(&(package::GNOME_UUID,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            3000,
            gio::Cancellable::NONE,
        )
    };
    call(if remove {
        "DisableExtension"
    } else {
        "EnableExtension"
    })?;
    if !remove {
        let (info,) = call("GetExtensionInfo")?
            .get::<(std::collections::BTreeMap<String, glib::Variant>,)>()
            .ok_or("扩展信息类型不符")?;
        let number = |key| {
            info.get(key)
                .and_then(|v| v.get::<f64>().or_else(|| v.get::<u32>().map(f64::from)))
        };
        if number("state") == Some(3.0) || number("state") == Some(4.0) {
            return Err("GNOME 候选扩展加载失败或 Shell 版本不支持，未假称已生效".into());
        }
        let metadata: serde_json::Value = serde_json::from_str(package::GNOME_FILES[0].1)?;
        if number("state") == Some(1.0) && number("version") == metadata["version"].as_f64() {
            println!("虎码 GNOME 候选外观已启用。 ");
        } else {
            // GNOME 50 不支持本地扩展首次热发现/代码重载，不能调用不安全 Eval 绕过。
            println!(
                "虎码候选扩展已自动登记；首次安装或扩展代码升级需注销登录一次，无需手动启用。输入引擎可立即使用，主题适配待下次登录生效。"
            );
        }
    }
    Ok(())
}
fn extension_list(names: &mut Vec<String>, remove: bool) {
    if remove {
        names.retain(|name| name != package::GNOME_UUID);
    } else if !names.iter().any(|name| name == package::GNOME_UUID) {
        names.push(package::GNOME_UUID.into());
    }
}

/// 安装完成后由官方启动器刷新 IBus；等待新总线所有者，再验证新引擎确实可发现。
pub fn refresh_ibus() -> Result<()> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    let (active,) = bus
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&("org.freedesktop.IBus",).to_variant()),
            None,
            gio::DBusCallFlags::NO_AUTO_START,
            2000,
            gio::Cancellable::NONE,
        )?
        .get::<(bool,)>()
        .ok_or("IBus 状态类型不符")?;
    let previous = if active {
        Some(
            bus.call_sync(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "GetNameOwner",
                Some(&("org.freedesktop.IBus",).to_variant()),
                None,
                gio::DBusCallFlags::NO_AUTO_START,
                2000,
                gio::Cancellable::NONE,
            )?
            .get::<(String,)>()
            .ok_or("IBus 所有者类型不符")?
            .0,
        )
    } else {
        None
    };
    println!("正在自动加载 TigerType：刷新 IBus 输入框架，不注销桌面；尚未选定的组合不会保留。");
    wait_ibus_owner(&bus, previous, Duration::from_secs(15), || {
        // 官方启动器识别 GNOME systemd / Wayland 启动方式，不能自行 kill 或覆盖进程参数。
        if !Command::new("/usr/bin/ibus")
            .args([if active { "restart" } else { "start" }, "--daemonize"])
            .status()?
            .success()
        {
            return Err("IBus 自动加载失败，未假称安装完成".into());
        }
        Ok(())
    })?;
    let connection = crate::native::ibus_connection()?;
    let reply = connection.call_sync(
        Some("org.freedesktop.IBus"),
        "/org/freedesktop/IBus",
        "org.freedesktop.IBus",
        "GetEnginesByNames",
        Some(&(vec!["tigertype"],).to_variant()),
        None,
        gio::DBusCallFlags::NO_AUTO_START,
        2000,
        gio::Cancellable::NONE,
    )?;
    let (engines,) = reply
        .get::<(Vec<glib::Variant>,)>()
        .ok_or("引擎列表类型不符")?;
    if engines.len() != 1
        || engines[0].n_children() < 3
        || engines[0].child_value(2).str() != Some("tigertype")
    {
        return Err("IBus 刷新后仍未发现 TigerType，请保留错误信息".into());
    }
    Ok(())
}

fn wait_ibus_owner(
    bus: &gio::DBusConnection,
    previous: Option<String>,
    timeout: Duration,
    start: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let main = glib::MainLoop::new(None, false);
    let ready = Rc::new(Cell::new(false));
    let timed_out = Rc::new(Cell::new(false));
    let deadline = Instant::now() + timeout;
    let done = ready.clone();
    let quit = main.clone();
    // 名字观察先于启动；不能把尚未退出的旧 daemon 当作刷新成功，也不忙轮询。
    let watch = gio::bus_watch_name_on_connection(
        bus,
        "org.freedesktop.IBus",
        gio::BusNameWatcherFlags::NONE,
        move |_, _, owner| {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if Some(owner) != previous.as_deref() && Instant::now() < deadline {
                    done.set(true);
                    quit.quit();
                }
            }));
        },
        |_, _| {},
    );
    let expired = timed_out.clone();
    let quit = main.clone();
    let timer = glib::timeout_add_local_once(timeout, move || {
        expired.set(true);
        quit.quit();
    });
    let result = start();
    if result.is_ok() {
        main.run();
    }
    gio::bus_unwatch_name(watch);
    if !timed_out.get() {
        timer.remove();
    }
    result?;
    if !ready.get() {
        return Err("等待新 IBus 会话超时，未要求用户重登来掩盖失败".into());
    }
    Ok(())
}

pub fn selected() -> Result<Framework> {
    let desktop = platform::desktop(&std::env::var("XDG_CURRENT_DESKTOP")?)?;
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    let owner = |name| {
        bus.call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "GetNameOwner",
            Some(&(name,).to_variant()),
            None,
            gio::DBusCallFlags::NO_AUTO_START,
            2000,
            gio::Cancellable::NONE,
        )
        .ok()
        .and_then(|v| v.get::<(String,)>())
        .map(|v| v.0)
    };
    let ibus = owner("org.freedesktop.IBus");
    let fcitx = owner("org.fcitx.Fcitx5");
    Ok(platform::framework(
        desktop,
        ibus.is_some() && ibus != fcitx,
        fcitx.is_some(),
    )?)
}
pub fn name(framework: Framework) -> &'static str {
    match framework {
        Framework::IBus => "ibus",
        Framework::Fcitx5 => "fcitx",
    }
}
pub fn parse(value: &str) -> Result<Framework> {
    match value {
        "ibus" => Ok(Framework::IBus),
        "fcitx" => Ok(Framework::Fcitx5),
        _ => Err("未知输入框架".into()),
    }
}
fn autostart(framework: Framework) -> String {
    package::session_entry(framework)
}
fn startup_path() -> Result<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("缺少 HOME")?);
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    if !config.is_absolute() {
        return Err("配置根不是绝对路径".into());
    }
    storage::reject_links(&config)?;
    let directory = config.join("autostart");
    storage::reject_links(&directory)?;
    fs::create_dir_all(&directory)?;
    Ok(directory.join("tigertype.desktop"))
}
fn startup(framework: Framework, remove: bool) -> Result<()> {
    let path = startup_path()?;
    let old = match storage::read_file(&path, 16 * 1024) {
        Ok(bytes) => Some(bytes),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    if let Some(bytes) = old.as_ref()
        && ![Framework::IBus, Framework::Fcitx5]
            .iter()
            .any(|f| bytes == autostart(*f).as_bytes())
    {
        return Err("已有自启动文件不是 TigerType 生成的版本，保留原件".into());
    }
    if remove {
        if old.is_some() {
            fs::remove_file(path)?;
        }
        return Ok(());
    }
    atomic_user_file(&path, autostart(framework).as_bytes(), old.as_deref())
}
fn atomic_user_file(path: &Path, bytes: &[u8], expected: Option<&[u8]>) -> Result<()> {
    let parent = path.parent().ok_or("路径无父目录")?;
    storage::reject_links(parent)?;
    let permissions = fs::symlink_metadata(path)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.permissions());
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let stage = parent.join(format!(".tigertype-{}-{nonce}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&stage)?;
    file.write_all(bytes)?;
    // 原配置的权限原样保留，不借替换文件扩大或收紧用户已有权限。
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)?;
    }
    file.sync_all()?;
    let actual = match fs::symlink_metadata(path) {
        Ok(_) => Some(storage::read_file(path, 256 * 1024)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if actual.as_deref() != expected {
        return Err("自启动文件已被外部修改，未覆盖".into());
    }
    fs::rename(&stage, path)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
pub fn configure(framework: Framework, remove: bool) -> Result<()> {
    match framework {
        Framework::IBus => {
            let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
            if platform::desktop(&desktop)? == platform::Desktop::Gnome {
                let schema = gio::SettingsSchemaSource::default()
                    .and_then(|source| source.lookup("org.gnome.desktop.input-sources", true))
                    .ok_or("缺少 GNOME 输入源设置")?;
                let settings = gio::Settings::new_full(&schema, gio::SettingsBackend::NONE, None);
                let old = settings.value("sources");
                let mut sources = old
                    .get::<Vec<(String, String)>>()
                    .ok_or("输入源列表类型不符")?;
                update_sources(&mut sources, remove);
                if settings.value("sources") != old {
                    return Err("输入源列表已被修改，请重试".into());
                }
                settings.set_value("sources", &sources.to_variant())?;
                gio::Settings::sync();
            } else {
                // 非 GNOME 保留既有 IBus 框架，只把引擎加入预载列表，不更改键盘布局。
                let schema = gio::SettingsSchemaSource::default()
                    .and_then(|source| source.lookup("org.freedesktop.ibus.general", true))
                    .ok_or("缺少 IBus 设置")?;
                let settings = gio::Settings::new_full(&schema, gio::SettingsBackend::NONE, None);
                let mut names: Vec<String> = settings
                    .value("preload-engines")
                    .get()
                    .ok_or("IBus 列表类型不符")?;
                names.retain(|name| name != "tigertype");
                if !remove {
                    names.push("tigertype".into());
                }
                settings.set_value("preload-engines", &names.to_variant())?;
                gio::Settings::sync();
            }
        }
        Framework::Fcitx5 => {
            let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
            if remove {
                let active = bus
                    .call_sync(
                        Some("org.freedesktop.DBus"),
                        "/org/freedesktop/DBus",
                        "org.freedesktop.DBus",
                        "NameHasOwner",
                        Some(&("org.fcitx.Fcitx5",).to_variant()),
                        None,
                        gio::DBusCallFlags::NO_AUTO_START,
                        2000,
                        gio::Cancellable::NONE,
                    )?
                    .get::<(bool,)>()
                    .ok_or("框架状态类型不符")?
                    .0;
                if !active {
                    let path = startup_path()?
                        .parent()
                        .and_then(|p| p.parent())
                        .ok_or("配置路径不符")?
                        .join("fcitx5/profile");
                    match fs::symlink_metadata(&path) {
                        Ok(_) => {
                            let old = storage::read_file(&path, 256 * 1024)?;
                            let next = remove_fcitx_profile(std::str::from_utf8(&old)?)?;
                            if next.as_bytes() != old {
                                atomic_user_file(&path, next.as_bytes(), Some(&old))?;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error.into()),
                    }
                    return startup(framework, true);
                }
            }
            let call = |method, args: glib::Variant| {
                bus.call_sync(
                    Some("org.fcitx.Fcitx5"),
                    "/controller",
                    "org.fcitx.Fcitx.Controller1",
                    method,
                    Some(&args),
                    None,
                    gio::DBusCallFlags::NO_AUTO_START,
                    2000,
                    gio::Cancellable::NONE,
                )
            };
            let (groups,) = call("InputMethodGroups", ().to_variant())?
                .get::<(Vec<String>,)>()
                .ok_or("框架分组类型不符")?;
            if groups.is_empty() || groups.len() > 32 {
                return Err("Fcitx 分组为空或超限，未修改配置".into());
            }
            for group in groups {
                let (layout, mut entries) = call("InputMethodGroupInfo", (&group,).to_variant())?
                    .get::<(String, Vec<(String, String)>)>()
                    .ok_or("框架列表类型不符")?;
                entries.retain(|(name, _)| name != "tigertype");
                if !remove {
                    entries.push(("tigertype".into(), String::new()));
                }
                call(
                    "SetInputMethodGroupInfo",
                    (&group, layout, entries).to_variant(),
                )?;
            }
        }
    }
    startup(framework, remove)?;
    Ok(())
}
fn update_sources(sources: &mut Vec<(String, String)>, remove: bool) {
    let present = sources
        .iter()
        .any(|(kind, name)| kind == "ibus" && name == "tigertype");
    if remove {
        sources.retain(|(kind, name)| kind != "ibus" || name != "tigertype");
    } else if !present {
        sources.push(("ibus".into(), "tigertype".into()));
    }
}
/// 框架未运行时仅删除 profile 中本产品的条目，保留其他输入法、注释与分组；重排剩余项避免序号空洞。
fn remove_fcitx_profile(text: &str) -> Result<String> {
    let mut sections: Vec<(String, Vec<&str>)> = vec![(String::new(), Vec::new())];
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            sections.push((trimmed[1..trimmed.len() - 1].into(), vec![line]));
        } else {
            sections.last_mut().ok_or("配置分段失败")?.1.push(line);
        }
    }
    let mut removed = std::collections::BTreeMap::<String, Vec<u32>>::new();
    let mut seen = std::collections::BTreeSet::new();
    for (name, lines) in &sections {
        let parts: Vec<_> = name.split('/').collect();
        if parts.len() == 4
            && parts[0] == "Groups"
            && parts[2] == "Items"
            && let (Ok(group), Ok(index)) = (parts[1].parse::<u32>(), parts[3].parse::<u32>())
        {
            if !seen.insert((group, index)) {
                return Err("Fcitx 条目编号重复，保留配置".into());
            }
            if lines.iter().any(|line| {
                line.trim()
                    .split_once('=')
                    .is_some_and(|(k, v)| k.trim() == "Name" && v.trim() == "tigertype")
            }) {
                removed.entry(parts[1].into()).or_default().push(index);
            }
        }
    }
    let mut output = String::new();
    for (name, lines) in sections {
        let parts: Vec<_> = name.split('/').collect();
        if parts.len() == 4
            && parts[0] == "Groups"
            && parts[2] == "Items"
            && parts[1].parse::<u32>().is_ok()
            && parts[3].parse::<u32>().is_ok()
        {
            let names: Vec<_> = lines
                .iter()
                .filter_map(|line| line.trim().split_once('='))
                .filter(|(k, _)| k.trim() == "Name")
                .collect();
            if names.len() != 1 {
                return Err("Fcitx 条目名称缺失或重复，保留配置".into());
            }
            if names[0].1.trim() == "tigertype" {
                continue;
            }
            // 按原编号扣除被删项，不按文本出现顺序重排，保留用户原有的语义顺序。
            let old = parts[3].parse::<u32>()?;
            let index = old
                - removed
                    .get(parts[1])
                    .map(|items| items.iter().filter(|index| **index < old).count() as u32)
                    .unwrap_or(0);
            let ending = if lines[0].ends_with("\r\n") {
                "\r\n"
            } else if lines[0].ends_with('\n') {
                "\n"
            } else {
                ""
            };
            output.push_str(&format!("[Groups/{}/Items/{}]{ending}", parts[1], index));
            for line in &lines[1..] {
                output.push_str(line);
            }
        } else {
            for line in lines {
                if parts.len() == 2
                    && parts[0] == "Groups"
                    && line
                        .trim()
                        .split_once('=')
                        .is_some_and(|(k, v)| k.trim() == "DefaultIM" && v.trim() == "tigertype")
                {
                    output.push_str("DefaultIM=\n");
                } else {
                    output.push_str(line);
                }
            }
        }
    }
    Ok(output)
}
pub fn prepare_login(framework: Framework) -> Result<()> {
    if framework == Framework::Fcitx5
        && platform::desktop(&std::env::var("XDG_CURRENT_DESKTOP")?)? == platform::Desktop::Plasma
        && std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
    {
        // KWin 的输入法 socket 由合成器交付，不能仅靠通用自启动；只填空值或保留同一框架，不覆盖其他虚拟键盘。
        let version = if Path::new("/usr/bin/kwriteconfig6").is_file() {
            "6"
        } else {
            "5"
        };
        let home = PathBuf::from(std::env::var_os("HOME").ok_or("缺少 HOME")?);
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        storage::reject_links(&config)?;
        if fs::symlink_metadata(config.join("kwinrc")).is_ok() {
            storage::read_file(&config.join("kwinrc"), 1024 * 1024)?;
        }
        let output = Command::new(format!("/usr/bin/kreadconfig{version}"))
            .args([
                "--file",
                "kwinrc",
                "--group",
                "Wayland",
                "--key",
                "InputMethod",
                "--default",
                "",
            ])
            .output()?;
        if !output.status.success()
            && !(output.status.code() == Some(1) && output.stdout.is_empty())
        {
            return Err("无法读取 KWin 输入法设置".into());
        }
        kwin_choice(String::from_utf8(output.stdout)?.trim())?;
        if !Command::new(format!("/usr/bin/kwriteconfig{version}"))
            .args([
                "--file",
                "kwinrc",
                "--group",
                "Wayland",
                "--key",
                "InputMethod",
                "/usr/share/applications/fcitx5-wayland-launcher.desktop",
            ])
            .status()?
            .success()
        {
            return Err("KWin 输入法登记失败".into());
        }
        startup(framework, false)?;
        return Ok(());
    }
    // 框架未启动时由发行版 im-config 设置下一次登录；不强杀当前框架或抢占 Wayland socket。
    let tool = match framework {
        Framework::IBus => "ibus",
        Framework::Fcitx5 => "fcitx5",
    };
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    let name = if framework == Framework::IBus {
        "org.freedesktop.IBus"
    } else {
        "org.fcitx.Fcitx5"
    };
    let active = bus
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&(name,).to_variant()),
            None,
            gio::DBusCallFlags::NO_AUTO_START,
            2000,
            gio::Cancellable::NONE,
        )?
        .get::<(bool,)>()
        .ok_or("框架状态类型不符")?
        .0;
    if !active
        && !Command::new("/usr/bin/im-config")
            .args(["-n", tool])
            .status()?
            .success()
    {
        return Err("框架会话配置失败，未假称安装完成".into());
    }
    startup(framework, false)?;
    Ok(())
}
fn kwin_choice(current: &str) -> Result<()> {
    if !current.is_empty() && current != "/usr/share/applications/fcitx5-wayland-launcher.desktop" {
        return Err("已有其他 KWin 虚拟键盘，未覆盖；需要先明确是否替换".into());
    }
    Ok(())
}
/// 登录时等框架名字出现再登记，不忙轮询、不按键读文件；panic 不跨 GIO 回调。
pub fn watch(framework: Framework) -> impl Drop {
    struct Watch<F: FnOnce()>(Option<F>);
    impl<F: FnOnce()> Drop for Watch<F> {
        fn drop(&mut self) {
            if let Some(close) = self.0.take() {
                close();
            }
        }
    }
    let id = gio::bus_watch_name(
        gio::BusType::Session,
        "org.fcitx.Fcitx5",
        gio::BusNameWatcherFlags::NONE,
        move |_, _, _| {
            let _ = std::panic::catch_unwind(|| {
                if let Err(error) = configure(framework, false) {
                    eprintln!("登录登记未完成：{error}");
                }
            });
        },
        |_, _| {},
    );
    Watch(Some(move || gio::bus_unwatch_name(id)))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extension_registration_only_changes_our_entry() {
        let mut list = vec!["other@example".into()];
        extension_list(&mut list, false);
        extension_list(&mut list, false);
        assert_eq!(list, ["other@example", package::GNOME_UUID]);
        extension_list(&mut list, true);
        assert_eq!(list, ["other@example"]);
    }
    #[test]
    fn reload_wait_requires_a_new_owner_and_propagates_start_failure() {
        use std::io::{BufRead, BufReader};
        use std::process::{Child, Stdio};
        struct Bus(Child);
        impl Drop for Bus {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        // 只在私有总线模拟名字交接；不启动、重启或登记现用桌面的 IBus。
        let mut daemon = Bus(Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap());
        let mut address = String::new();
        BufReader::new(daemon.0.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let connect = || {
            gio::DBusConnection::for_address_sync(
                address.trim(),
                gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
                    | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
                None,
                gio::Cancellable::NONE,
            )
            .unwrap()
        };
        let first = connect();
        let second = connect();
        let own = |bus: &gio::DBusConnection, acquire| {
            let args = if acquire {
                ("org.freedesktop.IBus", 4u32).to_variant()
            } else {
                ("org.freedesktop.IBus",).to_variant()
            };
            bus.call_sync(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                if acquire {
                    "RequestName"
                } else {
                    "ReleaseName"
                },
                Some(&args),
                None,
                gio::DBusCallFlags::NONE,
                1000,
                gio::Cancellable::NONE,
            )
            .unwrap();
        };
        let context = glib::MainContext::default();
        let _guard = context.acquire().unwrap();
        own(&first, true);
        wait_ibus_owner(&first, None, Duration::from_secs(1), || Ok(())).unwrap();
        let old = first.unique_name().unwrap().to_string();
        assert!(
            wait_ibus_owner(&first, Some(old.clone()), Duration::from_millis(30), || Ok(
                ()
            ))
            .is_err()
        );
        assert!(
            wait_ibus_owner(&first, None, Duration::from_secs(1), || Err(
                "启动失败".into()
            ))
            .unwrap_err()
            .to_string()
            .contains("启动失败")
        );
        wait_ibus_owner(&first, Some(old), Duration::from_secs(1), || {
            own(&first, false);
            own(&second, true);
            Ok(())
        })
        .unwrap();
    }
    #[test]
    fn kwin_does_not_replace_an_existing_unrelated_virtual_keyboard() {
        assert!(kwin_choice("").is_ok());
        assert!(kwin_choice("/usr/share/applications/fcitx5-wayland-launcher.desktop").is_ok());
        assert!(kwin_choice("/usr/share/applications/maliit-keyboard.desktop").is_err());
    }
    #[test]
    fn offline_removal_only_removes_our_profile_entry_and_closes_index_gaps() {
        let before = "[Groups/0]\nName=Default\nDefaultIM=tigertype\n[Groups/0/Items/0]\nName=keyboard-us\nLayout=us\n[Groups/0/Items/1]\nName=tigertype\nLayout=\n[Groups/0/Items/2]\n# retain comment\nName=pinyin\nLayout=\n[GroupOrder]\n0=Default\n";
        let next = remove_fcitx_profile(before).unwrap();
        assert!(!next.contains("tigertype"));
        assert!(next.contains("[Groups/0/Items/1]\n# retain comment\nName=pinyin"));
        assert!(next.contains("Name=keyboard-us\nLayout=us"));
        assert!(next.contains("[GroupOrder]\n0=Default"));
        assert_eq!(remove_fcitx_profile(&next).unwrap(), next);
        assert!(remove_fcitx_profile("[Groups/0/Items/0]\nName=pinyin\nName=tigertype\n").is_err());
        let unordered = "[Groups/0/Items/2]\nName=pinyin\n[Groups/0/Items/0]\nName=keyboard-us\n[Groups/0/Items/1]\nName=tigertype\n";
        let next = remove_fcitx_profile(unordered).unwrap();
        assert!(next.starts_with("[Groups/0/Items/1]\nName=pinyin"));
        assert!(next.contains("[Groups/0/Items/0]\nName=keyboard-us"));
    }
    #[test]
    fn startup_replacement_preserves_conflicts_and_rejects_dangling_links() {
        let root = tigertype_backend::resources::new_directory(
            &std::env::temp_dir().canonicalize().unwrap(),
            "tigertype-session-files",
        )
        .unwrap();
        let path = root.join("autostart.desktop");
        atomic_user_file(&path, b"first", None).unwrap();
        assert!(atomic_user_file(&path, b"bad", None).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"first");
        atomic_user_file(&path, b"second", Some(b"first")).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        let alias = root.join("alias.desktop");
        std::os::unix::fs::symlink(root.join("missing"), &alias).unwrap();
        assert!(atomic_user_file(&alias, b"bad", None).is_err());
        assert!(fs::symlink_metadata(alias).unwrap().is_symlink());
    }
    #[test]
    fn adding_and_removing_our_source_preserves_other_entries_and_order() {
        let original = vec![
            ("xkb".into(), "us".into()),
            ("ibus".into(), "pinyin".into()),
        ];
        let mut list = original.clone();
        update_sources(&mut list, false);
        update_sources(&mut list, false);
        assert_eq!(list.len(), 3);
        update_sources(&mut list, true);
        assert_eq!(list, original);
    }
}
