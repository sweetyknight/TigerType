//! 独立查字工具的进程内消息；不走输入会话，也不能产生上屏提交。
use crate::{LookupPage, Options};

#[derive(Debug)]
pub enum Request {
    Lookup {
        query: String,
        page: usize,
    },
    SaveSettings(Options),
    /// 重新读入用户手工修改过的 settings.toml；成功同样以 `Reply::Saved` 回复新设置。
    ReloadSettings,
}

#[derive(Debug)]
pub enum Reply {
    Lookup(Result<LookupPage, String>),
    Saved(Result<Options, String>),
}
