//! 安装包打包驱动：构建 release 程序（静态 CRT）、校验锁定码表、随包字体与语言文件（均在仓库内），
//! 再调用 Inno Setup 编译 installer/windows/tigertype.iss。不安装、不注册、不修改系统；产物在 target\package。
use std::{ffi::OsString, fs, path::Path, process::Command, time::Duration};
use tigertype_backend::{Dictionary, TABLES, import, resources, storage::plain};
use tigertype_dev::{Result, process, project_root};
use tigertype_windows_app::fonts;

/// 取自 Inno Setup 官方仓库 is-6_7_3 标签的 Files/Languages/Unofficial/ChineseSimplified.isl。
const LANGUAGE_SHA256: &str = "7d544b9bb1d142cfa11f2e5d3cc8abe2e55f8e066c5124e3772675aa236e1278";

fn cargo(root: &Path, args: &[&str]) -> Result<()> {
    println!("\n== cargo {} ==", args.join(" "));
    let status = process::run(
        Command::new("cargo").args(args).current_dir(root),
        Duration::from_secs(1800),
    )?;
    if !status.success() {
        return Err(format!("构建失败：cargo {} ({status})", args.join(" ")).into());
    }
    Ok(())
}

/// 安装脚本按文件名硬写随包字体与码表；清单变动而脚本未跟上时在打包前失败，不产出缺文件的安装包。
fn check_script(script: &str, locked: &[fonts::Font]) -> Result<()> {
    let listed = |source: &str, other: &str| {
        script
            .lines()
            .any(|l| l.contains(source) && l.contains(other))
    };
    // 源码包名决定构建文件名，部署名属于安装/注册边界；两种位数必须各自映射，不能装错 DLL。
    for (source, destination) in [
        (
            r#"target\release\tigertype_windows_input_method.dll""#,
            r#"DestName: "tigertype_tsf.dll""#,
        ),
        (
            r#"target\i686-pc-windows-msvc\release\tigertype_windows_input_method.dll""#,
            r#"DestName: "tigertype_tsf_x86.dll""#,
        ),
    ] {
        if !listed(source, destination) {
            return Err(
                format!("安装脚本缺少 DLL 构建/部署映射：{source} -> {destination}").into(),
            );
        }
    }
    // 安装定义移动到 installer/windows 后，资源根目录与锁定语言文件也必须同步。
    if !script.contains(r#"#define Root AddBackslash(SourcePath) + "..\..""#)
        || !script.contains(r#"MessagesFile: "{#Root}\installer\windows\ChineseSimplified.isl""#)
    {
        return Err("安装脚本的仓库根目录或锁定语言文件路径不符".into());
    }
    for font in locked {
        let file = format!("resources\\fonts\\{}\"", font.path);
        if !listed(&file, &format!("FontInstall: \"{}\";", font.family)) {
            return Err(format!("安装脚本缺少锁定字体 {}（{}）", font.path, font.family).into());
        }
    }
    // 发布 DLL/EXE 从自身目录加载私有副本，不能只有全机 FontInstall 条目。
    if !listed(
        r#"resources\fonts\LXGWWenKaiGBScreen.ttf""#,
        r#"DestName: "{#PrivateFont}""#,
    ) || !listed(r#"DestName: "{#PrivateFont}""#, r#"DestDir: "{app}""#)
    {
        return Err("安装脚本缺少按摘要命名的私有文楷副本".into());
    }
    // 码表须装到程序旁的 TABLES 目录且保持相对路径，后台按 installed() 在那里加载。
    for name in import::SOURCES {
        let name = name.replace('/', "\\");
        let dest = match name.rsplit_once('\\') {
            Some((dir, _)) => format!("DestDir: \"{{app}}\\{TABLES}\\{dir}\";"),
            None => format!("DestDir: \"{{app}}\\{TABLES}\";"),
        };
        if !listed(&format!("resources\\{TABLES}\\{name}\""), &dest) {
            return Err(format!("安装脚本缺少锁定码表 {name}（须装到 {dest}）").into());
        }
    }
    // 注册 DLL 时要求同目录已有配置图标，图标须排在 DLL 之前安装。
    let icon = script.find("DestName: \"tigertype.ico\"");
    let dll = script.find("DestName: \"tigertype_tsf.dll\"");
    if !matches!((icon, dll), (Some(icon), Some(dll)) if icon < dll) {
        return Err("安装脚本须在 DLL 之前安装 tigertype.ico".into());
    }
    Ok(())
}

fn run() -> Result<()> {
    let root = project_root()?;
    let script = root.join("installer/windows/tigertype.iss");
    let locked = fonts::locked()?;
    check_script(&fs::read_to_string(&script)?, &locked)?;
    for font in &locked {
        let path = root.join("resources").join("fonts").join(&font.path);
        if fs::metadata(&path)?.len() != font.size || resources::sha256(&path)? != font.sha256 {
            return Err(format!("字体与锁定清单不符：{}", path.display()).into());
        }
    }
    // 与后台启动走同一加载与词表校验，坏码表不进安装包。
    let data = import::load(&root.join("resources").join(TABLES))?;
    let (entries, phrases) = (data.entries.len(), data.phrases.len());
    Dictionary::new(data)?;
    println!("码表校验通过：{entries} 条单字编码，{phrases} 条词编码。");
    let language = root.join(r"installer\windows\ChineseSimplified.isl");
    if resources::sha256(&language)? != LANGUAGE_SHA256 {
        return Err(format!("语言文件与锁定摘要不符：{}", language.display()).into());
    }
    let local = std::env::var_os("LOCALAPPDATA").ok_or("缺少 LOCALAPPDATA")?;
    let iscc = Path::new(&local).join(r"Programs\Inno Setup 6\ISCC.exe");
    if !iscc.is_file() {
        return Err(format!(
            "未找到 Inno Setup 编译器：{}（安装方法见 README）",
            iscc.display()
        )
        .into());
    }

    cargo(
        &root,
        &[
            "build",
            "--locked",
            "--offline",
            "--release",
            "-p",
            "tigertype-windows-app",
            "-p",
            "tigertype-windows-input-method",
        ],
    )?;
    cargo(
        &root,
        &[
            "build",
            "--locked",
            "--offline",
            "--release",
            "-p",
            "tigertype-windows-input-method",
            "--target",
            "i686-pc-windows-msvc",
        ],
    )?;

    // 虎码印章图标每次现画（需本机装有霞鹜文楷，缺失时退到楷体）到新建目录，编译后只删除这一个文件与空目录。
    let output = root.join("target").join("package");
    let staging = resources::new_directory(&output, "icon")?;
    let icon = staging.join("tigertype.ico");
    fs::write(&icon, tigertype_windows_ui::icon_file()?)?;

    let version = env!("CARGO_PKG_VERSION");
    println!("\n== ISCC ==");
    // ISCC 不认 canonicalize 产生的 `\\?\` 前缀，路径都先转成常见写法；UNC 原样交给 ISCC 报错。
    let mut define = OsString::from("/DIcon=");
    define.push(plain(&icon));
    let status = process::run(
        // 不加静默参数：编译输出逐个列出打包文件，留作打包记录。
        Command::new(&iscc)
            .arg(format!("/DVersion={version}"))
            .arg(format!(
                "/DPrivateFont={}",
                tigertype_windows_fonts::installed_name().map_err(|e| e.to_string())?
            ))
            .arg(define)
            .arg(plain(&script)),
        Duration::from_secs(1800),
    );
    fs::remove_file(&icon)?;
    fs::remove_dir(&staging)?;
    let status = status?;
    if !status.success() {
        return Err(format!("Inno Setup 编译失败 ({status})").into());
    }
    let setup = output.join(format!("TigerType-Setup-{version}.exe"));
    println!(
        "\n安装包：{}\nSHA256：{}\n资源整体授权未确认，仅限个人在自己的电脑上使用，不公开分发。",
        plain(&setup).display(),
        resources::sha256(&setup)?
    );
    Ok(())
}
pub fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn script_must_list_every_locked_font_and_table() -> Result<()> {
        let script =
            std::fs::read_to_string(project_root()?.join("installer/windows/tigertype.iss"))?;
        let locked = fonts::locked()?;
        check_script(&script, &locked)?;
        // 反例：删掉一个字体或码表行、或把子目录码表装错位置都必须被拒绝。
        let without = |needle: &str| -> String {
            script
                .lines()
                .filter(|l| !l.contains(needle))
                .map(|l| format!("{l}\n"))
                .collect()
        };
        assert!(check_script(&without("TumanPUA.ttf"), &locked).is_err());
        assert!(check_script(&without("DestName: \"{#PrivateFont}\""), &locked).is_err());
        assert!(check_script(&without("PY_c.dict.yaml"), &locked).is_err());
        let flattened = script.replace(
            r#"DestDir: "{app}\tiger\opencc""#,
            r#"DestDir: "{app}\tiger""#,
        );
        assert!(check_script(&flattened, &locked).is_err());
        // 迁移反例：旧构建文件名、位数映射错配和旧层级根目录均不得产出安装包。
        for (from, to) in [
            ("tigertype_windows_input_method.dll", "tigertype_tsf.dll"),
            (
                "DestName: \"tigertype_tsf_x86.dll\"",
                "DestName: \"tigertype_tsf.dll\"",
            ),
            (r#"SourcePath) + "..\..""#, r#"SourcePath) + "..""#),
            (
                r"installer\windows\ChineseSimplified.isl",
                r"installer\ChineseSimplified.isl",
            ),
        ] {
            assert!(
                check_script(&script.replace(from, to), &locked).is_err(),
                "{from}"
            );
        }
        Ok(())
    }
}
