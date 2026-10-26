//! TIP 组装：激活时登记按键/焦点 sink 并取得进程期驻留传输；按键在同步编辑会话内完成 IPC、写入与取组合位置，
//! 拿不到编辑锁、后台不可用或结果不确定都放行原键。候选窗在本线程自绘，点击经异步编辑会话选字。
//! 激活期间保持键盘开闭状态为“开”：TSF 不把按键交给关闭状态下的 TIP。
//! 激活时登记任务栏输入法图标，按每次回复显示中英状态，其菜单经同一连接请后台显示设置/查字窗。
//! 线程取得/失去输入焦点时告诉后台任务栏是否正显示本输入法图标，后台据此隐藏/恢复通知区图标。
use crate::{
    candidate::{Popup, fallback_caret, portable, window_area},
    edit::{EditReport, Editor, Verdict},
    indicator::{Indicator, Status},
    key,
};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    ptr,
    rc::{Rc, Weak},
    sync::OnceLock,
    time::{Duration, Instant},
};
use tigertype_contracts::{
    InputState, Key, Options, Palette,
    protocol::{Command, Panel, Reply},
};
use tigertype_frontend::{
    Channel, Error as SessionError, Host, Modifiers, ShiftTap, bounded, locate, map,
};
use tigertype_ipc::{Error as IpcError, pipe::Runtime, service::Link};
use windows::{
    Win32::{
        Foundation::{
            CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_POINTER,
            E_UNEXPECTED, LPARAM, S_FALSE, WPARAM,
        },
        System::{
            Com::{IClassFactory, IClassFactory_Impl},
            Variant::VARIANT,
        },
        UI::{
            Input::KeyboardAndMouse::{
                GetKeyState, GetKeyboardLayout, GetKeyboardState, ToUnicodeEx, VIRTUAL_KEY,
                VK_CONTROL, VK_MENU, VK_SHIFT,
            },
            TextServices::*,
            WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow},
        },
    },
    core::{
        BOOL, ComObject, Error, GUID, HRESULT, IUnknown, IUnknownImpl, Interface, Ref, Result,
        implement,
    },
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
/// 新线程管理器的开闭状态为空即关闭，此时宿主只把原键交给应用（只出英文）；宿主切换输入上下文或开关热键也会关上它。
/// 中英文由本输入法自己的状态切换，所以激活与每次被关闭时都重新打开。
fn open_keyboard(compartment: &ITfCompartment, client: u32) -> Result<()> {
    // SAFETY: 读取线程管理器级开闭状态；VT_EMPTY 或 0 都是关闭。
    let value = unsafe { compartment.GetValue()? };
    if i32::try_from(&value).is_ok_and(|open| open != 0) {
        return Ok(());
    }
    // SAFETY: 以本 TIP 客户端编号写入 VT_I4 1，值在调用期间有效。
    unsafe { compartment.SetValue(client, &VARIANT::from(1i32)) }
}
/// 打开键盘并监听其被关闭；返回状态容器与 sink cookie 供停用时注销。取不到时照常激活，只是本线程可能收不到按键。
fn watch_keyboard(
    manager: &ITfThreadMgr,
    client: u32,
    sink: &ITfCompartmentEventSink,
) -> Option<(ITfCompartment, u32)> {
    let compartments: ITfCompartmentMgr = manager.cast().ok()?;
    // SAFETY: 系统定义的开闭状态 GUID，取线程管理器级容器。
    let compartment =
        unsafe { compartments.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE) }.ok()?;
    let _ = open_keyboard(&compartment, client);
    let source: ITfSource = compartment.cast().ok()?;
    // SAFETY: 登记本对象的开闭状态 sink，停用时以同一 cookie 注销。
    let cookie = unsafe { source.AdviseSink(&ITfCompartmentEventSink::IID, sink) }.ok()?;
    Some((compartment, cookie))
}
/// panic 不穿过系统回调。
pub(crate) fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(E_UNEXPECTED.into()))
}
fn busy<T>(_: T) -> Error {
    E_FAIL.into()
}
/// 传输层错误对会话只意味着连接已作废。
fn lost(_: IpcError) -> SessionError {
    SessionError::Transport
}

struct Backend {
    link: Link,
    runtime: &'static Runtime,
}
impl Channel for Backend {
    fn ensure(&mut self, deadline: Instant) -> std::result::Result<bool, SessionError> {
        self.link
            .ensure(self.runtime, deadline)
            .map(|(_, fresh)| fresh)
            .map_err(lost)
    }
    fn request(
        &mut self,
        session: u64,
        command: Command,
        deadline: Instant,
    ) -> std::result::Result<Reply, SessionError> {
        self.link
            .client()
            .ok_or(SessionError::Transport)?
            .request(session, command, deadline)
            .map_err(lost)
    }
    fn show(&mut self, panel: Panel, deadline: Instant) -> std::result::Result<(), SessionError> {
        self.link
            .client()
            .ok_or(SessionError::Transport)?
            .show(panel, deadline)
            .map_err(lost)
    }
    fn indicate(
        &mut self,
        visible: bool,
        deadline: Instant,
    ) -> std::result::Result<(), SessionError> {
        self.link
            .client()
            .ok_or(SessionError::Transport)?
            .indicate(visible, deadline)
            .map_err(lost)
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
    /// 键盘开闭状态容器与其 sink cookie。
    keyboard: Option<(ITfCompartment, u32)>,
    /// 已登记的任务栏输入法图标；登记失败为 None，只是没有图标菜单与中英显示。
    indicator: Option<ComObject<Indicator>>,
    /// 线程焦点 sink cookie；登记失败为 None，只是后台不隐藏通知区图标。
    focus: Option<u32>,
}
/// 登记任务栏输入法图标；按钮经弱引用回到激活状态，不形成引用环。
fn indicator(manager: &ITfThreadMgr, owner: &Weak<Active>) -> Option<ComObject<Indicator>> {
    let items: ITfLangBarItemMgr = manager.cast().ok()?;
    let (ready, show, toggle) = (owner.clone(), owner.clone(), owner.clone());
    let button = ComObject::new(Indicator::new(
        move || match ready.upgrade() {
            // 先确认连接：刚重连时状态回到默认，配色与全角随之取默认值，直到下一键拿到后台状态。
            Some(active) => {
                let ready = active.ready(Instant::now() + BUDGET);
                let (palette, status) = active.look();
                (ready, palette, status)
            }
            None => (false, Options::default().palette(), Status::default()),
        },
        move |panel| {
            if let Some(active) = show.upgrade() {
                active.show(panel);
            }
        },
        move |key| {
            if let Some(active) = toggle.upgrade() {
                active.toggle(key);
            }
        },
    ));
    // SAFETY: 登记本按钮，停用时以同一对象注销。
    unsafe { items.AddItem(&button.to_interface::<ITfLangBarItemButton>()) }.ok()?;
    Some(button)
}
impl Active {
    fn reopen(&self) -> Result<()> {
        match &self.keyboard {
            Some((compartment, _)) => open_keyboard(compartment, self.client),
            None => Ok(()),
        }
    }
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
    /// 后台可用（必要时在期限内只做连接握手）；连不上则原键一律放行。
    fn ready(&self, deadline: Instant) -> bool {
        self.host
            .try_borrow_mut()
            .is_ok_and(|mut host| host.ready(deadline))
    }
    /// 吃键前的后台确认（按下与 Shift 抬起共用）：离线时撤回仍一致的自有组合并收起候选，
    /// 原键交还宿主；预判不吃键时宿主不再调用处理回调，清理只能在这里做。
    fn online(self: &Rc<Self>, deadline: Instant) -> bool {
        if self.ready(deadline) {
            return true;
        }
        if self
            .current_editor()
            .is_some_and(|editor| editor.composing())
        {
            let _ = self.leave();
        }
        self.hide();
        false
    }
    /// 菜单配色与状态（中英、标点、全角）：取上次回复里的后台状态，配色与候选窗一致。
    fn look(&self) -> (Palette, Status) {
        self.host.try_borrow().map_or_else(
            |_| (Options::default().palette(), Status::default()),
            |host| {
                let state = host.state();
                (
                    state.options.palette(),
                    Status {
                        ascii: state.ascii_mode,
                        punctuation: state.ascii_punctuation,
                        full: state.full_shape,
                    },
                )
            },
        )
    }
    /// 映射本键并按上次状态判断是否吃键；不发输入请求，只读上下文或后台连不上则不吃。
    /// 宿主未必交还“预判吃下、处理时放行”的原键，所以编辑会话会拒绝的条件须在这里先排除。
    fn judge(
        self: &Rc<Self>,
        context: &ITfContext,
        wparam: WPARAM,
        lparam: LPARAM,
        deadline: Instant,
    ) -> Option<Key> {
        let vk = wparam.0 as u16;
        // 换了上下文或宿主已终止组合时，下一键会先让后台清空，按非组合判定。
        let composing = self
            .current_editor()
            .is_some_and(|editor| editor.is(context) && editor.composing());
        let previous = self.host.try_borrow().ok()?.state().composing;
        let key = map(
            key::physical(vk),
            modifiers(),
            typed(vk, lparam),
            composing && previous,
        )?;
        // SAFETY: GetStatus 不需要编辑锁，只读宿主动态标志；读不到与只读一样放行（会话内同样会拒绝）。
        match unsafe { context.GetStatus() } {
            Ok(status) if status.dwDynamicFlags & TS_SD_READONLY == 0 => {}
            _ => return None,
        }
        if !self.online(deadline) {
            return None;
        }
        let mut state = self.host.try_borrow().ok()?.state().clone();
        state.composing &= composing;
        state.consumes(key).then_some(key)
    }
    /// 取得该上下文的编辑器；换上下文时旧编辑器先失效并取消其自有组合，后台下次按键先报告焦点。
    fn focus(self: &Rc<Self>, context: &ITfContext) -> Result<Rc<Editor>> {
        if let Some(editor) = self.current_editor().filter(|editor| editor.is(context)) {
            return Ok(editor);
        }
        let epoch = self.epoch.get().checked_add(1).ok_or(E_FAIL)?;
        let owner = Rc::downgrade(self);
        // 宿主终止组合（如点击别处）时立即收起候选窗，后台清空留给下一键。
        let ended = move || {
            if let Some(active) = owner.upgrade() {
                active.hide();
            }
        };
        let editor = Rc::new(Editor::new(context.clone(), self.client, epoch, ended)?);
        let owner = Rc::downgrade(self);
        // 宿主组合矩形晚到（Chromium 等由渲染进程异步回报）时，布局通知再摆放候选窗。
        editor.watch_layout(move || {
            if let Some(active) = owner.upgrade() {
                active.relayout();
            }
        });
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
        if let Some(indicator) = &self.indicator {
            indicator.update(state.ascii_mode);
        }
        self.present(&state, report);
    }
    /// 摆放候选窗，位置逐级退：组合矩形（整段或起点）→ 本线程系统光标 → 正在显示时的原位置 →
    /// 本输入框上次组合位置 → 输入区域（视图范围，取不到则视图/焦点窗口），候选窗贴着区域外沿放。
    /// 宿主随后补来布局时再移到真实位置。空码等无候选时不显示，编码只留在宿主内的组合下划线里。
    fn present(self: &Rc<Self>, state: &InputState, report: &EditReport) {
        if report.error.is_some() || !state.composing || state.candidates.is_empty() {
            return self.hide();
        }
        let anchor = || self.current_popup().and_then(|popup| popup.anchor());
        let place = locate(&[
            &|| report.caret.map(portable),
            &fallback_caret,
            &anchor,
            &|| report.recent.map(portable),
            &|| report.area.map(portable),
            &|| window_area(report.owner),
        ]);
        match (place, self.popup()) {
            (Some(caret), Some(popup)) => {
                if popup.show(state, caret, report.owner).is_err() {
                    popup.hide();
                }
            }
            _ => self.hide(),
        }
    }
    /// 宿主布局变化：组合中且有候选时只读重取组合矩形，取得后显示或移动候选窗；
    /// 取不到或组合已变就保持现状，下一键或下一次通知再定。
    fn relayout(self: &Rc<Self>) {
        let Some(editor) = self.current_editor().filter(|editor| editor.composing()) else {
            return;
        };
        if !self.host.try_borrow().is_ok_and(|host| {
            let state = host.state();
            state.composing && !state.candidates.is_empty()
        }) {
            return;
        }
        let owner = Rc::downgrade(self);
        editor.locate(move |report| {
            let Some(active) = owner.upgrade() else {
                return;
            };
            // 布局通知只按真实组合矩形移动，不走退路位置。
            if report.error.is_some()
                || !report.caret.is_some_and(|caret| bounded(&portable(caret)))
            {
                return;
            }
            let Ok(state) = active.host.try_borrow().map(|host| host.state().clone()) else {
                return;
            };
            active.present(&state, &report);
        });
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
        // finish 在会话的每条终止路径上恰好调用一次；未受理时由这里解锁。
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
    /// 图标翻转开关：单击切换中英（效果同单击 Shift），菜单切换全角。点击不在按键事件内，拿不到同步编辑会话：
    /// 中英切换遇到自有组合时排队异步会话（后台把编码原样上屏，须写入文档）；其余情况不碰文档直接交换，
    /// 中英切换前先让后台丢弃宿主已终止的残留组合，不会补上屏旧编码；全角只翻开关，组合原样保留。
    /// 失败只作废连接并收起候选。
    fn toggle(self: &Rc<Self>, key: Key) {
        let switch = key == Key::ToggleAscii;
        if let Some(editor) = self
            .current_editor()
            .filter(|editor| switch && editor.composing())
        {
            let (active, done) = (Rc::downgrade(self), Rc::downgrade(self));
            // 复用点击选字的异步会话：开始时同样核对自有组合仍完整，否则不发请求。
            editor.select(
                move |deadline| {
                    let Some(active) = active.upgrade() else {
                        return Verdict::Skip;
                    };
                    let Ok(mut host) = active.host.try_borrow_mut() else {
                        return Verdict::Skip;
                    };
                    match host.key(Key::ToggleAscii, deadline) {
                        Ok(outcome) => Verdict::Apply(outcome),
                        Err(_) => Verdict::Lost,
                    }
                },
                move |report| {
                    if let Some(active) = done.upgrade() {
                        active.settle(&report);
                    }
                },
            );
            return;
        }
        let ascii = self.host.try_borrow_mut().ok().and_then(|mut host| {
            if switch && host.state().composing {
                host.discard();
            }
            let outcome = host.key(key, Instant::now() + BUDGET).ok()?;
            Some(outcome.state.ascii_mode)
        });
        match (ascii, &self.indicator) {
            (Some(ascii), Some(indicator)) => indicator.update(ascii),
            (Some(_), None) => {}
            (None, _) => self.hide(),
        }
    }
    /// 图标菜单：放行前台切换后请后台显示窗口。后台不可用时菜单已置灰，失败只作废连接并收起旧候选。
    fn show(&self, panel: Panel) {
        // SAFETY: 只放宽下一次前台切换；本进程没有前台权时调用失败，窗口照常显示，只是可能不到前台。
        let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
        let shown = self
            .host
            .try_borrow_mut()
            .is_ok_and(|mut host| host.show(panel, Instant::now() + BUDGET));
        if !shown {
            self.hide();
        }
    }
    /// 本线程取得/失去输入焦点：任务栏随之显示/不再显示本输入法图标，告诉后台以隐藏/恢复通知区图标。
    /// 失败只作废连接并收起旧候选，不影响按键。
    fn indicate(&self, visible: bool) {
        let sent = self
            .host
            .try_borrow_mut()
            .is_ok_and(|mut host| host.indicate(visible, Instant::now() + BUDGET));
        if !sent {
            self.hide();
        }
    }
    /// 停用：取消组合、销毁候选窗、断开后台、注销图标与 sink；单项失败不阻止其余释放。
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
        if let (Some(button), Ok(items)) =
            (&self.indicator, self.manager.cast::<ITfLangBarItemMgr>())
        {
            // SAFETY: 以登记时的同一对象注销任务栏图标。
            let _ = unsafe { items.RemoveItem(&button.to_interface::<ITfLangBarItemButton>()) };
        }
        if let Ok(keystrokes) = self.manager.cast::<ITfKeystrokeMgr>() {
            // SAFETY: 以激活时的同一客户端编号注销，配对释放系统持有的按键 sink 引用。
            let _ = unsafe { keystrokes.UnadviseKeyEventSink(self.client) };
        }
        if let Ok(source) = self.manager.cast::<ITfSource>() {
            // SAFETY: 以激活时返回的 cookie 注销线程管理器 sink 与线程焦点 sink。
            unsafe {
                let _ = source.UnadviseSink(self.cookie);
                if let Some(focus) = self.focus {
                    let _ = source.UnadviseSink(focus);
                }
            }
        }
        if let Some((compartment, cookie)) = &self.keyboard
            && let Ok(source) = compartment.cast::<ITfSource>()
        {
            // SAFETY: 以登记时的 cookie 注销开闭状态 sink；停用后不再干预该状态。
            let _ = unsafe { source.UnadviseSink(*cookie) };
        }
    }
}

#[implement(
    ITfTextInputProcessorEx,
    ITfKeyEventSink,
    ITfThreadMgrEventSink,
    ITfThreadFocusSink,
    ITfCompartmentEventSink
)]
pub struct TextService {
    active: RefCell<Option<Rc<Active>>>,
    // 单击 Shift 切换中英；判定规则在 frontend。
    shift: Cell<ShiftTap>,
    // 测试改用独立命名空间与预期映像，不连接用户正在运行的后台；quiet 保持候选窗隐藏。
    namespace: String,
    image: Option<PathBuf>,
    quiet: bool,
}
impl TextService {
    fn new() -> Self {
        Self {
            active: RefCell::new(None),
            shift: Cell::new(ShiftTap::default()),
            namespace: "backend".into(),
            image: None,
            quiet: false,
        }
    }
    fn active(&self) -> Option<Rc<Active>> {
        self.active.try_borrow().ok()?.clone()
    }
    fn press(&self, wparam: WPARAM) {
        let shift = wparam.0 == VK_SHIFT.0 as usize;
        self.shift.set(self.shift.get().press(shift, modifiers()));
    }
    fn released(&self, wparam: WPARAM) -> bool {
        let shift = wparam.0 == VK_SHIFT.0 as usize;
        self.shift.get().released(shift, modifiers())
    }
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, ptim: Ref<ITfThreadMgr>, tid: u32) -> Result<()> {
        ITfTextInputProcessorEx_Impl::ActivateEx(self, ptim, tid, 0)
    }
    fn Deactivate(&self) -> Result<()> {
        guarded(|| {
            self.shift.set(ShiftTap::default());
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
            let keyboard = watch_keyboard(&manager, tid, &self.to_interface());
            let focus: ITfThreadFocusSink = self.to_interface();
            // SAFETY: 登记本对象的线程焦点 sink，停用时以同一 cookie 注销；失败照常激活。
            let focus = unsafe { source.AdviseSink(&ITfThreadFocusSink::IID, &focus) }.ok();
            // SAFETY: 只读本线程是否持有输入焦点。
            let focused = unsafe { manager.IsThreadFocus() }.is_ok_and(|focused| focused.as_bool());
            let active = Rc::new_cyclic(|owner| Active {
                indicator: indicator(&manager, owner),
                focus,
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
                keyboard,
            });
            *self.active.try_borrow_mut().map_err(busy)? = Some(active.clone());
            // 在持有焦点的线程里切到本输入法：任务栏此刻即显示本输入法图标。
            if focused {
                active.indicate(true);
            }
            Ok(())
        })
    }
}
impl ITfThreadFocusSink_Impl for TextService_Impl {
    fn OnSetThreadFocus(&self) -> Result<()> {
        guarded(|| {
            if let Some(active) = self.active() {
                active.indicate(true);
            }
            Ok(())
        })
    }
    fn OnKillThreadFocus(&self) -> Result<()> {
        guarded(|| {
            if let Some(active) = self.active() {
                active.indicate(false);
            }
            Ok(())
        })
    }
}
impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, foreground: BOOL) -> Result<()> {
        self.shift.set(ShiftTap::default());
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
            let deadline = Instant::now() + BUDGET;
            self.press(wparam);
            let active = self.active().ok_or(E_UNEXPECTED)?;
            Ok(active.judge(pic.ok()?, wparam, lparam, deadline).is_some())
        }))
    }
    fn OnKeyDown(&self, pic: Ref<ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        Ok(shielded(|| {
            let deadline = Instant::now() + BUDGET;
            self.press(wparam);
            let active = self.active().ok_or(E_UNEXPECTED)?;
            let context = pic.ok()?;
            match active.judge(context, wparam, lparam, deadline) {
                Some(key) => active.input(context, key, deadline),
                None => Ok(false),
            }
        }))
    }
    fn OnTestKeyUp(&self, _: Ref<ITfContext>, wparam: WPARAM, _: LPARAM) -> Result<BOOL> {
        Ok(shielded(|| {
            let deadline = Instant::now() + BUDGET;
            Ok(
                self.released(wparam)
                    && self.active().is_some_and(|active| active.online(deadline)),
            )
        }))
    }
    fn OnKeyUp(&self, pic: Ref<ITfContext>, wparam: WPARAM, _: LPARAM) -> Result<BOOL> {
        Ok(shielded(|| {
            let deadline = Instant::now() + BUDGET;
            if !self.released(wparam) {
                return Ok(false);
            }
            self.shift.set(ShiftTap::default());
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

impl ITfCompartmentEventSink_Impl for TextService_Impl {
    fn OnChange(&self, guid: *const GUID) -> Result<()> {
        guarded(|| {
            // SAFETY: 系统传入有效 GUID 指针，只比较值；重新打开引起的再次通知读到“开”即返回。
            if guid.is_null() || unsafe { *guid } != GUID_COMPARTMENT_KEYBOARD_OPENCLOSE {
                return Ok(());
            }
            self.active().map_or(Ok(()), |active| active.reopen())
        })
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
