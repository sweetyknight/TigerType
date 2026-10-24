//! 纯 Rust 单字/词输入核心：只接受已加载的数据，不访问平台、文件或线程。
#![forbid(unsafe_code)]
mod dictionary;
mod session;
pub use dictionary::{Data, Detail, Dictionary, Entry, Mapping, Phrase, Pronunciation};
pub use session::Session;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub &'static str);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
