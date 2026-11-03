//! 启动时验证并建立有序索引；按键路径只访问对应键的有界切片。
use crate::{Error, Result};
use std::collections::{BTreeSet, HashMap, HashSet};
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
/// 官方拼音滤镜（pinyin.json）的词读音：带调全拼，音节多连写，偶有隔音符、连字符、制表符，
/// 或以“/”接又读。
#[derive(Debug, Clone)]
pub struct WordReading<'a> {
    pub text: &'a str,
    pub reading: &'a str,
}
#[derive(Debug, Clone)]
pub struct Detail<'a> {
    pub character: char,
    // 解析后不再追加，精确长度切片避免每字保留 Vec 的增长余量。
    pub full_codes: Box<[&'a str]>,
    pub decomposition: Option<&'a str>,
    pub readings: Box<[&'a str]>,
}
/// 一个字根及其大码+小码。
#[derive(Debug, Clone)]
pub struct Root<'a> {
    pub character: char,
    pub code: &'a str,
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
    /// 查字在拆分字根下显示编码用。
    pub roots: Vec<Root<'a>>,
    /// 官方 PYPhrases 词读音，查字给词注音用。
    pub word_readings: Vec<WordReading<'a>>,
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
    // 字根 → 编码，见 root_table。
    roots: Table,
    // 词 → 读音，只存与各字首个读音不同的词，见 reading_table。
    readings: Table,
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
/// 字根 → 大码+小码。数据已覆盖拆分用到的全部字根（官网字根表加拆分查询补充），
/// 与取码规则的一致性由完整资源验收核对；同一字根两种编码说明数据坏了，拒绝加载。
fn root_table(data: &Data<'_>) -> Result<Table> {
    let mut roots: Vec<_> = data
        .roots
        .iter()
        .map(|root| (root.character, root.code))
        .collect();
    roots.sort_unstable();
    roots.dedup();
    if roots.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(Error("字根表同一字根编码矛盾"));
    }
    let mut table = Table::default();
    let mut buffer = [0; 4];
    for (root, code) in roots {
        table.push(root.encode_utf8(&mut buffer), code)?;
    }
    Ok(table.finish())
}
/// 带调拼音字母去调、转小写，ü 记作 v，与 PY_c 的无调全拼同形。
fn bare(c: char) -> char {
    // 建表时对每个音节字母都要调用，绝大多数是 ASCII，免去 Unicode 小写表查找。
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    match c.to_lowercase().next().unwrap_or(c) {
        'ā' | 'á' | 'ǎ' | 'à' => 'a',
        'ē' | 'é' | 'ě' | 'è' | 'ê' => 'e',
        'ī' | 'í' | 'ǐ' | 'ì' => 'i',
        'ō' | 'ó' | 'ǒ' | 'ò' => 'o',
        'ū' | 'ú' | 'ǔ' | 'ù' => 'u',
        'ü' | 'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' => 'v',
        'ń' | 'ň' | 'ǹ' => 'n',
        'ḿ' => 'm',
        c => c,
    }
}
/// 官方词读音里音节之间可能出现的分隔符。
fn separator(c: char) -> bool {
    matches!(c, '\'' | '’' | '-' | ',' | '/' | '\t' | ' ')
}
/// 从 text[at..] 起先跳过分隔符，再按无调形逐字对上 syllable，返回它在 text 中的范围。
fn take(text: &str, at: usize, syllable: &str) -> Option<std::ops::Range<usize>> {
    let start = at + text[at..].find(|c| !separator(c))?;
    let mut end = start;
    let mut rest = text[start..].chars();
    for want in syllable.chars() {
        let got = rest.next().filter(|got| bare(*got) == bare(want))?;
        end += got.len_utf8();
    }
    Some(start..end)
}
/// 学习资料表里某字的读音段（第三个空串之后）。
fn char_readings(learning: &Table, character: char) -> impl Iterator<Item = &str> {
    let mut gaps = 0;
    let values = learning.get(character.encode_utf8(&mut [0; 4]));
    values.into_iter().flatten().filter(move |value| {
        gaps += usize::from(value.is_empty());
        gaps == 3 && !value.is_empty()
    })
}
/// 建表期间各字的读音，按字缓存：13 万词反复查同一批字，学习资料表每次都要二分并跳段。
type Sounds<'t> = HashMap<char, Vec<&'t str>>;
fn sounds_of<'s, 't>(sounds: &'s Sounds<'t>, character: char) -> &'s [&'t str] {
    sounds.get(&character).map_or(&[], Vec::as_slice)
}
/// 把官方整段读音按各字读音的无调形切成逐字音节，保留原文声调与大小写；
/// 末尾以“/”起头的又读舍去。对不上（如儿化 r）为 None。
fn split<'r>(text: &'r str, chars: &[char], sounds: &Sounds<'_>) -> Option<Vec<&'r str>> {
    fn go<'r>(
        text: &'r str,
        at: usize,
        chars: &[char],
        sounds: &Sounds<'_>,
        out: &mut Vec<&'r str>,
    ) -> bool {
        let Some((&first, rest)) = chars.split_first() else {
            let tail = text[at..].trim_start_matches(['\t', ' ']);
            return tail.is_empty() || tail.starts_with('/');
        };
        for reading in sounds_of(sounds, first) {
            if let Some(range) = take(text, at, reading) {
                out.push(&text[range.clone()]);
                if go(text, range.end, rest, sounds, out) {
                    return true;
                }
                out.pop();
            }
        }
        false
    }
    let mut out = Vec::with_capacity(chars.len());
    go(text, 0, chars, sounds, &mut out).then_some(out)
}
/// 词的一段：官方片段只有一串音节；单字的每个读音各是一个候选。
enum Part<'r> {
    Official(Vec<&'r str>),
    Character(&'r [&'r str]),
}
impl<'r> Part<'r> {
    fn first(&self) -> &[&'r str] {
        match self {
            Part::Official(syllables) => syllables,
            Part::Character(readings) => &readings[..1],
        }
    }
}
/// 各段依次对上词的无调全拼，每段取第一个对得上的候选；整串对上才算。
fn fit<'r>(pinyin: &str, at: usize, parts: &[Part<'r>], out: &mut Vec<&'r str>) -> bool {
    let Some((first, rest)) = parts.split_first() else {
        return at == pinyin.len();
    };
    let attempt = |syllables: &[&'r str], out: &mut Vec<&'r str>| {
        let mut end = at;
        for syllable in syllables {
            match take(pinyin, end, syllable) {
                Some(range) => end = range.end,
                None => return false,
            }
        }
        let len = out.len();
        out.extend_from_slice(syllables);
        fit(pinyin, end, rest, out) || {
            out.truncate(len);
            false
        }
    };
    match first {
        Part::Official(syllables) => attempt(syllables, out),
        Part::Character(readings) => readings
            .iter()
            .any(|reading| attempt(std::slice::from_ref(reading), out)),
    }
}
/// 词 → 读音（逐字音节以空格分隔）。同官方拼音滤镜 pinyin.json：按 PYPhrases 正向最长匹配分段，
/// 命中段用官方词读音，其余各字用单字读音；再用 PY_c 的无调全拼筛掉音节对不上的读音，
/// 仍只差声调的取单字表首个读音。结果与“各字首个读音”相同的词不存，查询时现拼；
/// 有字没有读音、拼不出的词也不存，显示缺资料。words 须按词排序。
fn reading_table<'w>(
    official: Vec<WordReading<'_>>,
    pinyin: &[WordPinyin<'_>],
    learning: &Table,
    words: impl Iterator<Item = &'w str>,
) -> Result<Table> {
    let longest = official
        .iter()
        .map(|word| word.text.chars().count())
        .max()
        .unwrap_or(0);
    let official: HashMap<&str, &str> = official
        .into_iter()
        .map(|word| (word.text, word.reading))
        .collect();
    let mut by_word: Vec<&WordPinyin> = pinyin.iter().collect();
    by_word.sort_by_key(|word| word.text);
    let mut words: Vec<&str> = words.collect();
    words.dedup();
    let mut sounds = Sounds::new();
    for character in words.iter().flat_map(|word| word.chars()) {
        sounds
            .entry(character)
            .or_insert_with(|| char_readings(learning, character).collect());
    }
    let mut table = Table::default();
    let (mut chars, mut offsets, mut chosen) = (Vec::new(), Vec::new(), Vec::new());
    let mut cursor = 0;
    for word in words {
        chars.clear();
        chars.extend(word.chars());
        offsets.clear();
        offsets.extend(word.char_indices().map(|(index, _)| index));
        offsets.push(word.len());
        let mut parts = Vec::with_capacity(chars.len());
        let mut start = 0;
        while start < chars.len() {
            let most = longest.min(chars.len() - start);
            let found = (2..=most)
                .rev()
                .find_map(|n| Some((n, *official.get(&word[offsets[start]..offsets[start + n]])?)));
            if let Some((n, reading)) = found {
                let syllables =
                    split(reading, &chars[start..start + n], &sounds).unwrap_or_else(|| {
                        reading.split(separator).filter(|s| !s.is_empty()).collect()
                    });
                parts.push(Part::Official(syllables));
                start += n;
            } else {
                let readings = sounds_of(&sounds, chars[start]);
                if readings.is_empty() {
                    break;
                }
                parts.push(Part::Character(readings));
                start += 1;
            }
        }
        if start < chars.len() {
            continue;
        }
        // 两边都按词排序，游标只进不退，不必每词二分。
        while by_word
            .get(cursor)
            .is_some_and(|candidate| candidate.text < word)
        {
            cursor += 1;
        }
        let fitted = by_word[cursor..]
            .iter()
            .take_while(|candidate| candidate.text == word)
            .any(|candidate| {
                chosen.clear();
                fit(candidate.pinyin, 0, &parts, &mut chosen)
            });
        if !fitted {
            chosen.clear();
            chosen.extend(parts.iter().flat_map(Part::first));
        }
        // 与查询时现拼的默认读音（各字首个读音）相同就不存。
        let default = chosen.len() == chars.len()
            && chars
                .iter()
                .zip(&chosen)
                .all(|(c, syllable)| sounds_of(&sounds, *c).first() == Some(syllable));
        if !default {
            table.push(word, &chosen.join(" "))?;
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
                data.roots.len(),
                data.word_readings.len(),
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
                // 拆分每个字符就是一个字根，空白会被当成字根，也会和词的逐字分隔混淆。
                || detail.decomposition.is_some_and(|s| {
                    s.is_empty()
                        || s.len() > 1024
                        || s.chars().any(|c| c.is_control() || c.is_whitespace())
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
        if data.roots.iter().any(|root| {
            root.code.len() != 2
                || !valid_code(root.code)
                || root.character.is_control()
                || root.character.is_whitespace()
        }) {
            return Err(Error("字根表编码或字根非法"));
        }
        if data.word_readings.iter().any(|word| {
            word.text.chars().count() < 2
                || word.text.chars().any(char::is_control)
                // 只有分隔符的读音切不出音节，会显示成空读音。
                || !word.reading.chars().any(char::is_alphabetic)
                || word.reading.len() > 256
                || word.reading.chars().any(|c| c.is_control() && c != '\t')
        }) {
            return Err(Error("词读音数据非法"));
        }
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
        let learning = learning_table(&data)?;
        let roots = root_table(&data)?;
        drop(data.details);
        drop(data.roots);
        // 词读音要用词的无调全拼筛音节，须在拼音桶消费 word_pinyin 之前建。
        let readings = reading_table(
            std::mem::take(&mut data.word_readings),
            &data.word_pinyin,
            &learning,
            word_order
                .iter()
                .map(|segment| word_buckets.segment(*segment as usize)),
        )?;
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
            roots,
            readings,
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
                .map(|text| self.lookup_item(text))
                .collect(),
        })
    }
    /// 单字给全部资料。词本身只有词码：完整码、读音与拆分逐字拼起，任一字缺就整项标缺，
    /// 不拿首字资料冒充整词；读音不按词猜多音字，各字读音全列（多音以“/”分隔）。
    fn lookup_item(&self, text: String) -> LookupItem {
        let mut infos: Vec<_> = text.chars().map(|c| self.learning(c)).collect();
        let roots = infos
            .iter()
            .map(|info| {
                info.decomposition
                    .iter()
                    .flat_map(|parts| parts.chars())
                    .map(|root| (root, self.root_code(root).unwrap_or_default().to_owned()))
                    .collect()
            })
            .collect();
        let word = self.word_codes(&text).next();
        let learning = match (word, infos.len()) {
            (None, 1) => infos.pop().unwrap_or_default(),
            _ => LearningInfo {
                readings: self.word_reading(&text, &infos).into_iter().collect(),
                ..Self::spell(word, &infos)
            },
        };
        LookupItem {
            text,
            learning,
            roots,
        }
    }
    fn spell(word: Option<&str>, infos: &[LearningInfo]) -> LearningInfo {
        let each = |part: fn(&LearningInfo) -> Option<String>| {
            infos.iter().map(part).collect::<Option<Vec<_>>>()
        };
        LearningInfo {
            codes: word.into_iter().map(str::to_owned).collect(),
            full_codes: each(|i| i.full_codes.first().cloned()).unwrap_or_default(),
            decomposition: each(|i| i.decomposition.clone()).map(|parts| parts.join(" ")),
            readings: Vec::new(),
        }
    }
    /// 词读音：建表时存了就用，否则各字首个读音现拼（与建表时比较的默认一致）；有字缺读音为 None。
    fn word_reading(&self, text: &str, infos: &[LearningInfo]) -> Option<String> {
        if let Some(mut stored) = self.readings.get(text) {
            return stored.next().map(str::to_owned);
        }
        let first: Option<Vec<_>> = infos.iter().map(|info| info.readings.first()).collect();
        first.map(|readings| {
            readings
                .into_iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
    }
    /// 字根的大码+小码。
    pub fn root_code(&self, root: char) -> Option<&str> {
        self.roots.get(root.encode_utf8(&mut [0; 4]))?.next()
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
                    decomposition: Some("乙乙"),
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
                decomposition: Some("乙乙".into()),
                ..Default::default()
            }
        );
        assert_eq!(dictionary.learning('丙'), LearningInfo::default());
    }

    fn detail<'a>(character: char, parts: &'a str, code: &'a str) -> Detail<'a> {
        Detail {
            character,
            full_codes: Box::new([code]),
            decomposition: Some(parts),
            readings: Box::default(),
        }
    }
    #[test]
    fn root_codes_come_from_the_data_and_contradictions_are_rejected() {
        let root = |character, code| Root { character, code };
        let data = |roots| Data {
            entries: vec![Entry {
                character: '你',
                code: "jx",
                weight: 1,
            }],
            details: vec![detail('你', "亻\u{e4e4}小", "jxyx")],
            roots,
            ..Default::default()
        };
        // 同一字根重复列出、编码相同（字根表与拆分查询补充重叠）照常接受。
        let dictionary = Dictionary::new(data(vec![
            root('亻', "jr"),
            root('\u{e4e4}', "xd"),
            root('小', "yx"),
            root('亻', "jr"),
        ]))
        .unwrap();
        assert_eq!(dictionary.root_code('亻'), Some("jr"));
        assert_eq!(dictionary.root_code('丑'), None);
        assert_eq!(
            dictionary.lookup("你", 0).unwrap().items[0].roots,
            [vec![
                ('亻', "jr".to_owned()),
                ('\u{e4e4}', "xd".into()),
                ('小', "yx".into())
            ]]
        );
        // 反例：同一字根两种编码、空白字根或非两码都拒绝加载。
        for roots in [
            vec![root('亻', "jr"), root('亻', "jx")],
            vec![root(' ', "ab")],
            vec![root('丁', "a")],
            vec![root('丁', "abc")],
            vec![root('丁', "aB")],
        ] {
            assert!(Dictionary::new(data(roots.clone())).is_err(), "{roots:?}");
        }
    }
    #[test]
    fn word_lookup_spells_each_character_without_borrowing_the_first_one() {
        let dictionary = Dictionary::new(Data {
            entries: vec![Entry {
                character: '你',
                code: "jx",
                weight: 1,
            }],
            word_entries: vec![Entry {
                character: '你',
                code: "jx",
                weight: 1,
            }],
            phrases: vec![
                Phrase {
                    text: "你好",
                    code: "jxbh",
                    weight: 1,
                },
                Phrase {
                    text: "你们",
                    code: "jxja",
                    weight: 1,
                },
            ],
            details: vec![
                Detail {
                    readings: Box::new(["nǐ"]),
                    ..detail('你', "亻\u{e4e4}小", "jxyx")
                },
                Detail {
                    readings: Box::new(["hǎo", "hào"]),
                    ..detail('好', "女子", "bhi")
                },
            ],
            roots: vec![
                Root {
                    character: '女',
                    code: "bn",
                },
                Root {
                    character: '子',
                    code: "hi",
                },
            ],
            ..Default::default()
        })
        .unwrap();
        let item = |text: &str| dictionary.lookup(text, 0).unwrap().items.remove(0);
        let word = item("你好");
        assert_eq!(
            word.learning,
            LearningInfo {
                codes: vec!["jxbh".into()],
                full_codes: vec!["jxyx".into(), "bhi".into()],
                decomposition: Some("亻\u{e4e4}小 女子".into()),
                // 没有词读音资料时取各字首个读音，不把多音字全列。
                readings: vec!["nǐ hǎo".into()],
            }
        );
        assert_eq!(word.roots.len(), 2);
        assert_eq!(
            word.roots[1],
            [('女', "bn".to_owned()), ('子', "hi".into())]
        );
        // 数据里没有的字根编码为空，不现场猜。
        assert_eq!(word.roots[0][0], ('亻', String::new()));
        // 任一字缺资料就整项标缺，不拿首字冒充整词。
        let partial = item("你们");
        assert_eq!(partial.learning.codes, ["jxja"]);
        assert!(partial.learning.full_codes.is_empty());
        assert!(partial.learning.readings.is_empty());
        assert_eq!(partial.learning.decomposition, None);
        assert_eq!(partial.roots[1], []);
        // 单字照旧给本字全部资料。
        assert_eq!(item("好").learning.readings, ["hǎo", "hào"]);
    }
    #[test]
    fn word_readings_follow_the_official_filter_then_toneless_pinyin_then_first_reading() {
        let say = |character, readings: &'static [&'static str]| Detail {
            character,
            full_codes: Box::new(["a"]),
            decomposition: None,
            readings: readings.into(),
        };
        let words = [
            "你好",
            "银行",
            "银行家",
            "行列",
            "几个",
            "一会儿",
            "忽闪",
            "令爱",
            "姘头",
            "你㐀",
        ];
        let dictionary = Dictionary::new(Data {
            entries: vec![Entry {
                character: '你',
                code: "a",
                weight: 1,
            }],
            phrases: words
                .iter()
                .map(|text| Phrase {
                    text,
                    code: "ab",
                    weight: 1,
                })
                .collect(),
            details: vec![
                say('你', &["nǐ"]),
                say('好', &["hǎo", "hào"]),
                say('银', &["yín"]),
                say('行', &["xíng", "háng"]),
                say('家', &["jiā", "jia"]),
                say('列', &["liè"]),
                say('几', &["jī", "jǐ"]),
                say('个', &["gè", "gě"]),
                say('一', &["yī"]),
                say('会', &["huì", "kuài"]),
                say('儿', &["ér"]),
                say('忽', &["hū"]),
                say('闪', &["shǎn"]),
                say('令', &["lìng"]),
                say('爱', &["ài"]),
                say('姘', &["pīn"]),
                say('头', &["tóu", "tou"]),
            ],
            word_readings: [
                ("你好", "nǐhǎo"),
                ("银行", "yínháng"),
                // 儿化切不开，整段原样；“/”后是又读，隔音符不进显示。
                ("一会儿", "yīhuìr"),
                ("忽闪", "hūshǎn/hūshan"),
                ("令爱", "lìng'ài"),
                // 原件个别行在读音里夹制表符。
                ("姘头", "pīn\ttou"),
            ]
            .into_iter()
            .map(|(text, reading)| WordReading { text, reading })
            .collect(),
            word_pinyin: [("行列", "hanglie"), ("几个", "jige")]
                .into_iter()
                .map(|(text, pinyin)| WordPinyin {
                    text,
                    pinyin,
                    weight: 1,
                })
                .collect(),
            ..Default::default()
        })
        .unwrap();
        let reading = |text: &str| {
            dictionary.lookup(text, 0).unwrap().items[0]
                .learning
                .readings
                .clone()
        };
        // 官方整词读音逐字切开；词里的官方片段照用，其余字取单字读音。
        assert_eq!(reading("你好"), ["nǐ hǎo"]);
        assert_eq!(reading("银行"), ["yín háng"]);
        assert_eq!(reading("银行家"), ["yín háng jiā"]);
        assert_eq!(reading("一会儿"), ["yīhuìr"]);
        assert_eq!(reading("忽闪"), ["hū shǎn"]);
        assert_eq!(reading("令爱"), ["lìng ài"]);
        // 官方轻声保留，不被单字读音改回本调。
        assert_eq!(reading("姘头"), ["pīn tou"]);
        // 无调全拼筛掉音节不符的首个读音；只差声调时仍取首个读音（已知局限：几个应为 jǐ）。
        assert_eq!(reading("行列"), ["háng liè"]);
        assert_eq!(reading("几个"), ["jī gè"]);
        // 有字没有读音：拼不出就标缺，不拿其余字凑。
        assert!(reading("你㐀").is_empty());
        // 与各字首个读音相同的词（你好、忽闪、令爱、几个）不存。
        assert_eq!(dictionary.readings.keys.len(), 5);
        // 反例：空读音、只有分隔符的读音和单字键都拒绝。
        for (text, reading) in [("你好", ""), ("你好", "'-/"), ("你", "nǐ")] {
            let data = Data {
                entries: vec![Entry {
                    character: '你',
                    code: "a",
                    weight: 1,
                }],
                word_readings: vec![WordReading { text, reading }],
                ..Default::default()
            };
            assert!(Dictionary::new(data).is_err(), "{text} {reading:?}");
        }
    }
}
