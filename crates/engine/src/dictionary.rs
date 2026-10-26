//! 启动时验证并建立有序索引；按键路径只访问对应键的有界切片。
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use tigertype_contracts::{
    Candidate, InputMode, LearningInfo, LookupItem, LookupPage, MAX_LEARNING_ITEMS,
    MAX_PINYIN_BYTES, MAX_QUERY_CHARS, Options, OutputForm, PAGE_SIZE,
};

const MAX_ROWS: usize = 300_000;
const MAX_MATCHES: usize = 8192;
const MAX_VARIANTS: usize = 32;

#[derive(Debug, Clone)]
pub struct Entry {
    pub character: char,
    pub code: String,
    pub weight: u64,
}
#[derive(Debug, Clone)]
pub struct Phrase {
    pub text: String,
    pub code: String,
    pub weight: u64,
}
#[derive(Debug, Clone)]
pub struct Pronunciation {
    pub character: char,
    pub pinyin: String,
    pub weight: u64,
}
#[derive(Debug, Clone)]
pub struct Detail {
    pub character: char,
    pub full_codes: Vec<String>,
    pub decomposition: Option<String>,
    pub readings: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct Mapping {
    pub character: char,
    pub variants: Vec<char>,
}
#[derive(Debug, Clone, Default)]
pub struct Data {
    pub entries: Vec<Entry>,
    /// 小狼毫 tigress_ci 词表；只保留词、权重和最终四码，不加载 Lua 编码器。
    pub phrases: Vec<Phrase>,
    pub common: Vec<char>,
    pub pinyin: Vec<Pronunciation>,
    pub details: Vec<Detail>,
    pub simplified: Vec<Mapping>,
    pub traditional: Vec<Mapping>,
}

pub struct Dictionary {
    entries: Vec<Entry>,
    common: BTreeSet<char>,
    pinyin: Vec<Pronunciation>,
    // 启动时把同码桶整理为按权重顺序的去重字符，输入路径不再扫描整张排序表。
    tiger_buckets: BTreeMap<String, Vec<char>>,
    pinyin_buckets: BTreeMap<String, Vec<char>>,
    phrases: Vec<Phrase>,
    word_buckets: BTreeMap<String, Vec<String>>,
    learning: Vec<(char, LearningInfo)>,
    simplified: BTreeMap<char, Vec<char>>,
    traditional: BTreeMap<char, Vec<char>>,
}

pub fn valid_code(code: &str) -> bool {
    (1..=4).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_lowercase())
}
fn distinct_push(items: &mut Vec<String>, item: String) {
    if !items.contains(&item) {
        items.push(item);
    }
}
fn maps(values: Vec<Mapping>) -> Result<BTreeMap<char, Vec<char>>> {
    let mut map = BTreeMap::new();
    for value in values {
        if value.variants.is_empty()
            || value.variants.len() > MAX_VARIANTS
            || value.character.is_control()
            || value.variants.iter().any(|c| c.is_control())
            || map.insert(value.character, value.variants).is_some()
        {
            return Err(Error("简繁映射为空、重复或超限"));
        }
    }
    Ok(map)
}

/// 不做声调和模糊纠错；ü/v 仅作为同一个无调输入码。
pub fn normalize_pinyin(text: &str) -> Result<String> {
    let normalized = text.trim().to_lowercase().replace('ü', "v");
    if normalized.is_empty()
        || normalized.len() > MAX_PINYIN_BYTES
        || !normalized.bytes().all(|b| b.is_ascii_lowercase())
    {
        return Err(Error("请输入一个无调全拼音节（ü 可输入 v）"));
    }
    Ok(normalized)
}

impl Dictionary {
    pub fn new(mut data: Data) -> Result<Self> {
        if data.entries.is_empty()
            || [
                data.entries.len(),
                data.phrases.len(),
                data.common.len(),
                data.pinyin.len(),
                data.details.len(),
                data.simplified.len(),
                data.traditional.len(),
            ]
            .iter()
            .any(|n| *n > MAX_ROWS)
        {
            return Err(Error("码表为空或数据行数超限"));
        }
        let mut learning = BTreeMap::<char, LearningInfo>::new();
        if data.common.iter().any(|c| c.is_control()) {
            return Err(Error("常用字集含控制字符"));
        }
        for entry in &data.entries {
            if !valid_code(&entry.code) || entry.character.is_control() {
                return Err(Error("单字编码或字符非法"));
            }
            distinct_push(
                &mut learning.entry(entry.character).or_default().codes,
                entry.code.clone(),
            );
        }
        for entry in &data.pinyin {
            if normalize_pinyin(&entry.pinyin)? != entry.pinyin || entry.character.is_control() {
                return Err(Error("拼音数据必须为规范化单音节"));
            }
        }
        let mut details = BTreeSet::new();
        for detail in data.details {
            if detail.character.is_control()
                || !details.insert(detail.character)
                || detail.full_codes.iter().any(|s| !valid_code(s))
                || detail.decomposition.as_ref().is_some_and(|s| {
                    s.is_empty() || s.len() > 1024 || s.chars().any(char::is_control)
                })
                || detail
                    .readings
                    .iter()
                    .any(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
            {
                return Err(Error("学习资料重复或格式非法"));
            }
            let info = learning.entry(detail.character).or_default();
            info.full_codes = detail.full_codes;
            info.decomposition = detail.decomposition;
            info.readings = detail.readings;
        }
        // 一次建立后备读音索引，避免每个字都重新扫描拼音表。
        let mut readings = BTreeMap::<char, BTreeSet<String>>::new();
        for entry in &data.pinyin {
            readings
                .entry(entry.character)
                .or_default()
                .insert(entry.pinyin.clone());
        }
        for (character, values) in readings {
            let info = learning.entry(character).or_default();
            if info.readings.is_empty() {
                info.readings = values.into_iter().collect();
            }
        }
        for info in learning.values_mut() {
            info.codes
                .sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
            if [info.codes.len(), info.full_codes.len(), info.readings.len()]
                .iter()
                .any(|n| *n > MAX_LEARNING_ITEMS)
            {
                return Err(Error("单字学习资料条目超限"));
            }
        }
        // 词候选沿用码表权重顺序，不能用 Hash/BTree 的键序替代排序。
        data.phrases
            .sort_by(|a, b| a.code.cmp(&b.code).then_with(|| b.weight.cmp(&a.weight)));
        let mut word_buckets = BTreeMap::<String, Vec<String>>::new();
        for phrase in &data.phrases {
            if phrase.text.chars().count() < 2
                || phrase.text.chars().any(char::is_control)
                || !valid_code(&phrase.code)
            {
                return Err(Error("词条文本或编码非法"));
            }
            word_buckets
                .entry(phrase.code.clone())
                .or_default()
                .push(phrase.text.clone());
        }
        for values in word_buckets.values_mut() {
            values.dedup();
            if values.len() > MAX_MATCHES {
                return Err(Error("同码词候选超过有界容量"));
            }
        }
        // 稳定排序让同权重候选保留原始顺序，不使用随机哈希序作为排序。
        data.entries
            .sort_by(|a, b| a.code.cmp(&b.code).then_with(|| b.weight.cmp(&a.weight)));
        data.pinyin.sort_by(|a, b| {
            a.pinyin
                .cmp(&b.pinyin)
                .then_with(|| b.weight.cmp(&a.weight))
        });
        // 预建两类短码桶；去重工作只在加载阶段做一次，避免每个按键重新维护 BTreeSet。
        let mut tiger_buckets: BTreeMap<String, Vec<char>> = BTreeMap::new();
        let mut tiger_seen = BTreeSet::<(String, char)>::new();
        for entry in &data.entries {
            if tiger_seen.insert((entry.code.clone(), entry.character)) {
                tiger_buckets
                    .entry(entry.code.clone())
                    .or_default()
                    .push(entry.character);
            }
        }
        let mut pinyin_buckets: BTreeMap<String, Vec<char>> = BTreeMap::new();
        let mut pinyin_seen = BTreeSet::<(String, char)>::new();
        for entry in &data.pinyin {
            if pinyin_seen.insert((entry.pinyin.clone(), entry.character)) {
                pinyin_buckets
                    .entry(entry.pinyin.clone())
                    .or_default()
                    .push(entry.character);
            }
        }
        let result = Self {
            entries: data.entries,
            common: data.common.into_iter().collect(),
            pinyin: data.pinyin,
            tiger_buckets,
            pinyin_buckets,
            phrases: data.phrases,
            word_buckets,
            learning: learning.into_iter().collect(),
            simplified: maps(data.simplified)?,
            traditional: maps(data.traditional)?,
        };
        // 在加载时拒绝过大的同码桶，不能在查询时截断并悄悄丢掉后部候选。
        for mode in [InputMode::Tiger, InputMode::Pinyin] {
            let mut counts = BTreeMap::<&str, usize>::new();
            let rows: Vec<(&str, char)> = match mode {
                InputMode::Tiger => result
                    .entries
                    .iter()
                    .map(|e| (e.code.as_str(), e.character))
                    .collect(),
                InputMode::Pinyin => result
                    .pinyin
                    .iter()
                    .map(|e| (e.pinyin.as_str(), e.character))
                    .collect(),
            };
            for (code, character) in rows {
                let count = counts.entry(code).or_default();
                *count += result
                    .simplified
                    .get(&character)
                    .map_or(1, Vec::len)
                    .max(result.traditional.get(&character).map_or(1, Vec::len));
                if *count > MAX_MATCHES {
                    return Err(Error("同码转换候选超过有界容量"));
                }
            }
        }
        Ok(result)
    }

    pub fn learning(&self, character: char) -> LearningInfo {
        self.learning
            .binary_search_by_key(&character, |e| e.0)
            .ok()
            .map(|index| self.learning[index].1.clone())
            .unwrap_or_default()
    }

    pub(crate) fn candidates(
        &self,
        code: &str,
        mode: InputMode,
        options: Options,
    ) -> Vec<Candidate> {
        if code.is_empty() {
            return Vec::new();
        }
        if mode == InputMode::Tiger && options.typing_mode == tigertype_contracts::TypingMode::Word
        {
            return self
                .word_buckets
                .get(code)
                .into_iter()
                .flatten()
                .map(|text| Candidate {
                    text: text.clone(),
                    ..Default::default()
                })
                .collect();
        }
        let characters: Vec<char> = match mode {
            InputMode::Tiger => self
                .tiger_buckets
                .get(code)
                .into_iter()
                .flatten()
                .copied()
                .filter(|character| !options.common_only || self.common.contains(character))
                .collect(),
            InputMode::Pinyin => self
                .pinyin_buckets
                .get(code)
                .into_iter()
                .flatten()
                .copied()
                .collect(),
        };
        let conversion = match options.output_form {
            OutputForm::Original => None,
            OutputForm::Simplified => Some(&self.simplified),
            OutputForm::Traditional => Some(&self.traditional),
        };
        if conversion.is_none() {
            // 原字模式的桶已在加载时去重，直接生成候选，避免每个按键重复分配去重树。
            return characters
                .into_iter()
                .map(|character| Candidate {
                    text: character.to_string(),
                    ..Default::default()
                })
                .collect();
        }
        let mut seen = BTreeSet::new();
        let mut result = Vec::new();
        for source in characters {
            let original = [source];
            let variants = conversion
                .and_then(|m| m.get(&source))
                .map_or(original.as_slice(), Vec::as_slice);
            for &character in variants {
                if seen.insert(character) {
                    result.push(Candidate {
                        text: character.to_string(),
                        source: (source != character).then_some(source),
                        ..Default::default()
                    });
                }
            }
        }
        result
    }

    /// 查询只读，不持有 Session，也不会改变输入模式或产生提交。
    pub fn lookup(&self, query: &str, page: usize) -> Result<LookupPage> {
        if query.chars().count() > MAX_QUERY_CHARS {
            return Err(Error("查字文本超过 64 个字符"));
        }
        let query = query.trim();
        let offset = page.checked_mul(PAGE_SIZE).ok_or(Error("查询页码溢出"))?;
        if page > u32::MAX as usize {
            return Err(Error("查询页码超限"));
        }
        let items = if query.is_empty() {
            Vec::new()
        } else if query
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == 'ü' || c == 'Ü')
        {
            let mut values = self
                .word_buckets
                .get(&query.to_ascii_lowercase())
                .cloned()
                .unwrap_or_default();
            values.extend(
                self.candidates(
                    &normalize_pinyin(query)?,
                    InputMode::Pinyin,
                    Options::default(),
                )
                .into_iter()
                .map(|c| c.text),
            );
            values.dedup();
            values
        } else {
            let mut words = self
                .phrases
                .iter()
                .filter(|phrase| phrase.text == query)
                .map(|phrase| phrase.text.clone())
                .collect::<Vec<_>>();
            if words.is_empty() {
                let mut seen = BTreeSet::new();
                words = query
                    .chars()
                    .filter(|c| !c.is_whitespace() && seen.insert(*c))
                    .map(|c| c.to_string())
                    .collect();
            }
            words
        };
        Ok(LookupPage {
            has_more: items.len().saturating_sub(offset) > PAGE_SIZE,
            items: items
                .into_iter()
                .skip(offset)
                .take(PAGE_SIZE)
                .map(|text| {
                    let mut learning = text
                        .chars()
                        .next()
                        .map_or_else(LearningInfo::default, |character| self.learning(character));
                    // 词查询展示词本身的四码，不把首字编码误当作词码。
                    if let Some(code) = self
                        .phrases
                        .iter()
                        .find(|phrase| phrase.text == text)
                        .map(|phrase| phrase.code.clone())
                    {
                        learning.codes = vec![code];
                        learning.full_codes.clear();
                        learning.decomposition = None;
                        learning.readings.clear();
                    }
                    LookupItem { text, learning }
                })
                .collect(),
        })
    }
}
