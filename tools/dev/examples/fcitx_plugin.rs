//! Linux 门禁单列编译 C++ 系统胶水；生成在 Cargo 产物目录，不加载到现用输入框架。
fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    let result = (|| -> tigertype_dev::Result<()> {
        let root = tigertype_dev::project_root()?;
        let directory = std::env::var_os("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| root.join("target"))
            .join("fcitx-check");
        std::fs::create_dir_all(&directory)?;
        tigertype_dev::fcitx::build(&root, &directory.join("tigertype.so"))
    })();
    #[cfg(not(target_os = "linux"))]
    let result: tigertype_dev::Result<()> = Err("Fcitx5 胶水须在 Linux 编译".into());
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
