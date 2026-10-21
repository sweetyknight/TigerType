//! 可复用的单线程 librime 适配；仅在独立引擎或部署进程创建。
//! 实际系统调用仍可能卡住，截止时间与进程终止必须由外部监督者负责。

mod decode;
mod library;
mod raw;

use std::{
    ffi::{CString, NulError},
    fmt,
    marker::PhantomData,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};
use tigerrime_contracts::{InputState, KeyOutcome};

pub use library::DLL_SHA256;
pub const VERSION: &str = "1.17.0";
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Nul(NulError),
    Utf8(std::str::Utf8Error),
    Invalid(&'static str),
    MissingApi(&'static str),
    Api(&'static str),
    AlreadyInitialized,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Nul(e) => write!(f, "{e}"),
            Self::Utf8(e) => write!(f, "{e}"),
            Self::Invalid(message) => f.write_str(message),
            Self::MissingApi(name) => write!(f, "缺少 Rime API: {name}"),
            Self::Api(name) => write!(f, "Rime API 调用失败: {name}"),
            Self::AlreadyInitialized => {
                f.write_str("librime 每进程只初始化一次；恢复时必须重启进程")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Nul(e) => Some(e),
            Self::Utf8(e) => Some(e),
            _ => None,
        }
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<NulError> for Error {
    fn from(e: NulError) -> Self {
        Self::Nul(e)
    }
}
impl From<std::str::Utf8Error> for Error {
    fn from(e: std::str::Utf8Error) -> Self {
        Self::Utf8(e)
    }
}

#[derive(Debug, Clone)]
pub struct EnginePaths {
    /// 由调用者明确提供目录；不推断、共享或迁移小狼毫的用户数据库。
    pub shared: PathBuf,
    pub user: PathBuf,
    pub logs: PathBuf,
}

struct OwnedTraits {
    raw: raw::Traits,
    _strings: [CString; 3],
    modules: [*const std::ffi::c_char; 3],
}

impl OwnedTraits {
    fn new(paths: &EnginePaths, deploy: bool) -> Result<Box<Self>> {
        let shared = directory_string(&paths.shared)?;
        let user = directory_string(&paths.user)?;
        let logs = directory_string(&paths.logs)?;
        if shared == user {
            return Err(Error::Invalid("共享目录与用户目录必须分离"));
        }
        // Box 保持 traits 和模块数组地址稳定，所有 CString 直到 finalize 后才释放。
        let mut owned = Box::new(Self {
            raw: raw::Traits::default(),
            _strings: [shared, user, logs],
            modules: [
                if deploy {
                    c"deployer".as_ptr()
                } else {
                    c"default".as_ptr()
                },
                c"lua".as_ptr(),
                std::ptr::null(),
            ],
        });
        owned.raw = raw::Traits {
            data_size: raw::data_size::<raw::Traits>(),
            shared_data_dir: owned._strings[0].as_ptr(),
            user_data_dir: owned._strings[1].as_ptr(),
            log_dir: owned._strings[2].as_ptr(),
            modules: owned.modules.as_ptr(),
            distribution_name: c"TigerRime".as_ptr(),
            distribution_code_name: c"tigerrime".as_ptr(),
            distribution_version: c"0.1.0".as_ptr(),
            app_name: c"rime.tigerrime".as_ptr(),
            // 不记逐键内容；保留原始错误给进程监督者和诊断工具。
            min_log_level: 2,
            ..Default::default()
        };
        Ok(owned)
    }
}

fn directory_string(path: &Path) -> Result<CString> {
    if !path.is_absolute() {
        return Err(Error::Invalid("引擎目录必须是已存在的绝对目录"));
    }
    // 保留权限和不存在等原始 I/O 错误，避免把配置故障错误归因于路径格式。
    if !std::fs::metadata(path)?.is_dir() {
        return Err(Error::Invalid("引擎路径不是目录"));
    }
    let path = path.canonicalize()?;
    let value = path
        .to_str()
        .ok_or(Error::Invalid("引擎目录必须可表示为 UTF-8"))?;
    // C++ / Lua 使用 UTF-8 路径，移除 Rust canonicalize 的 Windows verbatim 前缀。
    let value = if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        value.strip_prefix(r"\\?\").unwrap_or(value).to_owned()
    };
    Ok(CString::new(value)?)
}

// librime 和 glog 含进程级单例；只做一次性准入，不用全局锁串联业务或 I/O。
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// 必须在引擎专用线程创建，并在同一线程使用和销毁。
/// ```compile_fail
/// fn requires_send<T: Send>() {}
/// requires_send::<tigerrime_engine::Engine>();
/// ```
pub struct Engine {
    api: raw::Api,
    traits: Box<OwnedTraits>,
    _library: library::Library,
    configured: bool,
    _thread_affinity: PhantomData<Rc<()>>,
}

impl Engine {
    pub fn start(dll: &Path, paths: &EnginePaths) -> Result<Self> {
        let mut engine = Self::open(dll, paths, false)?;
        // SAFETY: API 与 traits 已验证，所有调用受线程亲和与一次初始化约束。
        unsafe {
            engine.api.initialize.unwrap()(&mut engine.traits.raw);
        }
        Ok(engine)
    }

    /// 部署单独占用一个进程，不能与已经启动的引擎交替复用全局 Rime 状态。
    pub fn deploy(dll: &Path, paths: &EnginePaths) -> Result<()> {
        let mut engine = Self::open(dll, paths, true)?;
        // SAFETY: 显式加载 deployer 任务；最终由 Engine::drop 完成清理。
        unsafe {
            engine.api.deployer_initialize.unwrap()(&mut engine.traits.raw);
            if engine.api.deploy.unwrap()() == 0 {
                return Err(Error::Api("deploy"));
            }
        }
        Ok(())
    }

    fn open(dll: &Path, paths: &EnginePaths, deploy: bool) -> Result<Self> {
        if INITIALIZED.load(Ordering::Acquire) {
            return Err(Error::AlreadyInitialized);
        }
        let traits = OwnedTraits::new(paths, deploy)?;
        let library = library::Library::open(dll)?;
        let api = library.api()?;
        let mut engine = Self {
            api,
            traits,
            _library: library,
            configured: false,
            _thread_affinity: PhantomData,
        };
        INITIALIZED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::AlreadyInitialized)?;
        // SAFETY: 所有函数槽存在，DLL 仍被持有，setup 后失败也会 finalize。
        unsafe {
            engine.api.setup.unwrap()(&mut engine.traits.raw);
            engine.configured = true;
            if decode::text(engine.api.get_version.unwrap()())? != VERSION {
                return Err(Error::Invalid("librime 版本不符"));
            }
            if engine.api.find_module.unwrap()(c"lua".as_ptr()).is_null() {
                return Err(Error::Invalid("缺少 Lua 模块"));
            }
        }
        Ok(engine)
    }

    pub fn create_session(&self, schema: &str) -> Result<Session<'_>> {
        let schema = CString::new(schema)?;
        // SAFETY: Engine 已初始化且不能跨线程；非零 ID 的所有权交给 Session。
        let id = NonZeroUsize::new(unsafe { self.api.create_session.unwrap()() })
            .ok_or(Error::Api("create_session"))?;
        let session = Session {
            engine: self,
            id: Some(id),
        };
        // SAFETY: 失败时 Session 的 Drop 仍释放刚创建的会话。
        if unsafe { self.api.select_schema.unwrap()(id.get(), schema.as_ptr()) } == 0 {
            return Err(Error::Api("select_schema"));
        }
        Ok(session)
    }

    pub fn config_bool(&self, schema: &str, key: &str) -> Result<bool> {
        Ok(self.config_number(schema, key, true)? != 0)
    }

    pub fn config_int(&self, schema: &str, key: &str) -> Result<i32> {
        self.config_number(schema, key, false)
    }

    fn config_number(&self, schema: &str, key: &str, boolean: bool) -> Result<i32> {
        let schema = CString::new(schema)?;
        let key = CString::new(key)?;
        let mut config = raw::Config::default();
        // SAFETY: 成功打开后 guard 覆盖读取失败路径，确保 config_close 配对执行。
        unsafe {
            if self.api.schema_open.unwrap()(schema.as_ptr(), &mut config) == 0 {
                return Err(Error::Api("schema_open"));
            }
            let mut config = Allocation {
                value: config,
                release: self.api.config_close.unwrap(),
            };
            let get = if boolean {
                self.api.config_get_bool.unwrap()
            } else {
                self.api.config_get_int.unwrap()
            };
            let mut value = 0;
            if get(&mut config.value, key.as_ptr(), &mut value) == 0 {
                return Err(Error::Api("config_get"));
            }
            Ok(value)
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if self.configured {
            // SAFETY: 借用规则保证所有 Session 已结束；traits 和 DLL 尚未释放。
            unsafe {
                self.api.finalize.unwrap()();
            }
        }
    }
}

/// 生命周期保证会话不能比引擎活得更久；不暴露原始 RimeSessionId。
pub struct Session<'engine> {
    engine: &'engine Engine,
    id: Option<NonZeroUsize>,
}

impl Session<'_> {
    fn id(&self) -> usize {
        self.id.expect("已关闭的会话不能再被访问").get()
    }

    pub fn set_option(&mut self, name: &str, enabled: bool) -> Result<()> {
        let name = CString::new(name)?;
        // SAFETY: C 字符串在调用期间有效；会话由 self 独占管理。
        unsafe {
            self.engine.api.set_option.unwrap()(self.id(), name.as_ptr(), i32::from(enabled));
        }
        Ok(())
    }

    pub fn option(&self, name: &str) -> Result<bool> {
        let name = CString::new(name)?;
        // SAFETY: 会话仍存活，名称已排除内部 NUL。
        Ok(unsafe { self.engine.api.get_option.unwrap()(self.id(), name.as_ptr()) } != 0)
    }

    /// keycode / modifiers 使用 librime 的 X11 语义，不直接接受 Windows 虚拟键值。
    pub fn process_key(&mut self, keycode: i32, modifiers: i32) -> Result<KeyOutcome> {
        // SAFETY: 此同步调用仅允许在独立进程的专用线程执行，不放进 TSF 回调。
        let handled =
            unsafe { self.engine.api.process_key.unwrap()(self.id(), keycode, modifiers) } != 0;
        let state = self.snapshot()?;
        let commit = self.take_commit()?;
        Ok(KeyOutcome {
            handled,
            state,
            commit,
        })
    }

    pub fn clear(&mut self) {
        // SAFETY: 会话仍存活，此操作只取消未上屏组合，不触碰宿主已有文字。
        unsafe {
            self.engine.api.clear_composition.unwrap()(self.id());
        }
    }

    pub fn snapshot(&self) -> Result<InputState> {
        let api = &self.engine.api;
        let mut context = raw::Context {
            data_size: raw::data_size::<raw::Context>(),
            ..Default::default()
        };
        // SAFETY: 数据来自已锁定 DLL；成功取得后立即由 guard 接管释放责任。
        unsafe {
            if api.get_context.unwrap()(self.id(), &mut context) == 0 {
                return Err(Error::Api("get_context"));
            }
            let context = Allocation {
                value: context,
                release: api.free_context.unwrap(),
            };
            let mut state = decode::context(&context.value)?;
            let mut status = raw::Status {
                data_size: raw::data_size::<raw::Status>(),
                ..Default::default()
            };
            if api.get_status.unwrap()(self.id(), &mut status) == 0 {
                return Err(Error::Api("get_status"));
            }
            let status = Allocation {
                value: status,
                release: api.free_status.unwrap(),
            };
            state.schema_id = decode::text(status.value.schema_id)?;
            state.schema_name = decode::text(status.value.schema_name)?;
            state.ascii_mode = status.value.is_ascii_mode != 0;
            state.composing = status.value.is_composing != 0;
            Ok(state)
        }
    }

    pub fn take_commit(&mut self) -> Result<Option<String>> {
        let api = &self.engine.api;
        let mut value = raw::Commit {
            data_size: raw::data_size::<raw::Commit>(),
            ..Default::default()
        };
        // SAFETY: get_commit 是消费操作，返回的字符串必须先复制再 free_commit。
        unsafe {
            if api.get_commit.unwrap()(self.id(), &mut value) == 0 {
                return Ok(None);
            }
            let commit = Allocation {
                value,
                release: api.free_commit.unwrap(),
            };
            Ok(Some(decode::text(commit.value.text)?))
        }
    }

    pub fn close(mut self) -> Result<()> {
        let id = self.id.take().expect("close 消费 self，只能调用一次");
        // SAFETY: 先移走 ID，避免错误返回触发 Drop 时重复销毁。
        if unsafe { self.engine.api.destroy_session.unwrap()(id.get()) } == 0 {
            return Err(Error::Api("destroy_session"));
        }
        Ok(())
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            // SAFETY: Drop 仅回收仍持有的会话；不 panic、不调用宿主或外部线程。
            unsafe {
                self.engine.api.destroy_session.unwrap()(id.get());
            }
        }
    }
}

struct Allocation<T> {
    value: T,
    release: unsafe extern "C" fn(*mut T) -> raw::Bool,
}

impl<T> Drop for Allocation<T> {
    fn drop(&mut self) {
        // SAFETY: 仅在对应 Rime get/open 成功后构造，回调与类型始终配对。
        unsafe {
            (self.release)(&mut self.value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn allocations_release_on_error_and_unwind() {
        static FREED: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "C" fn release(_: *mut raw::Config) -> i32 {
            FREED.fetch_add(1, Ordering::SeqCst);
            1
        }
        let failing = || -> Result<()> {
            let _guard = Allocation {
                value: raw::Config::default(),
                release,
            };
            Err(Error::Invalid("模拟读取失败"))
        };
        assert!(failing().is_err());
        let _ = std::panic::catch_unwind(|| {
            let _guard = Allocation {
                value: raw::Config::default(),
                release,
            };
            panic!("模拟 Rust 解码 panic");
        });
        assert_eq!(FREED.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn repr_c_sizes_match_locked_x64_header() {
        if cfg!(target_pointer_width = "64") {
            assert_eq!(size_of::<raw::Traits>(), 96);
            assert_eq!(size_of::<raw::Context>(), 88);
            assert_eq!(size_of::<raw::Status>(), 56);
            // 已与官方头文件逐槽核对：get_version 是第 73 个指针槽。
            assert_eq!(std::mem::offset_of!(raw::Api, get_version), 584);
        }
    }

    #[test]
    fn relative_paths_and_nul_names_are_rejected() {
        assert!(directory_string(Path::new("user")).is_err());
        assert!(CString::new("tiger\0other").is_err());
    }
}
