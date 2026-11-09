//! 查字/设置 AppKit 视图。请求通过一次性 Completion 回到主线程；本模块不拥有词表、文件或工作线程。
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
    CandidateFont, Options, Palette, Theme,
    protocol::Panel,
    ui::{Reply, Request},
};
use tigertype_ui_model::editor::{Action, Editor, Task};
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
                // 补存设置优先，其回复后再按当前查询框补查。
                let next = shared.borrow_mut().editor.reply(reply);
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
            next_window: 0,
        }))))
    }
    pub fn show(&self, panel: Panel) {
        show(&self.0, panel);
    }
    pub fn close(&self) {
        let (lookup, settings) = {
            let mut s = self.0.borrow_mut();
            s.editor.close_settings();
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
    mtm: MainThreadMarker,
    editor: Editor,
    config: PathBuf,
    submit: Submit,
    lookup: Option<LookupWidgets>,
    settings: Option<SettingsWidgets>,
    next_window: u64,
}
fn handler(weak: &WeakState, action: impl Fn() -> Action + 'static) -> impl Fn() + 'static {
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
fn act(shared: &Shared, action: Action) {
    let focus_result = matches!(action, Action::Move(_) | Action::Select(_));
    // 共用状态机处理草稿与查询；剪贴板、打开文件与唤起设置窗留在 AppKit。
    let task = shared.borrow_mut().editor.act(action);
    match &task {
        Some(Task::ShowSettings) => return show(shared, Panel::Settings),
        Some(Task::OpenConfig) => {
            let path = shared.borrow().config.clone();
            let ok = path.to_str().is_some_and(|path| {
                NSWorkspace::sharedWorkspace()
                    .openURL(&NSURL::fileURLWithPath(&NSString::from_str(path)))
            });
            if !ok {
                shared.borrow_mut().editor.model.settings_notice =
                    "无法用默认编辑器打开配置文件".into();
            }
        }
        Some(Task::Copy(text)) => {
            let board = NSPasteboard::generalPasteboard();
            board.clearContents();
            // SAFETY: 仅在用户复制动作中写普通字符串，不读取剪贴板。
            if !unsafe {
                board.setString_forType(&NSString::from_str(text), NSPasteboardTypeString)
            } {
                shared.borrow_mut().editor.model.notice = "复制失败".into();
            }
        }
        _ => {}
    }
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
    if let Some(Task::Send(request)) = task {
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
            let had=shared.borrow_mut().editor.dismiss_swatch();if had{refresh(&shared);}had
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
                                s.editor.close_settings();
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
            Panel::Lookup => tigertype_ui_model::layout::LOOKUP_TITLE,
            Panel::Settings => tigertype_ui_model::layout::SETTINGS_TITLE,
        }));
        // 标题栏透明后露出窗口底色，由 decorate 按主题着色（同 Windows 11 标题栏着色）。
        window.setTitlebarAppearsTransparent(true);
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        window.center();
        Self {
            window,
            _delegate: delegate,
            token,
        }
    }
}
impl NativeWindow {
    /// 标题栏用主题色：设置窗为侧栏色、查字窗为窗口色；系统外观随主题深浅，使标题文字与按钮可读。
    fn decorate(&self, p: Palette, caption: u32) {
        self.window.setBackgroundColor(Some(&color(caption)));
        // SAFETY: 读取 AppKit 导出的只读外观名常量。
        let name = unsafe {
            if p.dark() {
                NSAppearanceNameDarkAqua
            } else {
                NSAppearanceNameAqua
            }
        };
        self.window
            .setAppearance(NSAppearance::appearanceNamed(name).as_deref());
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
    use tigertype_ui_model::editor::Setting;
    fn editing_shortcuts(mtm: MainThreadMarker) {
        let field = Field::new(
            mtm,
            "快捷键合成文本",
            rect(0.0, 0.0, 200.0, 30.0),
            Options::default().palette(),
            |_| {},
            |_| {},
        );
        // SAFETY: 独立隐藏窗口只承载合成字段编辑器，不改变用户剪贴板或真实应用。
        let window = unsafe {
            let window = NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, 200.0, 30.0),
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            );
            window.setReleasedWhenClosed(false);
            window
        };
        window.setContentView(Some(&field));
        field.setStringValue(&NSString::from_str("合成abc"));
        assert!(window.makeFirstResponder(Some(&field)));
        let editor = field.currentEditor().unwrap();
        editor.setSelectedRange(objc2_foundation::NSRange::new(5, 0));
        let text = NSString::from_str("a");
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown, NSPoint::ZERO, NSEventModifierFlags::Command,
            0.0, window.windowNumber(), None, &text, &text, false, 0,
        ).unwrap();
        assert!(
            window.performKeyEquivalent(&event),
            "无编辑菜单的输入法窗口也须处理 Command-A"
        );
        assert_eq!(editor.selectedRange(), objc2_foundation::NSRange::new(0, 5));
        window.makeFirstResponder(None);
        assert!(
            !field.performKeyEquivalent(&event),
            "未编辑的字段不得截获快捷键"
        );
    }
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
    fn selected(view: &NSView, title: &str) -> Option<bool> {
        for child in view.subviews() {
            if let Some(button) = child.downcast_ref::<Button>()
                && button.title().to_string() == title
            {
                assert_eq!(
                    button.accessibilityRole().unwrap().to_string(),
                    "AXRadioButton"
                );
                return Some(button.state() == NSControlStateValueOn);
            }
            if let Some(value) = selected(&child, title) {
                return Some(value);
            }
        }
        None
    }
    // 原生组件测试不启动产品进程；显式私有注册仓库字体，不要求用户安装系统字体。
    fn test_font() -> impl Drop {
        use objc2_foundation::NSURL;
        #[link(name = "CoreText", kind = "framework")]
        unsafe extern "C" {
            fn CTFontManagerRegisterFontsForURL(
                url: *const std::ffi::c_void,
                scope: u32,
                error: *mut *const std::ffi::c_void,
            ) -> bool;
            fn CTFontManagerUnregisterFontsForURL(
                url: *const std::ffi::c_void,
                scope: u32,
                error: *mut *const std::ffi::c_void,
            ) -> bool;
        }
        struct Font(Retained<NSURL>);
        impl Drop for Font {
            fn drop(&mut self) {
                // SAFETY: 与测试进程范围的注册配对，窗口已先释放。
                unsafe {
                    CTFontManagerUnregisterFontsForURL(
                        (&*self.0 as *const NSURL).cast(),
                        1,
                        std::ptr::null_mut(),
                    );
                }
            }
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../resources/fonts/LXGWWenKaiGBScreen.ttf");
        let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
        // SAFETY: NSURL 与 CFURL 桥接，scope=1 只对本测试进程生效。
        assert!(unsafe {
            CTFontManagerRegisterFontsForURL(
                (&*url as *const NSURL).cast(),
                1,
                std::ptr::null_mut(),
            )
        });
        Font(url)
    }
    pub fn run() {
        let _font = test_font();
        let mtm = MainThreadMarker::new().expect("AppKit 检查必须在主线程");
        let _app = NSApplication::sharedApplication(mtm);
        editing_shortcuts(mtm);
        // 深浅主题都使用自己的次要色作占位文字，不依赖当前系统外观。
        let field = Field::new(
            mtm,
            "合成查询框",
            rect(0.0, 0.0, 200.0, 30.0),
            Options::default().palette(),
            |_| {},
            |_| {},
        );
        field.setPlaceholderString(Some(&NSString::from_str("输入拼音")));
        for theme in Theme::ALL {
            let p = Options {
                theme,
                ..Default::default()
            }
            .palette();
            field.update(p);
            let placeholder = field.placeholderAttributedString().unwrap();
            // SAFETY: 非空属性字符串的索引 0 有效；无需返回属性范围。
            let ink = unsafe {
                placeholder.attribute_atIndex_effectiveRange(
                    NSForegroundColorAttributeName,
                    0,
                    std::ptr::null_mut(),
                )
            }
            .unwrap();
            assert_eq!(ink.downcast_ref::<NSColor>().unwrap(), &*color(p.muted));
        }
        let pressed = Rc::new(Cell::new(0));
        let clicks = pressed.clone();
        let button = Button::new(
            mtm,
            "合成按钮",
            rect(0.0, 0.0, 100.0, 60.0),
            Options::default().palette(),
            None,
            move || clicks.set(clicks.get() + 1),
        );
        assert!(button.isFlipped(), "自绘布局按 NSButton 的左上原点绘制");
        // SAFETY: 只对本进程合成控件执行其公开无障碍动作，不操控其他应用。
        assert!(unsafe { msg_send![&*button, accessibilityPerformPress] });
        assert_eq!(pressed.get(), 1);
        button.setEnabled(false);
        // SAFETY: 禁用控件的同一动作必须拒绝，不能绕过界面状态。
        assert!(!unsafe { msg_send![&*button, accessibilityPerformPress] });
        assert_eq!(pressed.get(), 1);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "tigertype-windows-appkit-{}-{stamp}",
            std::process::id()
        ));
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
                    s.editor.page = page;
                    s.editor.draft.theme = theme;
                    // 外观预览同时覆盖空注释和实际文楷注释的测量/绘制分支。
                    s.editor.draft.show_learning = matches!(theme, Theme::Amber | Theme::Rose);
                }
                refresh(&views.0);
                let s = views.0.borrow();
                let window = &s.settings.as_ref().unwrap().native.window;
                assert!(!window.isVisible());
                let content = window.contentView().unwrap();
                // 同一页的不同单选组均须保留原生选中状态，否则读屏器与绘制结果不一致。
                for title in match page {
                    0 => &["单字", "原字"][..],
                    1 => &["霞鹜文楷", "5"][..],
                    _ => &[],
                } {
                    assert_eq!(selected(&content, title), Some(true), "{title}");
                }
                render(
                    &content,
                    &root.join(format!("settings-{theme:?}-{page}.png")),
                );
                // 超出 520 点的分区（外观页、自定义四色）另存滚动区全长，供人工核对视口外部分。
                let document = content
                    .subviews()
                    .iter()
                    .find_map(|v| v.downcast::<NSScrollView>().ok())
                    .and_then(|s| s.documentView())
                    .expect("设置窗内容在滚动区中");
                render(
                    &document,
                    &root.join(format!("settings-{theme:?}-{page}-full.png")),
                );
                if page == 0 {
                    // 含系统标题栏的整窗：核对标题栏随主题着色、外观随深浅切换。
                    // SAFETY: 只读取本测试窗口内容视图的父视图（系统窗口框架），不保存或改动它。
                    let frame = unsafe { content.superview() }.expect("内容视图位于窗口框架中");
                    render(&frame, &root.join(format!("settings-{theme:?}-frame.png")));
                }
            }
        }
        views.0.borrow_mut().editor = Editor::new(Options::default());
        // 非 BMP、长编码与拆分均使用合成资料，不读个人词库。
        act(&views.0, Action::Query("合成".into()));
        let (_, reply) = requests.borrow_mut().pop().unwrap();
        reply.finish(Reply::Lookup(Ok(tigertype_contracts::LookupPage {
            has_more: true,
            items: vec![tigertype_contracts::LookupItem {
                text: "𠀀".into(),
                learning: tigertype_contracts::LearningInfo {
                    codes: vec!["aaaa".into(), "bbbb".into()],
                    decomposition: Some("亻\u{e4e4}小 文辶 日\u{e46a}页 缶\u{e432}人".into()),
                    ..Default::default()
                },
                // 多字拆分放不下时截断；私用区字根只有大码时编码为一个字母。
                roots: ["亻\u{e4e4}小", "文辶", "日\u{e46a}页", "缶\u{e432}人"]
                    .iter()
                    .map(|parts| parts.chars().map(|root| (root, "x".into())).collect())
                    .collect(),
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
        act(&views.0, Action::Set(Setting::Size(7)));
        let (request, reply) = requests.borrow_mut().pop().unwrap();
        let Request::SaveSettings(expected) = request else {
            panic!("请求错配");
        };
        views.0.borrow_mut().editor.swatch = Some(0);
        views.close();
        assert!(
            views.0.borrow().editor.swatch.is_none(),
            "关闭不能留下旧色板状态"
        );
        {
            let mut s = views.0.borrow_mut();
            s.settings = Some(SettingsWidgets::new(&s, &Rc::downgrade(&views.0), 3));
        }
        reply.finish(Reply::Saved(Ok(expected)));
        drain(|| !views.0.borrow().editor.model.busy);
        assert_eq!(views.0.borrow().editor.model.options.page_size, 7);
        act(&views.0, Action::Set(Setting::Size(9)));
        let (_, reply) = requests.borrow_mut().pop().unwrap();
        reply.fail("合成保存失败".into());
        drain(|| !views.0.borrow().editor.model.busy);
        assert_eq!(views.0.borrow().editor.draft.page_size, 7);
        act(&views.0, Action::Reload);
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
