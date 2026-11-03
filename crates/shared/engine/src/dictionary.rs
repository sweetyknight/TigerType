//! 启动时验证并建立有序索引；按键路径只访问对应键的有界切片。
use crate::{Error, Result};
use std::collections::{BTreeSet, HashSet};
use tigertype_contracts::{
    Candidate, InputMode, LOOKUP_PAGE_SIZE, LearningInfo, LookupItem, LookupPage,
    MAX_LEARNING_ITEMS, MAX_PINYIN_BYTES, MAX_QUERY_CHARS, Options, OutputForm, TypingMode,
};

const MAX_ROWS: usize = 300_000;
const MAX_MATCHES: usize = 8192;
const MAX_VARIANTS: usize = 32;

// 以下类型只在启动时存在，文本直接借用码表原文，解析不逐行分配。
#[derive(Debug, Clone)]
pub struct Entry<'a> {
    pub character: char,
    pub code: &'a str,
    pub weight: u64,
}
#[derive(Debug, Clone)]
pub struct Phrase<'a> {
    pub text: &'a str,
    pub code: &'a str,
    pub weight: u64,
}
#[derive(Debug, Clone)]
pub struct Pronunciation<'a> {
    pub character: char,
    pub pinyin: &'a str,
    pub weight: u64,
}
/// 词的全拼，音节连写；拼音反查按整串匹配。
#[derive(Debug, Clone)]
pub struct WordPinyin<'a> {
    pub text: &'a str,
    pub pinyin: &'a str,
    pub weight: u64,
}
#[derive(Debug, Clone)]
pub struct Detail<'a> {
    pub character: char,
    // 解析后不再追加，精确长度切片避免每字保留 Vec 的增长余量。
    pub full_codes: Box<[&'a str]>,
    pub decomposition: Option<&'a str>,
    pub readings: Box<[&'a str]>,
}
/// 字形映射的键是单字或短语；单字的一对多全部列出，词取整词命中或逐段最长匹配。
#[derive(Debug, Clone)]
pub struct Mapping<'a> {
    pub text: &'a str,
    pub variants: Box<[&'a str]>,
}
#[derive(Debug, Clone, Default)]
pub struct Data<'a> {
    /// 单字模式用的 tiger 单字表。
    pub entries: Vec<Entry<'a>>,
    /// 词模式用的 tigress 单字表：字码同上，权重为官方字词混排而定，不能与 entries 互换。
    pub word_entries: Vec<Entry<'a>>,
    /// 小狼毫 tigress_ci 词表；只保留词、权重和最终四码，不加载 Lua 编码器。
    pub phrases: Vec<Phrase<'a>>,
    pub common: Vec<char>,
    pub pinyin: Vec<Pronunciation<'a>>,
    /// 拼音反查用的词，只含词表里有的词：没有虎码的词反查不出东西。
    pub word_pinyin: Vec<WordPinyin<'a>>,
    pub details: Vec<Detail<'a>>,
    pub simplified: Vec<Mapping<'a>>,
    pub traditional: Vec<Mapping<'a>>,
}

/// 建成后只读的“有序键 → 文本列表”：键和值首尾相接存进一块缓冲区，查找走二分。
/// 码表文本只有几 MB，逐条 String、Vec 和 BTreeMap 节点的开销却是它的几十倍，这里只多两张下标。
#[derive(Default)]
struct Table {
    text: String,
    /// 每段文本在 text 里的结束位置。
    ends: Vec<u32>,
    /// 每个键的段号；键后紧跟它的值，直到下一个键。
    keys: Vec<u32>,
}

fn offset(value: usize) -> Result<u32> {
    u32::try_from(value).map_err(|_| Error("码表文本超过紧凑存储上限"))
}

impl Table {
    /// 逐行追加：键须按字节序非递减，同键的行相邻；值保持追加顺序。
    fn push(&mut self, key: &str, value: &str) -> Result<()> {
        let last = self.keys.last().map(|first| self.segment(*first as usize));
        debug_assert!(last <= Some(key));
        if last != Some(key) {
            self.keys.push(offset(self.ends.len())?);
            self.append(key)?;
        }
        self.append(value)
    }
    fn append(&mut self, text: &str) -> Result<()> {
        self.text.push_str(text);
        self.ends.push(offset(self.text.len())?);
        Ok(())
    }
    /// 建完收回增长时多留的容量。
    fn finish(mut self) -> Self {
        self.text.shrink_to_fit();
        self.ends.shrink_to_fit();
        self.keys.shrink_to_fit();
        self
    }
    fn segment(&self, index: usize) -> &str {
        let start = index
            .checked_sub(1)
            .map_or(0, |last| self.ends[last] as usize);
        &self.text[start..self.ends[index] as usize]
    }
    fn value_segments(&self, key: usize) -> std::ops::Range<usize> {
        let first = self.keys[key] as usize + 1;
        let last = self
            .keys
            .get(key + 1)
            .map_or(self.ends.len(), |next| *next as usize);
        first..last
    }
    fn values(&self, key: usize) -> impl ExactSizeIterator<Item = &str> {
        self.value_segments(key).map(|index| self.segment(index))
    }
    /// 值段所属的键。
    fn key_of(&self, segment: usize) -> &str {
        let next = self
            .keys
            .partition_point(|first| *first as usize <= segment);
        self.segment(self.keys[next - 1] as usize)
    }
    /// 键不存在时为 None。
    fn get<'a>(&'a self, key: &str) -> Option<impl ExactSizeIterator<Item = &'a str> + use<'a>> {
        let found = self
            .keys
            .binary_search_by(|first| self.segment(*first as usize).cmp(key))
            .ok()?;
        Some(self.values(found))
    }
    fn lists(&self) -> impl Iterator<Item = impl ExactSizeIterator<Item = &str>> {
        (0..self.keys.len()).map(|key| self.values(key))
    }
}

pub struct Dictionary {
    /// 升序，二分判断。
    common: Vec<char>,
    // 启动时把同码桶整理为按权重顺序的去重文本，输入路径不再扫描整张排序表。
    tiger_buckets: Table,
    pinyin_buckets: Table,
    // 词模式的同码候选：单字与词同桶，查字的词码入口再滤掉单字。
    word_buckets: Table,
    // word_buckets 里词的段号，按词再按码排序；查字据此由词找码，不再另存一份词表。
    word_order: Vec<u32>,
    // 字 → 学习资料，分段见 learning_table。
    learning: Table,
    simplified: Table,
    traditional: Table,
}

/// 与官方 core2022_filter 相同的汉字范围（含私用区字根），升序不相交以便二分；范围外的标点等不受常用字过滤。
/// 官方的“私用区补充A”写作 0xf0000..=0xffff，下限大于上限不匹配任何字，这里等价略去。
const HAN_RANGES: [(u32, u32); 21] = [
    (0x2e80, 0x2eff),
    (0x2f00, 0x2fdf),
    (0x2ff0, 0x2fff),
    (0x3100, 0x312f),
    (0x31a0, 0x31bf),
    (0x31c0, 0x31ef),
    (0x3400, 0x4dbf),
    (0x4e00, 0x9fff),
    (0xe000, 0xf8ff),
    (0xf900, 0xfadf),
    (0x20000, 0x2a6df),
    (0x2a700, 0x2b73f),
    (0x2b740, 0x2b81f),
    (0x2b820, 0x2ceaf),
    (0x2ceb0, 0x2ebef),
    (0x2ebf0, 0x2ee5d),
    (0x2f800, 0x2fa1f),
    (0x30000, 0x3134f),
    (0x31350, 0x323af),
    (0x323b0, 0x33479),
    (0x100000, 0x10ffff),
];

fn is_han(character: char) -> bool {
    let code = character as u32;
    let next = HAN_RANGES.partition_point(|(first, _)| *first <= code);
    next > 0 && code <= HAN_RANGES[next - 1].1
}
pub fn valid_code(code: &str) -> bool {
    (1..=4).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_lowercase())
}
fn mappings(mut values: Vec<Mapping<'_>>) -> Result<Table> {
    values.sort_unstable_by_key(|value| value.text);
    let invalid = |value: &Mapping| {
        value.variants.is_empty()
            || value.variants.len() > MAX_VARIANTS
            || std::iter::once(&value.text)
                .chain(&value.variants)
                .any(|text| text.is_empty() || text.chars().any(char::is_control))
    };
    if values.iter().any(invalid) || values.windows(2).any(|pair| pair[0].text == pair[1].text) {
        return Err(Error("简繁映射为空、重复或超限"));
    }
    let mut table = Table::default();
    for value in &values {
        for variant in &value.variants {
            table.push(value.text, variant)?;
        }
    }
    Ok(table.finish())
}
/// 同码候选：单字不为每行分配 String，写表时才编码。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Text<'a> {
    Character(char),
    Word(&'a str),
}
/// 同码按权重降序稳定排序（同权重保持入参顺序），再按文本去重写成码表。
/// 候选顺序只由这次排序决定，哈希集仅用于判重。
fn buckets<'a>(rows: impl Iterator<Item = (&'a str, u64, Text<'a>)>) -> Result<Table> {
    let mut rows: Vec<_> = rows.collect();
    rows.sort_by(|a, b| a.0.cmp(b.0).then_with(|| b.1.cmp(&a.1)));
    // 判重集逐桶复用，内存只随最大的桶，不随全表。
    let mut seen = HashSet::new();
    let mut table = Table::default();
    let mut buffer = [0; 4];
    for group in rows.chunk_by(|a, b| a.0 == b.0) {
        seen.clear();
        for (code, _, text) in group {
            if seen.insert(*text) {
                let text = match *text {
                    Text::Character(character) => character.encode_utf8(&mut buffer),
                    Text::Word(word) => word,
                };
                table.push(code, text)?;
            }
        }
    }
    Ok(table.finish())
}
/// 字 → 编码、完整码、拆分、读音四段，以空串分隔；各段内容已校验不为空，不会与分隔混淆。
fn learning_table(data: &Data<'_>) -> Result<Table> {
    // (字, 段, 段内序, 文本)：排一次就按字分组、段内有序，不必给每个字建一组 Vec。
    // 编码按长度再按字母序；完整码与读音保持资料顺序；段 4 是后备读音，字母序。
    let detail_rows = data
        .details
        .iter()
        .map(|detail| 1 + detail.full_codes.len() + detail.readings.len());
    let mut rows =
        Vec::with_capacity(data.entries.len() + data.pinyin.len() + detail_rows.sum::<usize>());
    rows.extend(
        data.entries
            .iter()
            .map(|e| (e.character, 0, e.code.len(), e.code)),
    );
    for detail in &data.details {
        let character = detail.character;
        rows.extend(
            detail
                .full_codes
                .iter()
                .enumerate()
                .map(|(index, code)| (character, 1, index, *code)),
        );
        rows.extend(
            detail
                .decomposition
                .iter()
                .map(|text| (character, 2, 0, *text)),
        );
        rows.extend(
            detail
                .readings
                .iter()
                .enumerate()
                .map(|(index, reading)| (character, 3, index, *reading)),
        );
    }
    rows.extend(data.pinyin.iter().map(|p| (p.character, 4, 0, p.pinyin)));
    rows.sort_unstable();
    rows.dedup();
    let mut table = Table::default();
    let mut buffer = [0; 4];
    for group in rows.chunk_by(|a, b| a.0 == b.0) {
        let key = group[0].0.encode_utf8(&mut buffer);
        let section = |number| {
            group
                .iter()
                .filter(move |row| row.1 == number)
                .map(|row| row.3)
        };
        // 资料自带读音优先，没有才退回拼音表。
        let readings = section(if section(3).next().is_some() { 3 } else { 4 });
        if [
            section(0).count(),
            section(1).count(),
            readings.clone().count(),
        ]
        .iter()
        .any(|n| *n > MAX_LEARNING_ITEMS)
        {
            return Err(Error("单字学习资料条目超限"));
        }
        let values = section(0)
            .chain([""])
            .chain(section(1))
            .chain([""])
            .chain(section(2))
            .chain([""])
            .chain(readings);
        for value in values {
            table.push(key, value)?;
        }
    }
    Ok(table.finish())
}
/// 同 OpenCC（经 Rime simplifier）：整词命中列全部字形；否则从左到右取最长键的首选字形，表外字保留。
fn forms(table: &Table, text: &str) -> Vec<String> {
    if let Some(forms) = table.get(text) {
        return forms.map(str::to_owned).collect();
    }
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(first) = rest.chars().next() {
        let longest = std::iter::once(rest.len())
            .chain(rest.char_indices().rev().map(|(index, _)| index))
            .filter(|end| *end > 0)
            .find_map(|end| {
                let form = table.get(&rest[..end])?.next()?;
                Some((end, form))
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
    if !valid_pinyin(&normalized) {
        return Err(Error("请输入无调全拼（ü 可输入 v）"));
    }
    Ok(normalized)
}
// 建表输入已规范化，直接校验，避免逐条分配临时小写/替换字符串。
fn valid_pinyin(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_PINYIN_BYTES
        && text.bytes().all(|b| b.is_ascii_lowercase())
}

impl Dictionary {
    pub fn new(mut data: Data<'_>) -> Result<Self> {
        if data.entries.is_empty()
            || [
                data.entries.len(),
                data.word_entries.len(),
                data.phrases.len(),
                data.common.len(),
                data.pinyin.len(),
                data.word_pinyin.len(),
                data.details.len(),
                data.simplified.len(),
                data.traditional.len(),
            ]
            .iter()
            .any(|n| *n > MAX_ROWS)
        {
            return Err(Error("码表为空或数据行数超限"));
        }
        if data.common.iter().any(|c| c.is_control()) {
            return Err(Error("常用字集含控制字符"));
        }
        if data
            .entries
            .iter()
            .any(|entry| !valid_code(entry.code) || entry.character.is_control())
        {
            return Err(Error("单字编码或字符非法"));
        }
        for entry in &data.pinyin {
            if !valid_pinyin(entry.pinyin) || entry.character.is_control() {
                return Err(Error("拼音数据必须为规范化全拼"));
            }
        }
        for word in &data.word_pinyin {
            if !valid_pinyin(word.pinyin)
                || word.text.chars().count() < 2
                || word.text.chars().any(char::is_control)
            {
                return Err(Error("词的拼音数据非法"));
            }
        }
        let mut details = BTreeSet::new();
        for detail in &data.details {
            if detail.character.is_control()
                || !details.insert(detail.character)
                || detail.full_codes.iter().any(|s| !valid_code(s))
                || detail.decomposition.is_some_and(|s| {
                    s.is_empty() || s.len() > 1024 || s.chars().any(char::is_control)
                })
                || detail
                    .readings
                    .iter()
                    .any(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
            {
                return Err(Error("学习资料重复或格式非法"));
            }
        }
        drop(details);
        if data.phrases.iter().any(|phrase| {
            phrase.text.chars().count() < 2
                || phrase.text.chars().any(char::is_control)
                || !valid_code(phrase.code)
        }) || data
            .word_entries
            .iter()
            .any(|entry| !valid_code(entry.code) || entry.character.is_control())
        {
            return Err(Error("词条文本或编码非法"));
        }
        let phrase_count = data.phrases.len();
        // 同官方 tigress：单字表在前、词表在后，同权重保持此顺序。
        let word_buckets = buckets(
            data.word_entries
                .iter()
                .map(|e| (e.code, e.weight, Text::Character(e.character)))
                .chain(
                    data.phrases
                        .iter()
                        .map(|p| (p.code, p.weight, Text::Word(p.text))),
                ),
        )?;
        // 词桶不再借用解析记录；先释放它们，再建立临时行较多的学习资料表。
        drop(std::mem::take(&mut data.word_entries));
        drop(std::mem::take(&mut data.phrases));
        let learning = learning_table(&data)?;
        drop(data.details);
        let tiger_buckets = buckets(
            data.entries
                .iter()
                .map(|e| (e.code, e.weight, Text::Character(e.character))),
        )?;
        drop(data.entries);
        let pinyin_buckets = buckets(
            data.pinyin
                .iter()
                .map(|p| (p.pinyin, p.weight, Text::Character(p.character)))
                .chain(
                    data.word_pinyin
                        .iter()
                        .map(|w| (w.pinyin, w.weight, Text::Word(w.text))),
                ),
        )?;
        drop(data.pinyin);
        drop(data.word_pinyin);
        let mut word_order = Vec::with_capacity(phrase_count);
        for key in 0..word_buckets.keys.len() {
            for segment in word_buckets.value_segments(key) {
                if word_buckets.segment(segment).chars().nth(1).is_some() {
                    word_order.push(offset(segment)?);
                }
            }
        }
        // 桶已按码升序，稳定排序后同一个词的码仍升序。
        word_order.sort_by_key(|segment| word_buckets.segment(*segment as usize));
        let simplified = mappings(data.simplified)?;
        let traditional = mappings(data.traditional)?;
        // 在加载时拒绝过大的同码桶，不能在查询时截断并悄悄丢掉后部候选。
        // 每项至多展开 MAX_VARIANTS 个字形，够不到上限的桶不必逐项查表。
        let expansion = |text| {
            let forms = |table: &Table| table.get(text).map_or(1, |forms| forms.len());
            forms(&simplified).max(forms(&traditional))
        };
        if [&tiger_buckets, &pinyin_buckets, &word_buckets]
            .into_iter()
            .flat_map(Table::lists)
            .any(|bucket| {
                bucket.len() * MAX_VARIANTS > MAX_MATCHES
                    && bucket.map(expansion).sum::<usize>() > MAX_MATCHES
            })
        {
            return Err(Error("同码转换候选超过有界容量"));
        }
        let mut common = data.common;
        common.sort_unstable();
        common.dedup();
        Ok(Self {
            common,
            tiger_buckets,
            pinyin_buckets,
            word_buckets,
            word_order,
            learning,
            simplified,
            traditional,
        })
    }
    /// 词的全部词码，码升序；按 word_order 二分，不扫描词表。
    fn word_codes<'a>(&'a self, text: &str) -> impl ExactSizeIterator<Item = &'a str> + use<'a> {
        let word = |segment: &u32| self.word_buckets.segment(*segment as usize);
        let first = self
            .word_order
            .partition_point(|segment| word(segment) < text);
        let rest = &self.word_order[first..];
        let count = rest.partition_point(|segment| word(segment) == text);
        rest[..count]
            .iter()
            .map(|segment| self.word_buckets.key_of(*segment as usize))
    }
    /// 拼音反查的词要显示词码，这正是反查的目的；打词时的词候选不显示，同官方。
    fn word_comment(&self, mode: InputMode, text: &str) -> String {
        if mode != InputMode::Pinyin {
            return String::new();
        }
        let codes: Vec<_> = self.word_codes(text).collect();
        if codes.is_empty() {
            return String::new();
        }
        format!("〔{}〕", codes.join(" / "))
    }
    fn is_common(&self, character: char) -> bool {
        self.common.binary_search(&character).is_ok()
    }
    /// 常用字过滤同官方：词中每个汉字都须在常用字集。
    fn common_text(&self, text: &str) -> bool {
        text.chars().all(|c| !is_han(c) || self.is_common(c))
    }
    pub fn learning(&self, character: char) -> LearningInfo {
        let mut info = LearningInfo::default();
        let mut decomposition = Vec::new();
        let sections = [
            &mut info.codes,
            &mut info.full_codes,
            &mut decomposition,
            &mut info.readings,
        ];
        let mut section = 0;
        let values = self.learning.get(character.encode_utf8(&mut [0; 4]));
        for value in values.into_iter().flatten() {
            if value.is_empty() {
                section += 1;
            } else {
                sections[section].push(value.to_owned());
            }
        }
        info.decomposition = decomposition.pop();
        info
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
                .filter(|text| !options.common_only || text.chars().all(|c| self.is_common(c)))
                .map(str::to_owned)
                .collect(),
            (InputMode::Tiger, TypingMode::Word) => self
                .word_buckets
                .get(code)
                .into_iter()
                .flatten()
                .filter(|text| !options.common_only || self.common_text(text))
                .map(str::to_owned)
                .collect(),
            (InputMode::Pinyin, _) => self
                .pinyin_buckets
                .get(code)
                .into_iter()
                .flatten()
                .map(str::to_owned)
                .collect(),
        };
        let table = match options.output_form {
            OutputForm::Original => {
                // 原字模式的桶已在加载时去重，直接生成候选，避免每个按键重复分配去重树。
                return texts
                    .into_iter()
                    .map(|text| Candidate {
                        comment: self.word_comment(mode, &text),
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
            // 词码按转换前的词查，繁体模式下同样要看得到。
            let comment = self.word_comment(mode, &original);
            for text in forms(table, &original) {
                if seen.insert(text.clone()) {
                    result.push(Candidate {
                        source: single.filter(|_| text != original),
                        comment: comment.clone(),
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
                .map(str::to_owned)
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
            // 同一个词可能既是词码又是拼音，两处结果不一定相邻，保序去重。
            let mut seen = BTreeSet::new();
            values.retain(|text| seen.insert(text.clone()));
            values
        } else {
            let mut words = self
                .word_codes(query)
                .map(|_| query.to_owned())
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
                    if let Some(code) = self.word_codes(&text).next() {
                        learning.codes = vec![code.to_owned()];
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_whole_keys_and_keeps_value_order() {
        let mut table = Table::default();
        for (key, value) in [
            ("ab", "乙"),
            ("ab", "甲"),
            ("abc", ""),
            ("头发", "頭髮"),
            ("头发", ""),
            ("𠀀", "x"),
        ] {
            table.push(key, value).unwrap();
        }
        let get = |key| table.get(key).map(Iterator::collect::<Vec<_>>);
        assert_eq!(get("ab"), Some(vec!["乙", "甲"]));
        // 空段是合法的值，要和“键不存在”分开。
        assert_eq!(get("abc"), Some(vec![""]));
        assert_eq!(get("头发"), Some(vec!["頭髮", ""]));
        assert_eq!(get("𠀀"), Some(vec!["x"]));
        // 前缀和超长键都不算命中，值也不能被当成键。
        for missing in ["", "a", "abcd", "头", "甲"] {
            assert_eq!(get(missing), None, "{missing}");
        }
        assert_eq!(table.lists().map(|list| list.len()).sum::<usize>(), 6);
        assert!(Table::default().get("a").is_none());
    }

    #[test]
    fn word_codes_are_found_by_whole_word_in_code_order() {
        let phrase = |text, code, weight| Phrase { text, code, weight };
        let dictionary = Dictionary::new(Data {
            entries: vec![Entry {
                character: '头',
                code: "aa",
                weight: 1,
            }],
            word_entries: vec![Entry {
                character: '头',
                code: "tf",
                weight: 9,
            }],
            phrases: vec![
                phrase("头发", "tf", 5),
                phrase("头发型", "tf", 7),
                phrase("头发", "af", 1),
            ],
            ..Default::default()
        })
        .unwrap();
        let codes = |text| dictionary.word_codes(text).collect::<Vec<_>>();
        assert_eq!(codes("头发"), ["af", "tf"]);
        assert_eq!(codes("头发型"), ["tf"]);
        // 单字不进词序，词的前缀和表外文本都查不到。
        assert!(codes("头").is_empty());
        assert!(codes("头发型号").is_empty());
        assert_eq!(
            dictionary.lookup("头发", 0).unwrap().items[0]
                .learning
                .codes,
            ["af"]
        );
    }

    #[test]
    fn han_ranges_are_sorted_and_match_boundaries() {
        assert!(HAN_RANGES.iter().all(|(first, last)| first <= last));
        assert!(HAN_RANGES.windows(2).all(|pair| pair[0].1 < pair[1].0));
        for (first, last) in HAN_RANGES {
            for code in [first, last] {
                if let Some(character) = char::from_u32(code) {
                    assert!(is_han(character), "{code:x}");
                }
            }
        }
        // 范围间隙、私用区补充A（官方范围为空）和标点都不是汉字范围。
        for character in ['a', '，', '\u{2e7f}', '\u{33480}', '\u{f0000}', '\u{ffffd}'] {
            assert!(!is_han(character), "{character:?}");
        }
    }
    #[test]
    fn learning_sections_survive_missing_parts() {
        let dictionary = Dictionary::new(Data {
            entries: vec![
                Entry {
                    character: '甲',
                    code: "a",
                    weight: 1,
                },
                Entry {
                    character: '甲',
                    code: "ab",
                    weight: 1,
                },
            ],
            details: vec![
                Detail {
                    character: '甲',
                    full_codes: Box::new(["abcd"]),
                    decomposition: None,
                    readings: Box::new(["jiǎ", "jia"]),
                },
                Detail {
                    character: '乙',
                    full_codes: Box::default(),
                    decomposition: Some("乙 乙"),
                    readings: Box::default(),
                },
            ],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            dictionary.learning('甲'),
            LearningInfo {
                codes: vec!["a".into(), "ab".into()],
                full_codes: vec!["abcd".into()],
                decomposition: None,
                readings: vec!["jiǎ".into(), "jia".into()],
            }
        );
        assert_eq!(
            dictionary.learning('乙'),
            LearningInfo {
                decomposition: Some("乙 乙".into()),
                ..Default::default()
            }
        );
        assert_eq!(dictionary.learning('丙'), LearningInfo::default());
    }
}
