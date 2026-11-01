//! 只读解析字体文件的字族名与字符覆盖，供安装判重：sudo 下 CoreText 看不到调用者的用户字体，
//! 也不能为判重注册任意文件。纯 Rust，在 Windows 上用仓库字体验证。
use std::io::{Read, Seek, SeekFrom};

/// 拆分依赖的文楷与代表字根；字根是私用区字形，只有虎码融合字根版才有。
pub const LEARNING_FAMILY: &str = "LXGW WenKai GB Screen";
pub const LEARNING_ROOTS: [u32; 3] = [0xe4e4, 0xe46a, 0xe432];
/// 完整 cmap 也只有几 MB；上限防止畸形文件让安装读入过量数据。
const MAX_TABLE: u32 = 16 << 20;
const MAX_FACES: u32 = 64;

fn read(file: &mut (impl Read + Seek), offset: u64, length: usize) -> Option<Vec<u8>> {
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut bytes = vec![0; length];
    file.read_exact(&mut bytes).ok()?;
    Some(bytes)
}
fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}
fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}
/// 每个字面的表目录偏移；TTC 含多个字面，不是字体时为空。
fn faces(file: &mut (impl Read + Seek)) -> Vec<u64> {
    let Some(head) = read(file, 0, 12) else {
        return Vec::new();
    };
    match &head[..4] {
        [0, 1, 0, 0] | b"OTTO" | b"true" => vec![0],
        b"ttcf" => {
            let count = u32_at(&head, 8).unwrap_or(0).min(MAX_FACES) as usize;
            read(file, 12, count * 4).map_or(Vec::new(), |offsets| {
                let (offsets, _) = offsets.as_chunks::<4>();
                offsets
                    .iter()
                    .map(|offset| u32::from_be_bytes(*offset).into())
                    .collect()
            })
        }
        _ => Vec::new(),
    }
}
fn table(file: &mut (impl Read + Seek), face: u64, tag: &[u8; 4]) -> Option<Vec<u8>> {
    let count = u16_at(&read(file, face + 4, 2)?, 0)? as usize;
    let records = read(file, face + 12, count * 16)?;
    let (records, _) = records.as_chunks::<16>();
    let record = records.iter().find(|r| &r[..4] == tag)?;
    let (offset, length) = (u32_at(record, 8)?, u32_at(record, 12)?);
    (length <= MAX_TABLE).then_some(())?;
    read(file, offset.into(), length as usize)
}
fn family_names(table: &[u8]) -> Option<Vec<String>> {
    let count = u16_at(table, 2)? as usize;
    let strings = u16_at(table, 4)? as usize;
    let mut names = Vec::new();
    for index in 0..count {
        let record = 6 + index * 12;
        let platform = u16_at(table, record)?;
        let id = u16_at(table, record + 6)?;
        let length = u16_at(table, record + 8)? as usize;
        let offset = u16_at(table, record + 10)? as usize + strings;
        // 1 为字族名，16 为排版字族名；同一字族的粗细变体在 16 里才同名。
        if id != 1 && id != 16 {
            continue;
        }
        let Some(bytes) = table.get(offset..offset + length) else {
            continue;
        };
        let name = match platform {
            0 | 3 => {
                let (units, _) = bytes.as_chunks::<2>();
                let units: Vec<_> = units.iter().map(|c| u16::from_be_bytes(*c)).collect();
                String::from_utf16_lossy(&units)
            }
            1 => String::from_utf8_lossy(bytes).into_owned(),
            _ => continue,
        };
        names.push(name.trim().to_owned());
    }
    Some(names)
}
/// 文件内全部字面的字族名（含本地化名）；元数据可读不代表字体可加载，安装跳过前须另验。
pub fn families(file: &mut (impl Read + Seek)) -> Vec<String> {
    let mut names = Vec::new();
    for face in faces(file) {
        if let Some(table) = table(file, face, b"name") {
            names.extend(family_names(&table).unwrap_or_default());
        }
    }
    names.retain(|name| !name.is_empty());
    names.sort();
    names.dedup();
    names
}
fn format4(subtable: &[u8], character: u32) -> Option<u32> {
    let character = u16::try_from(character).ok()?;
    let segments = u16_at(subtable, 6)? as usize / 2;
    let (ends, starts) = (14, 16 + segments * 2);
    let (deltas, ranges) = (16 + segments * 4, 16 + segments * 6);
    for index in 0..segments {
        if u16_at(subtable, ends + index * 2)? < character {
            continue;
        }
        let start = u16_at(subtable, starts + index * 2)?;
        if start > character {
            return None;
        }
        let delta = u16_at(subtable, deltas + index * 2)?;
        let range = ranges + index * 2;
        let offset = u16_at(subtable, range)? as usize;
        if offset == 0 {
            return Some(character.wrapping_add(delta).into());
        }
        // idRangeOffset 相对自身位置；数组里的 0 表示缺字，不再加 delta。
        let glyph = u16_at(subtable, range + offset + (character - start) as usize * 2)?;
        return Some(if glyph == 0 {
            0
        } else {
            glyph.wrapping_add(delta).into()
        });
    }
    None
}
fn format12(subtable: &[u8], character: u32) -> Option<u32> {
    let groups = u32_at(subtable, 12)? as usize;
    for index in 0..groups {
        let group = 16 + index * 12;
        let (start, end) = (u32_at(subtable, group)?, u32_at(subtable, group + 4)?);
        if start <= character && character <= end {
            return Some(u32_at(subtable, group + 8)?.wrapping_add(character - start));
        }
    }
    None
}
fn mapped(cmap: &[u8], character: u32) -> bool {
    let count = u16_at(cmap, 2).unwrap_or(0) as usize;
    (0..count).any(|index| {
        u32_at(cmap, 8 + index * 8)
            .and_then(|offset| cmap.get(offset as usize..))
            .and_then(|subtable| match u16_at(subtable, 0)? {
                4 => format4(subtable, character),
                12 => format12(subtable, character),
                _ => None,
            })
            .is_some_and(|glyph| glyph != 0)
    })
}
/// 某个字面覆盖全部字符；只支持统一码常用的 format 4/12，其余格式当作未覆盖。
pub fn covers(file: &mut (impl Read + Seek), characters: &[u32]) -> bool {
    faces(file).into_iter().any(|face| {
        table(file, face, b"cmap").is_some_and(|cmap| characters.iter().all(|c| mapped(&cmap, *c)))
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{fs::File, io::Cursor, path::PathBuf};

    fn repository_font(name: &str) -> File {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../resources/fonts");
        File::open(root.join(name)).unwrap()
    }
    /// 只含一张表的最小字面；base 为它在文件内的位置，TTC 的表偏移从文件头算起。
    pub(crate) fn face(base: u32, tag: &[u8; 4], table: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        bytes.extend_from_slice(tag);
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&(base + 28).to_be_bytes());
        bytes.extend_from_slice(&(table.len() as u32).to_be_bytes());
        bytes.extend_from_slice(table);
        bytes
    }
    pub(crate) fn name_table(family: &str) -> Vec<u8> {
        let text: Vec<u8> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let mut table = vec![0, 0, 0, 1, 0, 18];
        for value in [3u16, 1, 0x409, 1, text.len() as u16, 0] {
            table.extend_from_slice(&value.to_be_bytes());
        }
        table.extend_from_slice(&text);
        table
    }
    /// A 走 idRangeOffset 取到字形，B 的数组项为 0 表示缺字，C 走 delta，末段为规范要求的结束段。
    fn format4_cmap() -> Vec<u8> {
        let words: [u16; 20] = [
            4, 0, 0, 6, 0, 0, 0, 0x42, 0x43, 0xffff, 0, 0x41, 0x43, 0xffff, 0, 0x10, 1, 6, 0, 0,
        ];
        let mut subtable: Vec<u8> = words.iter().flat_map(|w| w.to_be_bytes()).collect();
        // ranges[0] 位于字节 34，偏移 6 后正好指向这两项 glyphIdArray。
        subtable.extend_from_slice(&[0, 7, 0, 0]);
        let mut cmap = vec![0, 0, 0, 1, 0, 3, 0, 1, 0, 0, 0, 12];
        cmap.extend(subtable);
        cmap
    }

    #[test]
    fn reads_family_and_coverage_from_the_locked_fonts() {
        let mut wenkai = repository_font("LXGWWenKaiGBScreen.ttf");
        assert!(families(&mut wenkai).contains(&LEARNING_FAMILY.to_owned()));
        assert!(covers(&mut wenkai, &LEARNING_ROOTS));
        // 扩展区只在 format 12 里，现场缺字样例也须查得到。
        assert!(covers(
            &mut wenkai,
            &['𤛭' as u32, '𨰻' as u32, '𪚥' as u32]
        ));
        assert!(!covers(&mut wenkai, &[LEARNING_ROOTS[0], 0x10ffff]));
        let mut mono = repository_font("CascadiaMono.ttf");
        assert!(families(&mut mono).contains(&"Cascadia Mono".to_owned()));
        // 反例：普通字体没有虎码字根，安装不能把它当成合格的文楷。
        assert!(!covers(&mut mono, &LEARNING_ROOTS));
        for name in [
            "TumanPUA.ttf",
            "TH-Tshyn-P0.ttf",
            "TH-Tshyn-P1.ttf",
            "TH-Tshyn-P2.ttf",
        ] {
            assert!(!families(&mut repository_font(name)).is_empty(), "{name}");
        }
    }
    #[test]
    fn collections_expose_every_face() {
        let (first, second) = (name_table("Alpha"), name_table("Beta"));
        let mut file = b"ttcf\0\x02\0\0\0\0\0\x02".to_vec();
        let start = 20;
        let next = start + 28 + first.len() as u32;
        file.extend_from_slice(&start.to_be_bytes());
        file.extend_from_slice(&next.to_be_bytes());
        file.extend(face(start, b"name", &first));
        file.extend(face(next, b"name", &second));
        assert_eq!(families(&mut Cursor::new(file)), ["Alpha", "Beta"]);
    }
    #[test]
    fn format4_follows_indirect_ranges_and_treats_zero_as_missing() {
        let file = face(0, b"cmap", &format4_cmap());
        let has = |c: u32| covers(&mut Cursor::new(&file), &[c]);
        assert!(has(0x41), "idRangeOffset");
        assert!(!has(0x42), "数组项 0 是缺字");
        assert!(has(0x43), "idDelta");
        assert!(!has(0x44), "落在段外");
        assert!(!has(0x10000), "format 4 只有基本平面");
    }
    #[test]
    fn broken_files_are_not_fonts_and_never_panic() {
        let real = {
            let mut bytes = Vec::new();
            repository_font("CascadiaMono.ttf")
                .read_to_end(&mut bytes)
                .unwrap();
            bytes
        };
        for length in [0, 3, 11, 12, 13, 40, 200, 4096] {
            let mut file = Cursor::new(&real[..length]);
            assert!(families(&mut file).is_empty(), "截断到 {length}");
            assert!(!covers(&mut file, &[0x41]), "截断到 {length}");
        }
        assert!(families(&mut Cursor::new(b"not a font at all")).is_empty());
        // 字面数量撒谎、表越界、表长度超限都只算不是字体，不读过量数据。
        let lying = b"ttcf\0\x02\0\0\xff\xff\xff\xff\0\0\0\x10".to_vec();
        assert!(families(&mut Cursor::new(lying)).is_empty());
        let mut outside = face(0, b"name", &name_table("Gone"));
        outside.truncate(30);
        assert!(families(&mut Cursor::new(outside)).is_empty());
        let mut huge = face(0, b"name", &name_table("Huge"));
        huge[24..28].copy_from_slice(&(MAX_TABLE + 1).to_be_bytes());
        assert!(families(&mut Cursor::new(huge)).is_empty());
        // 名称记录指到表外时跳过该记录。
        let mut record = name_table("Name");
        record[16..18].copy_from_slice(&0xffffu16.to_be_bytes());
        assert!(families(&mut Cursor::new(face(0, b"name", &record))).is_empty());
    }
}
