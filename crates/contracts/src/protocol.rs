//! 管理端与引擎端的最小会话协议；不包含传输句柄、路径或借用数据。

use crate::{InputState, KeyOutcome};
use serde::{Deserialize, Serialize};

pub const VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// 三个编号均非零；管理端在重启引擎时换代，不能沿用旧代次。
    pub generation: u64,
    /// 同一引擎通道内严格递增，失败也不得重放原请求。
    pub request_id: u64,
    /// 管理端递增分配；关闭后不能在同一代次中复用。
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
        schema: String,
    },
    /// 沿用 librime X11 语义，不把 Windows 虚拟键直接当作 keycode。
    Key {
        keycode: i32,
        modifiers: i32,
    },
    Clear,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    /// 原样回显请求身份；接收端必须同时核对三个编号和回复类型。
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
    Cleared(InputState),
    Closed,
    Rejected(Rejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rejection {
    StaleGeneration,
    OutOfOrder,
    UnknownSession,
    SessionReused,
    SessionLimit,
    Unavailable,
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
