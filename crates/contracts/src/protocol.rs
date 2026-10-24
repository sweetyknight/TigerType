//! v5 删除宿主通知、领取点击和焦点光标矩形；候选位置留在宿主进程。
//! v6 在输入状态中加入英文标点开关，供 TSF 本地按键判定。
//! v7 在选项中加入配色主题与候选字体，随输入状态带给宿主内候选窗；管理端可请求后台唤起查字窗。
//! v8 删除输入状态中的提示文字：空码等情况不再弹提示，候选窗只显示候选。
//! v9 唤起消息带目标窗口（查字/设置），已握手的宿主也可请求，供任务栏输入法图标菜单打开设置页。
//! v10 宿主报告其线程是否持有输入焦点（任务栏正显示本输入法图标），后台据此隐藏通知区图标，两处只显示一个。
//! v11 配色主题加入玫红（高亮候选只换字色）；加入全角开关（`ToggleFullShape` 键与输入状态 `full_shape`）。旧 DLL 无法解析，所以升版。
//! 旧协议直接拒绝。
use crate::{InputState, Key, KeyOutcome, Options};
use serde::{Deserialize, Serialize};
pub const VERSION: u16 = 11;

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
    // 请后台在其 UI 线程显示窗口；只投递信号、不改数据，已握手的宿主与管理端都可请求，不核对映像。
    Show(Panel),
    Shown,
    // 宿主线程取得（true）或失去（false）输入焦点：任务栏此时是否显示本输入法图标；断开连接等同失去。
    Indicator(bool),
    Noted,
    Welcome { generation: u64, host: u64 },
    Request(Request),
    Response(Response),
}
