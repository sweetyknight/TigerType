//! 合成线程管理器与进程内假后台：不激活系统 TSF、不注册 COM，也不连接用户正在运行的后台。
use super::*;
use crate::edit::tests::{document, text};
use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    thread,
};
use tigertype_contracts::{
    Candidate, Composition, InputState, KeyOutcome,
    protocol::{Message, Rejection, Response},
};
use tigertype_ipc::{decode, encode};
use windows::{
    Win32::{
        Foundation::{E_NOTIMPL, HWND, RECT, S_OK},
        UI::{
            Input::KeyboardAndMouse::{VK_OEM_COMMA, VK_SPACE, VK_U},
            WindowsAndMessaging::{
                DispatchMessageW, GetWindowRect, MSG, PM_REMOVE, PeekMessageW, SendMessageW,
                WM_APP, WM_LBUTTONDOWN, WM_LBUTTONUP,
            },
        },
    },
    core::{BSTR, ComObject, PCWSTR},
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
}
#[implement(ITfThreadMgr, ITfSource, ITfKeystrokeMgr, ITfCompartmentMgr)]
struct ThreadMgr(Rc<Manager>);
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
        if unsafe { *iid } != ITfThreadMgrEventSink::IID || sink.is_null() {
            return Err(E_INVALIDARG.into());
        }
        self.0.next.set(self.0.next.get() + 1);
        self.0.sinks.borrow_mut().push(self.0.next.get());
        Ok(self.0.next.get())
    }
    fn UnadviseSink(&self, cookie: u32) -> Result<()> {
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
        Err(E_NOTIMPL.into())
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

/// 进程内单槽假后台：真实管道、握手与协议校验；字母累积组合，空格提交首选，选字提交对应候选。
fn backend(namespace: &str) -> (thread::JoinHandle<Vec<Command>>, mpsc::Receiver<()>) {
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
        let (mut log, mut buffer, mut revision) = (Vec::new(), String::new(), 1);
        // 宿主停用即断开管道，读取失败结束服务。
        while let Ok(frame) = pipe.receive_frame(later()) {
            let Message::Request(request) = decode(&frame).unwrap() else {
                panic!("宿主只应发送请求");
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
            let reply = match request.command {
                Command::Open { .. } => Reply::Opened(state("", revision)),
                Command::Focus { .. } => {
                    buffer.clear();
                    Reply::Focused(state("", revision))
                }
                Command::Clear => {
                    buffer.clear();
                    Reply::Cleared(state("", revision))
                }
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
        log
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
    assert_eq!(
        *manager.released.borrow(),
        ["thread", "keys", "thread", "keyboard"]
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
        ["keys", "thread", "keyboard", "keys", "thread", "keyboard"]
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
    let (_manager, thread_mgr) = manager();
    let tip: ITfTextInputProcessorEx = object.to_interface();
    // SAFETY: 合成线程管理器；命名空间内没有后台，连接立即失败。
    unsafe { tip.ActivateEx(&thread_mgr, 7, 0) }.unwrap();
    let active = object.active().unwrap();
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
    assert_eq!(*manager.released.borrow(), ["keys", "thread", "keyboard"]);
    let key = |key, epoch| Command::Key { key, epoch };
    let focus = |epoch| Command::Focus {
        epoch,
        focused: true,
    };
    let log = server.join().unwrap();
    let Command::Select { revision, .. } = log[6] else {
        panic!("第七个请求应是选字：{log:?}");
    };
    assert_eq!(
        log,
        [
            Command::Open {
                options: Default::default()
            },
            focus(1),
            key(Key::Character('u'), 1),
            key(Key::Space, 1),
            focus(2),
            key(Key::Character('a'), 2),
            Command::Select {
                revision,
                index: 1,
                epoch: 2
            },
            key(Key::Character('a'), 2),
            focus(3),
            key(Key::Character('b'), 3),
        ]
    );
}
