//! v2 使用逻辑按键和可校验快照，旧 Rime/X11 协议直接拒绝。
use crate::{InputState, Key, KeyOutcome, Options};
use serde::{Deserialize, Serialize};
pub const VERSION: u16 = 2;

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
    },
    /// 索引针对当前页，并绑定快照，禁止旧候选点击产生提交。
    Select {
        revision: u64,
        index: u8,
    },
    Configure {
        options: Options,
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
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Message {
    Request(Request),
    Response(Response),
}
