//! Fcitx5 胶水编译统一入口；检查与打包使用相同警告门禁、链接约束和已锁定系统 SDK。
use crate::{Result, process};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
fn flags(package: &str) -> Result<String> {
    let output = Command::new("pkg-config")
        .args(["--cflags", "--libs", package])
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(String::from_utf8(output.stdout)?)
}
pub fn build(root: &Path, target: &Path) -> Result<()> {
    let mut compiler = Command::new("g++");
    compiler
        .args([
            "-std=c++20",
            "-fPIC",
            "-shared",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-Wl,--no-undefined",
        ])
        .arg(root.join("crates/linux/input-method/fcitx/plugin.cpp"));
    if let Some(sdk) = std::env::var_os("TIGERTYPE_FCITX_SDK") {
        let sdk = PathBuf::from(sdk);
        if !sdk.is_absolute() {
            return Err("SDK 路径须为绝对路径".into());
        }
        for name in ["Core", "Config", "Utils"] {
            compiler.arg(format!("-I{}/usr/include/Fcitx5/{name}", sdk.display()));
        }
        compiler.args(["-l:libFcitx5Core.so.7", "-l:libFcitx5Utils.so.2"]);
    } else {
        compiler.args(flags("Fcitx5Core")?.split_whitespace());
    }
    compiler
        .args(flags("gio-2.0")?.split_whitespace())
        .arg("-o")
        .arg(target);
    if !process::run(&mut compiler, Duration::from_secs(120))?.success() {
        return Err("Fcitx5 胶水编译或链接失败".into());
    }
    Ok(())
}
