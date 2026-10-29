//! 会话命令与回复：各平台前端与后台调度共用；跨进程帧、握手/管理信封与协议版本属于 ipc。
use crate::{InputState, Key, KeyOutcome, Options};
use serde::{Deserialize, Serialize};

/// 后台 UI 线程可唤起的窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Panel {
    Lookup,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub generation: u64,
    pub request_id: u64,
    pub session_id: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub route: Route,
    pub command: Command,
}

/// 按键的前置状态转换与本键在同一次后台调度中执行；开会话/焦点/清空没有单独命令。
/// 后台一律以当前设置开会话，宿主不读盘、也不携带选项；焦点代次每宿主单调递增，光标与候选窗位置不出宿主进程。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Prepare {
    Keep,
    /// 新会话，同时取得焦点。
    Open,
    /// 重新取得焦点，同时清空本会话组合。
    Focus,
    /// 只清空当前焦点会话的组合。
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Command {
    Key {
        key: Key,
        epoch: u64,
        prepare: Prepare,
    },
    /// 索引针对当前页，并绑定快照，禁止旧候选点击产生提交。
    Select {
        revision: u64,
        index: u8,
        epoch: u64,
    },
    Configure {
        options: Options,
    },
    Close,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub route: Route,
    pub reply: Reply,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Reply {
    Key(KeyOutcome),
    Configured(InputState),
    Closed,
    Rejected(Rejection),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rejection {
    StaleGeneration,
    OutOfOrder,
    SessionReused,
    UnknownSession,
    SessionLimit,
    Unavailable,
    StaleSnapshot,
    UnknownHost,
    NotFocused,
    StaleFocus,
}
