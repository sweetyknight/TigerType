//! 会话独占输入状态；设置变化只重算候选，不追溯修改或自动提交已有组合。
use crate::{Dictionary, Error, Result};
use tigertype_contracts::{
    Candidate, Composition, InputMode, InputState, Key, KeyOutcome, MAX_PINYIN_BYTES, Options,
    PAGE_SIZE,
};

pub struct Session<'a> {
    dictionary: &'a Dictionary,
    buffer: String,
    mode: InputMode,
    options: Options,
    ascii: bool,
    ascii_punctuation: bool,
    single_quote_open: bool,
    double_quote_open: bool,
    candidates: Vec<Candidate>,
    selected: usize,
    revision: u64,
    notice: String,
}
impl<'a> Session<'a> {
    pub fn new(dictionary: &'a Dictionary, options: Options) -> Self {
        Self {
            dictionary,
            buffer: String::new(),
            mode: InputMode::Tiger,
            options,
            ascii: false,
            ascii_punctuation: false,
            single_quote_open: false,
            double_quote_open: false,
            candidates: Vec::new(),
            selected: 0,
            revision: 1,
            notice: String::new(),
        }
    }
    fn advance(&mut self) -> Result<()> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(Error("会话快照编号耗尽"))?;
        Ok(())
    }
    fn rebuild(&mut self) {
        self.candidates = self
            .dictionary
            .candidates(&self.buffer, self.mode, self.options);
        self.selected = 0;
        if !self.buffer.is_empty() && self.candidates.is_empty() {
            self.notice = "暂无候选，可继续输入或退格".into();
        }
    }
    fn reset(&mut self) {
        self.buffer.clear();
        self.mode = InputMode::Tiger;
        self.candidates.clear();
        self.selected = 0;
    }
    pub fn clear(&mut self) -> Result<InputState> {
        self.advance()?;
        self.reset();
        self.notice.clear();
        Ok(self.snapshot())
    }
    pub fn configure(&mut self, options: Options) -> Result<InputState> {
        self.advance()?;
        self.options = options;
        self.notice.clear();
        self.rebuild();
        Ok(self.snapshot())
    }
    pub fn composing(&self) -> bool {
        !self.buffer.is_empty() || self.mode == InputMode::Pinyin
    }
    fn raw(&self) -> String {
        if self.mode == InputMode::Pinyin {
            format!("`{}", self.buffer)
        } else {
            self.buffer.clone()
        }
    }
    pub fn snapshot(&self) -> InputState {
        let page = self.selected / PAGE_SIZE;
        let show = self.options.show_learning || self.mode == InputMode::Pinyin;
        let candidates: Vec<_> = self
            .candidates
            .iter()
            .skip(page * PAGE_SIZE)
            .take(PAGE_SIZE)
            .enumerate()
            .map(|(index, candidate)| {
                let mut candidate = candidate.clone();
                candidate.label =
                    ["㊀", "㊁", "㊂", "㊃", "㊄", "㊅", "㊆", "㊇", "㊈"][index].into();
                if show {
                    let info = self
                        .dictionary
                        .learning(candidate.text.chars().next().unwrap());
                    candidate.comment = if info.codes.is_empty() {
                        "未收录虎码".into()
                    } else {
                        info.codes.join(" / ")
                    };
                    candidate.learning = Some(info);
                }
                if let Some(source) = candidate.source {
                    candidate.comment.push_str(&format!("〔由{source}转换〕"));
                }
                candidate
            })
            .collect();
        let text = self.raw();
        InputState {
            composition: Composition {
                cursor: text.len(),
                selection: 0..text.len(),
                text,
            },
            highlighted: (!candidates.is_empty()).then_some(self.selected % PAGE_SIZE),
            candidates,
            page_size: PAGE_SIZE,
            page_index: page,
            is_last_page: (page + 1) * PAGE_SIZE >= self.candidates.len(),
            commit_preview: self
                .candidates
                .get(self.selected)
                .map(|c| c.text.clone())
                .unwrap_or_default(),
            mode: self.mode,
            options: self.options,
            ascii_mode: self.ascii,
            ascii_punctuation: self.ascii_punctuation,
            composing: self.composing(),
            revision: self.revision,
            notice: self.notice.clone(),
        }
    }
    fn commit(&mut self, index: usize) -> Option<String> {
        if let Some(value) = self.candidates.get(index) {
            let text = value.text.clone();
            self.reset();
            Some(text)
        } else {
            self.notice = "当前没有对应候选，请退格或清屏".into();
            None
        }
    }
    pub fn select(&mut self, revision: u64, index: u8) -> Result<KeyOutcome> {
        if revision != self.revision {
            return Err(Error("候选快照已经过期"));
        }
        if index as usize >= PAGE_SIZE
            || self.selected / PAGE_SIZE * PAGE_SIZE + index as usize >= self.candidates.len()
        {
            return Err(Error("候选索引越界"));
        }
        self.advance()?;
        self.notice.clear();
        let commit = self.commit(self.selected / PAGE_SIZE * PAGE_SIZE + index as usize);
        Ok(KeyOutcome {
            handled: true,
            commit,
            state: self.snapshot(),
        })
    }
    pub fn process(&mut self, key: Key) -> Result<KeyOutcome> {
        self.advance()?;
        self.notice.clear();
        let mut handled = true;
        let mut commit = None;
        match key {
            Key::ToggleAscii => {
                if self.composing() {
                    commit = Some(self.raw());
                    self.reset();
                }
                self.ascii = !self.ascii;
            }
            Key::ToggleLearning => self.options.show_learning = !self.options.show_learning,
            Key::TogglePunctuation => self.ascii_punctuation = !self.ascii_punctuation,
            _ if self.ascii => handled = false,
            Key::Escape | Key::Tab => {
                handled = self.composing();
                self.reset();
            }
            Key::Backspace => {
                handled = self.composing();
                if self.buffer.pop().is_none() {
                    self.mode = InputMode::Tiger;
                }
                self.rebuild();
            }
            Key::Enter => {
                handled = self.composing();
                if handled {
                    commit = Some(self.raw());
                    self.reset();
                }
            }
            Key::Space => {
                handled = self.composing();
                if handled {
                    commit = self.commit(self.selected);
                }
            }
            Key::Previous | Key::Next => {
                handled = self.composing();
                if !self.candidates.is_empty() {
                    self.selected = if key == Key::Previous {
                        self.selected.saturating_sub(1)
                    } else {
                        (self.selected + 1).min(self.candidates.len() - 1)
                    };
                }
            }
            Key::PageUp | Key::PageDown => self.page(key == Key::PageDown, &mut handled),
            Key::Character(c) => {
                if c == '`' {
                    if !self.composing() {
                        self.mode = InputMode::Pinyin;
                    } else {
                        self.notice = "请先确认或清空当前编码".into();
                    }
                } else if self.composing() && ('1'..='9').contains(&c) {
                    commit = self.commit(
                        self.selected / PAGE_SIZE * PAGE_SIZE + (c as usize - '1' as usize),
                    );
                } else if self.composing() && (c == ';' || c == '\'') {
                    commit = self.commit(
                        self.selected / PAGE_SIZE * PAGE_SIZE + if c == ';' { 1 } else { 2 },
                    );
                } else if self.composing() && (c == '-' || c == '=') {
                    self.page(c == '=', &mut handled);
                } else if c.is_ascii_lowercase() || (self.mode == InputMode::Pinyin && c == 'ü') {
                    if self.mode == InputMode::Tiger && self.buffer.len() == 4 {
                        if self.candidates.is_empty() {
                            self.notice = "空码已满，请退格或清屏；本键未加入".into();
                            return Ok(KeyOutcome {
                                handled,
                                commit,
                                state: self.snapshot(),
                            });
                        }
                        commit = self.commit(self.selected);
                    }
                    if self.mode == InputMode::Pinyin && self.buffer.len() >= MAX_PINYIN_BYTES {
                        self.notice = "拼音长度已满，请退格或清屏；本键未加入".into();
                    } else {
                        self.buffer.push(if c == 'ü' { 'v' } else { c });
                        self.rebuild();
                        if self.mode == InputMode::Tiger
                            && self.options.auto_commit_four
                            && self.buffer.len() == 4
                            && self.candidates.len() == 1
                        {
                            let automatic = self.commit(0).unwrap();
                            if let Some(text) = commit.as_mut() {
                                text.push_str(&automatic);
                            } else {
                                commit = Some(automatic);
                            }
                        }
                    }
                } else if !c.is_control() {
                    let punctuation = if self.ascii_punctuation {
                        None
                    } else {
                        match c {
                            '\'' => Some(if self.single_quote_open { "’" } else { "‘" }.into()),
                            '"' => Some(if self.double_quote_open { "”" } else { "“" }.into()),
                            _ => punctuation(c),
                        }
                    };
                    if self.composing() {
                        if let Some(mut text) = self.commit(self.selected) {
                            text.push_str(&punctuation.unwrap_or_else(|| c.to_string()));
                            commit = Some(text);
                        }
                    } else if let Some(text) = punctuation {
                        commit = Some(text);
                    } else {
                        handled = false;
                    }
                    // 只在标点确实提交后推进配对；选第三候选和空码不会污染引号状态。
                    if commit.is_some() && !self.ascii_punctuation {
                        match c {
                            '\'' => self.single_quote_open = !self.single_quote_open,
                            '"' => self.double_quote_open = !self.double_quote_open,
                            _ => {}
                        }
                    }
                } else {
                    handled = false;
                }
            }
        }
        Ok(KeyOutcome {
            handled,
            commit,
            state: self.snapshot(),
        })
    }
    fn page(&mut self, down: bool, handled: &mut bool) {
        *handled = self.composing();
        if self.candidates.is_empty() {
            self.notice = "暂无可翻页的候选".into();
            return;
        }
        let current = self.selected / PAGE_SIZE;
        let last = (self.candidates.len() - 1) / PAGE_SIZE;
        let next = if down {
            (current + 1).min(last)
        } else {
            current.saturating_sub(1)
        };
        self.selected = next * PAGE_SIZE;
        if current == next {
            self.notice = "已到候选页边界".into();
        }
    }
}

/// 基础标点为明确映射，不解释旧 Rime 符号脚本。
fn punctuation(c: char) -> Option<String> {
    Some(
        match c {
            ',' => "，",
            '.' => "。",
            '?' => "？",
            '!' => "！",
            ':' => "：",
            ';' => "；",
            '(' => "（",
            ')' => "）",
            '<' => "《",
            '>' => "》",
            '[' => "【",
            ']' => "】",
            '\\' => "、",
            '_' => "——",
            '^' => "……",
            _ => return None,
        }
        .into(),
    )
}
