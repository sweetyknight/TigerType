//! Linux 原生交付：Rust 驱动编译薄插件和 Debian 包，不生成 shell 维护脚本，不安装到构建机。
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tigertype_backend::{resources, storage};
use tigertype_dev::{Result, process, project_root};
use tigertype_linux_app::package;
fn run(command: &mut Command) -> Result<()> {
    let status = process::run(command, Duration::from_secs(1800))?;
    if !status.success() {
        return Err(format!("Linux 打包命令失败：{status}").into());
    }
    Ok(())
}
fn output(command: &mut Command) -> Result<String> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().into())
}
fn write(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    fs::create_dir_all(path.parent().ok_or("输出没有父目录")?)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.set_permissions(fs::Permissions::from_mode(if executable {
        0o755
    } else {
        0o644
    }))?;
    file.sync_all()?;
    Ok(())
}
fn artifact(path: &Path) -> Result<Vec<u8>> {
    // Cargo 可能用硬链接发布编译产物；仅此固定构建入口允许硬链接，不放宽码表或安装包读取。
    storage::reject_links(path.parent().ok_or("产物无父目录")?)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err("产物非普通文件".into());
    }
    let mut bytes = Vec::new();
    file.take(128 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 128 * 1024 * 1024 {
        return Err("产物超限".into());
    }
    Ok(bytes)
}
pub fn main() -> std::process::ExitCode {
    match assemble() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
fn assemble() -> Result<()> {
    if !cfg!(target_arch = "x86_64") {
        return Err("Linux 首批仅支持 x86_64".into());
    }
    let root = project_root()?;
    let target = package::Target::parse(std::str::from_utf8(&storage::read_file(
        Path::new("/usr/lib/os-release"),
        16 * 1024,
    )?)?)?;
    let profile = std::env::var("TIGERTYPE_PACKAGE_PROFILE").unwrap_or_else(|_| "release".into());
    if !["debug", "release"].contains(&profile.as_str()) {
        return Err("打包配置只允许 debug/release".into());
    }
    let mut build = Command::new("cargo");
    build.current_dir(&root).args([
        "build",
        "--locked",
        "--offline",
        "-p",
        "tigertype-linux-app",
        "--bins",
    ]);
    if profile == "release" {
        build.arg("--release");
    }
    run(&mut build)?;
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let binary = artifact(&target_dir.join(&profile).join("tigertype-linux-app"))?;
    let setup = artifact(&target_dir.join(&profile).join("tigertype-linux-deploy"))?;
    let parent = target_dir.join("package");
    fs::create_dir_all(&parent)?;
    let stage = resources::new_directory(&parent.canonicalize()?, "TigerType-linux")?;
    let program = stage.join("program");
    let fonts = stage.join("fonts");
    let bundle = stage.join("bundle");
    fs::create_dir(&program)?;
    fs::create_dir(&fonts)?;
    fs::create_dir(&bundle)?;
    let plugin = stage.join("tigertype.so");
    tigertype_dev::fcitx::build(&root, &plugin)?;
    write(
        &program.join("opt/tigertype/bin/tigertype-linux-app"),
        &binary,
        true,
    )?;
    write(
        &program.join("opt/tigertype/bin/tigertype-setup"),
        &setup,
        true,
    )?;
    write(
        &program.join("usr/lib/x86_64-linux-gnu/fcitx5/tigertype.so"),
        &artifact(&plugin)?,
        false,
    )?;
    for (path, text) in [
        ("usr/share/ibus/component/tigertype.xml", package::IBUS_XML),
        ("usr/share/fcitx5/addon/tigertype.conf", package::ADDON),
        (
            "usr/share/fcitx5/inputmethod/tigertype.conf",
            package::INPUT_METHOD,
        ),
        ("usr/share/applications/tigertype.desktop", package::DESKTOP),
    ] {
        write(&program.join(path), text.as_bytes(), false)?;
    }
    for (path, text) in package::GNOME_FILES {
        write(&program.join(path), text.as_bytes(), false)?;
    }
    for (path, entry) in package::resources()? {
        let font = path.starts_with("usr/share/fonts/");
        let source = root
            .join("resources")
            .join(if font { "fonts" } else { "tiger" })
            .join(&entry.path);
        let bytes = storage::read_file(&source, entry.size)?;
        if bytes.len() as u64 != entry.size || resources::sha256_bytes(&bytes) != entry.sha256 {
            return Err(format!("锁定资源损坏：{}", entry.path).into());
        }
        write(
            &if font {
                fonts.join(path)
            } else {
                program.join(path)
            },
            &bytes,
            false,
        )?;
    }
    let version = format!(
        "{}-{}",
        env!("CARGO_PKG_VERSION"),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
    );
    // dpkg-shlibdeps 从本机已安装包元数据产生 ABI 下限，不猜测 GTK/GLib 的发行版包名。
    write(&stage.join("debian/control"),b"Source: tigertype\nSection: utils\nPriority: optional\nMaintainer: TigerType <noreply@localhost>\nStandards-Version: 4.6.0\n\nPackage: tigertype\nArchitecture: amd64\nDescription: TigerType\n",false)?;
    let dependencies = output(
        Command::new("dpkg-shlibdeps")
            .current_dir(&stage)
            .arg("-O")
            .arg(format!(
                "-e{}",
                program
                    .join("opt/tigertype/bin/tigertype-linux-app")
                    .display()
            ))
            .arg(format!("-e{}", plugin.display())),
    )?;
    let dependencies = dependencies
        .strip_prefix("shlibs:Depends=")
        .ok_or("动态库依赖输出不符")?;
    write(&program.join("DEBIAN/control"),format!("Package: tigertype\nVersion: {version}\nArchitecture: amd64\nMaintainer: TigerType <noreply@localhost>\nDepends: {dependencies}, tigertype-fonts (= {version}), im-config, fontconfig\nDescription: TigerType Rust input method and settings\n").as_bytes(),false)?;
    write(&fonts.join("DEBIAN/control"),format!("Package: tigertype-fonts\nVersion: {version}\nArchitecture: all\nMaintainer: TigerType <noreply@localhost>\nDepends: fontconfig\nDescription: TigerType locked fonts retained after input method removal\n").as_bytes(),false)?;
    // ELF postinst 是同一 Rust 安装器，不嵌入 shell/Python 脚本。
    write(&fonts.join("DEBIAN/postinst"), &setup, true)?;
    for (source, name) in [(&program, "tigertype.deb"), (&fonts, "tigertype-fonts.deb")] {
        run(Command::new("dpkg-deb")
            .args(["--root-owner-group", "-Zxz", "-z6", "--build"])
            .arg(source)
            .arg(bundle.join(name)))?;
    }
    write(&bundle.join("tigertype-setup"), &setup, true)?;
    let files = ["tigertype.deb", "tigertype-fonts.deb", "tigertype-setup"]
        .iter()
        .map(|name| {
            Ok(package::entry(
                name,
                &storage::read_file(&bundle.join(name), 256 * 1024 * 1024)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    write(
        &bundle.join("bundle.json"),
        &serde_json::to_vec_pretty(&package::Manifest {
            format: 1,
            target,
            files,
        })?,
        false,
    )?;
    package::inspect(&bundle)?;
    println!(
        "Linux 安装目录：{}（未安装，运行其中 tigertype-setup 即安装/升级；默认需联网补齐运行依赖）",
        bundle.display()
    );
    Ok(())
}
