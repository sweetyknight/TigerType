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
        font_coverage(&root);
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

/// 覆盖验收针对全部单字及词/简繁输出；四个现场缺字另检查 CoreText 实际回退，不能只看注册成功。
#[cfg(target_os = "macos")]
fn font_coverage(root: &std::path::Path) {
    use objc2::{rc::Retained, runtime::AnyObject};
    use objc2_app_kit::NSFont;
    use objc2_foundation::{NSRange, NSString, NSURL};
    use std::{collections::BTreeSet, ffi::c_void};

    #[link(name = "CoreText", kind = "framework")]
    unsafe extern "C" {
        fn CTFontManagerRegisterFontsForURL(
            url: *const NSURL,
            scope: u32,
            error: *mut *const c_void,
        ) -> bool;
        fn CTFontManagerUnregisterFontsForURL(
            url: *const NSURL,
            scope: u32,
            error: *mut *const c_void,
        ) -> bool;
        fn CTFontCreateWithName(
            name: *const NSString,
            size: f64,
            matrix: *const c_void,
        ) -> *mut AnyObject;
        fn CTFontCreateForString(
            font: *const AnyObject,
            text: *const NSString,
            range: NSRange,
        ) -> *mut AnyObject;
        fn CTFontCopyPostScriptName(font: *const AnyObject) -> *mut NSString;
        fn CTFontGetGlyphsForCharacters(
            font: *const AnyObject,
            chars: *const u16,
            glyphs: *mut u16,
            count: isize,
        ) -> bool;
    }
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../../../../resources/fonts.lock.json")).unwrap();
    let sets: Vec<_> = lock["fonts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            NSFont::fontWithName_size(&NSString::from_str(entry["family"].as_str().unwrap()), 24.0)
                .expect("锁定字体注册后必须可选")
                .coveredCharacterSet()
        })
        .collect();
    let data = tigertype_backend::import::load(&root.join("resources/tiger")).unwrap();
    let mut chars: BTreeSet<_> = data.entries.iter().map(|entry| entry.character).collect();
    let singles = chars.len();
    chars.extend(data.phrases.iter().flat_map(|phrase| phrase.text.chars()));
    for mapping in data.simplified.iter().chain(&data.traditional) {
        chars.insert(mapping.character);
        chars.extend(&mapping.variants);
    }
    let missing: Vec<_> = chars
        .iter()
        .filter(|c| !sets.iter().any(|set| set.longCharacterIsMember(**c as u32)))
        .collect();
    assert!(missing.is_empty(), "随包字体未覆盖码表输出：{missing:?}");
    println!(
        "随包字体覆盖全部 {singles} 个不同单字；含词与简繁映射共 {} 字符",
        chars.len()
    );
    // CGFont 内存私有注册不进入系统自动回退；用进程范围的 URL 注册模拟安装后可发现的字体文件。
    struct Registered(Retained<NSURL>);
    impl Drop for Registered {
        fn drop(&mut self) {
            // SAFETY: 只注销本测试注册的 URL；scope=1 不修改系统字体安装。
            unsafe {
                CTFontManagerUnregisterFontsForURL(&*self.0, 1, std::ptr::null_mut());
            }
        }
    }
    let _registered: Vec<_> = ["TH-Tshyn-P0", "TH-Tshyn-P1", "TH-Tshyn-P2"]
        .into_iter()
        .map(|name| {
            let path = root.join(format!("resources/fonts/{name}.ttf"));
            let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
            // SAFETY: 已由产品加载入口核对原件；URL 在同步注册期间存活，守卫配对注销。
            assert!(unsafe { CTFontManagerRegisterFontsForURL(&*url, 1, std::ptr::null_mut()) });
            Registered(url)
        })
        .collect();
    // 直接调用系统回退并确认真实 glyph；LastResort 也会返回非零 glyph，故同时核对字体身份。
    for family in ["LXGW WenKai GB Screen", "Kaiti SC", "Helvetica"] {
        // SAFETY: CoreText 创建/复制的 CF 对象由 Retained 接管释放；UTF-16 缓冲区在同步调用期间有效。
        unsafe {
            let base = Retained::from_raw(CTFontCreateWithName(
                &*NSString::from_str(family),
                24.0,
                std::ptr::null(),
            ))
            .unwrap();
            for c in "𡈾𡉘𣥢𱼷".chars() {
                let text = NSString::from_str(&c.to_string());
                let fallback = Retained::from_raw(CTFontCreateForString(
                    &*base,
                    &*text,
                    NSRange::new(0, c.len_utf16()),
                ))
                .unwrap();
                let name = Retained::from_raw(CTFontCopyPostScriptName(&*fallback)).unwrap();
                assert_ne!(name.to_string(), "LastResort", "{family} 对 {c} 仍缺字");
                let mut units = [0; 2];
                let units = c.encode_utf16(&mut units);
                let mut glyphs = [0; 2];
                assert!(CTFontGetGlyphsForCharacters(
                    &*fallback,
                    units.as_ptr(),
                    glyphs.as_mut_ptr(),
                    units.len() as isize
                ));
                assert_ne!(glyphs[0], 0);
                println!(
                    "{family}：{c} U+{:X} → {name}，glyph={}",
                    c as u32, glyphs[0]
                );
            }
        }
    }
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
