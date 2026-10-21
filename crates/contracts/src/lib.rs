//! 跨模块只传递拥有所有权的快照，不暴露 Rime 内存或 Windows 对象。

use std::ops::Range;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Composition {
    pub text: String,
    /// Rime 使用 UTF-8 字节位置；TSF 接入时再显式转换为 UTF-16。
    pub cursor: usize,
    pub selection: Range<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Candidate {
    pub text: String,
    pub comment: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputState {
    pub composition: Composition,
    pub candidates: Vec<Candidate>,
    pub page_size: usize,
    pub page_index: usize,
    pub is_last_page: bool,
    pub highlighted: Option<usize>,
    pub commit_preview: String,
    pub schema_id: String,
    pub schema_name: String,
    pub ascii_mode: bool,
    pub composing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyOutcome {
    /// 未消费的按键交由宿主继续处理，不能把它当成调用失败。
    pub handled: bool,
    pub commit: Option<String>,
    pub state: InputState,
}
