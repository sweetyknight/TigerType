//! 本机图标离屏测试；不读用户数据、不创建系统输入法连接，输出供人工视觉验收。
fn main() {
    #[cfg(target_os = "macos")]
    {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .canonicalize()
            .unwrap();
        let (icon, menu) = tigertype_macos_app::icons::render(&root).unwrap();
        // 再绘制一次必须重新注册成功，避免图标入口泄漏字体注册或误用系统同名字体。
        tigertype_macos_app::icons::render(&root).unwrap();
        let fonts = tigertype_macos_app::fonts::load(&root.join("resources/fonts")).unwrap();
        let wenkai = objc2_app_kit::NSFont::fontWithName_size(
            &objc2_foundation::NSString::from_str("LXGW WenKai GB Screen"),
            20.0,
        )
        .unwrap();
        let covered = wenkai.coveredCharacterSet();
        let parts = std::fs::read_to_string(root.join("resources/tiger/opencc/hu_cf.txt")).unwrap();
        let pua: std::collections::BTreeSet<_> = parts
            .chars()
            .filter(|c| ('\u{e000}'..='\u{f8ff}').contains(c))
            .collect();
        assert!(!pua.is_empty());
        for c in &pua {
            assert!(
                covered.longCharacterIsMember(*c as u32),
                "私有文楷缺少字根 U+{:04X}",
                *c as u32
            );
        }
        println!("私有文楷覆盖 {} 个码表私用区字根", pua.len());
        // 用户实测上屏缺字的三个扩展汉字必须由随包文楷覆盖，安装后宿主才能回退显示。
        for c in "𤛭𨰻𪚥".chars() {
            assert!(
                covered.longCharacterIsMember(c as u32),
                "随包文楷缺少 U+{:05X}",
                c as u32
            );
        }
        println!("随包文楷覆盖上屏缺字样例：𤛭、𨰻、𪚥");
        for name in [
            "LXGW WenKai GB Screen",
            "Kaiti SC",
            "Songti SC",
            "PingFang SC",
            "GB18030 Bitmap",
        ] {
            let font = objc2_app_kit::NSFont::fontWithName_size(
                &objc2_foundation::NSString::from_str(name),
                20.0,
            );
            println!(
                "字体 {name}：{}",
                font.map_or_else(
                    || "未安装".into(),
                    |f| {
                        format!(
                            "{}，U+20000={}，U+20BB7={}",
                            f.fontName(),
                            f.coveredCharacterSet().longCharacterIsMember(0x20000),
                            f.coveredCharacterSet().longCharacterIsMember(0x20bb7)
                        )
                    }
                )
            );
        }
        drop(wenkai);
        drop(fonts);
        assert_eq!(&icon[..4], b"icns");
        assert_eq!(
            u32::from_be_bytes(icon[4..8].try_into().unwrap()) as usize,
            icon.len()
        );
        assert_eq!(&menu[..8], b"\x89PNG\r\n\x1a\n");
        menu_icon_geometry(&menu);
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

/// 重新解码 PNG，检查菜单栏实际读到的逻辑尺寸和印章留白，防止把 Retina 像素当成点。
#[cfg(target_os = "macos")]
fn menu_icon_geometry(png: &[u8]) {
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageRep, NSImage};
    use objc2_foundation::{NSData, NSSize};

    let data = NSData::with_bytes(png);
    let image = NSImage::initWithData(NSImage::alloc(), &data).unwrap();
    let bitmap = NSBitmapImageRep::imageRepWithData(&data).unwrap();
    assert_eq!(image.size(), NSSize::new(16.0, 16.0));
    assert_eq!((bitmap.pixelsWide(), bitmap.pixelsHigh()), (32, 32));
    let mut bounds = (32, 32, 0, 0);
    for y in 0..32 {
        for x in 0..32 {
            if bitmap.colorAtX_y(x, y).unwrap().alphaComponent() > 0.1 {
                bounds.0 = bounds.0.min(x);
                bounds.1 = bounds.1.min(y);
                bounds.2 = bounds.2.max(x);
                bounds.3 = bounds.3.max(y);
            }
        }
    }
    assert!(bounds.0 > 0 && bounds.1 > 0 && bounds.2 < 31 && bounds.3 < 31);
    assert!((bounds.0 + bounds.2 - 31_isize).abs() <= 1);
    assert!((bounds.1 + bounds.3 - 31_isize).abs() <= 1);
    println!("菜单栏图标：32×32 像素、16×16 点，墨迹居中 {bounds:?}");
}
