//! TIP 组装：激活时登记按键/焦点 sink 并取得进程期驻留传输；按键在同步编辑会话内完成 IPC、写入与取组合位置，
//! 拿不到编辑锁、后台不可用或结果不确定都放行原键。候选窗在本线程自绘，点击经异步编辑会话选字。
use crate::{
    candidate::{Popup, bounded, fallback_caret},
    edit::{EditReport, Editor, Verdict},
    host::{Channel, Host},
    key::{self, Modifiers},
};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    ptr,
    rc::Rc,
    sync::OnceLock,
    time::{Duration, Instant},
};
use tigertype_contracts::{
    Key,
    protocol::{Command, Reply},
};
use tigertype_ipc::{Error as IpcError, pipe::Runtime, service::Link};
use windows::{
    Win32::{
        Foundation::{
            CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_POINTER,
            E_UNEXPECTED, LPARAM, S_FALSE, WPARAM,
        },
        System::Com::{IClassFactory, IClassFactory_Impl},
        UI::{
            Input::KeyboardAndMouse::{
                GetKeyState, GetKeyboardLayout, GetKeyboardState, ToUnicodeEx, VIRTUAL_KEY,
                VK_CONTROL, VK_MENU, VK_SHIFT,
            },
            TextServices::*,
        },
    },
    core::{BOOL, Error, GUID, HRESULT, IUnknown, IUnknownImpl, Interface, Ref, Result, implement},
};

/// TigerType TIP 的 COM 类标识；类工厂与注册（C 批）共用，开发期不兼容旧值。
pub const CLSID_TEXT_SERVICE: GUID = GUID::from_u128(0x79a65161_8689_43ac_a57c_574c6a74922a);
/// TSF 单次回调的主动等待总预算；连接、握手、编码与回复共用此绝对期限。
const BUDGET: Duration = Duration::from_millis(50);
// 每个宿主进程一个进程期驻留传输运行时：基础设施而非业务状态，各 TIP 线程的连接与会话各自独立。
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn runtime() -> std::result::Result<&'static Runtime, IpcError> {
    if let Some(runtime) = RUNTIME.get() {
        return Ok(runtime);
    }
    // 并发首次激活可能各建一个；落选者随即释放，系统 PIN 本身幂等。
    let created = Runtime::new_process_resident()?;
    Ok(RUNTIME.get_or_init(|| created))
}

/// 后台与 TIP 安装在同一目录；DLL 不读取配置或发现文件来定位后台。
pub(crate) const BACKEND: &str = "tigertype.exe";
fn backend_image() -> Result<PathBuf> {
    Ok(crate::module_path()?.with_file_name(BACKEND))
}

fn modifiers() -> Modifiers {
    // SAFETY: 只读本线程消息队列记录的按键状态。
    let down = |vk: VIRTUAL_KEY| unsafe { GetKeyState(vk.0 as i32) } < 0;
    Modifiers {
        shift: down(VK_SHIFT),
        control: down(VK_CONTROL),
        alt: down(VK_MENU),
    }
}
/// 当前布局下本键产生的单个字符；不改变键盘状态，死键与多字符返回 None。
fn typed(vk: u16, lparam: LPARAM) -> Option<char> {
    let mut state = [0u8; 256];
    // SAFETY: 输出数组固定 256 字节，只读本线程键盘状态。
    unsafe { GetKeyboardState(&mut state) }.ok()?;
    let mut buffer = [0u16; 4];
    // SAFETY: 标志位 0x4 要求不改变内核键盘状态（Win10 1607+），判定不会消耗死键。
    let count = unsafe {
        ToUnicodeEx(
            vk as u32,
            ((lparam.0 >> 16) & 0xff) as u32,
            &state,
            &mut buffer,
            0x4,
            Some(GetKeyboardLayout(0)),
        )
    };
    if count == 1 {
        char::from_u32(buffer[0] as u32)
    } else {
        None
    }
}

/// 失败或 panic 一律不吃键，原键交还宿主；panic 不穿过系统回调。
fn shielded(f: impl FnOnce() -> Result<bool>) -> BOOL {
    catch_unwind(AssertUnwindSafe(f))
        .ok()
        .and_then(|result| result.ok())
        .unwrap_or(false)
        .into()
}
fn guarded(f: impl FnOnce() -> Result<()>) -> Result<()> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(E_UNEXPECTED.into()))
}
fn busy<T>(_: T) -> Error {
    E_FAIL.into()
}

struct Backend {
    link: Link,
    runtime: &'static Runtime,
}
impl Channel for Backend {
    fn ensure(&mut self, deadline: Instant) -> std::result::Result<bool, IpcError> {
        self.link
            .ensure(self.runtime, deadline)
            .map(|(_, fresh)| fresh)
    }
    fn request(
        &mut self,
        session: u64,
        command: Command,
        deadline: Instant,
    ) -> std::result::Result<Reply, IpcError> {
        self.link
            .client()
            .ok_or(IpcError::Closed)?
            .request(session, command, deadline)
    }
    fn cancel(&mut self) {
        // 直接释放管道：后台立即发现断开，下次 ensure 重新连接。
        self.link.close();
    }
}

/// 一次激活的全部状态，只在本 TIP 线程使用；候选窗以弱引用回到这里，不形成引用环。
struct Active {
    manager: ITfThreadMgr,
    client: u32,
    cookie: u32,
    host: RefCell<Host<Backend>>,
    editor: RefCell<Option<Rc<Editor>>>,
    /// 本地编辑代次：每换一个上下文递增，旧上下文的迟到回调随之失效。
    epoch: Cell<u64>,
    popup: RefCell<Option<Rc<Popup>>>,
    quiet: bool,
}
impl Active {
    fn current_popup(&self) -> Option<Rc<Popup>> {
        self.popup.try_borrow().ok()?.clone()
    }
    fn current_editor(&self) -> Option<Rc<Editor>> {
        self.editor.try_borrow().ok()?.clone()
    }
    /// 惰性创建候选窗；失败只是不显示候选，不影响按键。
    fn popup(self: &Rc<Self>) -> Option<Rc<Popup>> {
        if let Some(popup) = self.current_popup() {
            return Some(popup);
        }
        let owner = Rc::downgrade(self);
        let popup = Popup::new(
            move |serial, index| {
                if let Some(active) = owner.upgrade() {
                    active.click(serial, index);
                }
            },
            self.quiet,
        )
        .ok()?;
        *self.popup.try_borrow_mut().ok()? = Some(popup.clone());
        Some(popup)
    }
    fn hide(&self) {
        if let Some(popup) = self.current_popup() {
            popup.hide();
        }
    }
    /// 映射本键并按上次状态判断是否吃键；只读本地状态，不发 IPC。
    fn judge(&self, context: &ITfContext, wparam: WPARAM, lparam: LPARAM) -> Option<Key> {
        let vk = wparam.0 as u16;
        let mut state = self.host.try_borrow().ok()?.state().clone();
        // 换了上下文或宿主已终止组合时，下一键会先让后台清空，按非组合判定。
        state.composing &= self
            .current_editor()
            .is_some_and(|editor| editor.is(context) && editor.composing());
        let key = key::map(vk, modifiers(), typed(vk, lparam), state.composing)?;
        state.consumes(key).then_some(key)
    }
    /// 取得该上下文的编辑器；换上下文时旧编辑器先失效并取消其自有组合，后台下次按键先报告焦点。
    fn focus(&self, context: &ITfContext) -> Result<Rc<Editor>> {
        if let Some(editor) = self.current_editor().filter(|editor| editor.is(context)) {
            return Ok(editor);
        }
        let epoch = self.epoch.get().checked_add(1).ok_or(E_FAIL)?;
        let editor = Rc::new(Editor::new(context.clone(), self.client, epoch)?);
        self.host.try_borrow_mut().map_err(busy)?.blur();
        self.epoch.set(epoch);
        let old = self
            .editor
            .try_borrow_mut()
            .map_err(busy)?
            .replace(editor.clone());
        self.hide();
        if let Some(old) = old {
            old.abandon();
        }
        Ok(editor)
    }
    fn leave(&self) -> Result<()> {
        self.host.try_borrow_mut().map_err(busy)?.blur();
        let old = self.editor.try_borrow_mut().map_err(busy)?.take();
        self.hide();
        if let Some(old) = old {
            old.abandon();
        }
        Ok(())
    }
    /// 同一同步编辑会话内：核对文档 → 后台交换 → 写入并取组合位置。返回值即是否吃掉原键。
    fn input(self: &Rc<Self>, context: &ITfContext, key: Key, deadline: Instant) -> Result<bool> {
        let editor = self.focus(context)?;
        let active = self.clone();
        let report = editor.key(deadline, move |reset, deadline| {
            let Ok(mut host) = active.host.try_borrow_mut() else {
                return Verdict::Skip;
            };
            if reset {
                host.discard();
            }
            match host.key(key, deadline) {
                Ok(outcome) => Verdict::Apply(outcome),
                Err(_) => Verdict::Lost,
            }
        });
        self.settle(&report);
        Ok(report.handled || report.committed)
    }
    /// 会话结束后同步后台标记与候选窗：后台未收到请求时一切不变；失败则下次先清空后台并隐藏候选。
    fn settle(self: &Rc<Self>, report: &EditReport) {
        if !report.exchanged {
            return;
        }
        let state = match self.host.try_borrow_mut() {
            Ok(mut host) => {
                if report.error.is_some() {
                    host.discard();
                }
                host.state().clone()
            }
            Err(_) => return self.hide(),
        };
        let visible = report.error.is_none()
            && state.composing
            && (!state.candidates.is_empty() || !state.notice.is_empty());
        let place = report.caret.filter(bounded).or_else(fallback_caret);
        match (visible, place, visible.then(|| self.popup()).flatten()) {
            (true, Some(caret), Some(popup)) => {
                if popup.show(&state, caret, report.owner).is_err() {
                    popup.hide();
                }
            }
            _ => self.hide(),
        }
    }
    /// 候选点击：展示编号与页内序号绑定当前快照，排队一个异步编辑会话；会话开始时编号未变才发选字。
    fn click(self: &Rc<Self>, serial: u32, index: u8) {
        let (Some(popup), Some(editor)) = (self.current_popup(), self.current_editor()) else {
            return;
        };
        let Some(revision) = popup.selectable(serial, index) else {
            return;
        };
        let (active, done) = (Rc::downgrade(self), Rc::downgrade(self));
        let (watched, unblock) = (Rc::downgrade(&popup), Rc::downgrade(&popup));
        let queued = editor.select(
            move |deadline| {
                if !watched.upgrade().is_some_and(|popup| popup.showing(serial)) {
                    return Verdict::Skip;
                }
                let Some(active) = active.upgrade() else {
                    return Verdict::Skip;
                };
                let Ok(mut host) = active.host.try_borrow_mut() else {
                    return Verdict::Skip;
                };
                match host.select(revision, index, deadline) {
                    Ok(Some(outcome)) => Verdict::Apply(outcome),
                    Ok(None) => Verdict::Skip,
                    Err(_) => Verdict::Lost,
                }
            },
            move |report| {
                if let Some(active) = done.upgrade() {
                    active.settle(&report);
                }
                if let Some(popup) = unblock.upgrade() {
                    popup.unblock(serial);
                }
            },
        );
        if !queued {
            popup.unblock(serial);
        }
    }
    /// 停用：取消组合、销毁候选窗、断开后台、注销 sink；单项失败不阻止其余释放。
    fn close(&self) {
        if let Some(editor) = self.current_editor() {
            editor.abandon();
        }
        self.hide();
        let popup = self.popup.try_borrow_mut().ok().and_then(|mut p| p.take());
        drop(popup);
        if let Ok(mut host) = self.host.try_borrow_mut() {
            host.close();
        }
        if let Ok(keystrokes) = self.manager.cast::<ITfKeystrokeMgr>() {
            // SAFETY: 以激活时的同一客户端编号注销，配对释放系统持有的按键 sink 引用。
            let _ = unsafe { keystrokes.UnadviseKeyEventSink(self.client) };
        }
        if let Ok(source) = self.manager.cast::<ITfSource>() {
            // SAFETY: 以激活时返回的 cookie 注销线程管理器 sink。
            let _ = unsafe { source.UnadviseSink(self.cookie) };
        }
    }
}

#[implement(ITfTextInputProcessorEx, ITfKeyEventSink, ITfThreadMgrEventSink)]
pub struct TextService {
    active: RefCell<Option<Rc<Active>>>,
    // 单击 Shift 切换中英；期间按下其他键或带 Ctrl/Alt 即作废。
    shift: Cell<bool>,
    // 测试改用独立命名空间与预期映像，不连接用户正在运行的后台；quiet 保持候选窗隐藏。
    namespace: String,
    image: Option<PathBuf>,
    quiet: bool,
}
impl TextService {
    fn new() -> Self {
        Self {
            active: RefCell::new(None),
            shift: Cell::new(false),
            namespace: "backend".into(),
            image: None,
            quiet: false,
        }
    }
    fn active(&self) -> Option<Rc<Active>> {
        self.active.try_borrow().ok()?.clone()
    }
    fn press(&self, wparam: WPARAM) {
        let modifiers = modifiers();
        self.shift
            .set(wparam.0 == VK_SHIFT.0 as usize && !modifiers.control && !modifiers.alt);
    }
    fn released(&self, wparam: WPARAM) -> bool {
        let modifiers = modifiers();
        wparam.0 == VK_SHIFT.0 as usize && self.shift.get() && !modifiers.control && !modifiers.alt
    }
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, ptim: Ref<ITfThreadMgr>, tid: u32) -> Result<()> {
        ITfTextInputProcessorEx_Impl::ActivateEx(self, ptim, tid, 0)
    }
    fn Deactivate(&self) -> Result<()> {
        guarded(|| {
            self.shift.set(false);
            let active = self.active.try_borrow_mut().map_err(busy)?.take();
            if let Some(active) = active {
                active.close();
            }
            // 释放连接即关闭管道；后台由常驻读失败移除本宿主，不等待确认。
            Ok(())
        })
    }
}
impl ITfTextInputProcessorEx_Impl for TextService_Impl {
    fn ActivateEx(&self, ptim: Ref<ITfThreadMgr>, tid: u32, _: u32) -> Result<()> {
        guarded(|| {
            let manager = ptim.ok()?.clone();
            if tid == 0 || self.active.try_borrow().map_err(busy)?.is_some() {
                return Err(E_UNEXPECTED.into());
            }
            let runtime = runtime().map_err(busy)?;
            let image = match &self.image {
                Some(image) => image.clone(),
                None => backend_image()?,
            };
            let source: ITfSource = manager.cast()?;
            let keystrokes: ITfKeystrokeMgr = manager.cast()?;
            let sink: ITfThreadMgrEventSink = self.to_interface();
            // SAFETY: 登记本对象的线程管理器 sink；失败路径在返回前配对注销。
            let cookie = unsafe { source.AdviseSink(&ITfThreadMgrEventSink::IID, &sink)? };
            let keys: ITfKeyEventSink = self.to_interface();
            // SAFETY: 前台按键 sink 绑定本客户端编号，Deactivate 以同一编号注销。
            if let Err(error) = unsafe { keystrokes.AdviseKeyEventSink(tid, &keys, true) } {
                // SAFETY: 回滚刚登记的线程管理器 sink，不留下悬挂引用。
                let _ = unsafe { source.UnadviseSink(cookie) };
                return Err(error);
            }
            *self.active.try_borrow_mut().map_err(busy)? = Some(Rc::new(Active {
                manager,
                client: tid,
                cookie,
                host: RefCell::new(Host::new(Backend {
                    link: Link::new(image, &self.namespace),
                    runtime,
                })),
                editor: RefCell::new(None),
                epoch: Cell::new(0),
                popup: RefCell::new(None),
                quiet: self.quiet,
            }));
            Ok(())
        })
    }
}
impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, foreground: BOOL) -> Result<()> {
        self.shift.set(false);
        // 本 TIP 不再是前台按键处理者时收起候选窗，组合留给下一键或失焦处理。
        if !foreground.as_bool()
            && let Some(active) = self.active()
        {
            active.hide();
        }
        Ok(())
    }
    fn OnTestKeyDown(&self, pic: Ref<ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        Ok(shielded(|| {
            self.press(wparam);
            let active = self.active().ok_or(E_UNEXPECTED)?;
            Ok(active.judge(pic.ok()?, wparam, lparam).is_some())
        }))
    }
    fn OnKeyDown(&self, pic: Ref<ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        Ok(shielded(|| {
            let deadline = Instant::now() + BUDGET;
            self.press(wparam);
            let active = self.active().ok_or(E_UNEXPECTED)?;
            let context = pic.ok()?;
            match active.judge(context, wparam, lparam) {
                Some(key) => active.input(context, key, deadline),
                None => Ok(false),
            }
        }))
    }
    fn OnTestKeyUp(&self, _: Ref<ITfContext>, wparam: WPARAM, _: LPARAM) -> Result<BOOL> {
        Ok(shielded(|| Ok(self.released(wparam))))
    }
    fn OnKeyUp(&self, pic: Ref<ITfContext>, wparam: WPARAM, _: LPARAM) -> Result<BOOL> {
        Ok(shielded(|| {
            let deadline = Instant::now() + BUDGET;
            if !self.released(wparam) {
                return Ok(false);
            }
            self.shift.set(false);
            let active = self.active().ok_or(E_UNEXPECTED)?;
            active.input(pic.ok()?, Key::ToggleAscii, deadline)
        }))
    }
    fn OnPreservedKey(&self, _: Ref<ITfContext>, _: *const GUID) -> Result<BOOL> {
        Ok(false.into())
    }
}
impl ITfThreadMgrEventSink_Impl for TextService_Impl {
    fn OnInitDocumentMgr(&self, _: Ref<ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }
    fn OnUninitDocumentMgr(&self, _: Ref<ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }
    fn OnSetFocus(&self, _: Ref<ITfDocumentMgr>, _: Ref<ITfDocumentMgr>) -> Result<()> {
        // 文档焦点变化不发 IPC：取消旧上下文的自有组合、收起候选窗，下一键再向后台报告焦点。
        guarded(|| match self.active() {
            Some(active) => active.leave(),
            None => Ok(()),
        })
    }
    fn OnPushContext(&self, _: Ref<ITfContext>) -> Result<()> {
        Ok(())
    }
    fn OnPopContext(&self, _: Ref<ITfContext>) -> Result<()> {
        Ok(())
    }
}

#[implement(IClassFactory)]
struct Factory;
impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> Result<()> {
        guarded(|| {
            if object.is_null() {
                return Err(E_POINTER.into());
            }
            // SAFETY: 调用方提供的输出指针已判空，先清零保证失败时不留旧值。
            unsafe { object.write(ptr::null_mut()) };
            if !outer.is_null() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let unknown: IUnknown = TextService::new().into();
            // SAFETY: iid/object 由 COM 调用方提供；QueryInterface 自行校验 iid 并写出引用。
            unsafe { unknown.query(iid, object) }.ok()
        })
    }
    fn LockServer(&self, _: BOOL) -> Result<()> {
        // 模块进程期驻留，不按锁计数卸载。
        Ok(())
    }
}

/// COM 入口：只提供本 TIP 的类工厂。
///
/// # Safety
/// 由 COM 按约定传入有效的 CLSID/IID 指针与输出指针。
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    catch_unwind(|| {
        if object.is_null() {
            return E_POINTER;
        }
        // SAFETY: 输出指针已判空，先清零保证失败时不留旧值。
        unsafe { object.write(ptr::null_mut()) };
        if clsid.is_null() || iid.is_null() {
            return E_INVALIDARG;
        }
        // SAFETY: CLSID 指针已判空，按 COM 约定指向有效 GUID。
        if unsafe { *clsid } != CLSID_TEXT_SERVICE {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = Factory.into();
        // SAFETY: iid/object 已判空，QueryInterface 写出引用或返回错误。
        unsafe { factory.query(iid, object) }
    })
    .unwrap_or(E_UNEXPECTED)
}

/// 进程期驻留：完成回调所在模块已被系统 PIN，永不允许卸载。
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
