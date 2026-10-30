//! 按宿主选择打包实现；只构建和组包，不安装或自动切换输入法。
#[cfg(windows)]
#[path = "../src/package_windows.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "../src/package_macos.rs"]
mod platform;
fn main() -> std::process::ExitCode {
    platform::main()
}
