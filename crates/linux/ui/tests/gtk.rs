//! 真实 GTK 检查使用同一源文件，独立测试进程主线程创建窗口；需要图形会话，非 Linux 宿主明确不计为通过。
#[cfg(target_os = "linux")]
#[path = "support/fonts.rs"]
mod fonts;
#[cfg(target_os = "linux")]
#[path = "../src/gtk.rs"]
mod gtk;
#[cfg(target_os = "linux")]
use tigertype_linux_ui::style;
fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    {
        // 显式读入仓库字体，扩展汉字与私用区字根不依赖本机是否装过。
        let resources =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../resources/fonts");
        if let Some(code) = fonts::relaunch(&resources) {
            return code;
        }
        gtk::tests::run();
    }
    #[cfg(not(target_os = "linux"))]
    eprintln!("GTK 原生检查未运行（非 Linux 宿主），不计为 passed。");
    std::process::ExitCode::SUCCESS
}
