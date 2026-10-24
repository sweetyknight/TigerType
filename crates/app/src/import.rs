//! 固定版本文本导入：不解释 YAML、Lua 或 OpenCC 配置，只读取已锁定的数据段。
use crate::{
    Result, data,
    resources::{self, Entry, Group},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};
use tigertype_engine::{Data, Detail, Dictionary, Mapping, Pronunciation};

// 2：去掉无人消费的旧 Win11Light 外观段，旧包不兼容。
const VERSION: u32 = 2;
const LIMIT: u64 = 128 * 1024 * 1024;
const SOURCES: &[&str] = &[
    "tiger.dict.yaml",
    "core2022.dict.yaml",
    "PY_c.dict.yaml",
    "opencc/hu_cf.txt",
    "opencc/PYCharacters.txt",
    "opencc/STCharacters_Tu.txt",
    "opencc/TSCharacters.txt",
];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub format_version: u32,
    pub dictionary_version: String,
    pub sources: Vec<Entry>,
    pub data: Data,
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
fn digest(data: &Data) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(data)?)))
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

pub fn build(source: &Path) -> Result<Package> {
    let sources = source_entries()?;
    let mut files = BTreeMap::new();
    for entry in &sources {
        files.insert(entry.path.clone(), verified(source, entry)?);
    }
    let data = parse_data(&files).map_err(|e| format!("数据表解析失败: {e}"))?;
    let payload_sha256 = digest(&data)?;
    Dictionary::new(data.clone())?;
    Ok(Package {
        format_version: VERSION,
        dictionary_version: "2026.08.14".into(),
        sources,
        data,
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
        || digest(&package.data)? != package.payload_sha256
    {
        return Err("数据包版本、来源或内容摘要不符".into());
    }
    Dictionary::new(package.data.clone())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
