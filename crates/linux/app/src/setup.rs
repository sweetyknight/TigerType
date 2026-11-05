//! 单入口安装/升级/卸载：权限分离，APT 处理系统包事务，用户配置仅由原用户修改，字体包保留。
use crate::package::{self, Target};
use std::{
    fs,
    io::{IsTerminal, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    process::Command,
};
use tigertype_backend::{Result, resources, storage};
fn target() -> Result<Target> {
    Target::parse(std::str::from_utf8(&storage::read_file(
        Path::new("/usr/lib/os-release"),
        16 * 1024,
    )?)?)
}
fn run(command: &mut Command) -> Result<()> {
    let status = command.status()?;
    if !status.success() {
        return Err(format!("命令失败（{status}），安装可能未完成，请保留错误信息后重试").into());
    }
    Ok(())
}
fn privilege_program(terminal: bool, sudo_available: bool) -> &'static str {
    // 终端使用 sudo 自带认证，桌面使用 polkit；认证被拒后绝不改走另一通道重试。
    if terminal && sudo_available {
        "/usr/bin/sudo"
    } else {
        "/usr/bin/pkexec"
    }
}
fn elevate(exe: &Path) -> Command {
    let mut command = Command::new(privilege_program(
        std::io::stdin().is_terminal(),
        Path::new("/usr/bin/sudo").is_file(),
    ));
    // exe 是绝对路径，不会被当成选项；pkexec 不认 `--`，会把它当作程序名。
    command.arg(exe);
    command
}
pub fn main() -> std::result::Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    // 维护脚本与提权阶段没有用户会话；只有普通用户入口在无终端（双击）时补发桌面通知。
    let user = !args
        .first()
        .is_some_and(|arg| arg.ends_with("tigertype-fonts.postinst"))
        && args.get(1).map(String::as_str) != Some("--system");
    let notify_user = user && !std::io::stdin().is_terminal();
    let result = execute();
    match &result {
        Ok(Some(message)) => {
            println!("{message}");
            if notify_user {
                notify("TigerType 已完成", message);
            }
        }
        Err(error) if notify_user => notify("TigerType 未完成", &error.to_string()),
        _ => {}
    }
    result.map(|_| ()).map_err(|error| error.to_string())
}
/// stdout 无人可见时用桌面通知告知结果；通知失败不影响安装结论。
fn notify(summary: &str, body: &str) {
    let _ = Command::new("/usr/bin/gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.Notifications",
            "--object-path",
            "/org/freedesktop/Notifications",
            "--method",
            "org.freedesktop.Notifications.Notify",
            "'TigerType'",
            "0",
            "'input-keyboard'",
        ])
        .arg(gvariant_string(summary))
        .arg(gvariant_string(body))
        .args(["[]", "{}", "-1"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}
/// GVariant 文本格式的字符串字面量，错误信息中的引号与反斜杠不能破坏参数解析。
fn gvariant_string(text: &str) -> String {
    let mut quoted = String::from("'");
    for c in text.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '\'' => quoted.push_str("\\'"),
            '\n' => quoted.push_str("\\n"),
            _ => quoted.push(c),
        }
    }
    quoted.push('\'');
    quoted
}
fn execute() -> Result<Option<String>> {
    let exe = std::env::current_exe()?;
    let invoked = std::env::args_os().next().unwrap_or_default();
    if invoked
        .to_string_lossy()
        .ends_with("tigertype-fonts.postinst")
    {
        // dpkg 直接执行 Rust ELF 维护程序；测试根不更新宿主字体缓存。
        if std::env::var_os("DPKG_ROOT").is_some_and(|root| !root.is_empty()) {
            return Ok(None);
        }
        return run(
            Command::new("/usr/bin/fc-cache").args(["-f", "/usr/share/fonts/truetype/tigertype"])
        )
        .map(|()| None);
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    // SAFETY: 仅查询有效身份；系统事务和用户桌面设置必须分开执行。
    let root = unsafe { libc::geteuid() } == 0;
    if args.first().map(String::as_str) == Some("--system") {
        if !root {
            return Err("系统安装阶段必须经提权运行".into());
        }
        return system(&args[1..]).map(|()| None);
    }
    if root {
        return Err("请以桌面普通用户运行安装器，由它请求提权".into());
    }
    if !cfg!(target_arch = "x86_64") {
        return Err("安装包仅支持 x86_64".into());
    }
    target()?;
    let command = args.first().map(String::as_str).unwrap_or("install");
    if args.len() > 1 || !["install", "upgrade", "uninstall"].contains(&command) {
        return Err("用法：tigertype-setup [install|upgrade|uninstall]".into());
    }
    if command == "uninstall" {
        // 不猜当初装在哪个框架：两种框架都只移除本产品条目，再停后台和卸系统包。
        if Path::new(package::PROGRAM).exists() {
            run(Command::new(package::PROGRAM).args(["integration", "remove"]))?;
            let _ = Command::new(package::PROGRAM).arg("stop").status();
        }
        run(elevate(&exe).args(["--system", "uninstall"]))?;
        return Ok(Some(
            "TigerType 已卸载；个人数据和全部字体保留，现用框架无需卸载。请注销重登以释放旧插件。"
                .into(),
        ));
    }
    // 选择在提权前进行，框架诊断由已存在的桌面 gdbus 完成，安装器本身不链接 GTK。
    let framework = select_framework()?;
    let bundle = exe.parent().ok_or("安装包目录缺失")?;
    let manifest = package::inspect(bundle)?;
    if manifest.target != target()? {
        return Err("安装包构建发行版与当前系统不符，请使用对应版本的包".into());
    }
    if Path::new(package::PROGRAM).exists() {
        let _ = Command::new(package::PROGRAM).arg("stop").status();
    }
    run(elevate(&exe)
        .args(["--system", "install"])
        .arg(bundle)
        .arg(framework))?;
    run(Command::new(package::PROGRAM).args(["integration", framework, "install"]))?;
    let activation = if framework == "ibus" {
        "IBus 已自动加载新引擎，可在系统输入法菜单选择 TigerType；GNOME 候选外观在首次安装或扩展更新后需注销重登一次。"
    } else {
        "Fcitx5 原生插件需注销重登后加载，再选择 TigerType。"
    };
    Ok(Some(format!(
        "安装文件与会话登记已完成。{activation}无需手改配置。若要卸载，运行 {}/bin/tigertype-setup uninstall。",
        package::PREFIX
    )))
}

fn select_framework() -> Result<&'static str> {
    let desktop = crate::platform::desktop(&std::env::var("XDG_CURRENT_DESKTOP")?)?;
    let owner = |name: &str| -> Result<Option<String>> {
        let output = Command::new("/usr/bin/gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.freedesktop.DBus",
                "--object-path",
                "/org/freedesktop/DBus",
                "--method",
                "org.freedesktop.DBus.GetNameOwner",
                name,
            ])
            .output()?;
        if output.status.success() {
            Ok(Some(String::from_utf8(output.stdout)?))
        } else if String::from_utf8_lossy(&output.stderr).contains("NameHasNoOwner") {
            Ok(None)
        } else {
            Err("不能确认现用输入框架，未修改系统".into())
        }
    };
    let ibus = owner("org.freedesktop.IBus")?;
    let fcitx = owner("org.fcitx.Fcitx5")?;
    Ok(
        match crate::platform::framework(desktop, ibus.as_deref(), fcitx.as_deref())? {
            crate::platform::Framework::IBus => "ibus",
            crate::platform::Framework::Fcitx5 => "fcitx",
        },
    )
}
fn system(args: &[String]) -> Result<()> {
    if args == ["uninstall"] {
        // 不调用 autoremove，字体包和用户目录不在卸载集合内。
        return run(Command::new("/usr/bin/dpkg").args(["--remove", "tigertype"]));
    }
    if args.len() != 3 || args[0] != "install" || !["ibus", "fcitx"].contains(&args[2].as_str()) {
        return Err("非法系统安装参数".into());
    }
    let source = Path::new(&args[1]);
    let manifest = package::inspect(source)?;
    let target = target()?;
    if manifest.target != target {
        return Err("系统安装目标不符".into());
    }
    // 不接管同名手工目录或包外文件；由包管理器持有的旧版本才允许升级。
    for (path, owner) in [
        (package::PREFIX, "tigertype"),
        ("/usr/share/fonts/truetype/tigertype", "tigertype-fonts"),
    ] {
        storage::reject_links(Path::new(path))?;
        if Path::new(path).exists()
            && !Command::new("/usr/bin/dpkg-query")
                .args(["-S", path])
                .output()?
                .stdout
                .starts_with(format!("{owner}:").as_bytes())
        {
            return Err(format!("已有非受管目录，未覆盖：{path}").into());
        }
    }
    let mut files: Vec<_> = package::program_files()
        .into_iter()
        .map(|path| (path.to_owned(), "tigertype"))
        .collect();
    files.extend(package::resources()?.into_iter().map(|(path, _)| {
        let owner = if path.starts_with("usr/share/fonts/") {
            "tigertype-fonts"
        } else {
            "tigertype"
        };
        (path, owner)
    }));
    for (path, owner) in files {
        let path = Path::new("/").join(path);
        storage::reject_links(path.parent().ok_or("安装项缺少父目录")?)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.is_file()
                    || metadata.nlink() != 1
                    || metadata.uid() != 0
                    || metadata.mode() & 0o022 != 0
                {
                    return Err(
                        format!("安装目标类型/权限不安全，保留原件：{}", path.display()).into(),
                    );
                }
                let output = Command::new("/usr/bin/dpkg-query")
                    .arg("-S")
                    .arg(&path)
                    .output()?;
                if !output.status.success()
                    || !output.stdout.starts_with(format!("{owner}:").as_bytes())
                {
                    return Err(format!("安装项不属于本程序，保留原件：{}", path.display()).into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let stage = resources::new_directory(Path::new("/var/tmp"), "tigertype-install")?;
    fs::set_permissions(&stage, fs::Permissions::from_mode(0o755))?;
    for file in &manifest.files {
        // 校验与复制使用同一份字节，提权后 APT 只读取 root 独占的副本，避免用户替换下载包。
        let bytes = package::read(source, file)?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(stage.join(&file.path))?;
        output.write_all(&bytes)?;
        output.sync_all()?;
    }
    let sources = stage.join("official.list");
    fs::write(&sources, target.sources())?;
    let lists = stage.join("lists");
    fs::create_dir(&lists)?;
    let apt = || {
        let mut command = Command::new("/usr/bin/apt-get");
        command.args([
            "-o",
            &format!("Dir::Etc::sourcelist={}", sources.display()),
            "-o",
            "Dir::Etc::sourceparts=-",
            "-o",
            &format!("Dir::State::lists={}", lists.display()),
            "-o",
            "Acquire::AllowInsecureRepositories=false",
            "-o",
            "Acquire::https::Verify-Peer=true",
        ]);
        command
    };
    run(apt().arg("update"))?;
    let mut install = apt();
    // 依赖求解不得顺手移除别的输入法或用户软件；发生冲突就失败保留原系统。
    install.args(["-y", "--no-remove", "install", "fontconfig", "im-config"]);
    if args[2] == "ibus" {
        install.arg("ibus");
    } else {
        install.args([
            "fcitx5",
            "fcitx5-frontend-gtk3",
            "fcitx5-frontend-gtk4",
            "fcitx5-frontend-qt5",
            "fcitx5-frontend-qt6",
        ]);
    }
    install
        .arg(stage.join("tigertype-fonts.deb"))
        .arg(stage.join("tigertype.deb"));
    run(&mut install)?;
    for (path, entry) in package::resources()? {
        let bytes = storage::read_file(&Path::new("/").join(&path), entry.size)?;
        if bytes.len() as u64 != entry.size || resources::sha256_bytes(&bytes) != entry.sha256 {
            return Err(format!("安装后资源校验失败：{path}；保留安装现场，请重试安装").into());
        }
    }
    // 系统安装成功后才清理本次 root 临时目录；失败保留全部证据，不删除旧用户数据。
    let resolved = stage.canonicalize()?;
    if resolved != stage
        || resolved.parent() != Some(Path::new("/var/tmp"))
        || fs::metadata(&resolved)?.uid() != 0
    {
        return Err("临时安装目录清理边界不符".into());
    }
    fs::remove_dir_all(resolved)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notification_text_is_a_valid_gvariant_string() {
        assert_eq!(gvariant_string("plain"), "'plain'");
        assert_eq!(gvariant_string("a'b"), r"'a\'b'");
        assert_eq!(gvariant_string(r"c\d"), r"'c\\d'");
        assert_eq!(gvariant_string("e\nf"), r"'e\nf'");
    }
    #[test]
    fn terminal_authentication_uses_sudo_only_when_available() {
        assert_eq!(privilege_program(true, true), "/usr/bin/sudo");
        for (terminal, available) in [(false, false), (false, true), (true, false)] {
            assert_eq!(privilege_program(terminal, available), "/usr/bin/pkexec");
        }
    }
}
