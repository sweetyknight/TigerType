//! IMK 系统适配：所有回调截住 Rust panic，Server 持有依赖，控制器只持有自己的会话和客户端。
use super::{CandidatePanel, Menu, callback, key_event, marked_text};
use crate::{Action, KeyAction, Keyboard, Location, MenuAction, NativeRect};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
    rc::{Allocated, Retained},
    runtime::{AnyObject, Sel},
    sel,
};
use objc2_app_kit::{NSEvent, NSEventMask, NSMenu};
use objc2_foundation::{
    NSAttributedString, NSDictionary, NSNotFound, NSObjectProtocol, NSRange, NSRect, NSString,
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
pub const IDENTIFIER: &str = "local.tigertype.inputmethod";

/// 公开类型满足 DefinedClass 的关联类型约束；依赖字段仍由 Server 私有持有。
pub struct Environment {
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
    client: Retained<AnyObject>,
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
        if old.is_empty() || !self.matches_marked(&old) {
            return;
        }
        // SAFETY: 客户端由 IMK 提供且已核对自有标记文本；空字符串只撤回该组合，绝不删已提交文字。
        unsafe {
            let _: () = msg_send![&*self.client, setMarkedText: &*NSString::from_str(""), selectionRange: NSRange::new(0, 0), replacementRange: NSRange::new(NSNotFound as usize, 0)];
        }
    }
    fn matches_marked(&self, expected: &str) -> bool {
        // SAFETY: IMK 客户端实现文本协议；范围先有界检查再请求子串，不把任意文档范围拿来替换。
        unsafe {
            let range: NSRange = msg_send![&*self.client, markedRange];
            if range.location == NSNotFound as usize
                || range.length > MAX_TEXT_BYTES
                || range.length != expected.encode_utf16().count()
            {
                return false;
            }
            let value: Option<Retained<NSAttributedString>> =
                msg_send![&*self.client, attributedSubstringFromRange: range];
            value.is_some_and(|v| v.length() == range.length && v.string().to_string() == expected)
        }
    }
    fn activate(self: &Rc<Self>) {
        if self.epoch.get() == u64::MAX {
            return;
        }
        let previous = self.environment.active.borrow().upgrade();
        if let Some(previous) = previous {
            previous.deactivate();
        }
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
        if !self.active.get() || !std::ptr::eq(sender, &*self.client) || self.busy.get() {
            return false;
        }
        if (self.environment.reason)().is_some() {
            self.fail();
            return false;
        }
        let old = self.marked.borrow().clone();
        if !old.is_empty() && !self.matches_marked(&old) {
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
        if handled && key == Key::ToggleAscii {
            let state = self.host.borrow().state().clone();
            let anchor = self.anchor(0, deadline);
            let _ = self.environment.panel.status(
                state.ascii_mode,
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
        if !old.is_empty() && !self.matches_marked(&old) {
            // 后台往返期间宿主可能外部改写；点击也必须核对，不覆盖已变化的文档。
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
                let _: () = msg_send![&*self.client, insertText: &*NSString::from_str(commit), replacementRange: NSRange::new(NSNotFound as usize, 0)];
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
                let _: () = msg_send![&*self.client, setMarkedText: &*text, selectionRange: selection, replacementRange: NSRange::new(NSNotFound as usize, 0)];
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
        // SAFETY: 矩形是调用期间存活的栈变量，返回字典按引用计数释放；只查询本客户端。
        unsafe {
            let _: Option<Retained<NSDictionary>> = msg_send![&*self.client, attributesForCharacterIndex: index, lineHeightRectangle: &mut rect];
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

define_class!(
    // SAFETY: IMKServer 子类仅增加强类型依赖，不改变父类协议；所有访问在主线程。
    #[unsafe(super = IMKServer)]
    #[name = "TigerTypeIMKServer"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Rc<Environment>]
    pub struct Server;
    // SAFETY: 保持 NSObject 的身份与引用计数约定。
    unsafe impl NSObjectProtocol for Server {}
);

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use objc2::AnyThread;
    use objc2_foundation::NSObject;
    use tigertype_contracts::{
        Composition, InputState,
        protocol::{Command, Reply},
    };
    use tigertype_input_client::Error;
    #[derive(Default)]
    struct Document {
        marked: RefCell<String>,
        committed: RefCell<String>,
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
            fn range(&self) -> NSRange { callback(|| { let text = self.ivars().marked.borrow(); NSRange::new(if text.is_empty() { NSNotFound as usize } else { 0 }, text.encode_utf16().count()) }).unwrap_or(NSRange::new(NSNotFound as usize, 0)) }
            #[unsafe(method_id(attributedSubstringFromRange:))]
            fn substring(&self, _: NSRange) -> Option<Retained<NSAttributedString>> { callback(|| NSAttributedString::initWithString(NSAttributedString::alloc(), &NSString::from_str(&self.ivars().marked.borrow()))) }
            #[unsafe(method(attributesForCharacterIndex:lineHeightRectangle:))]
            fn attributes(&self, _: usize, _: *mut NSRect) -> *mut NSDictionary { callback(std::ptr::null_mut).unwrap_or(std::ptr::null_mut()) }
        }
    );
    struct Offline;
    impl Channel for Offline {
        fn ensure(&mut self, _: Instant) -> Result<bool, Error> {
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
        // 检查公开构造入口和子类注册，但不创建正式命名连接或启用系统输入法。
        let _start = |mtm| {
            super::super::Server::start(
                mtm,
                || Box::new(Offline) as Box<dyn Channel>,
                |_| {},
                || None,
            )
        };
        assert_eq!(
            InputController::class().superclass(),
            Some(IMKInputController::class())
        );
        let this = Client::alloc(mtm).set_ivars(Document::default());
        // SAFETY: 合成 NSObject 按指定初始化器完成初始化，Rust ivars 已就绪。
        let client: Retained<Client> = unsafe { msg_send![super(this), init] };
        let env = Rc::new(Environment {
            factory: Box::new(|| Box::new(Offline)),
            show: Box::new(|_| {}),
            reason: Box::new(|| None),
            panel: CandidatePanel::new(mtm, Rc::new(|_| {})),
            active: RefCell::new(Weak::new()),
            active_id: Cell::new(0),
            next: Cell::new(0),
            mtm,
        });
        let session = Rc::new(Session {
            id: Cell::new(0),
            environment: env,
            client: Retained::from(&*client),
            host: RefCell::new(Host::new(Box::new(Offline))),
            keyboard: RefCell::new(Keyboard::default()),
            location: RefCell::new(Location::default()),
            active: Cell::new(false),
            busy: Cell::new(false),
            epoch: Cell::new(0),
            marked: RefCell::new(String::new()),
        });
        session.activate();
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
        session.activate();
        assert_ne!(old_id, session.id.get());
        session.environment.action(Action::Menu {
            owner: old_id,
            action: MenuAction::Key(Key::ToggleAscii),
        });
        assert_eq!(session.epoch.get(), old_epoch);
        drop(session);
        assert_eq!(&*client.ivars().committed.borrow(), "虎");
    }
}
impl Server {
    pub fn start(
        mtm: MainThreadMarker,
        factory: impl Fn() -> Box<dyn Channel> + 'static,
        show: impl Fn(Panel) + 'static,
        reason: impl Fn() -> Option<String> + 'static,
    ) -> Result<Retained<Self>, String> {
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
        let this = Self::alloc(mtm).set_ivars(environment);
        // SAFETY: 控制器类已注册，名称与随包 Info.plist 一致；父类初始化失败返回 None。
        let server: Option<Retained<Self>> = unsafe {
            msg_send![super(this), initWithName: &*NSString::from_str(CONNECTION), bundleIdentifier: &*NSString::from_str(IDENTIFIER)]
        };
        server.ok_or_else(|| "IMKServer 初始化失败".into())
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
                let environment = server?.downcast_ref::<Server>()?.ivars().clone();
                let client = client?;
                // SAFETY: IMK 提供的对象必须具备使用到的文本协议方法；缺方法时拒绝建立控制器。
                for selector in [sel!(setMarkedText:selectionRange:replacementRange:), sel!(insertText:replacementRange:), sel!(markedRange), sel!(attributedSubstringFromRange:), sel!(attributesForCharacterIndex:lineHeightRectangle:)] {
                    // SAFETY: 只询问有效 IMK 对象是否响应已知选择器，不执行文本操作。
                    if !unsafe { msg_send![client, respondsToSelector: selector] } { return None; }
                }
                let host = Host::new((environment.factory)());
                let session = Rc::new(Session { id: Cell::new(0), environment, client: client.retain(), host: RefCell::new(host), keyboard: RefCell::new(Keyboard::default()), location: RefCell::new(Location::default()), active: Cell::new(false), busy: Cell::new(false), epoch: Cell::new(0), marked: RefCell::new(String::new()) });
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
        fn activate(&self, _: Option<&AnyObject>) { callback(|| self.ivars().activate()); }
        #[unsafe(method(deactivateServer:))]
        fn deactivate(&self, _: Option<&AnyObject>) { callback(|| self.ivars().deactivate()); }
        #[unsafe(method(commitComposition:))]
        fn commit(&self, _: Option<&AnyObject>) { callback(|| self.ivars().fail()); }
        #[unsafe(method(hidePalettes))]
        fn hide(&self) { callback(|| { if self.ivars().current() { self.ivars().environment.panel.hide(); } }); }
        #[unsafe(method(doCommandBySelector:commandDictionary:))]
        fn command(&self, selector: Option<Sel>, info: Option<&NSDictionary<NSString>>) {
            callback(|| {
                if let Some(action) = Menu::forwarded(self.ivars().id.get(), selector, info) { self.ivars().environment.action(action); }
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
