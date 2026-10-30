//! 共享后台业务：码表核对与加载、settings.toml、受管目录、类型化调度与宿主回收策略、查字/设置控制器。
//! 不依赖 Windows IPC、input-client 或界面；传输与进程组装在平台 app，文件原语集中于 storage。
pub mod dispatch;
pub mod import;
pub mod lookup;
pub mod resources;
pub mod settings;
pub mod storage;

use tigertype_contracts::{Key, KeyOutcome, Options};
use tigertype_engine::Session;
/// 词表类型属于本 crate 的公开接口：调度器与控制器借用 Dictionary，Data/Entry 供组装层构造合成词表；
/// 组装层不直接依赖 engine。
pub use tigertype_engine::{Data, Dictionary, Entry};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// 诊断命令 demo-input：默认设置的新会话逐字处理（空格为选字键），返回每键结果；不经调度、不上屏。
pub fn demo(dictionary: &Dictionary, input: &str) -> Result<Vec<KeyOutcome>> {
    let mut session = Session::new(dictionary, Options::default());
    input
        .chars()
        .map(|c| {
            let key = if c == ' ' {
                Key::Space
            } else {
                Key::Character(c)
            };
            Ok(session.process(key)?)
        })
        .collect()
}

/// 码表目录在源码 resources 与安装程序旁使用同一名称。
pub const TABLES: &str = "tiger";
