//! 有界协议与在途状态；Windows 管道边界提供认证、限时等待和异步取消。
//! 传输保留认证和取消语义；尚无 TSF 端到端预算结论。

mod client;
mod frame;
#[cfg(windows)]
pub mod pipe;
#[cfg(windows)]
pub mod service;

pub use client::ClientState;
pub use frame::FrameAssembler;

use serde::{Deserialize, Serialize};
use std::{
    fmt,
    io::{self, Write},
};
use tigertype_contracts::{
    InputState, Key, MAX_CANDIDATES, MAX_LEARNING_ITEMS, MAX_TEXT_BYTES,
    protocol::{Command, Panel, Reply, Request, Response, Route},
};

/// 协议版本只约束跨进程帧；进程内通道直接传类型化消息，不经此版本。
/// v5 删除宿主通知、领取点击和焦点光标矩形；候选位置留在宿主进程。
/// v6 在输入状态中加入英文标点开关，供 TSF 本地按键判定。
/// v7 在选项中加入配色主题与候选字体，随输入状态带给宿主内候选窗；管理端可请求后台唤起查字窗。
/// v8 删除输入状态中的提示文字：空码等情况不再弹提示，候选窗只显示候选。
/// v9 唤起消息带目标窗口（查字/设置），已握手的宿主也可请求，供任务栏输入法图标菜单打开设置页。
/// v10 宿主报告其线程是否持有输入焦点（任务栏正显示本输入法图标），后台据此隐藏通知区图标，两处只显示一个。
/// v11 配色主题加入玫红（高亮候选只换字色）；加入全角开关（`ToggleFullShape` 键与输入状态 `full_shape`）。旧 DLL 无法解析，所以升版。
/// v12 选项加入单字/词输入模式；旧 DLL 无法解析，所以升版。
/// v13 选项加入每页候选数（3–9），候选页大小随之可变；旧 DLL 无法解析，所以升版。
/// v14 按键携带互斥前置操作，开会话/焦点/清空与本键一次往返，删除单独的 Open/Focus/Clear 命令，旧帧直接拒绝。
/// 旧协议直接拒绝。
pub const VERSION: u16 = 14;

/// 跨进程信封：握手、管理信号与会话请求/回复。
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

pub const HEADER_LEN: usize = 12;
pub const MAX_BODY_LEN: usize = 1024 * 1024;
// 产品标识与管道命名空间同步更名，直接拒绝旧程序的帧，不保留兼容分支。
const MAGIC: &[u8; 4] = b"TTYP";

#[derive(Debug)]
pub enum Error {
    Invalid(&'static str),
    Json(serde_json::Error),
    Io(io::Error),
    /// 调用方必须沿用同一个绝对截止时间，不能为每次分片重置预算。
    TimedOut,
    Busy,
    Closed,
    Cancelled,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::Json(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::TimedOut => f.write_str("通信请求已超过截止时间"),
            Self::Busy => f.write_str("已有在途请求，不能覆盖或静默丢弃"),
            Self::Closed => f.write_str("通信状态已关闭，不能接受迟到回复或复用"),
            Self::Cancelled => f.write_str("管道操作已取消"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

type Result<T> = std::result::Result<T, Error>;

/// 传输层应先读固定头并调用此函数，再分配正文，不能按未验证长度分配内存。
pub fn frame_len(header: &[u8; HEADER_LEN]) -> Result<usize> {
    if &header[..4] != MAGIC
        || u16::from_le_bytes([header[4], header[5]]) != VERSION
        || header[6..8] != [0, 0]
    {
        return Err(Error::Invalid("协议标识、版本或保留位不符"));
    }
    let length = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
    if length == 0 || length > MAX_BODY_LEN {
        return Err(Error::Invalid("协议正文长度超限或为空"));
    }
    Ok(HEADER_LEN + length)
}

// 在序列化过程中限制容量，不先生成无限大的 JSON 再检查长度。
struct BoundedFrame(Vec<u8>);

impl Write for BoundedFrame {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > HEADER_LEN + MAX_BODY_LEN - self.0.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "协议正文长度超限",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn encode(message: &Message) -> Result<Vec<u8>> {
    validate(message)?;
    let mut frame = BoundedFrame(vec![0; HEADER_LEN]);
    serde_json::to_writer(&mut frame, message)?;
    let length = (frame.0.len() - HEADER_LEN) as u32;
    frame.0[..4].copy_from_slice(MAGIC);
    frame.0[4..6].copy_from_slice(&VERSION.to_le_bytes());
    frame.0[8..12].copy_from_slice(&length.to_le_bytes());
    Ok(frame.0)
}

pub fn decode(frame: &[u8]) -> Result<Message> {
    let header = frame
        .get(..HEADER_LEN)
        .ok_or(Error::Invalid("协议头被截断"))?;
    if frame_len(header.try_into().unwrap())? != frame.len() {
        return Err(Error::Invalid("协议正文被截断或含额外数据"));
    }
    let message = serde_json::from_slice(&frame[HEADER_LEN..])?;
    validate(&message)?;
    Ok(message)
}

/// 只交付当前在途请求的回复；过期、超时后的帧应由通道调用方丢弃，不能重新上屏。
pub fn decode_response(frame: &[u8], expected: &Request) -> Result<Response> {
    let Message::Response(response) = decode(frame)? else {
        return Err(Error::Invalid("需要回复帧，却收到请求"));
    };
    if response.route != expected.route {
        return Err(Error::Invalid("回复会话、请求序号或引擎代次不符"));
    }
    if !matches!(
        (&expected.command, &response.reply),
        (Command::Key { .. }, Reply::Key(_))
            | (Command::Select { .. }, Reply::Key(_))
            | (Command::Configure { .. }, Reply::Configured(_))
            | (Command::Close, Reply::Closed)
            | (_, Reply::Rejected(_))
    ) {
        return Err(Error::Invalid("回复类型与请求不符"));
    }
    Ok(response)
}

fn route(route: &Route) -> Result<()> {
    if route.generation == 0 || route.request_id == 0 || route.session_id == 0 {
        return Err(Error::Invalid("协议身份编号不能为零"));
    }
    Ok(())
}

fn text(value: &str) -> Result<()> {
    if value.len() > MAX_TEXT_BYTES {
        return Err(Error::Invalid("协议文本长度超限"));
    }
    Ok(())
}

fn state(state: &InputState) -> Result<()> {
    let composition = &state.composition;
    text(&composition.text)?;
    if ![
        composition.cursor,
        composition.selection.start,
        composition.selection.end,
    ]
    .into_iter()
    .all(|index| composition.text.is_char_boundary(index))
        || composition.selection.start > composition.selection.end
    {
        return Err(Error::Invalid("协议组合位置越界或不在 UTF-8 边界"));
    }
    if state.page_size > MAX_CANDIDATES || state.candidates.len() > state.page_size
        || state.highlighted.is_some_and(|index| index >= state.candidates.len())
        || (state.candidates.is_empty() != state.highlighted.is_none())
        // JSON 数字不依赖本机指针宽度；限制页码以保证 x64/x86 接受范围一致。
        || state.page_index > u32::MAX as usize
    {
        return Err(Error::Invalid("协议候选页或高亮位置不符"));
    }
    text(&state.commit_preview)?;
    for candidate in &state.candidates {
        for value in [&candidate.text, &candidate.comment, &candidate.label] {
            text(value)?;
        }
        if let Some(info) = &candidate.learning {
            // 学习字段同样是跨进程输入，不能只限制候选文字而放任嵌套集合增长。
            for items in [&info.codes, &info.full_codes, &info.readings] {
                if items.len() > MAX_LEARNING_ITEMS {
                    return Err(Error::Invalid("学习资料条目超限"));
                }
                for item in items {
                    text(item)?;
                }
            }
            if let Some(decomposition) = &info.decomposition {
                text(decomposition)?;
            }
        }
    }
    Ok(())
}

fn validate(message: &Message) -> Result<()> {
    match message {
        Message::Hello
        | Message::Stop
        | Message::Stopped
        | Message::Show(_)
        | Message::Shown
        | Message::Indicator(_)
        | Message::Noted => {}
        Message::Welcome { generation, host } => {
            if *generation == 0 || *host == 0 {
                return Err(Error::Invalid("握手身份不能为零"));
            }
        }
        Message::Request(request) => {
            route(&request.route)?;
            match request.command {
                Command::Key {
                    key: Key::Character(c),
                    ..
                } if c.is_control() => {
                    return Err(Error::Invalid("控制键必须使用逻辑按键而非字符"));
                }
                Command::Select {
                    revision, index, ..
                } if revision == 0 || index as usize >= MAX_CANDIDATES => {
                    return Err(Error::Invalid("候选选择的快照或索引非法"));
                }
                Command::Key { epoch: 0, .. } | Command::Select { epoch: 0, .. } => {
                    return Err(Error::Invalid("按键和选字必须绑定焦点代次"));
                }
                _ => {}
            }
        }
        Message::Response(response) => {
            route(&response.route)?;
            match &response.reply {
                Reply::Configured(value) => state(value)?,
                Reply::Key(outcome) => {
                    state(&outcome.state)?;
                    if let Some(commit) = &outcome.commit {
                        text(commit)?;
                    }
                }
                Reply::Closed | Reply::Rejected(_) => {}
            }
        }
    }
    Ok(())
}
