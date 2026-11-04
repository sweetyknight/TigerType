//! 开发期字体：以自身为子进程重新启动，让 fontconfig 另读仓库锁定字体；不安装到系统或用户字体目录。
//! 原生测试与合成预览共用；正式安装由后续 Linux 安装器按字体清单完成。
use std::{
    path::Path,
    process::{Command, ExitCode},
};

/// 已在带字体配置的子进程中时返回 None，调用方继续；否则启动子进程并返回其退出码。
pub fn relaunch(fonts: &Path) -> Option<ExitCode> {
    const MARK: &str = "TIGERTYPE_DEV_FONTS";
    if std::env::var_os(MARK).is_some() {
        return None;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("tigertype-fonts-{}-{stamp}", std::process::id()));
    std::fs::create_dir(&dir).expect("创建临时字体配置目录失败");
    let xml = |path: &Path| {
        path.to_str()
            .expect("字体路径须为 UTF-8")
            .replace('&', "&amp;")
            .replace('<', "&lt;")
    };
    // 缓存写进临时目录（放在系统配置之前才会被选中），系统字体照常可用作回退。
    let config = dir.join("fonts.conf");
    std::fs::write(
        &config,
        format!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\">\n<fontconfig>\n  <cachedir>{}</cachedir>\n  <include ignore_missing=\"yes\">/etc/fonts/fonts.conf</include>\n  <dir>{}</dir>\n</fontconfig>\n",
            xml(&dir.join("cache")),
            xml(fonts)
        ),
    )
    .unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args(std::env::args_os().skip(1))
        .env(MARK, "1")
        .env("FONTCONFIG_FILE", &config)
        .status()
        .expect("启动带字体配置的子进程失败");
    std::fs::remove_dir_all(&dir).ok();
    Some(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
