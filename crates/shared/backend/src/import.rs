//! 锁定码表加载：直接读取原始文本（不另生成数据包），逐个核对大小/SHA256 后只解析所需数据段；
//! 不解释 YAML、Lua 或 OpenCC 配置。
use crate::{
    Result,
    resources::{self, Entry, Group},
    storage,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};
use tigertype_engine::{Data, Detail, Mapping, Phrase, Pronunciation};

const LIMIT: u64 = 128 * 1024 * 1024;
/// 码表目录内须有的文件（相对路径），即 resources/tiger 与安装目录 tiger 的全部内容。
pub const SOURCES: &[&str] = &[
    "tiger.dict.yaml",
    "tigress_ci.dict.yaml",
    "tigress_simp_ci.dict.yaml",
    "core2022.dict.yaml",
    "PY_c.dict.yaml",
    "opencc/hu_cf.txt",
    "opencc/PYCharacters.txt",
    "opencc/STCharacters_Tu.txt",
    "opencc/TSCharacters.txt",
];

fn source_entries() -> Result<Vec<Entry>> {
    let lock: Group = serde_json::from_str(include_str!("../../../../resources/tiger.lock.json"))?;
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
fn verified(root: &Path, entry: &Entry) -> Result<String> {
    let path = root.join(resources::relative(&entry.path)?);
    // 缺文件时带上路径，否则开机自启弹窗只有“找不到指定的文件”。
    let bytes = storage::read_file(&path, entry.size.min(LIMIT))
        .map_err(|e| format!("{}: {e}", path.display()))?;
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
    // 两张 tigress 词表已包含按虎码编码器生成的词码，导入最终四码而不执行 Lua。
    for name in ["tigress_ci.dict.yaml", "tigress_simp_ci.dict.yaml"] {
        for line in lines(dictionary_body(&files[name])?) {
            // 锁定词表正文实际省略 stem 列，只保留文本、权重和最终词码。
            let row = fields(line, 3)?;
            data.phrases.push(Phrase {
                text: row[0].to_owned(),
                weight: row[1].parse()?,
                code: row[2].to_owned(),
            });
        }
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

/// 后台、查字与打包共用：码表目录（开发为 resources/tiger，安装版为程序目录下的 tiger）须与锁定清单逐字节一致；
/// 词表的结构约束由调用方 Dictionary::new 检查。
pub fn load(tables: &Path) -> Result<Data> {
    let mut files = BTreeMap::new();
    for entry in source_entries()? {
        let text = verified(tables, &entry)?;
        files.insert(entry.path, text);
    }
    parse_data(&files).map_err(|e| format!("码表解析失败: {e}").into())
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
        // 系统临时目录在 Mac 可能经 /var 链接；测试先确定系统给定根，不放宽生产路径拒绝规则。
        let root =
            resources::new_directory(&std::env::temp_dir().canonicalize()?, "tigertype-import")?;
        let path = root.join("source.txt");
        storage::write_new(&path, b"original")?;
        let entry = Entry {
            path: "source.txt".into(),
            size: 8,
            sha256: resources::sha256(&path)?,
        };
        assert_eq!(verified(&root, &entry)?, "original");
        std::fs::write(&path, b"modified")?;
        assert!(verified(&root, &entry).is_err());
        assert!(storage::write_new(&path, b"replacement").is_err());
        assert_eq!(std::fs::read(path)?, b"modified");
        Ok(())
    }
}
