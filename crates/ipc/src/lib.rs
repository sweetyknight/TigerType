//! 有界协议与在途状态；Windows 管道边界提供认证、限时等待和异步取消。
//! 管理进程、监督与 TSF 尚未接入，不代表产品端到端预算已经达标。

mod client;
mod frame;
#[cfg(windows)]
pub mod pipe;

pub use client::ClientState;
pub use frame::FrameAssembler;

use std::{
    fmt,
    io::{self, Write},
};
use tigerrime_contracts::{
    InputState, MAX_CANDIDATES, MAX_TEXT_BYTES,
    protocol::{Command, Message, Reply, Request, Response, Route, VERSION},
};

pub const HEADER_LEN: usize = 12;
pub const MAX_BODY_LEN: usize = 1024 * 1024;
const MAGIC: &[u8; 4] = b"TRIM";

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
        (Command::Open { .. }, Reply::Opened(_))
            | (Command::Key { .. }, Reply::Key(_))
            | (Command::Clear, Reply::Cleared(_))
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
    for value in [&state.commit_preview, &state.schema_id, &state.schema_name] {
        text(value)?;
    }
    for candidate in &state.candidates {
        for value in [&candidate.text, &candidate.comment, &candidate.label] {
            text(value)?;
        }
    }
    Ok(())
}

fn validate(message: &Message) -> Result<()> {
    match message {
        Message::Request(request) => {
            route(&request.route)?;
            if let Command::Open { schema } = &request.command
                && (schema.is_empty()
                    || schema.len() > 64
                    || !schema
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)))
            {
                return Err(Error::Invalid(
                    "方案标识必须为有界的字母、数字、下划线或连字符",
                ));
            }
        }
        Message::Response(response) => {
            route(&response.route)?;
            match &response.reply {
                Reply::Opened(value) | Reply::Cleared(value) => state(value)?,
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
