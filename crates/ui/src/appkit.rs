//! 查字/设置 AppKit 视图。请求通过一次性 Completion 回到主线程；本模块不拥有词表、文件或工作线程。
use crate::{editor::Editor, model::details};
use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::ProtocolObject,
};
use objc2_app_kit::*;
use objc2_foundation::{
    NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use std::{
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    rc::{Rc, Weak},
};
use tigertype_contracts::{
    CandidateFont, Options, OutputForm, Palette, Theme, TypingMode,
    protocol::Panel,
    ui::{Reply, Request},
};
#[path = "appkit/art.rs"]
mod art;
#[path = "appkit/controls.rs"]
mod controls;
#[path = "appkit/layout.rs"]
mod layout;
use controls::*;
use layout::{LookupWidgets, SettingsWidgets};

fn callback<T>(f: impl FnOnce() -> T) -> Option<T> {
    catch_unwind(AssertUnwindSafe(f)).ok()
}
type Shared = Rc<RefCell<State>>;
type WeakState = Weak<RefCell<State>>;
type Submit = Rc<dyn Fn(Request, Completion)>;

/// 不可克隆的一次性回复。生产工作线程应在事务保存并提交热重载后才回复 Saved。
/// 丢弃也会唤醒界面并报告断开；不使用轮询、计时器或逐请求线程。
pub struct Completion(Option<MainThreadBound<WeakState>>);
impl Completion {
    pub fn finish(mut self, reply: Reply) {
        self.deliver(Ok(reply));
    }
    pub fn fail(mut self, error: String) {
        self.deliver(Err(error));
    }
    fn deliver(&mut self, reply: Result<Reply, String>) {
        let Some(target) = self.0.take() else {
            return;
        };
        DispatchQueue::main().exec_async(move || {
            callback(|| {
                let Some(mtm) = MainThreadMarker::new() else {
                    return;
                };
                let Some(shared) = target.get(mtm).upgrade() else {
                    return;
                };
                let next = {
                    let mut state = shared.borrow_mut();
                    match reply {
                        Ok(reply) => state.editor.receive(reply),
                        Err(error) => {
                            state.editor.failed(error);
                            None
                        }
                    }
                };
                refresh(&shared);
                if let Some(next) = next {
                    submit(&shared, next);
                }
            });
        });
    }
}
impl Drop for Completion {
    fn drop(&mut self) {
        self.deliver(Err("工作线程未返回结果，请重新启动输入法".into()));
    }
}

/// 由组装层在主线程持有；submit 必须非阻塞且把 Completion 交给有界工作队列。
pub struct Views(Shared);
impl Views {
    pub fn new(
        mtm: MainThreadMarker,
        options: Options,
        config: PathBuf,
        submit: impl Fn(Request, Completion) + 'static,
    ) -> Result<Self, String> {
        options.validate()?;
        if !config.is_absolute() {
            return Err("设置路径必须为绝对路径".into());
        }
        Ok(Self(Rc::new(RefCell::new(State {
            mtm,
            editor: Editor::new(options),
            config,
            submit: Rc::new(submit),
            lookup: None,
            settings: None,
            page: 0,
            swatch: None,
            next_window: 0,
            query: String::new(),
        }))))
    }
    pub fn show(&self, panel: Panel) {
        show(&self.0, panel);
    }
    pub fn close(&self) {
        let (lookup, settings) = {
            let mut s = self.0.borrow_mut();
            s.swatch = None;
            (s.lookup.take(), s.settings.take())
        };
        drop((lookup, settings));
    }
}
impl Drop for Views {
    fn drop(&mut self) {
        self.close();
    }
}
struct State {
    query: String,
    mtm: MainThreadMarker,
    editor: Editor,
    config: PathBuf,
    submit: Submit,
    lookup: Option<LookupWidgets>,
    settings: Option<SettingsWidgets>,
    page: usize,
    swatch: Option<usize>,
    next_window: u64,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Setting {
    Auto,
    Common,
    Learning,
    Mode(TypingMode),
    Form(OutputForm),
    Font(CandidateFont),
    Size(u8),
    Theme(Theme),
}
impl Setting {
    fn apply(self, o: &mut Options) {
        match self {
            Self::Auto => o.auto_commit_four = !o.auto_commit_four,
            Self::Common => o.common_only = !o.common_only,
            Self::Learning => o.show_learning = !o.show_learning,
            Self::Mode(v) => o.typing_mode = v,
            Self::Form(v) => o.output_form = v,
            Self::Font(v) => o.candidate_font = v,
            Self::Size(v) => o.page_size = v,
            Self::Theme(v) => o.theme = v,
        }
    }
    fn selected(self, o: Options) -> bool {
        match self {
            Self::Auto => o.auto_commit_four,
            Self::Common => o.common_only,
            Self::Learning => o.show_learning,
            Self::Mode(v) => o.typing_mode == v,
            Self::Form(v) => o.output_form == v,
            Self::Font(v) => o.candidate_font == v,
            Self::Size(v) => o.page_size == v,
            Self::Theme(v) => o.theme == v,
        }
    }
}
enum UiAction {
    Query(String),
    Move(bool),
    Reset,
    Page(usize),
    Set(Setting),
    Color(usize, String),
    Palette(usize),
    Swatch(u32),
    Search(String),
    Previous,
    Next,
    Select(usize),
    Copy,
    Settings,
    Reload,
    Open,
}
fn handler(weak: &WeakState, action: impl Fn() -> UiAction + 'static) -> impl Fn() + 'static {
    let weak = weak.clone();
    move || {
        if let Some(shared) = weak.upgrade() {
            act(&shared, action());
        }
    }
}
fn submit(shared: &Shared, request: Request) {
    let (send, mtm) = {
        let s = shared.borrow();
        (s.submit.clone(), s.mtm)
    };
    // 不在借用 Model 或窗口期间调用业务层，允许测试/快速失败立即完成回复。
    send(
        request,
        Completion(Some(MainThreadBound::new(Rc::downgrade(shared), mtm))),
    );
}
fn act(shared: &Shared, action: UiAction) {
    let focus_result = matches!(action, UiAction::Move(_) | UiAction::Select(_));
    if matches!(action, UiAction::Settings) {
        show(shared, Panel::Settings);
        return;
    }
    if matches!(action, UiAction::Open) {
        let path = shared.borrow().config.clone();
        let ok = path.to_str().is_some_and(|path| {
            NSWorkspace::sharedWorkspace()
                .openURL(&NSURL::fileURLWithPath(&NSString::from_str(path)))
        });
        if !ok {
            shared.borrow_mut().editor.model.settings_notice =
                "无法用默认编辑器打开配置文件".into();
        }
        refresh(shared);
        return;
    }
    let request = {
        let mut s = shared.borrow_mut();
        if s.editor.fatal {
            return;
        }
        match action {
            UiAction::Query(query) => {
                s.query = query;
                None
            }
            UiAction::Move(next) => {
                let count = s.editor.model.result.as_ref().map_or(0, |r| r.items.len());
                s.editor.selected = if next {
                    s.editor
                        .selected
                        .saturating_add(1)
                        .min(count.saturating_sub(1))
                } else {
                    s.editor.selected.saturating_sub(1)
                };
                None
            }
            UiAction::Reset => s.editor.reset_colors(),
            UiAction::Page(page) => {
                s.page = page.min(3);
                s.swatch = None;
                None
            }
            UiAction::Set(setting) => s.editor.edit(|o| setting.apply(o)),
            UiAction::Color(index, value) => s.editor.color(index, value),
            UiAction::Palette(index) => {
                s.swatch = Some(index);
                None
            }
            UiAction::Swatch(value) => {
                let index = s.swatch.take();
                index.and_then(|i| s.editor.color(i, format!("#{value:06X}")))
            }
            UiAction::Search(query) => match s.editor.model.lookup(query, 0) {
                Ok(r) => Some(r),
                Err(e) => {
                    s.editor.model.notice = e.into();
                    None
                }
            },
            UiAction::Previous | UiAction::Next => {
                let page = if matches!(action, UiAction::Previous) {
                    s.editor.model.page.saturating_sub(1)
                } else {
                    s.editor.model.page.saturating_add(1)
                };
                let query = s.editor.model.query.clone();
                match s.editor.model.lookup(query, page) {
                    Ok(r) => Some(r),
                    Err(e) => {
                        s.editor.model.notice = e.into();
                        None
                    }
                }
            }
            UiAction::Select(index) => {
                if s.editor
                    .model
                    .result
                    .as_ref()
                    .is_some_and(|r| index < r.items.len())
                {
                    s.editor.selected = index;
                }
                None
            }
            UiAction::Reload => match s.editor.model.reload() {
                Ok(r) => Some(r),
                Err(e) => {
                    s.editor.model.settings_notice = e.into();
                    None
                }
            },
            UiAction::Copy => {
                if let Some(item) = s
                    .editor
                    .model
                    .result
                    .as_ref()
                    .and_then(|r| r.items.get(s.editor.selected))
                {
                    let text = NSString::from_str(&details(item));
                    let board = NSPasteboard::generalPasteboard();
                    board.clearContents();
                    // SAFETY: 仅在用户复制动作中写普通字符串，不读取剪贴板。
                    if !unsafe { board.setString_forType(&text, NSPasteboardTypeString) } {
                        s.editor.model.notice = "复制失败".into();
                    }
                }
                None
            }
            UiAction::Open | UiAction::Settings => None,
        }
    };
    refresh(shared);
    if focus_result {
        let button = {
            let s = shared.borrow();
            s.lookup
                .as_ref()
                .and_then(|w| w.selected_button(s.editor.selected))
        };
        if let Some(button) = button
            && let Some(window) = button.window()
        {
            window.makeFirstResponder(Some(&button));
        }
    }
    if let Some(request) = request {
        submit(shared, request);
    }
}
fn refresh(shared: &Shared) {
    let s = shared.borrow();
    if let Some(view) = &s.lookup {
        view.update(&s);
    }
    if let Some(view) = &s.settings {
        view.update(&s);
    }
}
fn show(shared: &Shared, panel: Panel) {
    let window = {
        let mut s = shared.borrow_mut();
        let missing = match panel {
            Panel::Lookup => s
                .lookup
                .as_ref()
                .is_none_or(|w| !w.native.window.isVisible()),
            Panel::Settings => s
                .settings
                .as_ref()
                .is_none_or(|w| !w.native.window.isVisible()),
        };
        if missing {
            let Some(next) = s.next_window.checked_add(1) else {
                return;
            };
            s.next_window = next;
            let weak = Rc::downgrade(shared);
            match panel {
                Panel::Lookup => s.lookup = Some(LookupWidgets::new(&s, &weak, next)),
                Panel::Settings => s.settings = Some(SettingsWidgets::new(&s, &weak, next)),
            }
        }
        match panel {
            Panel::Lookup => s.lookup.as_ref().map(|w| w.native.window.clone()),
            Panel::Settings => s.settings.as_ref().map(|w| w.native.window.clone()),
        }
    };
    refresh(shared);
    if let Some(window) = window {
        if window.isMiniaturized() {
            window.deminiaturize(None);
        }
        window.makeKeyAndOrderFront(None);
        // 只在用户主动打开工具时激活；候选面板从不走这里。此旧 API 兼容最低 macOS 13。
        #[allow(deprecated)]
        NSApplication::sharedApplication(window.mtm()).activateIgnoringOtherApps(true);
    }
}

struct DelegateState {
    owner: WeakState,
    panel: Panel,
    token: u64,
}
define_class!(
    // SAFETY: 每窗口独立 delegate；关闭清理异步回主线程，避免在回调栈中释放自身。
    #[unsafe(super = NSObject)]
    #[name = "TigerTypeToolsWindowDelegate"]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateState]
    struct WindowDelegate;
    // SAFETY: 保留 NSObject 语义。
    unsafe impl NSObjectProtocol for WindowDelegate {}
    impl WindowDelegate {
        #[unsafe(method(dismissOverlay))]
        fn dismiss_overlay(&self)->bool {callback(||{
            let Some(shared)=self.ivars().owner.upgrade()else{return false;};
            if self.ivars().panel!=Panel::Settings{return false;}
            let had=shared.borrow_mut().swatch.take().is_some();if had{refresh(&shared);}had
        }).unwrap_or(false)}
    }
    // SAFETY: 回调签名遵守 NSWindowDelegate，Rust panic 被截断。
    unsafe impl NSWindowDelegate for WindowDelegate {
        #[unsafe(method(windowDidResize:))]
        fn resized(&self, _: &NSNotification) {
            callback(|| {
                if let Some(shared) = self.ivars().owner.upgrade()
                    && shared.try_borrow().is_ok()
                {
                    refresh(&shared);
                }
            });
        }
        #[unsafe(method(windowWillClose:))]
        fn closed(&self, _: &NSNotification) {
            callback(|| {
                let owner = MainThreadBound::new(self.ivars().owner.clone(), self.mtm());
                let token = self.ivars().token;
                let panel = self.ivars().panel;
                DispatchQueue::main().exec_async(move || {
                    callback(|| {
                        let Some(mtm) = MainThreadMarker::new() else {
                            return;
                        };
                        let Some(shared) = owner.get(mtm).upgrade() else {
                            return;
                        };
                        let mut s = shared.borrow_mut();
                        match panel {
                            Panel::Lookup
                                if s.lookup.as_ref().is_some_and(|w| w.native.token == token) =>
                            {
                                s.lookup = None;
                            }
                            Panel::Settings
                                if s.settings.as_ref().is_some_and(|w| w.native.token == token) =>
                            {
                                s.settings = None;
                                s.swatch = None;
                            }
                            _ => {}
                        }
                    });
                });
            });
        }
    }
);
define_class!(
    // SAFETY: Esc 走原生关闭路径，不终止进程或删除模型。
    #[unsafe(super = NSWindow)]
    #[name = "TigerTypeToolsWindow"]
    #[thread_kind = MainThreadOnly]
    struct ToolWindow;
    // SAFETY: 不改变 NSObject 语义。
    unsafe impl NSObjectProtocol for ToolWindow {}
    impl ToolWindow {
        #[unsafe(method(cancelOperation:))]
        fn cancel(&self,_:Option<&objc2::runtime::AnyObject>){callback(||{
            if let Some(delegate)=self.delegate(){
                // SAFETY: ToolWindow 的 delegate 只由 NativeWindow 安装为 WindowDelegate，签名为无参 BOOL。
                let handled:bool=unsafe{msg_send![&*delegate,dismissOverlay]};if handled{return;}
            }
            self.performClose(None);
        });}
    }
);
struct NativeWindow {
    window: Retained<ToolWindow>,
    _delegate: Retained<WindowDelegate>,
    token: u64,
}
impl NativeWindow {
    fn new(s: &State, owner: &WeakState, panel: Panel, token: u64, size: (f64, f64)) -> Self {
        let this = WindowDelegate::alloc(s.mtm).set_ivars(DelegateState {
            owner: owner.clone(),
            panel,
            token,
        });
        // SAFETY: 指定初始化器；关闭自动释放，Rust 所有权负责关闭和最终释放。
        let (window, delegate) = unsafe {
            let delegate: Retained<WindowDelegate> = msg_send![super(this), init];
            let window: Retained<ToolWindow> = msg_send![ToolWindow::alloc(s.mtm),initWithContentRect:rect(0.0,0.0,size.0,size.1),
                styleMask:NSWindowStyleMask::Titled|NSWindowStyleMask::Closable|NSWindowStyleMask::Miniaturizable,
                backing:NSBackingStoreType::Buffered,defer:false];
            window.setReleasedWhenClosed(false);
            (window, delegate)
        };
        window.setTitle(&NSString::from_str(match panel {
            Panel::Lookup => "TigerType 查字",
            Panel::Settings => "TigerType 设置",
        }));
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        window.center();
        Self {
            window,
            _delegate: delegate,
            token,
        }
    }
}
impl Drop for NativeWindow {
    fn drop(&mut self) {
        self.window.setDelegate(None);
        self.window.orderOut(None);
        self.window.close();
    }
}

#[cfg(test)]
pub mod tests {
    //! 主线程隐藏窗口与异步回复断言；只能在 Mac 执行，交叉编译不等于这些断言已通过。
    use super::*;
    use objc2_foundation::{NSDate, NSDictionary, NSRunLoop};
    fn drain(done: impl Fn() -> bool) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !done() {
            assert!(std::time::Instant::now() < until, "主线程回复未到达");
            NSRunLoop::mainRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.02));
        }
    }
    fn render(view: &NSView, path: &std::path::Path) {
        let image = view
            .bitmapImageRepForCachingDisplayInRect(view.bounds())
            .expect("无离屏缓冲");
        view.cacheDisplayInRect_toBitmapImageRep(view.bounds(), &image);
        // SAFETY: PNG 属性字典为空，使用 AppKit 默认编码参数。
        let png = unsafe {
            image.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            )
        }
        .expect("PNG 编码失败");
        assert!(png.len() > 100);
        std::fs::write(path, png.to_vec()).unwrap();
    }
    pub fn run() {
        let mtm = MainThreadMarker::new().expect("AppKit 检查必须在主线程");
        let _app = NSApplication::sharedApplication(mtm);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("tigertype-appkit-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let requests = Rc::new(RefCell::new(Vec::new()));
        let pending = requests.clone();
        let views = Views::new(
            mtm,
            Options::default(),
            root.join("settings.toml"),
            move |r, c| pending.borrow_mut().push((r, c)),
        )
        .unwrap();
        let _show: fn(&Views, Panel) = Views::show; // 只检查可调用契约；本测试不显示或激活窗口。
        {
            let mut s = views.0.borrow_mut();
            let weak = Rc::downgrade(&views.0);
            s.lookup = Some(LookupWidgets::new(&s, &weak, 1));
            s.settings = Some(SettingsWidgets::new(&s, &weak, 2));
        }
        for theme in Theme::ALL {
            for page in 0..4 {
                {
                    let mut s = views.0.borrow_mut();
                    s.page = page;
                    s.editor.draft.theme = theme;
                }
                refresh(&views.0);
                let s = views.0.borrow();
                let window = &s.settings.as_ref().unwrap().native.window;
                assert!(!window.isVisible());
                render(
                    &window.contentView().unwrap(),
                    &root.join(format!("settings-{theme:?}-{page}.png")),
                );
            }
        }
        views.0.borrow_mut().editor = Editor::new(Options::default());
        // 非 BMP、长编码与拆分均使用合成资料，不读个人词库。
        act(&views.0, UiAction::Search("合成".into()));
        let (_, reply) = requests.borrow_mut().pop().unwrap();
        reply.finish(Reply::Lookup(Ok(tigertype_contracts::LookupPage {
            has_more: true,
            items: vec![tigertype_contracts::LookupItem {
                text: "𠀀".into(),
                learning: tigertype_contracts::LearningInfo {
                    codes: vec!["aaaa".into(), "bbbb".into()],
                    ..Default::default()
                },
            }],
        })));
        drain(|| !views.0.borrow().editor.model.busy);
        {
            let s = views.0.borrow();
            assert_eq!(s.editor.model.result.as_ref().unwrap().items[0].text, "𠀀");
            render(
                &s.lookup
                    .as_ref()
                    .unwrap()
                    .native
                    .window
                    .contentView()
                    .unwrap(),
                &root.join("lookup.png"),
            );
        }
        // 保存途中关闭并重建窗口，旧回复只能更新模型，不能访问已释放视图。
        act(&views.0, UiAction::Set(Setting::Size(7)));
        let (request, reply) = requests.borrow_mut().pop().unwrap();
        let Request::SaveSettings(expected) = request else {
            panic!("请求错配");
        };
        views.0.borrow_mut().swatch = Some(0);
        views.close();
        assert!(views.0.borrow().swatch.is_none(), "关闭不能留下旧色板状态");
        {
            let mut s = views.0.borrow_mut();
            s.settings = Some(SettingsWidgets::new(&s, &Rc::downgrade(&views.0), 3));
        }
        reply.finish(Reply::Saved(Ok(expected)));
        drain(|| !views.0.borrow().editor.model.busy);
        assert_eq!(views.0.borrow().editor.model.options.page_size, 7);
        act(&views.0, UiAction::Set(Setting::Size(9)));
        let (_, reply) = requests.borrow_mut().pop().unwrap();
        reply.fail("合成保存失败".into());
        drain(|| !views.0.borrow().editor.model.busy);
        assert_eq!(views.0.borrow().editor.draft.page_size, 7);
        act(&views.0, UiAction::Reload);
        let (_, reply) = requests.borrow_mut().pop().unwrap();
        drop(reply);
        drain(|| !views.0.borrow().editor.model.busy);
        assert!(
            views
                .0
                .borrow()
                .editor
                .model
                .settings_notice
                .contains("未返回")
        );
        views.close();
        println!(
            "AppKit 原生断言通过；离屏图像：{}（仍须人工视觉核对）",
            root.display()
        );
    }
}
