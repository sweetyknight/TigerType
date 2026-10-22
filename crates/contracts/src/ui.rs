//! 独立查字工具的进程内消息；不走输入会话，也不能产生上屏提交。
use crate::{LookupPage, Options};

#[derive(Debug)]
pub enum Request {
    Lookup { query: String, page: usize },
    SaveSettings(Options),
}

#[derive(Debug)]
pub enum Reply {
    Lookup(Result<LookupPage, String>),
    Saved(Result<Options, String>),
}
