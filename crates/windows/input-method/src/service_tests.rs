//! 合成线程管理器与进程内假后台：不激活系统 TSF、不注册 COM，也不连接用户正在运行的后台。
use super::*;
use crate::edit::tests::{document, layout_changed, text};
use crate::indicator::tests::right_click;
use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    thread,
};
use tigertype_contracts::{
    Candidate, Composition, InputState, KeyOutcome,
    protocol::{Rejection, Response},
};
use tigertype_windows_ipc::{Message, decode, encode};
use windows::{
    Win32::{
        Foundation::{E_NOTIMPL, HWND, POINT, RECT, S_OK},
        UI::{
            Input::KeyboardAndMouse::{
                VK_DOWN, VK_ESCAPE, VK_OEM_COMMA, VK_RETURN, VK_SPACE, VK_U,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, DispatchMessageW, GetWindowRect, MSG, PM_REMOVE,
                PeekMessageW, SendMessageW, WINDOW_EX_STYLE, WM_APP, WM_LBUTTONDOWN, WM_LBUTTONUP,
                WS_POPUP,
            },
        },
    },
    core::{BSTR, ComObject, OutRef, PCWSTR, w},
};

#[derive(Default)]
struct Manager {
    sinks: RefCell<Vec<u32>>,
    next: Cell<u32>,
    keys: Cell<Option<u32>>,
    refuse_keys: Cell<bool>,
    released: RefCell<Vec<&'static str>>,
    /// 键盘开闭状态：None 为 VT_EMPTY（新线程管理器的初值）；writers 记录每次写入的客户端编号。
    open: Cell<Option<i32>>,
    writers: RefCell<Vec<u32>>,
    watcher: RefCell<Option<ITfCompartmentEventSink>>,
    /// 已登记的语言栏按钮（任务栏输入法图标）。
    items: RefCell<Vec<ITfLangBarItem>>,
    /// 本线程是否持有输入焦点，与已登记的线程焦点 sink。
    thread_focus: Cell<bool>,
    focus: RefCell<Option<ITfThreadFocusSink>>,
}
/// 线程焦点 sink 的固定 cookie，与线程管理器 sink 的递增 cookie 区分。
const FOCUS_COOKIE: u32 = 50;
#[implement(
    ITfThreadMgr,
    ITfSource,
    ITfKeystrokeMgr,
    ITfCompartmentMgr,
    ITfLangBarItemMgr
)]
struct ThreadMgr(Rc<Manager>);
impl ITfLangBarItemMgr_Impl for ThreadMgr_Impl {
    fn EnumItems(&self) -> Result<IEnumTfLangBarItems> {
        Err(E_NOTIMPL.into())
    }
    fn GetItem(&self, _: *const GUID) -> Result<ITfLangBarItem> {
        Err(E_NOTIMPL.into())
    }
    fn AddItem(&self, item: Ref<ITfLangBarItem>) -> Result<()> {
        self.0.items.borrow_mut().push(item.ok()?.clone());
        Ok(())
    }
    fn RemoveItem(&self, item: Ref<ITfLangBarItem>) -> Result<()> {
        let item = item.ok()?;
        let mut items = self.0.items.borrow_mut();
        let index = items.iter().position(|i| i == item).ok_or(E_INVALIDARG)?;
        items.remove(index);
        self.0.released.borrow_mut().push("indicator");
        Ok(())
    }
    fn AdviseItemSink(
        &self,
        _: Ref<ITfLangBarItemSink>,
        _: *mut u32,
        _: *const GUID,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn UnadviseItemSink(&self, _: u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetItemFloatingRect(&self, _: u32, _: *const GUID) -> Result<RECT> {
        Err(E_NOTIMPL.into())
    }
    fn GetItemsStatus(&self, _: u32, _: *const GUID, _: *mut u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetItemNum(&self) -> Result<u32> {
        Err(E_NOTIMPL.into())
    }
    fn GetItems(
        &self,
        _: u32,
        _: OutRef<ITfLangBarItem>,
        _: *mut TF_LANGBARITEMINFO,
        _: *mut u32,
        _: *mut u32,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn AdviseItemsSink(
        &self,
        _: u32,
        _: *const Option<ITfLangBarItemSink>,
        _: *const GUID,
        _: *mut u32,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn UnadviseItemsSink(&self, _: u32, _: *const u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
}
/// 已登记的唯一图标按钮：须为输入模式按钮，且只是普通按钮（系统因此右键只调用 OnClick，不调用 InitMenu）。
fn button(manager: &Manager) -> ITfLangBarItemButton {
    let items = manager.items.borrow();
    assert_eq!(items.len(), 1);
    let mut info = TF_LANGBARITEMINFO::default();
    // SAFETY: 本测试按钮在本线程同步回调。
    unsafe { items[0].GetInfo(&mut info) }.unwrap();
    assert_eq!(info.guidItem, GUID_LBI_INPUTMODE);
    assert_eq!(info.dwStyle & TF_LBI_STYLE_BTN_MENU, 0);
    items[0].cast().unwrap()
}
/// 线程管理器级键盘开闭状态；写入后同步通知已登记的 sink，与系统一致。
#[implement(ITfCompartment, ITfSource)]
struct Compartment(Rc<Manager>);
impl ITfCompartmentMgr_Impl for ThreadMgr_Impl {
    fn GetCompartment(&self, guid: *const GUID) -> Result<ITfCompartment> {
        // SAFETY: 调用方传入有效 GUID 指针。
        if unsafe { *guid } != GUID_COMPARTMENT_KEYBOARD_OPENCLOSE {
            return Err(E_INVALIDARG.into());
        }
        Ok(Compartment(self.0.clone()).into())
    }
    fn ClearCompartment(&self, _: u32, _: *const GUID) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn EnumCompartments(&self) -> Result<windows::Win32::System::Com::IEnumGUID> {
        Err(E_NOTIMPL.into())
    }
}
impl ITfCompartment_Impl for Compartment_Impl {
    fn SetValue(&self, tid: u32, value: *const VARIANT) -> Result<()> {
        // SAFETY: 调用方传入有效 VARIANT 指针。
        self.0.open.set(Some(i32::try_from(unsafe { &*value })?));
        self.0.writers.borrow_mut().push(tid);
        let watcher = self.0.watcher.borrow().clone();
        if let Some(watcher) = watcher {
            // SAFETY: 本线程同步通知；sink 可在回调内再次写入。
            unsafe { watcher.OnChange(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE)? };
        }
        Ok(())
    }
    fn GetValue(&self) -> Result<VARIANT> {
        Ok(self
            .0
            .open
            .get()
            .map_or_else(VARIANT::default, VARIANT::from))
    }
}
impl ITfSource_Impl for Compartment_Impl {
    fn AdviseSink(&self, iid: *const GUID, sink: Ref<IUnknown>) -> Result<u32> {
        // SAFETY: 调用方传入有效 IID 指针。
        if unsafe { *iid } != ITfCompartmentEventSink::IID || self.0.watcher.borrow().is_some() {
            return Err(E_INVALIDARG.into());
        }
        *self.0.watcher.borrow_mut() = Some(sink.ok()?.cast()?);
        Ok(99)
    }
    fn UnadviseSink(&self, cookie: u32) -> Result<()> {
        if cookie != 99 || self.0.watcher.take().is_none() {
            return Err(E_INVALIDARG.into());
        }
        self.0.released.borrow_mut().push("keyboard");
        Ok(())
    }
}
impl ITfSource_Impl for ThreadMgr_Impl {
    fn AdviseSink(&self, iid: *const GUID, sink: Ref<IUnknown>) -> Result<u32> {
        // SAFETY: 调用方传入有效 IID 指针；只核对类型，不保存未知接口。
        let iid = unsafe { *iid };
        if iid == ITfThreadFocusSink::IID && self.0.focus.borrow().is_none() {
            *self.0.focus.borrow_mut() = Some(sink.ok()?.cast()?);
            return Ok(FOCUS_COOKIE);
        }
        if iid != ITfThreadMgrEventSink::IID || sink.is_null() {
            return Err(E_INVALIDARG.into());
        }
        self.0.next.set(self.0.next.get() + 1);
        self.0.sinks.borrow_mut().push(self.0.next.get());
        Ok(self.0.next.get())
    }
    fn UnadviseSink(&self, cookie: u32) -> Result<()> {
        if cookie == FOCUS_COOKIE {
            self.0.focus.take().ok_or(E_INVALIDARG)?;
            self.0.released.borrow_mut().push("focus");
            return Ok(());
        }
        let mut sinks = self.0.sinks.borrow_mut();
        let index = sinks
            .iter()
            .position(|c| *c == cookie)
            .ok_or(E_INVALIDARG)?;
        sinks.remove(index);
        self.0.released.borrow_mut().push("thread");
        Ok(())
    }
}
impl ITfKeystrokeMgr_Impl for ThreadMgr_Impl {
    fn AdviseKeyEventSink(&self, tid: u32, sink: Ref<ITfKeyEventSink>, _: BOOL) -> Result<()> {
        if self.0.refuse_keys.get() || sink.is_null() || self.0.keys.get().is_some() {
            return Err(E_FAIL.into());
        }
        self.0.keys.set(Some(tid));
        Ok(())
    }
    fn UnadviseKeyEventSink(&self, tid: u32) -> Result<()> {
        if self.0.keys.take() != Some(tid) {
            return Err(E_INVALIDARG.into());
        }
        self.0.released.borrow_mut().push("keys");
        Ok(())
    }
    fn GetForeground(&self) -> Result<GUID> {
        Err(E_NOTIMPL.into())
    }
    fn TestKeyDown(&self, _: WPARAM, _: LPARAM) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn TestKeyUp(&self, _: WPARAM, _: LPARAM) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn KeyDown(&self, _: WPARAM, _: LPARAM) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn KeyUp(&self, _: WPARAM, _: LPARAM) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn GetPreservedKey(&self, _: Ref<ITfContext>, _: *const TF_PRESERVEDKEY) -> Result<GUID> {
        Err(E_NOTIMPL.into())
    }
    fn IsPreservedKey(&self, _: *const GUID, _: *const TF_PRESERVEDKEY) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn PreserveKey(
        &self,
        _: u32,
        _: *const GUID,
        _: *const TF_PRESERVEDKEY,
        _: &PCWSTR,
        _: u32,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn UnpreserveKey(&self, _: *const GUID, _: *const TF_PRESERVEDKEY) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn SetPreservedKeyDescription(&self, _: *const GUID, _: &PCWSTR, _: u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetPreservedKeyDescription(&self, _: *const GUID) -> Result<BSTR> {
        Err(E_NOTIMPL.into())
    }
    fn SimulatePreservedKey(&self, _: Ref<ITfContext>, _: *const GUID) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
}
impl ITfThreadMgr_Impl for ThreadMgr_Impl {
    fn Activate(&self) -> Result<u32> {
        Err(E_NOTIMPL.into())
    }
    fn Deactivate(&self) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn CreateDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        Err(E_NOTIMPL.into())
    }
    fn EnumDocumentMgrs(&self) -> Result<IEnumTfDocumentMgrs> {
        Err(E_NOTIMPL.into())
    }
    fn GetFocus(&self) -> Result<ITfDocumentMgr> {
        Err(E_NOTIMPL.into())
    }
    fn SetFocus(&self, _: Ref<ITfDocumentMgr>) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn AssociateFocus(&self, _: HWND, _: Ref<ITfDocumentMgr>) -> Result<ITfDocumentMgr> {
        Err(E_NOTIMPL.into())
    }
    fn IsThreadFocus(&self) -> Result<BOOL> {
        Ok(self.0.thread_focus.get().into())
    }
    fn GetFunctionProvider(&self, _: *const GUID) -> Result<ITfFunctionProvider> {
        Err(E_NOTIMPL.into())
    }
    fn EnumFunctionProviders(&self) -> Result<IEnumTfFunctionProviders> {
        Err(E_NOTIMPL.into())
    }
    fn GetGlobalCompartment(&self) -> Result<ITfCompartmentMgr> {
        Err(E_NOTIMPL.into())
    }
}

/// 每个测试独立命名空间，预期映像为本测试进程，绝不触碰正式 backend 命名空间；候选窗保持隐藏。
fn service() -> ComObject<TextService> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    ComObject::new(TextService {
        namespace: format!(
            "tsf-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ),
        image: Some(std::env::current_exe().unwrap()),
        quiet: true,
        // 与正式 COM 构造一致，在激活前加载；测试直接构造对象不能依赖机器预装字体。
        font: Some(Rc::new(
            tigertype_windows_fonts::PrivateFont::load(
                &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../resources/fonts/LXGWWenKaiGBScreen.ttf"),
            )
            .unwrap(),
        )),
        ..TextService::new()
    })
}
fn manager() -> (Rc<Manager>, ITfThreadMgr) {
    let manager = Rc::new(Manager::default());
    let interface = ThreadMgr(manager.clone()).into();
    (manager, interface)
}
/// 测试机调度抖动不应造成误报；生产按键路径仍使用 50ms 预算。
fn later() -> Instant {
    Instant::now() + Duration::from_secs(5)
}
const WORDS: [&str; 2] = ["的", "地"];
/// 组合非空时给出两个候选并高亮首项；快照编号逐次递增。
fn state(buffer: &str, revision: u64) -> InputState {
    let candidates: Vec<_> = if buffer.is_empty() {
        Vec::new()
    } else {
        WORDS
            .iter()
            .zip(["1", "2"])
            .map(|(text, label)| Candidate {
                text: (*text).into(),
                label: label.into(),
                ..Default::default()
            })
            .collect()
    };
    InputState {
        composing: !buffer.is_empty(),
        composition: Composition {
            text: buffer.into(),
            cursor: buffer.len(),
            selection: 0..buffer.len(),
        },
        highlighted: (!candidates.is_empty()).then_some(0),
        candidates,
        page_size: 9,
        revision,
        ..Default::default()
    }
}

/// 进程内单槽假后台：真实管道、握手与协议校验；字母累积组合，空格提交首选，选字提交对应候选，
/// 切换中英翻转英文模式；唤起请求记录目标窗口、焦点报告记录取得/失去后确认。
#[allow(clippy::type_complexity)]
fn backend(
    namespace: &str,
) -> (
    thread::JoinHandle<(Vec<Command>, Vec<Panel>, Vec<bool>)>,
    mpsc::Receiver<()>,
) {
    let label = format!("{namespace}-request-0");
    let (ready, bound) = mpsc::channel();
    let server = thread::spawn(move || {
        let runtime = Runtime::new().unwrap();
        let listener = runtime.bind(&label).unwrap();
        ready.send(()).unwrap();
        let mut pipe = listener.accept(later()).unwrap();
        assert_eq!(
            decode(&pipe.receive_frame(later()).unwrap()).unwrap(),
            Message::Hello
        );
        let welcome = Message::Welcome {
            generation: 7,
            host: 1,
        };
        pipe.send_frame(&encode(&welcome).unwrap(), later())
            .unwrap();
        let (mut log, mut shown, mut indicated, mut buffer, mut revision) =
            (Vec::new(), Vec::new(), Vec::new(), String::new(), 1);
        let (mut ascii, mut full) = (false, false);
        // 宿主停用即断开管道，读取失败结束服务。
        while let Ok(frame) = pipe.receive_frame(later()) {
            let request = match decode(&frame).unwrap() {
                Message::Request(request) => request,
                Message::Show(panel) => {
                    shown.push(panel);
                    pipe.send_frame(&encode(&Message::Shown).unwrap(), later())
                        .unwrap();
                    continue;
                }
                Message::Indicator(visible) => {
                    indicated.push(visible);
                    pipe.send_frame(&encode(&Message::Noted).unwrap(), later())
                        .unwrap();
                    continue;
                }
                other => panic!("宿主不应发送 {other:?}"),
            };
            log.push(request.command.clone());
            revision += 1;
            let commit = |buffer: &mut String, index: usize| {
                buffer.clear();
                Reply::Key(KeyOutcome {
                    handled: true,
                    commit: Some(WORDS[index].into()),
                    state: state("", revision),
                })
            };
            // 假后台只模拟按键前置清空；完整准入/焦点/代次由 backend 的真实调度测试覆盖。
            if matches!(request.command, Command::Key { prepare, .. } if prepare != tigertype_contracts::protocol::Prepare::Keep)
            {
                buffer.clear();
            }
            let reply = match request.command {
                // 同引擎：英文模式不处理字母。
                Command::Key {
                    key: Key::Character(_),
                    ..
                } if ascii => Reply::Key(KeyOutcome {
                    handled: false,
                    commit: None,
                    state: InputState {
                        ascii_mode: true,
                        ..state("", revision)
                    },
                }),
                Command::Key {
                    key: Key::Character(c),
                    ..
                } => {
                    buffer.push(c);
                    Reply::Key(KeyOutcome {
                        handled: true,
                        commit: None,
                        state: state(&buffer, revision),
                    })
                }
                Command::Key {
                    key: Key::Space, ..
                } => commit(&mut buffer, 0),
                Command::Key {
                    key: Key::ToggleAscii,
                    ..
                } => {
                    // 同引擎：组合中切换把编码原样上屏。
                    ascii = !ascii;
                    let raw = std::mem::take(&mut buffer);
                    Reply::Key(KeyOutcome {
                        handled: true,
                        commit: (!raw.is_empty()).then_some(raw),
                        state: InputState {
                            ascii_mode: ascii,
                            ..state(&buffer, revision)
                        },
                    })
                }
                // 全角只翻开关，组合原样保留。
                Command::Key {
                    key: Key::ToggleFullShape,
                    ..
                } => {
                    full = !full;
                    Reply::Key(KeyOutcome {
                        handled: true,
                        commit: None,
                        state: InputState {
                            full_shape: full,
                            ..state(&buffer, revision)
                        },
                    })
                }
                Command::Select { index, .. } => commit(&mut buffer, index as usize),
                _ => Reply::Rejected(Rejection::Unavailable),
            };
            let response = Message::Response(Response {
                route: request.route,
                reply,
            });
            pipe.send_frame(&encode(&response).unwrap(), later())
                .unwrap();
        }
        (log, shown, indicated)
    });
    (server, bound)
}

#[test]
fn class_factory_exposes_only_the_tip_and_the_module_never_unloads() {
    let mut object = ptr::null_mut();
    // SAFETY: 所有指针均指向本测试的有效局部变量或常量。
    unsafe {
        assert_eq!(
            DllGetClassObject(&GUID::zeroed(), &IClassFactory::IID, &mut object),
            CLASS_E_CLASSNOTAVAILABLE
        );
        assert!(object.is_null());
        assert_eq!(
            DllGetClassObject(&CLSID_TEXT_SERVICE, &IClassFactory::IID, ptr::null_mut()),
            E_POINTER
        );
        assert_eq!(
            DllGetClassObject(&CLSID_TEXT_SERVICE, &IClassFactory::IID, &mut object),
            S_OK
        );
    }
    // SAFETY: 成功返回的工厂引用由 from_raw 接管并在作用域结束时释放。
    let factory = unsafe { IClassFactory::from_raw(object) };
    let outer: IUnknown = document().1.cast().unwrap();
    // SAFETY: 合法的类工厂调用；聚合必须被拒绝。
    let aggregated = unsafe { factory.CreateInstance::<_, ITfTextInputProcessorEx>(&outer) };
    assert_eq!(aggregated.unwrap_err().code(), CLASS_E_NOAGGREGATION);
    // SAFETY: 无外部聚合的正常实例化。
    let tip: ITfTextInputProcessorEx = unsafe { factory.CreateInstance(None) }.unwrap();
    assert!(tip.cast::<ITfTextInputProcessor>().is_ok());
    assert!(tip.cast::<ITfKeyEventSink>().is_ok());
    assert!(tip.cast::<ITfThreadMgrEventSink>().is_ok());
    assert!(tip.cast::<ITfThreadFocusSink>().is_ok());
    // 显示属性与 UILess 候选接口未实现，不得冒充。
    assert!(tip.cast::<ITfDisplayAttributeProvider>().is_err());
    assert!(tip.cast::<ITfCandidateListUIElement>().is_err());
    assert_eq!(DllCanUnloadNow(), S_FALSE);
}

#[test]
fn activation_pairs_sinks_and_a_refused_key_sink_rolls_back_the_thread_sink() {
    let (manager, thread_mgr) = manager();
    let object = service();
    let tip: ITfTextInputProcessorEx = object.to_interface();
    manager.refuse_keys.set(true);
    // SAFETY: 合成线程管理器在本线程同步回调。
    assert!(unsafe { tip.ActivateEx(&thread_mgr, 7, 0) }.is_err());
    assert!(manager.sinks.borrow().is_empty());
    assert_eq!(*manager.released.borrow(), ["thread"]);
    assert!(object.active().is_none());
    manager.refuse_keys.set(false);
    // SAFETY: 同上；第二次激活必须拒绝，不能重复登记。
    unsafe {
        tip.ActivateEx(&thread_mgr, 7, 0).unwrap();
        assert!(tip.ActivateEx(&thread_mgr, 7, 0).is_err());
    }
    assert_eq!(
        (manager.sinks.borrow().len(), manager.keys.get()),
        (1, Some(7))
    );
    // SAFETY: 停用以同一编号/cookie 注销；重复停用无副作用。
    unsafe {
        tip.Deactivate().unwrap();
        tip.Deactivate().unwrap();
    }
    assert!(manager.sinks.borrow().is_empty() && manager.keys.get().is_none());
    // 失败的激活不登记图标与线程焦点 sink；成功激活各登记一个，停用时注销。
    assert!(manager.items.borrow().is_empty() && manager.focus.borrow().is_none());
    assert_eq!(
        *manager.released.borrow(),
        ["thread", "indicator", "keys", "thread", "focus", "keyboard"]
    );
}

#[test]
fn activation_opens_a_closed_keyboard_and_reopens_it_until_deactivated() {
    let (manager, thread_mgr) = manager();
    let tip: ITfTextInputProcessorEx = service().to_interface();
    let close = || {
        let compartments: ITfCompartmentMgr = thread_mgr.cast().unwrap();
        // SAFETY: 合成容器在本线程同步回调；模拟宿主或开关热键以其他客户端编号关闭键盘。
        unsafe {
            compartments
                .GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE)
                .unwrap()
                .SetValue(3, &VARIANT::from(0i32))
                .unwrap();
        }
    };
    // 新线程管理器的开闭状态为空：激活即以本客户端编号打开，否则宿主不把按键交给 TIP。
    // SAFETY: 合成线程管理器在本线程同步回调。
    unsafe { tip.ActivateEx(&thread_mgr, 7, 0) }.unwrap();
    assert_eq!(
        (manager.open.get(), manager.writers.take()),
        (Some(1), vec![7])
    );
    close();
    assert_eq!(
        (manager.open.get(), manager.writers.take()),
        (Some(1), vec![3, 7])
    );
    // SAFETY: 停用注销开闭状态 sink，之后不再干预。
    unsafe { tip.Deactivate() }.unwrap();
    close();
    assert_eq!(manager.open.get(), Some(0));
    // 已经打开时激活不重复写入。
    manager.open.set(Some(1));
    manager.writers.take();
    // SAFETY: 同上。
    unsafe {
        tip.ActivateEx(&thread_mgr, 7, 0).unwrap();
        tip.Deactivate().unwrap();
    }
    assert!(manager.writers.borrow().is_empty());
    assert_eq!(
        *manager.released.borrow(),
        [
            "indicator",
            "keys",
            "thread",
            "focus",
            "keyboard",
            "indicator",
            "keys",
            "thread",
            "focus",
            "keyboard"
        ]
    );
}

#[test]
fn unavailable_backend_or_inactive_service_passes_keys_without_touching_the_document() {
    let (doc, context) = document();
    let object = service();
    let keys: ITfKeyEventSink = object.to_interface();
    // SAFETY: 未激活的服务只能放行按键。
    unsafe {
        for vk in [VK_SHIFT, VK_SPACE] {
            assert!(
                !keys
                    .OnKeyDown(&context, WPARAM(vk.0 as usize), LPARAM(0))
                    .unwrap()
                    .as_bool()
            );
        }
    }
    let (manager, thread_mgr) = manager();
    let tip: ITfTextInputProcessorEx = object.to_interface();
    // SAFETY: 合成线程管理器；命名空间内没有后台，连接立即失败。
    unsafe { tip.ActivateEx(&thread_mgr, 7, 0) }.unwrap();
    let active = object.active().unwrap();
    // 后台不在：右键弹出的真实菜单各项置灰（内容见 indicator 测试），Enter 收起菜单但选不中，什么都不发。
    right_click(&button(&manager), &[VK_DOWN, VK_RETURN]);
    // 后台不在时预判不吃任何键：宿主未必把“预判吃下、处理时放行”的原键交还应用。
    assert!(!active.ready(later()));
    for vk in [VK_U, VK_OEM_COMMA, VK_SPACE] {
        // SAFETY: 合成上下文，同步调用本对象的按键 sink。
        let eaten = unsafe { keys.OnTestKeyDown(&context, WPARAM(vk.0 as usize), LPARAM(0)) };
        assert!(!eaten.unwrap().as_bool(), "{vk:?}");
    }
    assert!(
        !active
            .input(&context, Key::Character('u'), later())
            .unwrap()
    );
    assert_eq!((text(&doc), doc.selection.get()), (String::new(), (0, 0)));
    assert!(active.current_popup().is_none());
    // SAFETY: 停用释放 sink。
    unsafe { tip.Deactivate() }.unwrap();
}

/// 像宿主一样经按键 sink 走回调：预判不吃键就不再调用处理回调；返回（预判，处理）。
/// 预判吃下而处理放行即宿主可能吞键，调用方断言二者一致。
fn press(keys: &ITfKeyEventSink, context: &ITfContext, vk: VIRTUAL_KEY) -> (bool, bool) {
    let (w, l) = (WPARAM(vk.0 as usize), LPARAM(0));
    // SAFETY: 合成上下文，同步调用本对象的按键 sink。
    unsafe {
        if !keys.OnTestKeyDown(context, w, l).unwrap().as_bool() {
            return (false, false);
        }
        (true, keys.OnKeyDown(context, w, l).unwrap().as_bool())
    }
}
/// 单击 Shift：按下与抬起各经预判，吃下才调用处理回调；返回抬起的（预判，处理）。
fn tap_shift(keys: &ITfKeyEventSink, context: &ITfContext) -> (bool, bool) {
    let (w, l) = (WPARAM(VK_SHIFT.0 as usize), LPARAM(0));
    assert_eq!(press(keys, context, VK_SHIFT), (false, false));
    // SAFETY: 同上。
    unsafe {
        if !keys.OnTestKeyUp(context, w, l).unwrap().as_bool() {
            return (false, false);
        }
        (true, keys.OnKeyUp(context, w, l).unwrap().as_bool())
    }
}

#[test]
fn selection_and_read_only_contexts_get_the_same_verdict_in_test_and_key_down() {
    let object = service();
    let (server, bound) = backend(&object.namespace);
    bound.recv().unwrap();
    let (_manager, thread_mgr) = manager();
    let tip: ITfTextInputProcessorEx = object.to_interface();
    // SAFETY: 合成线程管理器在本线程同步回调。
    unsafe { tip.ActivateEx(&thread_mgr, 7, 0) }.unwrap();
    let keys: ITfKeyEventSink = object.to_interface();
    let (doc, context) = document();
    // 只读上下文或读不到宿主状态：预判即放行，不发请求。
    doc.readonly.set(true);
    assert_eq!(press(&keys, &context, VK_U), (false, false));
    doc.readonly.set(false);
    doc.fail_status.set(true);
    assert_eq!(press(&keys, &context, VK_U), (false, false));
    doc.fail_status.set(false);
    // 后台在线、选中已有文字：预判吃下，处理时以组合替换选区，而不是拒绝后吞键。
    *doc.text.borrow_mut() = "原文".encode_utf16().collect();
    doc.selection.set((0, 2));
    assert_eq!(press(&keys, &context, VK_U), (true, true));
    assert_eq!((text(&doc), doc.selection.get()), ("u".into(), (1, 1)));
    // SAFETY: 停用取消组合并关闭管道。
    unsafe { tip.Deactivate() }.unwrap();
    let (log, ..) = server.join().unwrap();
    assert_eq!(
        log.iter()
            .filter(|c| matches!(c, Command::Key { .. }))
            .count(),
        1,
        "{log:?}"
    );
}

#[test]
fn backend_loss_while_composing_withdraws_the_preedit_and_hides_candidates() {
    // 下一个字母与单击 Shift（按下不吃、抬起预判）两条路径都须清理。
    for shift in [false, true] {
        lose_backend_while_composing(shift);
    }
}
fn lose_backend_while_composing(shift: bool) {
    let object = service();
    let (server, bound) = backend(&object.namespace);
    bound.recv().unwrap();
    let (_manager, thread_mgr) = manager();
    let tip: ITfTextInputProcessorEx = object.to_interface();
    // SAFETY: 合成线程管理器在本线程同步回调。
    unsafe { tip.ActivateEx(&thread_mgr, 7, 0) }.unwrap();
    let active = object.active().unwrap();
    let keys: ITfKeyEventSink = object.to_interface();
    let (doc, context) = document();
    assert_eq!(press(&keys, &context, VK_U), (true, true));
    assert_eq!(text(&doc), "u");
    assert!(shown(&active).is_some());
    // 断开连接即后台消失（单槽假后台随之退出，重连失败）；候选窗此刻仍显示旧组合。
    active.host.borrow_mut().close();
    server.join().unwrap();
    assert!(shown(&active).is_some());
    // 下一键预判放行，同时撤回自有组合、收起候选，残余编码不留给宿主。
    let verdict = if shift {
        tap_shift(&keys, &context)
    } else {
        press(&keys, &context, VK_U)
    };
    assert_eq!(verdict, (false, false), "shift={shift}");
    assert_eq!(text(&doc), "");
    assert!(shown(&active).is_none());
    // SAFETY: 停用释放 sink。
    unsafe { tip.Deactivate() }.unwrap();
}

#[test]
fn taskbar_icon_follows_the_chinese_english_toggle_from_the_backend() {
    let object = service();
    let (server, bound) = backend(&object.namespace);
    bound.recv().unwrap();
    let (manager, thread_mgr) = manager();
    let tip: ITfTextInputProcessorEx = object.to_interface();
    // SAFETY: 合成线程管理器在本线程同步回调。
    unsafe { tip.ActivateEx(&thread_mgr, 7, 0) }.unwrap();
    let active = object.active().unwrap();
    let (doc, context) = document();
    let button = button(&manager);
    // SAFETY: 本线程同步回调本 TIP 的图标按钮。
    let tooltip = || unsafe { button.GetTooltipString() }.unwrap().to_string();
    // SAFETY: 同上；左键单击图标。
    let click = || unsafe { button.OnClick(TF_LBI_CLK_LEFT, POINT::default(), &RECT::default()) };
    assert!(tooltip().contains("中文"));
    // 单击 Shift 经后台翻转，图标按回复显示；再切回中文。
    assert!(active.input(&context, Key::ToggleAscii, later()).unwrap());
    assert!(tooltip().contains("英文"));
    assert!(active.input(&context, Key::ToggleAscii, later()).unwrap());
    assert!(tooltip().contains("中文"));
    // 单击图标同样切换：无组合时直接交换，不碰文档。
    click().unwrap();
    assert!(tooltip().contains("英文"));
    assert_eq!(text(&doc), "");
    click().unwrap();
    assert!(tooltip().contains("中文"));
    // 组合中单击图标：经异步会话把编码原样上屏、收起候选并切到英文，同单击 Shift。
    assert!(
        active
            .input(&context, Key::Character('u'), later())
            .unwrap()
    );
    assert!(shown(&active).is_some());
    click().unwrap();
    assert_eq!(text(&doc), "u");
    assert!(tooltip().contains("英文") && shown(&active).is_none());
    assert!(
        !active
            .input(&context, Key::Character('u'), later())
            .unwrap()
    );
    // 右键菜单第二项显示当前全角状态，点击只翻开关，不碰文档、不唤起窗口。
    assert!(!active.look().1.full);
    right_click(&button, &[VK_DOWN, VK_DOWN, VK_RETURN]);
    assert!(active.look().1.full);
    assert_eq!(text(&doc), "u");
    // SAFETY: 停用注销图标与 sink 并关闭管道。
    unsafe { tip.Deactivate() }.unwrap();
    let (log, panels, _) = server.join().unwrap();
    assert!(panels.is_empty());
    let toggles = log
        .iter()
        .filter(|command| {
            matches!(
                command,
                Command::Key {
                    key: Key::ToggleAscii,
                    ..
                }
            )
        })
        .count();
    assert_eq!(toggles, 5, "{log:?}");
    assert_eq!(
        log.last(),
        Some(&Command::Key {
            key: Key::ToggleFullShape,
            epoch: 1,
            prepare: tigertype_contracts::protocol::Prepare::Keep
        })
    );
}

#[test]
fn thread_focus_tells_the_backend_whether_the_taskbar_shows_this_input_method() {
    let object = service();
    let (server, bound) = backend(&object.namespace);
    bound.recv().unwrap();
    let (manager, thread_mgr) = manager();
    manager.thread_focus.set(true);
    let tip: ITfTextInputProcessorEx = object.to_interface();
    // 在持有焦点的线程里激活即报告取得；之后失去、再取得各报告一次。
    // SAFETY: 合成线程管理器在本线程同步回调。
    unsafe { tip.ActivateEx(&thread_mgr, 7, 0) }.unwrap();
    let sink = manager.focus.borrow().clone().unwrap();
    // SAFETY: 模拟系统的线程焦点通知。
    unsafe {
        sink.OnKillThreadFocus().unwrap();
        sink.OnSetThreadFocus().unwrap();
    }
    // 停用断开连接即等同失去，不另发报告。
    // SAFETY: 停用注销 sink 并关闭管道。
    unsafe { tip.Deactivate() }.unwrap();
    assert_eq!(
        *manager.released.borrow(),
        ["indicator", "keys", "thread", "focus", "keyboard"]
    );
    let (log, panels, indicated) = server.join().unwrap();
    assert_eq!(indicated, [true, false, true]);
    // 焦点报告不打开会话、不唤起窗口。
    assert!(log.is_empty() && panels.is_empty());
}

/// 当前展示的编号与候选行；未显示时为 None。
fn shown(active: &Active) -> Option<(u32, Vec<RECT>)> {
    let popup = active.current_popup()?;
    let view = popup.view.borrow();
    view.state.as_ref()?;
    Some((view.serial, view.layout.rows.clone()))
}
/// 经真实窗口消息点击候选：按下/抬起发给候选窗，再派发它投递给自己的点击通知。
fn click(active: &Active, row: &RECT) {
    let hwnd = active.current_popup().unwrap().hwnd;
    let at = LPARAM(((((row.top + row.bottom) / 2) << 16) | ((row.left + row.right) / 2)) as isize);
    let mut message = MSG::default();
    // SAFETY: 只向本测试线程的隐藏候选窗发送并派发消息，不移动真实鼠标。
    unsafe {
        SendMessageW(hwnd, WM_LBUTTONDOWN, None, Some(at));
        SendMessageW(hwnd, WM_LBUTTONUP, None, Some(at));
        while PeekMessageW(&mut message, Some(hwnd), WM_APP, WM_APP + 0x100, PM_REMOVE).as_bool() {
            DispatchMessageW(&message);
        }
    }
}

#[test]
fn activated_service_shows_candidates_selects_by_click_and_refocuses_after_focus_changes() {
    let object = service();
    let (server, bound) = backend(&object.namespace);
    bound.recv().unwrap();
    let (manager, thread_mgr) = manager();
    let tip: ITfTextInputProcessorEx = object.to_interface();
    // SAFETY: 合成线程管理器在本线程同步回调。
    unsafe { tip.ActivateEx(&thread_mgr, 7, 0) }.unwrap();
    let active = object.active().unwrap();
    let (doc, context) = document();
    assert!(
        active
            .input(&context, Key::Character('u'), later())
            .unwrap()
    );
    assert_eq!(text(&doc), "u");
    let (serial, rows) = shown(&active).unwrap();
    assert_eq!(rows.len(), 2);
    // 候选窗贴着合成视图给出的组合矩形放置。
    let popup = active.current_popup().unwrap();
    let mut window = RECT::default();
    // SAFETY: 只读本测试候选窗的位置。
    unsafe { GetWindowRect(popup.hwnd, &mut window).unwrap() };
    assert!(window.top == 120 || window.bottom == 100);
    assert!(active.input(&context, Key::Space, later()).unwrap());
    assert_eq!(text(&doc), "的");
    assert!(shown(&active).is_none() && !popup.showing(serial));
    // 后台可用：走系统右键的真实入口 OnClick(右键)，真实菜单里选“设置”“查字”（三个状态项与分隔线之后），
    // Esc 取消不发；都经同一连接唤起，不另开会话（下方请求记录里没有新的 Open）。
    let button = button(&manager);
    right_click(
        &button,
        &[VK_DOWN, VK_DOWN, VK_DOWN, VK_DOWN, VK_DOWN, VK_RETURN],
    );
    right_click(&button, &[VK_DOWN, VK_DOWN, VK_DOWN, VK_DOWN, VK_RETURN]);
    right_click(&button, &[VK_ESCAPE]);
    let focus: ITfThreadMgrEventSink = object.to_interface();
    // SAFETY: 模拟文档焦点变化；参数为空表示焦点离开本线程文档。
    unsafe { focus.OnSetFocus(None, None) }.unwrap();
    assert!(
        active
            .input(&context, Key::Character('a'), later())
            .unwrap()
    );
    assert_eq!(text(&doc), "的a");
    let (serial, rows) = shown(&active).unwrap();
    // 旧展示编号的点击不发请求；当前展示点击第二项经异步会话提交“地”。
    active.click(serial.wrapping_sub(1), 1);
    assert_eq!(text(&doc), "的a");
    click(&active, &rows[1]);
    assert_eq!(text(&doc), "的地");
    assert!(shown(&active).is_none());
    assert!(
        active
            .input(&context, Key::Character('a'), later())
            .unwrap()
    );
    // 换到另一上下文：旧上下文的自有组合被取消、候选窗收起，新上下文按新焦点处理。
    let (other, second) = document();
    assert!(active.input(&second, Key::Character('b'), later()).unwrap());
    assert_eq!((text(&doc), text(&other)), ("的地".into(), "b".into()));
    assert!(shown(&active).is_some());
    // SAFETY: 停用取消剩余组合、销毁候选窗、注销 sink 并关闭管道。
    unsafe { tip.Deactivate() }.unwrap();
    assert_eq!(text(&other), "");
    assert!(active.current_popup().is_none() && !popup.showing(serial));
    assert_eq!(
        *manager.released.borrow(),
        ["indicator", "keys", "thread", "focus", "keyboard"]
    );
    use tigertype_contracts::protocol::Prepare;
    let key = |key, epoch, prepare| Command::Key {
        key,
        epoch,
        prepare,
    };
    let (log, panels, indicated) = server.join().unwrap();
    assert_eq!(panels, [Panel::Settings, Panel::Lookup]);
    assert!(indicated.is_empty());
    let Command::Select { revision, .. } = log[3] else {
        panic!("第四个请求应是选字：{log:?}");
    };
    // 开会话、重新取焦点都不再单独往返，选择仍绑定原快照和代次。
    assert_eq!(
        log,
        [
            key(Key::Character('u'), 1, Prepare::Open),
            key(Key::Space, 1, Prepare::Keep),
            key(Key::Character('a'), 2, Prepare::Focus),
            Command::Select {
                revision,
                index: 1,
                epoch: 2
            },
            key(Key::Character('a'), 2, Prepare::Keep),
            key(Key::Character('b'), 3, Prepare::Focus),
        ]
    );
}

#[test]
fn late_host_layout_shows_candidates_and_a_missing_layout_keeps_them_in_place() {
    let object = service();
    let (server, bound) = backend(&object.namespace);
    bound.recv().unwrap();
    let (_manager, thread_mgr) = manager();
    let tip: ITfTextInputProcessorEx = object.to_interface();
    // SAFETY: 合成线程管理器在本线程同步回调。
    unsafe { tip.ActivateEx(&thread_mgr, 7, 0) }.unwrap();
    let active = object.active().unwrap();
    let (doc, context) = document();
    let window = || {
        let mut window = RECT::default();
        // SAFETY: 只读本测试候选窗的位置。
        unsafe { GetWindowRect(active.current_popup().unwrap().hwnd, &mut window).unwrap() };
        window
    };
    // 首键时宿主还给不出组合矩形（Chromium 等异步回报），也没有系统光标、上次位置与输入区域：先不显示。
    doc.no_layout.set(true);
    assert!(
        active
            .input(&context, Key::Character('u'), later())
            .unwrap()
    );
    assert!(shown(&active).is_none());
    // 宿主补来布局：同一快照立即显示在组合矩形处。
    doc.no_layout.set(false);
    layout_changed(&doc);
    let (first, _) = shown(&active).unwrap();
    let placed = window();
    assert!(placed.top == 120 || placed.bottom == 100);
    // 组合中途又拿不到布局：不收起，按新快照原位刷新。
    doc.no_layout.set(true);
    assert!(
        active
            .input(&context, Key::Character('a'), later())
            .unwrap()
    );
    let (second, rows) = shown(&active).unwrap();
    assert_ne!(second, first);
    assert_eq!(window().left, placed.left);
    // 布局通知只移动窗口：展示编号不变，此前看到的候选仍可点选。
    doc.no_layout.set(false);
    layout_changed(&doc);
    assert_eq!(shown(&active).unwrap().0, second);
    click(&active, &rows[1]);
    assert_eq!(text(&doc), "地");
    assert!(shown(&active).is_none());
    // 未组合时的布局变化不显示候选窗。
    layout_changed(&doc);
    assert!(shown(&active).is_none());
    // SAFETY: 停用取消剩余组合、销毁候选窗、注销 sink 并关闭管道。
    unsafe { tip.Deactivate() }.unwrap();
    let (log, _, _) = server.join().unwrap();
    assert!(matches!(log.last(), Some(Command::Select { index: 1, .. })));
}

#[test]
fn destroyed_candidate_window_is_recreated_for_the_next_input() {
    // 宿主换窗口时 Win32 自动销毁 owned popup；服务仍活着，下一键必须重建而非永久复用死句柄。
    let object = service();
    let (server, bound) = backend(&object.namespace);
    bound.recv().unwrap();
    let (_, thread_mgr) = manager();
    let tip: ITfTextInputProcessorEx = object.to_interface();
    // SAFETY: 合成线程管理器，不注册系统输入法。
    unsafe { tip.ActivateEx(&thread_mgr, 7, 0) }.unwrap();
    let active = object.active().unwrap();
    let (doc, context) = document();
    assert!(
        active
            .input(&context, Key::Character('u'), later())
            .unwrap()
    );
    let old = active.current_popup().unwrap();
    // SAFETY: 模拟宿主销毁其拥有的候选窗口；只触碰本测试窗口。
    unsafe { DestroyWindow(old.hwnd) }.unwrap();
    assert!(
        active
            .input(&context, Key::Character('a'), later())
            .unwrap()
    );
    let replacement = active.current_popup().unwrap();
    assert!(!Rc::ptr_eq(&old, &replacement), "不得复用已销毁的候选窗");
    assert!(shown(&active).is_some());
    assert!(active.input(&context, Key::Space, later()).unwrap());
    assert_eq!(text(&doc), "的");
    // SAFETY: 停用合成服务并关闭假后台连接。
    unsafe { tip.Deactivate() }.unwrap();
    server.join().unwrap();
}

#[test]
fn thread_focus_loss_hides_candidates_and_late_layout_cannot_reshow_them() {
    // 跨应用失焦不保证同时收到文档焦点通知；迟到布局不得把候选重新画到其他应用上。
    let object = service();
    let (server, bound) = backend(&object.namespace);
    bound.recv().unwrap();
    let (manager, thread_mgr) = manager();
    manager.thread_focus.set(true);
    let tip: ITfTextInputProcessorEx = object.to_interface();
    // SAFETY: 只激活合成服务。
    unsafe { tip.ActivateEx(&thread_mgr, 7, 0) }.unwrap();
    let active = object.active().unwrap();
    let (doc, context) = document();
    assert!(
        active
            .input(&context, Key::Character('u'), later())
            .unwrap()
    );
    let focus = manager.focus.borrow().clone().unwrap();
    // SAFETY: 模拟只发线程失焦，不发文档失焦的宿主。
    unsafe { focus.OnKillThreadFocus() }.unwrap();
    assert!(shown(&active).is_none(), "线程失焦必须立即收起候选");
    layout_changed(&doc);
    assert!(shown(&active).is_none(), "迟到布局不得复活失焦窗口");
    assert_eq!(text(&doc), "u", "仅失焦不得提交或删除组合");
    // SAFETY: 回到原输入线程后可重新定位仍有效的组合。
    unsafe { focus.OnSetThreadFocus() }.unwrap();
    assert!(shown(&active).is_some());
    // 已排队的布局在失焦后才执行，也不能显示；下一次获得线程焦点不能覆盖 TIP 自己的失焦。
    doc.defer.set(true);
    layout_changed(&doc);
    let queued = doc.deferred.take().unwrap();
    // SAFETY: 合成服务的焦点与排队编辑回调，正文仍是本测试的组合。
    unsafe {
        focus.OnKillThreadFocus().unwrap();
        queued.DoEditSession(1).unwrap();
    }
    assert!(shown(&active).is_none());
    doc.defer.set(false);
    let keys: ITfKeyEventSink = object.to_interface();
    // SAFETY: 模拟 TIP 被另一个输入法取代，但宿主线程随后重新获得焦点。
    unsafe {
        keys.OnSetFocus(false).unwrap();
        focus.OnSetThreadFocus().unwrap();
    }
    layout_changed(&doc);
    assert!(shown(&active).is_none(), "线程焦点不能覆盖 TIP 失焦");
    // SAFETY: 两种焦点都恢复后才显示原组合的候选。
    unsafe { keys.OnSetFocus(true) }.unwrap();
    assert!(shown(&active).is_some());
    // SAFETY: 停用合成服务并关闭假后台连接。
    unsafe { tip.Deactivate() }.unwrap();
    server.join().unwrap();
}

#[test]
fn without_any_host_position_candidates_use_the_last_composition_then_the_input_area() {
    let object = service();
    let (server, bound) = backend(&object.namespace);
    bound.recv().unwrap();
    let (_manager, thread_mgr) = manager();
    let tip: ITfTextInputProcessorEx = object.to_interface();
    // SAFETY: 合成线程管理器在本线程同步回调。
    unsafe { tip.ActivateEx(&thread_mgr, 7, 0) }.unwrap();
    let active = object.active().unwrap();
    let window = || {
        let mut window = RECT::default();
        // SAFETY: 只读本测试候选窗的位置。
        unsafe { GetWindowRect(active.current_popup().unwrap().hwnd, &mut window).unwrap() };
        window
    };
    // 候选窗贴着给定矩形的外沿：下方放得下就在下方，否则翻到上方，左边对齐。
    let beside = |placed: RECT, by: RECT| {
        placed.left == by.left && (placed.top == by.bottom || placed.bottom == by.top)
    };
    let area = RECT {
        left: 300,
        top: 400,
        right: 700,
        bottom: 440,
    };
    let (doc, context) = document();
    doc.screen.set(Some(area));
    let first = RECT {
        left: 100,
        top: 100,
        right: 110,
        bottom: 120,
    };
    assert!(
        active
            .input(&context, Key::Character('u'), later())
            .unwrap()
    );
    assert!(beside(window(), first));
    assert!(active.input(&context, Key::Space, later()).unwrap());
    assert!(shown(&active).is_none());
    // 新组合宿主给不出任何位置：先用本输入框上次组合位置，而非输入区域。
    doc.no_layout.set(true);
    assert!(
        active
            .input(&context, Key::Character('a'), later())
            .unwrap()
    );
    assert!(shown(&active).is_some() && beside(window(), first));
    assert!(active.input(&context, Key::Space, later()).unwrap());
    // 换到从未取得位置的输入框：贴着输入区域外沿放。
    let (other, second) = document();
    other.no_layout.set(true);
    other.screen.set(Some(area));
    assert!(active.input(&second, Key::Character('b'), later()).unwrap());
    assert!(shown(&active).is_some() && beside(window(), area));
    assert!(active.input(&second, Key::Space, later()).unwrap());
    // 连输入区域也没有：用视图窗口的屏幕矩形。
    // SAFETY: 本线程创建的隐藏系统类窗口，只作视图窗口，测试结束前销毁。
    let view = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            None,
            WS_POPUP,
            500,
            600,
            200,
            50,
            None,
            None,
            None,
            None,
        )
    }
    .unwrap();
    let (third, last) = document();
    third.no_layout.set(true);
    third.window.set(view);
    assert!(active.input(&last, Key::Character('c'), later()).unwrap());
    let mut frame = RECT::default();
    // SAFETY: 只读本测试窗口的位置。
    unsafe { GetWindowRect(view, &mut frame).unwrap() };
    assert!(shown(&active).is_some() && beside(window(), frame));
    // SAFETY: 停用取消剩余组合、销毁候选窗、注销 sink 并关闭管道；随后销毁测试窗口。
    unsafe {
        tip.Deactivate().unwrap();
        DestroyWindow(view).unwrap();
    }
    assert_eq!(text(&third), "");
    server.join().unwrap();
}
