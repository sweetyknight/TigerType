//! 模块间只传递拥有所有权的数据，逻辑按键与平台键码无关。
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::ops::Range;
pub mod candidate;
pub mod protocol;
pub mod ui;

pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_CANDIDATES: usize = 9;
pub const PAGE_SIZE: usize = 9;
pub const MAX_QUERY_CHARS: usize = 64;
pub const MAX_PINYIN_BYTES: usize = 6;
pub const MAX_LEARNING_ITEMS: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Composition {
    pub text: String,
    /// 内部位置使用 UTF-8 字节，TSF 边界再转换 UTF-16。
    pub cursor: usize,
    pub selection: Range<usize>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputForm {
    #[default]
    Original,
    Simplified,
    Traditional,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    #[default]
    Tiger,
    Pinyin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub auto_commit_four: bool,
    pub common_only: bool,
    pub show_learning: bool,
    pub output_form: OutputForm,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            auto_commit_four: false,
            common_only: true,
            show_learning: false,
            output_form: OutputForm::Original,
        }
    }
}

/// 修饰键组合由 TSF 转成动作；无对应动作的系统快捷键保持透传。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Key {
    Character(char),
    Space,
    Enter,
    Backspace,
    Escape,
    Tab,
    PageUp,
    PageDown,
    Previous,
    Next,
    ToggleAscii,
    ToggleLearning,
    TogglePunctuation,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningInfo {
    pub codes: Vec<String>,
    /// 资料中的完整码未必是可输入码，UI 必须与 codes 分开展示。
    pub full_codes: Vec<String>,
    pub decomposition: Option<String>,
    pub readings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub text: String,
    pub comment: String,
    pub label: String,
    pub source: Option<char>,
    pub learning: Option<LearningInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputState {
    pub composition: Composition,
    pub candidates: Vec<Candidate>,
    pub page_size: usize,
    pub page_index: usize,
    pub is_last_page: bool,
    pub highlighted: Option<usize>,
    pub commit_preview: String,
    pub mode: InputMode,
    pub options: Options,
    pub ascii_mode: bool,
    pub composing: bool,
    pub revision: u64,
    /// 空码、越界等可恢复情况对用户可见，不能伪装成成功选字。
    pub notice: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyOutcome {
    pub handled: bool,
    pub commit: Option<String>,
    pub state: InputState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupItem {
    pub character: char,
    pub learning: LearningInfo,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupPage {
    pub items: Vec<LookupItem>,
    pub has_more: bool,
}
