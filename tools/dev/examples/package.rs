//! 按宿主选择打包实现；只构建和组包，不安装或自动切换输入法。
#[cfg(windows)]
#[path = "../src/package_windows.rs"]
mod platform;
#[cfg(target_os = "linux")]
#[path = "../src/package_linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "../src/package_macos.rs"]
mod platform;
fn main() -> std::process::ExitCode {
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    {
        platform::main()
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        // 未实现的安装包不能返回成功；Linux 交付入口完成前明确拒绝。
        eprintln!("此平台的安装包尚未实现");
        std::process::ExitCode::FAILURE
    }
}
