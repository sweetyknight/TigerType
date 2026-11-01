//! 启动时验证并建立有序索引；按键路径只访问对应键的有界切片。
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use tigertype_contracts::{
    Candidate, InputMode, LOOKUP_PAGE_SIZE, LearningInfo, LookupItem, LookupPage,
    MAX_LEARNING_ITEMS, MAX_PINYIN_BYTES, MAX_QUERY_CHARS, Options, OutputForm, TypingMode,
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
/// 字形映射的键是单字或短语；单字的一对多全部列出，词取整词命中或逐段最长匹配。
#[derive(Debug, Clone)]
pub struct Mapping {
    pub text: String,
    pub variants: Vec<String>,
}
#[derive(Debug, Clone, Default)]
pub struct Data {
    /// 单字模式用的 tiger 单字表。
    pub entries: Vec<Entry>,
    /// 词模式用的 tigress 单字表：字码同上，权重为官方字词混排而定，不能与 entries 互换。
    pub word_entries: Vec<Entry>,
    /// 小狼毫 tigress_ci 词表；只保留词、权重和最终四码，不加载 Lua 编码器。
    pub phrases: Vec<Phrase>,
    pub common: Vec<char>,
    pub pinyin: Vec<Pronunciation>,
    pub details: Vec<Detail>,
    pub simplified: Vec<Mapping>,
    pub traditional: Vec<Mapping>,
}

pub struct Dictionary {
    common: BTreeSet<char>,
    // 启动时把同码桶整理为按权重顺序的去重字符，输入路径不再扫描整张排序表。
    tiger_buckets: BTreeMap<String, Vec<char>>,
    pinyin_buckets: BTreeMap<String, Vec<char>>,
    phrases: Vec<Phrase>,
    // 词模式的同码候选：单字与词同桶，查字的词码入口再滤掉单字。
    word_buckets: BTreeMap<String, Vec<String>>,
    learning: Vec<(char, LearningInfo)>,
    simplified: BTreeMap<String, Vec<String>>,
    traditional: BTreeMap<String, Vec<String>>,
}

/// 与官方 core2022_filter 相同的汉字范围（含私用区字根）；范围外的标点等不受常用字过滤。
const HAN_RANGES: [(u32, u32); 22] = [
    (0x4e00, 0x9fff),
    (0x3400, 0x4dbf),
    (0x20000, 0x2a6df),
    (0x2a700, 0x2b73f),
    (0x2b740, 0x2b81f),
    (0x2b820, 0x2ceaf),
    (0x2ceb0, 0x2ebef),
    (0x30000, 0x3134f),
    (0x31350, 0x323af),
    (0x2ebf0, 0x2ee5d),
    (0x323b0, 0x33479),
    (0x31c0, 0x31ef),
    (0x2e80, 0x2eff),
    (0x2f00, 0x2fdf),
    (0xf900, 0xfadf),
    (0x2f800, 0x2fa1f),
    (0x2ff0, 0x2fff),
    (0x3100, 0x312f),
    (0x31a0, 0x31bf),
    (0xe000, 0xf8ff),
    (0xf0000, 0xffff),
    (0x100000, 0x10ffff),
];

pub fn valid_code(code: &str) -> bool {
    (1..=4).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_lowercase())
}
fn distinct_push(items: &mut Vec<String>, item: String) {
    if !items.contains(&item) {
        items.push(item);
    }
}
fn maps(values: Vec<Mapping>) -> Result<BTreeMap<String, Vec<String>>> {
    let mut map = BTreeMap::new();
    for value in values {
        if value.variants.is_empty()
            || value.variants.len() > MAX_VARIANTS
            || std::iter::once(&value.text)
                .chain(&value.variants)
                .any(|text| text.is_empty() || text.chars().any(char::is_control))
            || map.insert(value.text, value.variants).is_some()
        {
            return Err(Error("简繁映射为空、重复或超限"));
        }
    }
    Ok(map)
}
/// 同 OpenCC（经 Rime simplifier）：整词命中列全部字形；否则从左到右取最长键的首选字形，表外字保留。
fn forms(table: &BTreeMap<String, Vec<String>>, text: &str) -> Vec<String> {
    if let Some(forms) = table.get(text) {
        return forms.clone();
    }
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(first) = rest.chars().next() {
        let longest = std::iter::once(rest.len())
            .chain(rest.char_indices().rev().map(|(index, _)| index))
            .filter(|end| *end > 0)
            .find_map(|end| {
                table
                    .get(&rest[..end])
                    .map(|forms| (end, forms[0].as_str()))
            });
        let (end, form) = longest.unwrap_or((first.len_utf8(), &rest[..first.len_utf8()]));
        output.push_str(form);
        rest = &rest[end..];
    }
    vec![output]
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
                data.word_entries.len(),
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
        if data.phrases.iter().any(|phrase| {
            phrase.text.chars().count() < 2
                || phrase.text.chars().any(char::is_control)
                || !valid_code(&phrase.code)
        }) || data
            .word_entries
            .iter()
            .any(|entry| !valid_code(&entry.code) || entry.character.is_control())
        {
            return Err(Error("词条文本或编码非法"));
        }
        // 同官方 tigress：单字表在前、词表在后，同码按权重降序稳定排序（同权重保持此顺序），再按文本去重。
        let mut word_rows: Vec<_> = data
            .word_entries
            .iter()
            .map(|entry| (&entry.code, entry.weight, entry.character.to_string()))
            .chain(
                data.phrases
                    .iter()
                    .map(|phrase| (&phrase.code, phrase.weight, phrase.text.clone())),
            )
            .collect();
        word_rows.sort_by(|a, b| a.0.cmp(b.0).then_with(|| b.1.cmp(&a.1)));
        let mut word_buckets = BTreeMap::<String, Vec<String>>::new();
        for (code, _, text) in word_rows {
            distinct_push(word_buckets.entry(code.clone()).or_default(), text);
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
            common: data.common.into_iter().collect(),
            tiger_buckets,
            pinyin_buckets,
            phrases: data.phrases,
            word_buckets,
            learning: learning.into_iter().collect(),
            simplified: maps(data.simplified)?,
            traditional: maps(data.traditional)?,
        };
        // 在加载时拒绝过大的同码桶，不能在查询时截断并悄悄丢掉后部候选。
        let expansion = |text: &str| {
            let simplified = result.simplified.get(text).map_or(1, Vec::len);
            simplified.max(result.traditional.get(text).map_or(1, Vec::len))
        };
        let characters = result
            .tiger_buckets
            .values()
            .chain(result.pinyin_buckets.values());
        if characters
            .map(|bucket| bucket.iter().map(|c| expansion(&c.to_string())).sum())
            .chain(
                result
                    .word_buckets
                    .values()
                    .map(|bucket| bucket.iter().map(|text| expansion(text)).sum()),
            )
            .any(|count: usize| count > MAX_MATCHES)
        {
            return Err(Error("同码转换候选超过有界容量"));
        }
        Ok(result)
    }
    /// 常用字过滤同官方：词中每个汉字都须在常用字集。
    fn common_text(&self, text: &str) -> bool {
        text.chars().all(|c| {
            let han = HAN_RANGES
                .iter()
                .any(|(first, last)| (*first..=*last).contains(&(c as u32)));
            !han || self.common.contains(&c)
        })
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
        let texts: Vec<String> = match (mode, options.typing_mode) {
            (InputMode::Tiger, TypingMode::Single) => self
                .tiger_buckets
                .get(code)
                .into_iter()
                .flatten()
                .filter(|character| !options.common_only || self.common.contains(character))
                .map(char::to_string)
                .collect(),
            (InputMode::Tiger, TypingMode::Word) => self
                .word_buckets
                .get(code)
                .into_iter()
                .flatten()
                .filter(|text| !options.common_only || self.common_text(text))
                .cloned()
                .collect(),
            (InputMode::Pinyin, _) => self
                .pinyin_buckets
                .get(code)
                .into_iter()
                .flatten()
                .map(char::to_string)
                .collect(),
        };
        let table = match options.output_form {
            OutputForm::Original => {
                // 原字模式的桶已在加载时去重，直接生成候选，避免每个按键重复分配去重树。
                return texts
                    .into_iter()
                    .map(|text| Candidate {
                        text,
                        ..Default::default()
                    })
                    .collect();
            }
            OutputForm::Simplified => &self.simplified,
            OutputForm::Traditional => &self.traditional,
        };
        // 官方的去重在字形转换之后：不同来源转成同一字形只留首个。
        let mut seen = BTreeSet::new();
        let mut result = Vec::new();
        for original in texts {
            let mut chars = original.chars();
            let single = chars.next().filter(|_| chars.next().is_none());
            for text in forms(table, &original) {
                if seen.insert(text.clone()) {
                    result.push(Candidate {
                        source: single.filter(|_| text != original),
                        text,
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
        let offset = page
            .checked_mul(LOOKUP_PAGE_SIZE)
            .ok_or(Error("查询页码溢出"))?;
        if page > u32::MAX as usize {
            return Err(Error("查询页码超限"));
        }
        let items = if query.is_empty() {
            Vec::new()
        } else if query
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == 'ü' || c == 'Ü')
        {
            // 词码入口只列词：单字虎码不作查询入口，字母串同时按拼音查单字。
            let mut values: Vec<String> = self
                .word_buckets
                .get(&query.to_ascii_lowercase())
                .into_iter()
                .flatten()
                .filter(|text| text.chars().nth(1).is_some())
                .cloned()
                .collect();
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
            has_more: items.len().saturating_sub(offset) > LOOKUP_PAGE_SIZE,
            items: items
                .into_iter()
                .skip(offset)
                .take(LOOKUP_PAGE_SIZE)
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
