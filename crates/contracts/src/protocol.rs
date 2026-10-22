//! v4 增加跨进程握手和独立宿主通知；旧协议直接拒绝，不保留兼容分支。
use crate::{InputState, Key, KeyOutcome, Options};
use serde::{Deserialize, Serialize};
pub const VERSION: u16 = 4;

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
    /// 每个宿主单调递增；None 表示失焦，矩形是屏幕物理像素而非窗口句柄。
    Focus {
        epoch: u64,
        caret: Option<crate::candidate::ScreenRect>,
    },
    /// UI 只能投递意图，必须由原宿主在再次确认焦点后领取并请求组合编辑。
    TakeSelection {
        epoch: u64,
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
    Busy,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Message {
    Hello,
    Stop,
    Stopped,
    Welcome {
        generation: u64,
        host: u64,
    },
    Notification {
        token: crate::candidate::CandidateToken,
        reset: bool,
    },
    Request(Request),
    Response(Response),
}
