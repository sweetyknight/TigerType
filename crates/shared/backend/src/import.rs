//! 锁定码表加载：直接读取原始文本（不另生成数据包），逐个核对大小/SHA256 后只解析所需数据段；
//! 不解释 YAML、Lua 或 OpenCC 配置。
use crate::{
    Result,
    resources::{self, Entry, Group},
    storage,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};
use tigertype_engine::{
    Data, Detail, Mapping, Phrase, Pronunciation, Root, WordPinyin, WordReading,
};

const LIMIT: u64 = 128 * 1024 * 1024;
/// 码表目录内须有的文件（相对路径），即 resources/tiger 与安装目录 tiger 的全部内容。
pub const SOURCES: &[&str] = &[
    "tiger.dict.yaml",
    "tigress.dict.yaml",
    "tigress_ci.dict.yaml",
    "tigress_simp_ci.dict.yaml",
    "core2022.dict.yaml",
    "PY_c.dict.yaml",
    "opencc/hu_cf.txt",
    "opencc/PYCharacters.txt",
    "opencc/PYPhrases.txt",
    "opencc/STCharacters_Tu.txt",
    "opencc/TSCharacters.txt",
    "opencc/STPhrases.txt",
    "opencc/TSPhrases.txt",
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
    Ok(String::from_utf8(bytes)?)
}
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
}
fn dictionary_body(text: &str) -> Result<&str> {
    // 锁定文件 LF 与 CRLF 都有；原地定位，不为换行复制整个文件。
    ["\n...\n", "\n...\r\n"]
        .iter()
        .find_map(|mark| text.split_once(mark))
        .map(|(_, body)| body)
        .ok_or_else(|| "缺少锁定词表正文分隔符".into())
}
/// 切成至多 N 列、至少 least 列，缺的末列留空；每行都走这里，不分配。
fn columns<const N: usize>(line: &str, least: usize) -> Result<[&str; N]> {
    let mut row = [""; N];
    let mut count = 0;
    for field in line.split('\t') {
        if let Some(slot) = row.get_mut(count) {
            *slot = field;
        }
        count += 1;
    }
    if !(least..=N).contains(&count) {
        return Err(format!("数据列数应为 {N}，实际 {count}").into());
    }
    Ok(row)
}
fn fields<const N: usize>(line: &str) -> Result<[&str; N]> {
    columns(line, N)
}
fn character(text: &str) -> Result<char> {
    let mut chars = text.chars();
    let value = chars.next().ok_or("字符为空")?;
    if chars.next().is_some() || value.is_control() {
        return Err("需要单个 Unicode 字符".into());
    }
    Ok(value)
}
fn mappings(text: &str) -> Result<Vec<Mapping<'_>>> {
    lines(text)
        .map(|line| {
            let row = fields::<2>(line)?;
            let variants: Box<[_]> = row[1].split_whitespace().collect();
            // 单字只能映射到单字；短语表的键至少两字，值按原文保留。
            if row[0].chars().nth(1).is_none() {
                variants.iter().try_for_each(|v| character(v).map(|_| ()))?;
            }
            Ok(Mapping {
                text: row[0],
                variants,
            })
        })
        .collect()
}
/// 字根大小码（官网字根表加拆分查询补充，来源见文件头），约 400 行，编进程序，不随码表目录安装。
const ROOTS: &str = include_str!("../../../../resources/roots.tsv");
/// 每行“编码<TAB>主根与变体”，一行拆成每个字根一条；字根是否重复由引擎按码位矛盾作废处理。
fn roots(text: &str) -> Result<Vec<Root<'_>>> {
    let mut roots = Vec::new();
    for line in lines(text) {
        let [code, characters] = fields::<2>(line)?;
        if characters.is_empty() {
            return Err("字根表行缺字根".into());
        }
        roots.extend(characters.chars().map(|character| Root { character, code }));
    }
    Ok(roots)
}
/// 资料以 &nbsp; 作空格，按它和普通空白切开，直接借用原文。
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split("&nbsp;").flat_map(str::split_whitespace)
}
/// 只在通过 hash 校验的内存副本里删去词音节间的空格，供解析记录借用，不逐词分配 String。
fn join_word_pinyin(text: &mut String) {
    let mut field = 0;
    let mut characters = 0;
    text.retain(|c| {
        match c {
            '\n' => {
                field = 0;
                characters = 0;
            }
            '\t' => field += 1,
            _ if field == 0 => characters += 1,
            _ => {}
        }
        // 单字读音保持原样，非法空格仍由引擎拒绝；文本列和权重列也不改写。
        !(field == 1 && characters > 1 && c == ' ')
    });
}
fn parse_data(files: &BTreeMap<String, String>) -> Result<Data<'_>> {
    let mut data = Data::default();
    let single = dictionary_body(&files["tiger.dict.yaml"])?;
    // 已校验原文可廉价计行，避免刚越过容量边界就保留近一倍空位。
    data.entries.reserve_exact(lines(single).count());
    for line in lines(single) {
        let row = fields::<3>(line)?;
        data.entries.push(tigertype_engine::Entry {
            character: character(row[0])?,
            code: row[1],
            weight: row[2].parse()?,
        });
    }
    // 词版单字表的字与码同上，但权重是官方为字词混排另定的；末列 stem 只供造词，可省略。
    let word = dictionary_body(&files["tigress.dict.yaml"])?;
    data.word_entries.reserve_exact(lines(word).count());
    for line in lines(word) {
        let row = columns::<4>(line, 3)?;
        data.word_entries.push(tigertype_engine::Entry {
            character: character(row[0])?,
            weight: row[1].parse()?,
            code: row[2],
        });
    }
    // 两张 tigress 词表已包含按虎码编码器生成的词码，导入最终四码而不执行 Lua。
    let phrases = [
        dictionary_body(&files["tigress_ci.dict.yaml"])?,
        dictionary_body(&files["tigress_simp_ci.dict.yaml"])?,
    ];
    data.phrases
        .reserve_exact(phrases.iter().map(|text| lines(text).count()).sum());
    for text in phrases {
        for line in lines(text) {
            // 锁定词表正文实际省略 stem 列，只保留文本、权重和最终词码。
            let row = fields::<3>(line)?;
            data.phrases.push(Phrase {
                text: row[0],
                weight: row[1].parse()?,
                code: row[2],
            });
        }
    }
    let common = dictionary_body(&files["core2022.dict.yaml"])?;
    data.common.reserve_exact(lines(common).count());
    for line in lines(common) {
        let row = fields::<2>(line)?;
        if row[1] != "t" {
            return Err("常用字集标记不符".into());
        }
        data.common.push(character(row[0])?);
    }
    // 原表近 80 万行词组大多是全拼方案用的；只有词表里的词才有虎码可查，其余不进内存。
    let typable: HashSet<&str> = data.phrases.iter().map(|phrase| phrase.text).collect();
    let pinyin = dictionary_body(&files["PY_c.dict.yaml"])?;
    // 用已有词数预留，避免为精确计数再扫描大拼音表；多音词照常增长，解析后收紧。
    data.word_pinyin.reserve_exact(typable.len());
    for line in lines(pinyin) {
        let row = fields::<3>(line)?;
        if row[0].chars().count() != 1 {
            if typable.contains(row[0]) {
                data.word_pinyin.push(WordPinyin {
                    text: row[0],
                    pinyin: row[1],
                    weight: row[2].parse()?,
                });
            }
            continue;
        }
        data.pinyin.push(Pronunciation {
            character: character(row[0])?,
            pinyin: row[1],
            weight: row[2].parse()?,
        });
    }
    drop(typable);
    data.pinyin.shrink_to_fit();
    data.word_pinyin.shrink_to_fit();
    let mut details = BTreeMap::<char, Detail>::new();
    for line in lines(&files["opencc/hu_cf.txt"]) {
        let row = fields::<2>(line)?;
        let ch = character(row[0])?;
        let inner = row[1]
            .strip_prefix('〔')
            .and_then(|s| s.strip_suffix('〕'))
            .ok_or("拆分括号格式不符")?;
        let (parts, codes) = inner.split_once('·').ok_or("拆分缺少编码分隔符")?;
        let parts = parts.trim_end_matches("&nbsp;").trim();
        // 拆分整段借用，&nbsp; 只应出现在分隔符前；内部出现时不能原样显示给用户。
        if parts.contains("&nbsp;") {
            return Err("拆分内含未处理的转义".into());
        }
        let detail = Detail {
            character: ch,
            full_codes: words(codes).collect(),
            decomposition: Some(parts),
            readings: Box::default(),
        };
        if detail.full_codes.is_empty() || details.insert(ch, detail).is_some() {
            return Err("拆分缺码或重复".into());
        }
    }
    for line in lines(&files["opencc/PYCharacters.txt"]) {
        let row = fields::<2>(line)?;
        let ch = character(row[0])?;
        let readings = row[1]
            .strip_prefix('（')
            .and_then(|s| s.strip_suffix('）'))
            .ok_or("读音括号格式不符")?;
        let detail = details.entry(ch).or_insert(Detail {
            character: ch,
            full_codes: Box::default(),
            decomposition: None,
            readings: Box::default(),
        });
        if !detail.readings.is_empty() {
            return Err("读音资料重复".into());
        }
        detail.readings = words(readings).collect();
    }
    data.details = details.into_values().collect();
    data.roots = roots(ROOTS)?;
    // 官方拼音滤镜的词读音表；原件个别行在读音里夹了制表符，只按首个制表符切出词，读音原样交引擎对齐。
    for line in lines(&files["opencc/PYPhrases.txt"]) {
        let (text, reading) = line.split_once('\t').ok_or("词读音缺少分隔符")?;
        data.word_readings.push(WordReading { text, reading });
    }
    // 同官方 t2s.json、st_tu.json 的字典组：短语在前、单字在后，合为一张表。
    data.simplified = mappings(&files["opencc/TSPhrases.txt"])?;
    data.simplified
        .extend(mappings(&files["opencc/TSCharacters.txt"])?);
    data.traditional = mappings(&files["opencc/STPhrases.txt"])?;
    data.traditional
        .extend(mappings(&files["opencc/STCharacters_Tu.txt"])?);
    Ok(data)
}

/// 核对过的码表内存副本，词的拼音已连写；Data 借用它，解析和建表期间须保持存活。
pub struct Sources(BTreeMap<String, String>);

impl Sources {
    pub fn data(&self) -> Result<Data<'_>> {
        parse_data(&self.0).map_err(|e| format!("码表解析失败: {e}").into())
    }
}

/// 后台、查字与打包共用：码表目录（开发为 resources/tiger，安装版为程序目录下的 tiger）须与锁定清单逐字节一致；
/// 词表的结构约束由调用方 Dictionary::new 检查。
pub fn load(tables: &Path) -> Result<Sources> {
    let mut files = BTreeMap::new();
    for entry in source_entries()? {
        let mut text = verified(tables, &entry)?;
        if entry.path == "PY_c.dict.yaml" {
            join_word_pinyin(&mut text);
            text.shrink_to_fit();
        }
        files.insert(entry.path, text);
    }
    Ok(Sources(files))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn joining_word_pinyin_preserves_text_single_readings_columns_and_unicode() {
        let mut source = String::from(
            "# header with spaces\r\n...\r\n𠀀\the a\t1\r\n你好\tni hao\t2\r\n大 空\t da kong \t3\r\n西安\txi\tan\t4\n",
        );
        join_word_pinyin(&mut source);
        assert_eq!(
            source,
            "# header with spaces\r\n...\r\n𠀀\the a\t1\r\n你好\tnihao\t2\r\n大 空\tdakong\t3\r\n西安\txi\tan\t4\n"
        );
        // 重复处理不继续改写，额外列仍可被原有格式校验拒绝。
        let joined = source.clone();
        join_word_pinyin(&mut source);
        assert_eq!(source, joined);
        assert!(fields::<3>("西安\txi\tan\t4").is_err());
    }
    #[test]
    fn compiled_root_chart_is_well_formed_and_unambiguous() {
        let chart = roots(ROOTS).unwrap();
        let mut codes = std::collections::BTreeMap::new();
        for root in &chart {
            let code = *codes.entry(root.character).or_insert(root.code);
            assert_eq!(code, root.code, "{:04X}", root.character as u32);
        }
        // 官网字根表 241 行加拆分查询补充，覆盖锁定拆分用到的 1074 个字根。
        assert!(codes.len() >= 1074, "{}", codes.len());
        assert_eq!(codes.get(&'\u{e4e4}'), Some(&"xd"));
        assert!(roots("ab\t").is_err() && roots("ab").is_err());
    }
    #[test]
    fn parsers_reject_ambiguous_or_malformed_records() {
        assert!(character("你好").is_err());
        assert_eq!(character("𠀀").unwrap(), '𠀀');
        assert!(fields::<2>("a\tb\tc").is_err());
        assert!(fields::<3>("a\tb").is_err());
        assert_eq!(columns::<4>("a\tb\tc", 3).unwrap(), ["a", "b", "c", ""]);
        assert!(columns::<4>("a\tb\tc\td\te", 3).is_err());
        assert_eq!(dictionary_body("x\r\n...\r\nbody").unwrap(), "body");
        assert_eq!(dictionary_body("x\n...\nbody").unwrap(), "body");
        assert!(dictionary_body("没有正文").is_err());
        assert!(mappings("发\t發髮").is_err());
        assert_eq!(
            mappings("发\t發 髮").unwrap()[0].variants.as_ref(),
            ["發", "髮"]
        );
        assert_eq!(
            mappings("头发\t頭髮").unwrap()[0].variants.as_ref(),
            ["頭髮"]
        );
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
