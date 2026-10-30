//! 本机图标离屏测试；不读用户数据、不创建系统输入法连接，输出供人工视觉验收。
fn main() {
    #[cfg(target_os = "macos")]
    {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .canonicalize()
            .unwrap();
        let (icon, menu) = tigertype_macos_app::icons::render(&root).unwrap();
        assert_eq!(&icon[..4], b"icns");
        assert_eq!(
            u32::from_be_bytes(icon[4..8].try_into().unwrap()) as usize,
            icon.len()
        );
        assert_eq!(&menu[..8], b"\x89PNG\r\n\x1a\n");
        let output = tigertype_backend::resources::new_directory(
            &std::env::temp_dir().canonicalize().unwrap(),
            "tigertype-mac-icons",
        )
        .unwrap();
        std::fs::write(output.join("TigerType.icns"), icon).unwrap();
        std::fs::write(output.join("TigerTypeTemplate.png"), menu).unwrap();
        println!("图标格式断言通过；需人工检查：{}", output.display());
    }
    #[cfg(not(target_os = "macos"))]
    eprintln!("Mac 图标原生测试未运行（非 macOS），不计为 passed。");
}
