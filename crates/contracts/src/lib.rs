//! 跨模块只传递拥有所有权的快照，不暴露 Rime 内存或 Windows 对象。

use serde::{Deserialize, Serialize};
use std::ops::Range;

pub mod protocol;

// 引擎复制与通信校验共用边界，避免两端对同一快照接受条件不同。
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_CANDIDATES: usize = 100;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Composition {
    pub text: String,
    /// Rime 使用 UTF-8 字节位置；TSF 接入时再显式转换为 UTF-16。
    pub cursor: usize,
    pub selection: Range<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub text: String,
    pub comment: String,
    pub label: String,
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
    pub schema_id: String,
    pub schema_name: String,
    pub ascii_mode: bool,
    pub composing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyOutcome {
    /// 未消费的按键交由宿主继续处理，不能把它当成调用失败。
    pub handled: bool,
    pub commit: Option<String>,
    pub state: InputState,
}
