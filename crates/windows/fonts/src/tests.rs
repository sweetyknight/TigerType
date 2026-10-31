//! 仅用新建合成目录和进程私有字体；不安装/卸载系统字体。
use super::*;

fn source() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../resources/fonts/LXGWWenKaiGBScreen.ttf")
        .canonicalize()
        .unwrap()
}
fn checksum(bytes: &[u8]) -> u32 {
    bytes.chunks(4).fold(0u32, |sum, part| {
        let mut word = [0; 4];
        word[..part.len()].copy_from_slice(part);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}
/// 仅给测试副本换唯一族名；补齐 SFNT 校验和，原始锁定文件绝不改动。
fn rename_test_font(bytes: &mut [u8], name: &str) {
    let from: Vec<_> = crate::FAMILY
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect();
    let to: Vec<_> = name.encode_utf16().flat_map(u16::to_be_bytes).collect();
    assert_eq!(from.len(), to.len());
    let mut replaced = 0;
    for at in 0..=bytes.len() - from.len() {
        if bytes[at..at + from.len()] == from {
            bytes[at..at + from.len()].copy_from_slice(&to);
            replaced += 1;
        }
    }
    assert!(replaced > 0);
    let count = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
    let mut head = 0;
    for at in (12..12 + count * 16).step_by(16) {
        if &bytes[at..at + 4] == b"head" {
            head = u32::from_be_bytes(bytes[at + 8..at + 12].try_into().unwrap()) as usize;
        }
    }
    assert!(head > 0);
    bytes[head + 8..head + 12].fill(0);
    for at in (12..12 + count * 16).step_by(16) {
        let offset = u32::from_be_bytes(bytes[at + 8..at + 12].try_into().unwrap()) as usize;
        let size = u32::from_be_bytes(bytes[at + 12..at + 16].try_into().unwrap()) as usize;
        let sum = checksum(&bytes[offset..offset + size]);
        bytes[at + 4..at + 8].copy_from_slice(&sum.to_be_bytes());
    }
    let adjustment = 0xb1b0afbau32.wrapping_sub(checksum(bytes));
    bytes[head + 8..head + 12].copy_from_slice(&adjustment.to_be_bytes());
    assert_eq!(checksum(bytes), 0xb1b0afba);
}
fn selected(name: &str, expected: bool) {
    let wide: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: 字体和 DC 只在本测试作用域使用，先还原选中对象再配对释放。
    unsafe {
        let dc = CreateCompatibleDC(ptr::null_mut());
        assert!(!dc.is_null());
        let font = CreateFontW(
            -24,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            0,
            0,
            wide.as_ptr(),
        );
        assert!(!font.is_null());
        let old = SelectObject(dc, font);
        let mut face = [0; 128];
        let n = GetTextFaceW(dc, 128, face.as_mut_ptr());
        assert!(n > 0);
        assert_eq!(
            String::from_utf16_lossy(&face[..n as usize - 1]) == name,
            expected
        );
        if expected {
            let mut roots: Vec<_> = "亻\u{e4e4}小文辶日\u{e46a}页缶\u{e432}人"
                .encode_utf16()
                .collect();
            // 真实字体再核对码表中的全部 BMP 私用区字根，而非仅凭三个样例声称覆盖。
            if name == crate::FAMILY {
                // 核对 GDI 实际选中的原始字体数据，不能只凭同名字体声称加载正确。
                let length = GetFontData(dc, 0, 0, ptr::null_mut(), 0);
                assert_eq!(u64::from(length), crate::locked().unwrap().0);
                let mut selected = vec![0u8; length as usize];
                assert_eq!(
                    GetFontData(dc, 0, 0, selected.as_mut_ptr().cast(), length),
                    length
                );
                assert_eq!(
                    format!("{:x}", Sha256::digest(&selected)),
                    crate::locked().unwrap().1
                );
                let table = std::fs::read_to_string(
                    source()
                        .parent()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .join("tiger/opencc/hu_cf.txt"),
                )
                .unwrap();
                let private: std::collections::BTreeSet<_> = table
                    .chars()
                    .filter(|c| ('\u{e000}'..='\u{f8ff}').contains(c))
                    .map(|c| c as u16)
                    .collect();
                assert!(!private.is_empty());
                println!("锁定码表 BMP 私用区字根：{}", private.len());
                roots.extend(private);
            }
            let mut glyphs = vec![0; roots.len()];
            assert_ne!(
                GetGlyphIndicesW(
                    dc,
                    roots.as_ptr(),
                    roots.len() as i32,
                    glyphs.as_mut_ptr(),
                    GGI_MARK_NONEXISTING_GLYPHS
                ),
                u32::MAX
            );
            assert!(glyphs.iter().all(|g| *g != 0 && *g != 0xffff));
        }
        SelectObject(dc, old);
        DeleteObject(font);
        DeleteDC(dc);
    }
}
#[test]
fn private_load_works_without_an_installed_family_and_rejects_bad_sources() -> Result<()> {
    let root = std::env::temp_dir().canonicalize()?.join(format!(
        "tigertype-private-font-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir(&root)?;
    let path = root.join("synthetic.ttf");
    assert!(
        PrivateFont::load(&path).is_err(),
        "缺少随包文件时不得采用同名系统字体"
    );
    let mut bytes = std::fs::read(source())?;
    let family = format!("TigerType{:012}", std::process::id());
    rename_test_font(&mut bytes, &family);
    std::fs::write(&path, &bytes)?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    selected(&family, false);
    assert!(
        PrivateFont::load(&path).is_err(),
        "合成字体不得通过正式清单"
    );
    assert!(PrivateFont::verified(&path, bytes.len() as u64 + 1, &hash).is_err());
    assert!(PrivateFont::verified(&path, bytes.len() as u64, &"0".repeat(64)).is_err());
    let font = PrivateFont::verified(&path, bytes.len() as u64, &hash)?;
    selected(&family, true);
    // 固定目录对象不能阻止安装器替换其他 DLL/EXE；字体自身和目录改名则必须拒绝。
    let sibling = root.join("other.dll");
    std::fs::write(&sibling, b"synthetic")?;
    let retired = root.join("other.old");
    std::fs::rename(&sibling, &retired)?;
    std::fs::remove_file(retired)?;
    assert!(OpenOptions::new().write(true).open(&path).is_err());
    assert!(std::fs::rename(&root, root.with_extension("moved")).is_err());
    drop(font);
    selected(&family, false);
    let link = root.join("link.ttf");
    std::fs::hard_link(&path, &link)?;
    assert!(PrivateFont::verified(&link, bytes.len() as u64, &hash).is_err());
    std::fs::remove_file(link)?;
    std::fs::remove_file(path)?;
    std::fs::remove_dir(root)?;
    // 真实随包字体也经同一严格校验路径加载；不依赖系统安装的是哪个版本。
    let started = std::time::Instant::now();
    let font = PrivateFont::load(&source())?;
    println!("锁定字体校验/私有加载：{:?}", started.elapsed());
    selected(crate::FAMILY, true);
    drop(font);
    Ok(())
}
