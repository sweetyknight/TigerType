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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Command {
    /// 宿主不读盘，选项只是占位；后台调度一律以其当前设置打开会话。
    Open {
        options: Options,
    },
    Key {
        key: Key,
        epoch: u64,
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
    /// 每个宿主单调递增，只报告取得/失去；光标与候选窗位置不出宿主进程。
    Focus {
        epoch: u64,
        focused: bool,
    },
    Clear,
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
    Opened(InputState),
    Key(KeyOutcome),
    Configured(InputState),
    Cleared(InputState),
    Focused(InputState),
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
