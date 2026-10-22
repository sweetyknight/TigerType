//! 固定版本文本导入：不解释 YAML、Lua 或 OpenCC 配置，只读取已锁定的数据段。
use crate::{
    Result, data,
    resources::{self, Entry, Group},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};
use tigertype_engine::{Data, Detail, Dictionary, Mapping, Pronunciation};

const VERSION: u32 = 1;
const LIMIT: u64 = 128 * 1024 * 1024;
const SOURCES: &[&str] = &[
    "tiger.dict.yaml",
    "core2022.dict.yaml",
    "PY_c.dict.yaml",
    "opencc/hu_cf.txt",
    "opencc/PYCharacters.txt",
    "opencc/STCharacters_Tu.txt",
    "opencc/TSCharacters.txt",
    "weasel.custom.yaml",
    "tiger.custom.yaml",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub name: String,
    /// 存储统一 RGBA，不能把小狼毫 ABGR 数字直接交给绘图接口。
    pub colors: BTreeMap<String, [u8; 4]>,
    pub font_face: String,
    pub comment_font_face: String,
    pub label_font_face: String,
    pub font_point: u16,
    pub align_type: String,
    pub layout: BTreeMap<String, u16>,
    pub page_size: u8,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub format_version: u32,
    pub dictionary_version: String,
    pub sources: Vec<Entry>,
    pub data: Data,
    pub theme: Theme,
    pub payload_sha256: String,
}

fn source_entries() -> Result<Vec<Entry>> {
    let lock: Group = serde_json::from_str(include_str!("../../../resources/tiger.lock.json"))?;
    SOURCES
        .iter()
        .map(|name| {
            let matches: Vec<_> = lock
                .files
                .iter()
                .filter(|entry| &entry.path == name)
                .collect();
            if matches.len() != 1 {
                return Err(format!("资源锁缺失或重复: {name}").into());
            }
            Ok(matches[0].clone())
        })
        .collect()
}
fn digest(data: &Data, theme: &Theme) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(data, theme))?)
    ))
}
fn verified(root: &Path, entry: &Entry) -> Result<String> {
    let path = root.join(resources::relative(&entry.path)?);
    let bytes = data::read_file(&path, entry.size.min(LIMIT))?;
    // 对实际解析的同一份字节做校验，避免校验后重新打开导致竞态。
    if bytes.len() as u64 != entry.size || format!("{:x}", Sha256::digest(&bytes)) != entry.sha256 {
        return Err(format!("资源校验失败: {}", entry.path).into());
    }
    Ok(String::from_utf8(bytes)?.replace("\r\n", "\n"))
}
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
}
fn dictionary_body(text: &str) -> Result<&str> {
    text.split_once("\n...\n")
        .map(|(_, body)| body)
        .ok_or_else(|| "缺少锁定词表正文分隔符".into())
}
fn fields(line: &str, count: usize) -> Result<Vec<&str>> {
    let fields: Vec<_> = line.split('\t').collect();
    if fields.len() != count {
        return Err(format!("数据列数应为 {count}，实际 {}", fields.len()).into());
    }
    Ok(fields)
}
fn character(text: &str) -> Result<char> {
    let mut chars = text.chars();
    let value = chars.next().ok_or("字符为空")?;
    if chars.next().is_some() || value.is_control() {
        return Err("需要单个 Unicode 字符".into());
    }
    Ok(value)
}
fn mappings(text: &str) -> Result<Vec<Mapping>> {
    lines(text)
        .map(|line| {
            let row = fields(line, 2)?;
            Ok(Mapping {
                character: character(row[0])?,
                variants: row[1]
                    .split_whitespace()
                    .map(character)
                    .collect::<Result<_>>()?,
            })
        })
        .collect()
}
fn parse_data(files: &BTreeMap<String, String>) -> Result<Data> {
    let mut data = Data::default();
    for line in lines(dictionary_body(&files["tiger.dict.yaml"])?) {
        let row = fields(line, 3)?;
        data.entries.push(tigertype_engine::Entry {
            character: character(row[0])?,
            code: row[1].into(),
            weight: row[2].parse()?,
        });
    }
    for line in lines(dictionary_body(&files["core2022.dict.yaml"])?) {
        let row = fields(line, 2)?;
        if row[1] != "t" {
            return Err("常用字集标记不符".into());
        }
        data.common.push(character(row[0])?);
    }
    for line in lines(dictionary_body(&files["PY_c.dict.yaml"])?) {
        let row = fields(line, 3)?;
        // 原表包含词组，只导入单字；没有把被排除词组当成拼音解析错误忽略。
        if row[0].chars().count() != 1 {
            continue;
        }
        data.pinyin.push(Pronunciation {
            character: character(row[0])?,
            pinyin: row[1].into(),
            weight: row[2].parse()?,
        });
    }
    let mut details = BTreeMap::<char, Detail>::new();
    for line in lines(&files["opencc/hu_cf.txt"]) {
        let row = fields(line, 2)?;
        let ch = character(row[0])?;
        let value = row[1].replace("&nbsp;", " ");
        let inner = value
            .strip_prefix('〔')
            .and_then(|s| s.strip_suffix('〕'))
            .ok_or("拆分括号格式不符")?;
        let (parts, codes) = inner.split_once('·').ok_or("拆分缺少编码分隔符")?;
        let detail = Detail {
            character: ch,
            full_codes: codes.split_whitespace().map(str::to_owned).collect(),
            decomposition: Some(parts.trim().into()),
            readings: Vec::new(),
        };
        if detail.full_codes.is_empty() || details.insert(ch, detail).is_some() {
            return Err("拆分缺码或重复".into());
        }
    }
    for line in lines(&files["opencc/PYCharacters.txt"]) {
        let row = fields(line, 2)?;
        let ch = character(row[0])?;
        let value = row[1].replace("&nbsp;", " ");
        let readings = value
            .strip_prefix('（')
            .and_then(|s| s.strip_suffix('）'))
            .ok_or("读音括号格式不符")?;
        let detail = details.entry(ch).or_insert(Detail {
            character: ch,
            full_codes: Vec::new(),
            decomposition: None,
            readings: Vec::new(),
        });
        if !detail.readings.is_empty() {
            return Err("读音资料重复".into());
        }
        detail.readings = readings.split_whitespace().map(str::to_owned).collect();
    }
    data.details = details.into_values().collect();
    data.simplified = mappings(&files["opencc/TSCharacters.txt"])?;
    data.traditional = mappings(&files["opencc/STCharacters_Tu.txt"])?;
    Ok(data)
}

fn rgba(text: &str) -> Result<[u8; 4]> {
    let hex = text.strip_prefix("0x").ok_or("颜色须为锁定十六进制格式")?;
    if hex.len() > 8 || hex.is_empty() {
        return Err("颜色长度非法".into());
    }
    let value = u32::from_str_radix(hex, 16)?;
    Ok([
        value as u8,
        (value >> 8) as u8,
        (value >> 16) as u8,
        if hex.len() > 6 {
            (value >> 24) as u8
        } else {
            255
        },
    ])
}
fn scalar<'a>(text: &'a str, key: &str) -> Result<&'a str> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix(&format!("{key}:")))
        .map(|s| s.trim().trim_matches('"'))
        .ok_or_else(|| format!("缺少外观字段: {key}").into())
}
fn parse_theme(weasel: &str, tiger: &str) -> Result<Theme> {
    if !weasel.contains("\"style/color_scheme\": win11light") {
        return Err("只接受批准的 Win11Light".into());
    }
    let colors = weasel
        .split_once("  \"preset_color_schemes/win11light\":\n")
        .ok_or("缺少 Win11Light")?
        .1
        .split_once("  style:\n")
        .ok_or("缺少 style")?
        .0;
    let mut palette = BTreeMap::new();
    for line in lines(colors) {
        let (key, value) = line.split_once(':').ok_or("颜色行格式不符")?;
        if key.ends_with("_color") {
            palette.insert(key.into(), rgba(value.trim())?);
        }
    }
    if !palette.contains_key("back_color") || !palette.contains_key("candidate_text_color") {
        return Err("主题缺少必需颜色".into());
    }
    let style = weasel.split_once("  style:\n").unwrap().1;
    let layout = scalar(style, "layout")?.trim_matches(['{', '}']);
    let mut dimensions = BTreeMap::new();
    let mut align_type = None;
    for pair in layout.split(',') {
        let (key, value) = pair.trim().split_once(':').ok_or("布局格式不符")?;
        // 锁定包的 enhanced_position 为非数值占位，不解释为启用的功能。
        if key == "enhanced_position" {
            continue;
        }
        // 对齐方式不是像素数；保留语义字段，不能送入整数解析器。
        if key == "align_type" {
            if value.trim() != "center" {
                return Err("未批准的候选对齐方式".into());
            }
            align_type = Some(value.trim().to_owned());
            continue;
        }
        dimensions.insert(key.into(), value.trim().parse()?);
    }
    let page = scalar(tiger, "menu/page_size")?
        .split_whitespace()
        .next()
        .ok_or("缺少候选数")?
        .parse()?;
    let theme = Theme {
        name: "win11light".into(),
        colors: palette,
        font_face: scalar(style, "font_face")?.into(),
        comment_font_face: scalar(style, "comment_font_face")?.into(),
        label_font_face: scalar(style, "label_font_face")?.into(),
        font_point: scalar(style, "font_point")?.parse()?,
        align_type: align_type.ok_or("缺少候选对齐方式")?,
        layout: dimensions,
        page_size: page,
    };
    if theme.page_size != 9 || theme.font_point != 15 {
        return Err("外观参数偏离批准默认值".into());
    }
    Ok(theme)
}

pub fn build(source: &Path) -> Result<Package> {
    let sources = source_entries()?;
    let mut files = BTreeMap::new();
    for entry in &sources {
        files.insert(entry.path.clone(), verified(source, entry)?);
    }
    let data = parse_data(&files).map_err(|e| format!("数据表解析失败: {e}"))?;
    let theme = parse_theme(&files["weasel.custom.yaml"], &files["tiger.custom.yaml"])
        .map_err(|e| format!("外观解析失败: {e}"))?;
    let payload_sha256 = digest(&data, &theme)?;
    Dictionary::new(data.clone())?;
    Ok(Package {
        format_version: VERSION,
        dictionary_version: "2026.08.14".into(),
        sources,
        data,
        theme,
        payload_sha256,
    })
}

pub fn save(package: &Package, target: &Path) -> Result<()> {
    validate(package)?;
    let bytes = serde_json::to_vec(package)?;
    if bytes.len() as u64 > LIMIT {
        return Err("数据包超限".into());
    }
    // 只新建文件，不覆盖已有包；失败文件保留诊断，加载时仍会严格校验。
    data::write_new(target, &bytes)
}
pub fn load(path: &Path) -> Result<Package> {
    let package: Package = serde_json::from_slice(&data::read_file(path, LIMIT)?)?;
    validate(&package)?;
    Ok(package)
}
fn validate(package: &Package) -> Result<()> {
    if package.format_version != VERSION
        || package.dictionary_version != "2026.08.14"
        || package.sources != source_entries()?
        || digest(&package.data, &package.theme)? != package.payload_sha256
        || package.theme.name != "win11light"
        || package.theme.page_size != 9
        || package.theme.font_point != 15
    {
        return Err("数据包版本、来源、外观或内容摘要不符".into());
    }
    Dictionary::new(package.data.clone())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn theme_keeps_text_alignment_separate_from_pixel_dimensions() {
        let source = "  \"preset_color_schemes/win11light\":\n    back_color: 0xfff9f9f9\n    candidate_text_color: 0x000000\n  style:\n    font_face: \"Consolas\"\n    comment_font_face: \"TumanPUA\"\n    label_font_face: \"SimSun\"\n    font_point: 15\n    layout: {align_type: center, margin_x: 5, enhanced_position: bool}\n  \"style/color_scheme\": win11light\n";
        let theme = parse_theme(source, "menu/page_size: 9 # 九候选").unwrap();
        assert_eq!(theme.align_type, "center");
        assert_eq!(theme.layout["margin_x"], 5);
        assert!(!theme.layout.contains_key("align_type"));
        assert!(parse_theme(&source.replace("center", "unknown"), "menu/page_size: 9").is_err());
    }
    #[test]
    fn colors_preserve_abgr_order_and_explicit_alpha() {
        assert_eq!(rgba("0x0000ff").unwrap(), [255, 0, 0, 255]);
        assert_eq!(rgba("0x800000ff").unwrap(), [255, 0, 0, 128]);
        assert_eq!(rgba("0x00303030").unwrap(), [48, 48, 48, 0]);
        assert!(rgba("0x123456789").is_err());
    }
    #[test]
    fn parsers_reject_ambiguous_or_malformed_records() {
        assert!(character("你好").is_err());
        assert_eq!(character("𠀀").unwrap(), '𠀀');
        assert!(fields("a\tb\tc", 2).is_err());
        assert!(dictionary_body("没有正文").is_err());
        assert!(mappings("发\t發髮").is_err());
        assert_eq!(mappings("发\t發 髮").unwrap()[0].variants, vec!['發', '髮']);
    }
    #[test]
    fn verified_bytes_reject_corruption_and_keep_original_file() -> Result<()> {
        let root = resources::new_directory(&std::env::temp_dir(), "tigertype-import")?;
        let path = root.join("source.txt");
        data::write_new(&path, b"original")?;
        let entry = Entry {
            path: "source.txt".into(),
            size: 8,
            sha256: resources::sha256(&path)?,
        };
        assert_eq!(verified(&root, &entry)?, "original");
        std::fs::write(&path, b"modified")?;
        assert!(verified(&root, &entry).is_err());
        assert!(data::write_new(&path, b"replacement").is_err());
        assert_eq!(std::fs::read(path)?, b"modified");
        Ok(())
    }
}
