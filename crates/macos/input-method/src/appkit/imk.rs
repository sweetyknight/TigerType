//! IMK 系统适配：所有回调截住 Rust panic，Server 持有依赖，控制器只持有自己的会话和客户端。
use super::{CandidatePanel, Menu, callback, key_event, marked_text};
use crate::{Action, KeyAction, Keyboard, Location, MenuAction, NativeRect};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class,
    msg_send,
    rc::{Allocated, Retained},
    runtime::AnyObject,
    sel,
};
use objc2_app_kit::{NSEvent, NSEventMask, NSMenu};
use objc2_foundation::{
    NSDictionary, NSNotFound, NSObject, NSObjectProtocol, NSRange, NSRect, NSString,
};
use objc2_input_method_kit::{IMKInputController, IMKServer};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};
use tigertype_contracts::{Key, KeyOutcome, MAX_TEXT_BYTES, protocol::Panel};
use tigertype_input_client::{Channel, Host};
const BUDGET: Duration = Duration::from_millis(50);
// 重入保护在异常展开时也恢复，不能留下永远阻挡输入的 busy 位。
struct Busy<'a>(&'a Cell<bool>);
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}
pub const CONNECTION: &str = "TigerType_Connection";
// macOS 用中间独立的 .inputmethod. 段识别输入法，放在末尾不会出现在输入源列表。
pub const IDENTIFIER: &str = "local.inputmethod.tigertype";

fn text_client(client: &AnyObject) -> bool {
    [
        sel!(setMarkedText:selectionRange:replacementRange:),
        sel!(insertText:replacementRange:),
        sel!(markedRange),
        sel!(attributesForCharacterIndex:lineHeightRectangle:),
    ]
    .into_iter()
    .all(|selector| {
        // SAFETY: IMK 传入有效对象；只询问已知文本接口，不执行文档操作。
        unsafe { msg_send![client, respondsToSelector: selector] }
    })
}

struct Environment {
    factory: Box<dyn Fn() -> Box<dyn Channel>>,
    show: Box<dyn Fn(Panel)>,
    reason: Box<dyn Fn() -> Option<String>>,
    panel: CandidatePanel,
    active: RefCell<Weak<Session>>,
    active_id: Cell<u64>,
    next: Cell<u64>,
    mtm: MainThreadMarker,
}
impl Environment {
    fn action(&self, action: Action) {
        let Some(session) = self.active.borrow().upgrade() else {
            return;
        };
        if !session.active.get() {
            return;
        }
        let deadline = Instant::now() + BUDGET;
        match action {
            Action::Select(selection)
                if selection.owner == session.id.get() && self.panel.accepts(selection) =>
            {
                if session.busy.replace(true) {
                    return;
                }
                let _busy = Busy(&session.busy);
                let epoch = session.epoch.get();
                let result =
                    session
                        .host
                        .borrow_mut()
                        .select(selection.revision, selection.index, deadline);
                match result {
                    Ok(Some(outcome)) => {
                        session.apply(outcome, epoch, deadline);
                    }
                    Ok(None) => {}
                    Err(_) => session.fail(),
                }
            }
            Action::Menu { owner, action } if owner == session.id.get() => match action {
                MenuAction::Show(panel) => (self.show)(panel),
                MenuAction::Key(key) => {
                    session.process(key, deadline);
                }
            },
            _ => {}
        }
    }
}
struct Session {
    id: Cell<u64>,
    environment: Rc<Environment>,
    client: RefCell<Retained<AnyObject>>,
    host: RefCell<Host<Box<dyn Channel>>>,
    keyboard: RefCell<Keyboard>,
    location: RefCell<Location>,
    active: Cell<bool>,
    busy: Cell<bool>,
    epoch: Cell<u64>,
    marked: RefCell<String>,
}
impl Session {
    fn current(&self) -> bool {
        self.id.get() != 0 && self.environment.active_id.get() == self.id.get()
    }
    fn fail(&self) {
        // 故障/系统终止也使在途结果过期；重入的 commitComposition 不能复活后继组合。
        self.epoch.set(self.epoch.get().saturating_add(1));
        if self.epoch.get() == u64::MAX {
            self.active.set(false);
        }
        self.host.borrow_mut().close();
        self.keyboard.borrow_mut().reset();
        self.clear_marked();
        if self.current() {
            self.environment.panel.hide();
        }
    }
    fn clear_marked(&self) {
        let old = std::mem::take(&mut *self.marked.borrow_mut());
        let client = self.client.borrow().clone();
        if old.is_empty() || !Self::matches_marked(&client, &old) {
            return;
        }
        // SAFETY: 已核对绑定客户端仍有对应长度的组合；NSNotFound 只撤回当前组合，不指定已提交范围。
        unsafe {
            let _: () = msg_send![&*client, setMarkedText: &*NSString::from_str(""), selectionRange: NSRange::new(0, 0), replacementRange: NSRange::new(NSNotFound as usize, 0)];
        }
    }
    fn matches_marked(client: &AnyObject, expected: &str) -> bool {
        // SAFETY: 当前客户端的 markedRange 是组合状态；文档子串缓存可能滞后，不能据此否定有效组合。
        unsafe {
            let range: NSRange = msg_send![client, markedRange];
            range.location < NSNotFound as usize
                && range.length <= MAX_TEXT_BYTES
                && range.length == expected.encode_utf16().count()
        }
    }
    fn activate(self: &Rc<Self>, client: &AnyObject) {
        if !text_client(client) {
            self.deactivate();
            return;
        }
        if self.epoch.get() == u64::MAX {
            return;
        }
        let previous = self.environment.active.borrow().upgrade();
        if let Some(previous) = previous {
            previous.deactivate();
        }
        // IMK 初始化传入通信代理，激活/按键传入文本包装对象；先撤回旧客户端组合再绑定新对象。
        *self.client.borrow_mut() = client.retain();
        // 每次激活换身份，旧菜单/点击即使回到同一控制器也不能复用。
        let Some(id) = self.environment.next.get().checked_add(1) else {
            self.fail();
            return;
        };
        self.environment.next.set(id);
        self.id.set(id);
        self.environment.active_id.set(id);
        self.keyboard.borrow_mut().reset();
        self.host.borrow_mut().blur();
        self.active.set(true);
        *self.environment.active.borrow_mut() = Rc::downgrade(self);
    }
    fn deactivate(&self) {
        self.active.set(false);
        self.epoch.set(self.epoch.get().saturating_add(1));
        self.keyboard.borrow_mut().reset();
        self.host.borrow_mut().blur();
        self.clear_marked();
        if self.current() {
            self.environment.panel.hide();
            *self.environment.active.borrow_mut() = Weak::new();
            self.environment.active_id.set(0);
        }
    }
    fn event(&self, event: &NSEvent, sender: &AnyObject, deadline: Instant) -> bool {
        if !self.active.get() || !std::ptr::eq(sender, &**self.client.borrow()) || self.busy.get() {
            return false;
        }
        if (self.environment.reason)().is_some() {
            self.fail();
            return false;
        }
        let old = self.marked.borrow().clone();
        if !old.is_empty() && !Self::matches_marked(sender, &old) {
            self.marked.borrow_mut().clear();
            self.host.borrow_mut().discard();
            self.environment.panel.hide();
        }
        let action = key_event(
            &mut self.keyboard.borrow_mut(),
            event,
            self.host.borrow().state(),
        );
        match action {
            KeyAction::Pass => false,
            KeyAction::Input(key) => self.process(key, deadline),
            KeyAction::ShiftTap => {
                self.process(Key::ToggleAscii, deadline);
                false
            }
        }
    }
    fn process(&self, key: Key, deadline: Instant) -> bool {
        if !self.active.get() || self.busy.replace(true) {
            return false;
        }
        let _busy = Busy(&self.busy);
        let epoch = self.epoch.get();
        let result = self.host.borrow_mut().key(key, deadline);
        let handled = match result {
            Ok(outcome) => self.apply(outcome, epoch, deadline),
            Err(_) => {
                self.fail();
                false
            }
        };
        // 中英与全半角切换都在光标旁短暂提示，状态取自后台回复；组合中切全角时候选窗仍在，不用提示顶掉它。
        let state = (handled && matches!(key, Key::ToggleAscii | Key::ToggleFullShape))
            .then(|| self.host.borrow().state().clone())
            .filter(|state| !state.composing);
        if let Some(state) = state {
            let label = match key {
                Key::ToggleAscii if state.ascii_mode => "英",
                Key::ToggleAscii => "中",
                _ if state.full_shape => "全",
                _ => "半",
            };
            let anchor = self.anchor(0, deadline);
            let _ = self.environment.panel.status(
                label,
                state.options,
                &mut self.location.borrow_mut(),
                anchor,
            );
        }
        handled
    }
    fn apply(&self, outcome: KeyOutcome, epoch: u64, deadline: Instant) -> bool {
        if !self.active.get() || epoch != self.epoch.get() || Instant::now() >= deadline {
            self.fail();
            return false;
        }
        if !outcome.handled {
            return false;
        }
        let old = self.marked.borrow().clone();
        let client = self.client.borrow().clone();
        if !old.is_empty() && !Self::matches_marked(&client, &old) {
            // 后台往返期间宿主可能结束组合；点击也须核对，不替换已经提交的文字。
            self.marked.borrow_mut().clear();
            self.fail();
            return false;
        }
        let prepared = marked_text(&outcome.state.composition);
        if prepared.is_err()
            || outcome
                .commit
                .as_ref()
                .is_some_and(|s| s.len() > MAX_TEXT_BYTES)
        {
            self.fail();
            return false;
        }
        let (text, selection) = prepared.unwrap();
        if !self.active.get() || epoch != self.epoch.get() || Instant::now() >= deadline {
            self.fail();
            return false;
        }
        let mut committed = false;
        if let Some(commit) = &outcome.commit {
            // 在调用前解除标记所有权；重入或后继组合失败都不能把提交后的文字当作可撤回内容。
            self.marked.borrow_mut().clear();
            // SAFETY: IMK 客户端协议；NSNotFound 表示当前组合/插入点，不指定其他文档范围。
            unsafe {
                let _: () = msg_send![&*client, insertText: &*NSString::from_str(commit), replacementRange: NSRange::new(NSNotFound as usize, 0)];
            }
            committed = true;
        }
        if !self.active.get() || epoch != self.epoch.get() || Instant::now() >= deadline {
            self.fail();
            return committed;
        }
        if outcome.state.composing {
            *self.marked.borrow_mut() = outcome.state.composition.text.clone();
            // SAFETY: 文本和 UTF-16 选区已校验，持有属性字符串覆盖整个同步调用。
            unsafe {
                let _: () = msg_send![&*client, setMarkedText: &*text, selectionRange: selection, replacementRange: NSRange::new(NSNotFound as usize, 0)];
            }
        } else {
            self.clear_marked();
        }
        if self.active.get() && epoch == self.epoch.get() {
            let anchor = self.anchor(0, deadline);
            let caret = if anchor.is_none() {
                self.anchor(selection.location, deadline)
            } else {
                None
            };
            let _ = self.environment.panel.show(
                self.id.get(),
                outcome.state,
                &mut self.location.borrow_mut(),
                anchor,
                caret,
            );
        }
        true
    }
    fn anchor(&self, index: usize, deadline: Instant) -> Option<NativeRect> {
        if Instant::now() >= deadline {
            return None;
        }
        let mut rect = NSRect::ZERO;
        let client = self.client.borrow().clone();
        // SAFETY: 矩形是调用期间存活的栈变量，返回字典按引用计数释放；只查询本客户端。
        unsafe {
            let _: Option<Retained<NSDictionary>> = msg_send![&*client, attributesForCharacterIndex: index, lineHeightRectangle: &mut rect];
        }
        (rect != NSRect::ZERO).then_some(NativeRect {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        })
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        // 最后一个控制器释放时也撤回自有组合和面板；弱引用已无法升级，因此按代次判断归属。
        callback(|| self.deactivate());
        self.host.get_mut().close();
    }
}

// IMKServer 是类簇，不能继承；系统实例强持有依赖对象，控制器按类型取回，无全局业务状态。
static ENVIRONMENT_KEY: u8 = 0;
pub struct Server {
    native: Retained<IMKServer>,
    _main: MainThreadMarker,
}
impl Drop for Server {
    fn drop(&mut self) {
        // SAFETY: 主线程结束服务时配对解除本键的强关联；系统可能继续持有服务，后续控制器须拒绝初始化。
        unsafe {
            objc2::ffi::objc_setAssociatedObject(
                Retained::as_ptr(&self.native).cast_mut().cast(),
                (&ENVIRONMENT_KEY as *const u8).cast(),
                std::ptr::null_mut(),
                objc2::ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
            );
        }
    }
}
define_class!(
    // SAFETY: 普通 NSObject 只持有主线程依赖，生命周期随 IMKServer 的强关联释放。
    #[unsafe(super = NSObject)]
    #[name = "TigerTypeIMKEnvironment"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Rc<Environment>]
    struct EnvironmentObject;
    // SAFETY: 保持 NSObject 的身份与引用计数约定。
    unsafe impl NSObjectProtocol for EnvironmentObject {}
);
impl EnvironmentObject {
    fn from_server(server: &IMKServer, _mtm: MainThreadMarker) -> Option<Rc<Environment>> {
        // SAFETY: 私有静态键只标识此强关联；server 在借用期存活，主线程核对类型后克隆 Rc。
        unsafe {
            objc2::ffi::objc_getAssociatedObject(
                server.as_ref(),
                (&ENVIRONMENT_KEY as *const u8).cast(),
            )
            .as_ref()?
            .downcast_ref::<Self>()
            .map(|value| value.ivars().clone())
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use objc2_foundation::NSAttributedString;
    use tigertype_contracts::{
        Composition, InputState,
        protocol::{Command, Reply},
    };
    use tigertype_input_client::Error;
    #[derive(Default)]
    struct Document {
        marked: RefCell<String>,
        committed: RefCell<String>,
        cached: RefCell<Option<String>>,
        offset: Cell<usize>,
    }
    define_class!(
        // SAFETY: 合成客户端只实现测试实际使用的 IMK 文本方法，不连接或注入真实应用。
        #[unsafe(super = NSObject)]
        #[name = "TigerTypeSyntheticIMKClient"]
        #[thread_kind = MainThreadOnly]
        #[ivars = Document]
        struct Client;
        // SAFETY: 保持 NSObject 的所有权协议。
        unsafe impl NSObjectProtocol for Client {}
        impl Client {
            #[unsafe(method(setMarkedText:selectionRange:replacementRange:))]
            fn marked(&self, text: &AnyObject, _: NSRange, _: NSRange) {
                callback(|| { *self.ivars().marked.borrow_mut() = if let Some(text) = text.downcast_ref::<NSString>() { text.to_string() } else { text.downcast_ref::<NSAttributedString>().unwrap().string().to_string() }; });
            }
            #[unsafe(method(insertText:replacementRange:))]
            fn insert(&self, text: &NSString, _: NSRange) { callback(|| { self.ivars().committed.borrow_mut().push_str(&text.to_string()); self.ivars().marked.borrow_mut().clear(); }); }
            #[unsafe(method(markedRange))]
            fn range(&self) -> NSRange { callback(|| { let text = self.ivars().marked.borrow(); NSRange::new(if text.is_empty() { NSNotFound as usize } else { self.ivars().offset.get() }, text.encode_utf16().count()) }).unwrap_or(NSRange::new(NSNotFound as usize, 0)) }
            #[unsafe(method_id(attributedSubstringFromRange:))]
            fn substring(&self, _: NSRange) -> Option<Retained<NSAttributedString>> { callback(|| {
                let text = self.ivars().cached.borrow().clone().unwrap_or_else(|| self.ivars().marked.borrow().clone());
                NSAttributedString::initWithString(NSAttributedString::alloc(), &NSString::from_str(&text))
            }) }
            #[unsafe(method(attributesForCharacterIndex:lineHeightRectangle:))]
            fn attributes(&self, _: usize, _: *mut NSRect) -> *mut NSDictionary { callback(std::ptr::null_mut).unwrap_or(std::ptr::null_mut()) }
        }
    );
    #[derive(Default)]
    struct Offline(Rc<Cell<usize>>);
    impl Channel for Offline {
        fn ensure(&mut self, _: Instant) -> Result<bool, Error> {
            self.0.set(self.0.get() + 1);
            Err(Error::Transport)
        }
        fn request(&mut self, _: u64, _: Command, _: Instant) -> Result<Reply, Error> {
            Err(Error::Transport)
        }
        fn show(&mut self, _: Panel, _: Instant) -> Result<(), Error> {
            Err(Error::Transport)
        }
        fn indicate(&mut self, _: bool, _: Instant) -> Result<(), Error> {
            Err(Error::Transport)
        }
        fn cancel(&mut self) {}
    }
    pub fn run(mtm: MainThreadMarker) {
        let lifetime = objc2::rc::autoreleasepool(|_| run_inner(mtm));
        assert!(
            lifetime.upgrade().is_none(),
            "自动释放池清空后不得遗留依赖环"
        );
    }
    fn run_inner(mtm: MainThreadMarker) -> Weak<Environment> {
        assert_eq!(
            InputController::class().superclass(),
            Some(IMKInputController::class())
        );
        let this = Client::alloc(mtm).set_ivars(Document::default());
        // SAFETY: 合成 NSObject 按指定初始化器完成初始化，Rust ivars 已就绪。
        let client: Retained<Client> = unsafe { msg_send![super(this), init] };
        let attempts = Rc::new(Cell::new(0));
        let attempted = attempts.clone();
        let shown = Rc::new(RefCell::new(Vec::new()));
        let shown_panels = shown.clone();
        let env = Rc::new(Environment {
            factory: Box::new(move || Box::new(Offline(attempted.clone()))),
            show: Box::new(move |panel| shown_panels.borrow_mut().push(panel)),
            reason: Box::new(|| None),
            panel: CandidatePanel::new(mtm, Rc::new(|_| {})),
            active: RefCell::new(Weak::new()),
            active_id: Cell::new(0),
            next: Cell::new(0),
            mtm,
        });
        // 真实初始化使用独立包标识/连接与合成客户端，既不占正式服务也不读现用数据。
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("TigerType_Test_{}_{nonce}", std::process::id());
        let identifier = format!("local.tigertype.test.{nonce}");
        let root = std::env::temp_dir().join(&name);
        std::fs::create_dir(&root).unwrap();
        let contents = root.join("Probe.bundle/Contents");
        std::fs::create_dir_all(&contents).unwrap();
        std::fs::write(contents.join("Info.plist"), format!(r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{identifier}</string><key>InputMethodServerControllerClass</key><string>TigerTypeInputController</string></dict></plist>"#)).unwrap();
        let bundle = objc2_foundation::NSBundle::bundleWithPath(&NSString::from_str(
            root.join("Probe.bundle").to_str().unwrap(),
        ))
        .unwrap();
        assert_eq!(bundle.bundleIdentifier().unwrap().to_string(), identifier);
        let server = Server::connect(mtm, env.clone(), &name, &identifier).unwrap();
        assert!(Rc::ptr_eq(
            &env,
            &EnvironmentObject::from_server(&server.native, mtm).unwrap()
        ));
        // IMK 仍要求 NSDistantObject；仅给合成客户端套本地代理，不连接真实宿主。
        #[allow(deprecated)]
        // SAFETY: 真正执行 IMK 控制器指定初始化器及生命周期回调，客户端全部为进程内合成对象。
        unsafe {
            let connection = objc2_foundation::NSConnection::new();
            let proxy = objc2_foundation::NSDistantObject::proxyWithLocal_connection(
                client.as_ref(),
                &connection,
            );
            let controller: Retained<InputController> = msg_send![InputController::alloc(mtm), initWithServer: &*server.native, delegate: std::ptr::null::<AnyObject>(), client: &*proxy];
            assert!(Rc::ptr_eq(&env, &controller.ivars().environment));
            let _: () = msg_send![&*controller, activateServer: &*client];
            assert!(controller.ivars().active.get());
            let events: usize = msg_send![&*controller, recognizedEvents: &*client];
            assert_eq!(
                events,
                (NSEventMask::KeyDown | NSEventMask::FlagsChanged).0 as usize
            );
            let menu: Option<Retained<NSMenu>> = msg_send![&*controller, menu];
            let menu = menu.unwrap();
            // 系统先核对控制器能响应菜单 action，再直接发送命令字典；只实现 doCommand 不会被调用。
            let settings = menu.itemAtIndex(5).unwrap();
            let action = settings.action().unwrap();
            assert!(controller.respondsToSelector(action));
            let info = NSDictionary::<NSString>::from_slices(
                &[objc2_input_method_kit::kIMKCommandMenuItemName],
                &[settings.as_ref()],
            );
            let _: () = msg_send![&*controller, invoke: &*info];
            assert_eq!(&*shown.borrow(), &[Panel::Settings]);
            let _: () = msg_send![&*controller, activateServer: &*client];
            let _: () = msg_send![&*controller, invoke: &*info];
            assert_eq!(shown.borrow().len(), 1, "旧菜单仍须拒绝");
            let text = NSString::from_str("a");
            let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                objc2_app_kit::NSEventType::KeyDown, objc2_foundation::NSPoint::ZERO,
                objc2_app_kit::NSEventModifierFlags::empty(), 0.0, 0, None, &text, &text, false, 0,
            ).unwrap();
            // 初始化代理与系统激活的文本对象不同；后者的按键必须到达后台，前者不得冒用。
            let handled: bool = msg_send![&*controller, handleEvent: &*event, client: &*proxy];
            assert!(!handled);
            assert_eq!(attempts.get(), 0);
            let handled: bool = msg_send![&*controller, handleEvent: &*event, client: &*client];
            assert!(!handled, "后台离线必须放行原键");
            assert_eq!(attempts.get(), 1, "激活后的文本包装对象不能被错误拒绝");
            // 真机诊断：IMK 把每个修饰键事件投递两次（按下、按下、抬起、抬起）；重复的按下不能作废单击，
            // 单击 Shift 只切换一次，原事件始终交还应用、不写入文档。
            let empty = NSString::from_str("");
            let shift = |flags| {
                NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                    objc2_app_kit::NSEventType::FlagsChanged, objc2_foundation::NSPoint::ZERO,
                    flags, 0.0, 0, None, &empty, &empty, false, 56,
                ).unwrap()
            };
            let down = shift(objc2_app_kit::NSEventModifierFlags::Shift);
            let up = shift(objc2_app_kit::NSEventModifierFlags::empty());
            for event in [&down, &down] {
                let handled: bool =
                    msg_send![&*controller, handleEvent: &**event, client: &*client];
                assert!(!handled);
            }
            assert_eq!(attempts.get(), 1, "按下 Shift 尚不构成单击");
            for event in [&up, &up] {
                let handled: bool =
                    msg_send![&*controller, handleEvent: &**event, client: &*client];
                assert!(!handled, "Shift 原事件必须交还应用");
            }
            assert_eq!(attempts.get(), 2, "重复投递的单击 Shift 须切换且只切换一次");
            assert!(client.ivars().committed.borrow().is_empty());
            let _: () = msg_send![&*controller, deactivateServer: &*client];
            assert!(!controller.ivars().active.get());
            drop(controller);
            connection.invalidate();
        }
        let lifetime = Rc::downgrade(&env);
        let session = Rc::new(Session {
            id: Cell::new(0),
            environment: env,
            client: RefCell::new(Retained::from(&*client)),
            host: RefCell::new(Host::new(Box::<Offline>::default())),
            keyboard: RefCell::new(Keyboard::default()),
            location: RefCell::new(Location::default()),
            active: Cell::new(false),
            busy: Cell::new(false),
            epoch: Cell::new(0),
            marked: RefCell::new(String::new()),
        });
        session.activate(&client);
        let outcome = |text: &str, commit: Option<&str>| KeyOutcome {
            handled: true,
            commit: commit.map(str::to_owned),
            state: InputState {
                composing: !text.is_empty(),
                composition: Composition {
                    text: text.into(),
                    cursor: text.len(),
                    selection: 0..text.len(),
                },
                ..Default::default()
            },
        };
        // Codex 实测 markedRange=(6,1)，文档子串仍为换行；滞后快照不能把每一码当成新组合。
        client.ivars().offset.set(6);
        *client.ivars().cached.borrow_mut() = Some("\n".into());
        for text in ["a", "ab", "abc", "abcd"] {
            assert!(session.apply(
                outcome(text, None),
                session.epoch.get(),
                Instant::now() + BUDGET
            ));
            assert_eq!(&*client.ivars().marked.borrow(), text);
        }
        assert!(session.apply(
            outcome("", Some("测试")),
            session.epoch.get(),
            Instant::now() + BUDGET
        ));
        assert_eq!(&*client.ivars().committed.borrow(), "测试");
        client.ivars().committed.borrow_mut().clear();
        *client.ivars().cached.borrow_mut() = None;
        assert!(session.apply(
            outcome("ab", None),
            session.epoch.get(),
            Instant::now() + BUDGET
        ));
        assert_eq!(&*client.ivars().marked.borrow(), "ab");
        assert!(session.apply(
            outcome("", Some("虎")),
            session.epoch.get(),
            Instant::now() + BUDGET
        ));
        session.fail();
        assert_eq!(&*client.ivars().committed.borrow(), "虎");
        // 宿主已经结束组合时，即使缓存还留着旧编码，也不得把已上屏文字当成可替换组合。
        assert!(session.apply(
            outcome("ab", None),
            session.epoch.get(),
            Instant::now() + BUDGET
        ));
        *client.ivars().cached.borrow_mut() = Some("ab".into());
        client.ivars().marked.borrow_mut().clear();
        assert!(!session.apply(
            outcome("", Some("错误")),
            session.epoch.get(),
            Instant::now() + BUDGET
        ));
        assert_eq!(&*client.ivars().committed.borrow(), "虎");
        *client.ivars().cached.borrow_mut() = None;
        assert!(!session.apply(
            outcome("", Some("迟到")),
            session.epoch.get(),
            Instant::now()
        ));
        assert_eq!(&*client.ivars().committed.borrow(), "虎");
        assert!(session.apply(
            outcome("ab", None),
            session.epoch.get(),
            Instant::now() + BUDGET
        ));
        session.fail();
        assert!(client.ivars().marked.borrow().is_empty());
        assert!(session.apply(
            outcome("ab", None),
            session.epoch.get(),
            Instant::now() + BUDGET
        ));
        *client.ivars().marked.borrow_mut() = "外部改写".into();
        session.fail();
        assert_eq!(&*client.ivars().marked.borrow(), "外部改写");
        session.deactivate();
        assert!(!session.process(Key::Character('a'), Instant::now() + BUDGET));
        let old_id = session.id.get();
        let old_epoch = session.epoch.get();
        session.activate(&client);
        assert_ne!(old_id, session.id.get());
        session.environment.action(Action::Menu {
            owner: old_id,
            action: MenuAction::Key(Key::ToggleAscii),
        });
        assert_eq!(session.epoch.get(), old_epoch);
        // 再激活换客户端时只撤回旧组合；新提交只能写入新客户端，缺接口的对象不得激活。
        *client.ivars().marked.borrow_mut() = String::new();
        assert!(session.apply(
            outcome("ab", None),
            session.epoch.get(),
            Instant::now() + BUDGET
        ));
        let this = Client::alloc(mtm).set_ivars(Document::default());
        // SAFETY: 第二个客户端同样完全由测试持有，不访问真实宿主。
        let other: Retained<Client> = unsafe { msg_send![super(this), init] };
        session.activate(&other);
        assert!(client.ivars().marked.borrow().is_empty());
        assert!(session.apply(
            outcome("", Some("新")),
            session.epoch.get(),
            Instant::now() + BUDGET
        ));
        assert_eq!(&*other.ivars().committed.borrow(), "新");
        session.activate(&NSObject::new());
        assert!(!session.active.get());
        drop(session);
        assert_eq!(&*client.ivars().committed.borrow(), "虎");
        let native = server.native.clone();
        drop(server);
        assert!(EnvironmentObject::from_server(&native, mtm).is_none());
        // SAFETY: 结束后的服务仍有效但依赖已解除，必须拒绝新控制器而非读悬空数据。
        let refused: Option<Retained<InputController>> = unsafe {
            msg_send![InputController::alloc(mtm), initWithServer: &*native, delegate: std::ptr::null::<AnyObject>(), client: &*client]
        };
        assert!(refused.is_none());
        println!("IMK 服务实际初始化、控制器代理与离线按键放行通过（独立测试连接）");
        lifetime
    }
}
impl Server {
    pub fn start(
        mtm: MainThreadMarker,
        factory: impl Fn() -> Box<dyn Channel> + 'static,
        show: impl Fn(Panel) + 'static,
        reason: impl Fn() -> Option<String> + 'static,
    ) -> Result<Self, String> {
        let _ = InputController::class();
        let environment = Rc::new_cyclic(|weak: &Weak<Environment>| {
            let target = weak.clone();
            Environment {
                factory: Box::new(factory),
                show: Box::new(show),
                reason: Box::new(reason),
                panel: CandidatePanel::new(
                    mtm,
                    Rc::new(move |action| {
                        if let Some(env) = target.upgrade() {
                            env.action(action);
                        }
                    }),
                ),
                active: RefCell::new(Weak::new()),
                active_id: Cell::new(0),
                next: Cell::new(0),
                mtm,
            }
        });
        Self::connect(mtm, environment, CONNECTION, IDENTIFIER)
    }
    fn connect(
        mtm: MainThreadMarker,
        environment: Rc<Environment>,
        name: &str,
        identifier: &str,
    ) -> Result<Self, String> {
        // SAFETY: 让系统分配具体 IMKServer 子类，再调用其指定初始化器；不向类簇父类发 super 消息。
        let native = unsafe {
            IMKServer::initWithName_bundleIdentifier(
                IMKServer::alloc(),
                Some(&NSString::from_str(name)),
                Some(&NSString::from_str(identifier)),
            )
        }
        .ok_or("IMKServer 初始化失败")?;
        let this = EnvironmentObject::alloc(mtm).set_ivars(environment);
        // SAFETY: NSObject 初始化后由系统强关联持有，键地址稳定；不存储借用指针或制造引用环。
        unsafe {
            let owner: Retained<EnvironmentObject> = msg_send![super(this), init];
            objc2::ffi::objc_setAssociatedObject(
                Retained::as_ptr(&native).cast_mut().cast(),
                (&ENVIRONMENT_KEY as *const u8).cast(),
                Retained::as_ptr(&owner).cast_mut().cast(),
                objc2::ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
            );
        }
        Ok(Self { native, _main: mtm })
    }
}
define_class!(
    // SAFETY: IMK 控制器只实现已公布回调签名；依赖从 Server 取得，没有全局表或隐藏窗口句柄。
    #[unsafe(super = IMKInputController)]
    #[name = "TigerTypeInputController"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Rc<Session>]
    struct InputController;
    // SAFETY: 沿用 NSObject 的对象生命周期。
    unsafe impl NSObjectProtocol for InputController {}
    impl InputController {
        #[unsafe(method_id(initWithServer:delegate:client:))]
        fn init(this: Allocated<Self>, server: Option<&IMKServer>, delegate: Option<&AnyObject>, client: Option<&AnyObject>) -> Option<Retained<Self>> {
            callback(|| {
                let environment = EnvironmentObject::from_server(server?, MainThreadMarker::new()?)?;
                let client = client?;
                if !text_client(client) { return None; }
                let host = Host::new((environment.factory)());
                let session = Rc::new(Session { id: Cell::new(0), environment, client: RefCell::new(client.retain()), host: RefCell::new(host), keyboard: RefCell::new(Keyboard::default()), location: RefCell::new(Location::default()), active: Cell::new(false), busy: Cell::new(false), epoch: Cell::new(0), marked: RefCell::new(String::new()) });
                let this = this.set_ivars(session);
                // SAFETY: 参数原样转交 IMK 指定初始化器；Rust ivars 已初始化，失败由引用计数清理。
                unsafe { msg_send![super(this), initWithServer: server, delegate: delegate, client: client] }
            }).flatten()
        }
        #[unsafe(method(recognizedEvents:))]
        fn recognized(&self, _: Option<&AnyObject>) -> usize { callback(|| (NSEventMask::KeyDown | NSEventMask::FlagsChanged).0 as usize).unwrap_or(0) }
        #[unsafe(method(handleEvent:client:))]
        fn handle(&self, event: Option<&NSEvent>, sender: Option<&AnyObject>) -> bool {
            let deadline = Instant::now() + BUDGET;
            let result = callback(|| {
                // SAFETY: 父控制器持有其 IMKServer；只查询系统的死键状态，不自行翻译键盘布局。
                let dead = event.is_some_and(|e| e.r#type() == objc2_app_kit::NSEventType::KeyDown) && unsafe { self.server().is_some_and(|s| s.lastKeyEventWasDeadKey()) };
                if dead { self.ivars().keyboard.borrow_mut().reset(); return false; }
                event.zip(sender).is_some_and(|(event, sender)| self.ivars().event(event, sender, deadline))
            });
            if result.is_none() { callback(|| self.ivars().fail()); }
            result.unwrap_or(false)
        }
        #[unsafe(method(activateServer:))]
        fn activate(&self, sender: Option<&AnyObject>) { callback(|| { if let Some(client) = sender { self.ivars().activate(client); } }); }
        #[unsafe(method(deactivateServer:))]
        fn deactivate(&self, _: Option<&AnyObject>) { callback(|| self.ivars().deactivate()); }
        #[unsafe(method(commitComposition:))]
        fn commit(&self, _: Option<&AnyObject>) { callback(|| self.ivars().fail()); }
        #[unsafe(method(hidePalettes))]
        fn hide(&self) { callback(|| { if self.ivars().current() { self.ivars().environment.panel.hide(); } }); }
        #[unsafe(method(invoke:))]
        fn command(&self, info: Option<&NSDictionary<NSString>>) {
            callback(|| {
                // IMK 直接在控制器查找菜单 action；仍按命令字典校验菜单身份与禁用状态。
                if let Some(action) = Menu::forwarded(self.ivars().id.get(), Some(sel!(invoke:)), info) { self.ivars().environment.action(action); }
            });
        }
        #[unsafe(method_id(menu))]
        fn menu(&self) -> Option<Retained<NSMenu>> {
            callback(|| {
                let session = self.ivars(); let env = &session.environment; let weak = Rc::downgrade(env);
                let action = Rc::new(move |a| { if let Some(env) = weak.upgrade() { env.action(a); } });
                let reason = (env.reason)();
                let menu = Menu::new(env.mtm, session.id.get(), session.host.borrow().state(), reason.as_deref(), action);
                Retained::from(menu.native())
            })
        }
    }
);
