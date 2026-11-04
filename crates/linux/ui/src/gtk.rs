//! 查字/设置 GTK4 视图。请求经一次性 Completion 回到主线程；本模块不拥有词表、文件或工作线程。
//! 结构同 AppKit 视图：共用 Editor 持有草稿与查询，GTK 只建控件、绘制并转发用户动作。
use crate::style;
use gtk::{gdk, gio, glib, prelude::*};
use gtk4 as gtk;
use std::{
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    rc::{Rc, Weak},
};
use tigertype_contracts::{
    Options,
    protocol::Panel,
    ui::{Reply, Request},
};
use tigertype_ui_model::editor::{Action, Editor, Task};
#[path = "gtk/layout.rs"]
mod layout;
#[path = "gtk/paint.rs"]
mod paint;
use layout::{LookupWidgets, SettingsWidgets};

/// GTK 信号与绘制回调从 C 调入；panic 在此截断，不穿过系统回调。
fn callback<T>(f: impl FnOnce() -> T) -> Option<T> {
    catch_unwind(AssertUnwindSafe(f)).ok()
}
type Shared = Rc<RefCell<State>>;
type WeakState = Weak<RefCell<State>>;
type Submit = Rc<dyn Fn(Request, Completion)>;

/// 不可克隆的一次性回复，可在工作线程完成。生产工作线程应在事务保存并提交热重载后才回复 Saved。
/// 回复经默认主上下文的一次性空闲源回到 GTK 主线程；丢弃也会唤醒界面并报告断开，不轮询、不定时。
pub struct Completion(Option<glib::thread_guard::ThreadGuard<WeakState>>);
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
        // 默认主上下文只由 GTK 主线程迭代，守卫在那里取出并释放。
        glib::idle_add_once(move || {
            callback(|| {
                let Some(shared) = target.get_ref().upgrade() else {
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

/// 由组装层在 GTK 主线程持有；submit 必须非阻塞且把 Completion 交给有界工作队列。
pub struct Views(Shared);
impl Views {
    pub fn new(
        options: Options,
        config: PathBuf,
        submit: impl Fn(Request, Completion) + 'static,
    ) -> Result<Self, String> {
        if !gtk::is_initialized_main_thread() {
            return Err("GTK 须先在主线程初始化".into());
        }
        options.validate()?;
        if !config.is_absolute() {
            return Err("设置路径必须为绝对路径".into());
        }
        let display = gdk::Display::default().ok_or("没有可用的图形显示")?;
        let css = gtk::CssProvider::new();
        // 应用级优先于系统主题，系统强调色（常为蓝）不会透进本程序窗口。
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        Ok(Self(Rc::new(RefCell::new(State {
            editor: Editor::new(options),
            config,
            submit: Rc::new(submit),
            lookup: None,
            settings: None,
            next_window: 0,
            display,
            css,
            loaded: RefCell::default(),
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
        let s = self.0.borrow();
        gtk::style_context_remove_provider_for_display(&s.display, &s.css);
    }
}
struct State {
    editor: Editor,
    config: PathBuf,
    submit: Submit,
    lookup: Option<LookupWidgets>,
    settings: Option<SettingsWidgets>,
    next_window: u64,
    display: gdk::Display,
    css: gtk::CssProvider,
    /// 已载入的样式表；主题不变时不重载，避免每次刷新都重算全部样式。
    loaded: RefCell<String>,
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
    let send = shared.borrow().submit.clone();
    // 不在借用状态或窗口期间调用业务层，允许测试/快速失败立即完成回复。
    send(
        request,
        Completion(Some(glib::thread_guard::ThreadGuard::new(Rc::downgrade(
            shared,
        )))),
    );
}
fn act(shared: &Shared, action: Action) {
    let focus_result = matches!(action, Action::Move(_) | Action::Select(_));
    // 共用状态机处理草稿与查询；剪贴板、打开文件与唤起设置窗留在 GTK。
    let task = shared.borrow_mut().editor.act(action);
    match &task {
        Some(Task::ShowSettings) => return show(shared, Panel::Settings),
        Some(Task::OpenConfig) => open_config(shared),
        Some(Task::Copy(text)) => shared.borrow().display.clipboard().set_text(text),
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
        if let Some(button) = button {
            button.grab_focus();
        }
    }
    if let Some(Task::Send(request)) = task {
        submit(shared, request);
    }
}
/// 用桌面默认程序打开受管设置文件；异步完成，失败写进设置窗提示，不阻塞主线程。
fn open_config(shared: &Shared) {
    let (uri, context) = {
        let s = shared.borrow();
        (
            gio::File::for_path(&s.config).uri(),
            s.display.app_launch_context(),
        )
    };
    let weak = Rc::downgrade(shared);
    gio::AppInfo::launch_default_for_uri_async(
        &uri,
        Some(&context),
        None::<&gio::Cancellable>,
        move |result| {
            callback(|| {
                if let (Err(error), Some(shared)) = (result, weak.upgrade()) {
                    shared.borrow_mut().editor.model.settings_notice =
                        format!("无法用默认编辑器打开配置文件：{error}");
                    refresh(&shared);
                }
            });
        },
    );
}
fn refresh(shared: &Shared) {
    let s = shared.borrow();
    let css = style::css(s.editor.appearance().palette());
    if *s.loaded.borrow() != css {
        s.css.load_from_data(&css);
        *s.loaded.borrow_mut() = css;
    }
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
                .is_none_or(|w| !w.native.window.is_visible()),
            Panel::Settings => s
                .settings
                .as_ref()
                .is_none_or(|w| !w.native.window.is_visible()),
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
    // 只在用户主动打开工具时前置；Wayland 下是否夺取焦点由合成器决定。
    if let Some(window) = window {
        window.present();
    }
}
/// 关闭先隐藏，状态与控件在空闲回调里释放，避免在信号栈中销毁发出信号的窗口。
fn closed(owner: &WeakState, panel: Panel, token: u64) {
    let owner = owner.clone();
    glib::idle_add_local_once(move || {
        callback(|| {
            let Some(shared) = owner.upgrade() else {
                return;
            };
            let mut s = shared.borrow_mut();
            let lookup = match panel {
                Panel::Lookup if s.lookup.as_ref().is_some_and(|w| w.native.token == token) => {
                    s.lookup.take()
                }
                _ => None,
            };
            let settings = match panel {
                Panel::Settings if s.settings.as_ref().is_some_and(|w| w.native.token == token) => {
                    s.editor.close_settings();
                    s.settings.take()
                }
                _ => None,
            };
            // 先放开借用再销毁窗口：销毁引起的焦点与调整量通知可能回到本模块。
            drop(s);
            drop((lookup, settings));
        });
    });
}

/// 工具窗口：标题栏按主题着色（同 Windows 11/Mac 的标题栏着色），Esc 先收起色板再关闭。
pub(super) struct NativeWindow {
    pub window: gtk::Window,
    pub token: u64,
}
impl NativeWindow {
    fn new(owner: &WeakState, panel: Panel, token: u64) -> Self {
        let window = gtk::Window::new();
        window.add_css_class(style::ROOT);
        window.set_title(Some(match panel {
            Panel::Lookup => tigertype_ui_model::layout::LOOKUP_TITLE,
            Panel::Settings => tigertype_ui_model::layout::SETTINGS_TITLE,
        }));
        let header = gtk::HeaderBar::new();
        header.set_decoration_layout(Some(match panel {
            Panel::Lookup => ":minimize,maximize,close",
            Panel::Settings => ":close",
        }));
        window.set_titlebar(Some(&header));
        let weak = owner.clone();
        window.connect_close_request(move |window| {
            callback(|| {
                window.set_visible(false);
                closed(&weak, panel, token);
            });
            glib::Propagation::Stop
        });
        let keys = gtk::EventControllerKey::new();
        let weak = owner.clone();
        let target = window.downgrade();
        // 冒泡阶段：输入法组合中的 Esc 先由文本框的输入法处理。
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let plain = !modifiers.intersects(
                gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SHIFT_MASK
                    | gdk::ModifierType::SUPER_MASK,
            );
            if key != gdk::Key::Escape || !plain {
                return glib::Propagation::Proceed;
            }
            callback(|| {
                let dismissed = panel == Panel::Settings
                    && weak.upgrade().is_some_and(|s| {
                        s.try_borrow_mut()
                            .is_ok_and(|mut s| s.editor.dismiss_swatch())
                    });
                match (dismissed, weak.upgrade()) {
                    (true, Some(shared)) => refresh(&shared),
                    _ => {
                        if let Some(window) = target.upgrade() {
                            window.close();
                        }
                    }
                }
            });
            glib::Propagation::Stop
        });
        window.add_controller(keys);
        Self { window, token }
    }
}
impl Drop for NativeWindow {
    fn drop(&mut self) {
        self.window.destroy();
    }
}

#[cfg(test)]
pub mod tests {
    //! 主线程真实窗口与异步回复断言；需要图形会话，只能在 Linux 执行，交叉编译不等于这些断言已通过。
    use super::*;
    use tigertype_contracts::{LearningInfo, LookupItem, LookupPage, Theme};
    use tigertype_ui_model::editor::Setting;
    fn drain(done: impl Fn() -> bool) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let context = glib::MainContext::default();
        while !done() {
            assert!(std::time::Instant::now() < until, "主线程回复未到达");
            context.iteration(false);
        }
    }
    /// 跑完挂起的布局与绘制；只迭代已就绪的事件源，不阻塞等待。
    fn settle() {
        let context = glib::MainContext::default();
        for _ in 0..50 {
            while context.iteration(false) {}
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    /// 用窗口自己的渲染器把整窗（含标题栏）存成 PNG，供人工核对。
    fn render(window: &gtk::Window, path: &std::path::Path) {
        let paintable = gtk::WidgetPaintable::new(Some(window));
        let (w, h) = (window.width() as f64, window.height() as f64);
        assert!(w > 100.0 && h > 100.0, "窗口未完成布局");
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, w, h);
        let node = snapshot.to_node().expect("窗口没有绘制内容");
        let texture = window
            .renderer()
            .expect("窗口未实现")
            .render_texture(&node, None);
        texture.save_to_png(path).unwrap();
    }
    pub fn run() {
        gtk::init().expect("GTK 原生检查需要图形会话（Wayland 或 X11）");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "tigertype-linux-gtk-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let requests = Rc::new(RefCell::new(Vec::new()));
        let pending = requests.clone();
        let views = Views::new(
            Options::default(),
            root.join("settings.toml"),
            move |r, c| pending.borrow_mut().push((r, c)),
        )
        .unwrap();
        assert!(Views::new(Options::default(), "settings.toml".into(), |_, _| {}).is_err());
        views.show(Panel::Lookup);
        views.show(Panel::Settings);
        settle();
        for theme in Theme::ALL {
            for page in 0..4 {
                {
                    let mut s = views.0.borrow_mut();
                    s.editor.page = page;
                    s.editor.draft.theme = theme;
                    // 外观预览同时覆盖空注释和文楷注释两个分支。
                    s.editor.draft.show_learning = matches!(theme, Theme::Amber | Theme::Rose);
                }
                refresh(&views.0);
                settle();
                let s = views.0.borrow();
                let window = &s.settings.as_ref().unwrap().native.window;
                render(window, &root.join(format!("settings-{theme:?}-{page}.png")));
            }
        }
        views.0.borrow_mut().editor = Editor::new(Options::default());
        // 非 BMP、长编码与拆分均使用合成资料，不读个人词库。
        act(&views.0, Action::Query("合成".into()));
        let (_, reply) = requests.borrow_mut().pop().unwrap();
        reply.finish(Reply::Lookup(Ok(LookupPage {
            has_more: true,
            items: vec![LookupItem {
                text: "𠀀".into(),
                learning: LearningInfo {
                    codes: vec!["aaaa".into(), "bbbb".into()],
                    readings: vec!["qiū".into()],
                    decomposition: Some("亻\u{e4e4}小 文辶 日\u{e46a}页 缶\u{e432}人".into()),
                    ..Default::default()
                },
                roots: ["亻\u{e4e4}小", "文辶", "日\u{e46a}页", "缶\u{e432}人"]
                    .iter()
                    .map(|parts| parts.chars().map(|root| (root, "x".into())).collect())
                    .collect(),
            }],
        })));
        drain(|| !views.0.borrow().editor.model.busy);
        refresh(&views.0);
        settle();
        {
            let s = views.0.borrow();
            assert_eq!(s.editor.model.result.as_ref().unwrap().items[0].text, "𠀀");
            render(
                &s.lookup.as_ref().unwrap().native.window,
                &root.join("lookup.png"),
            );
        }
        // 键盘左右键移动选中项，焦点随之落到选中字块。
        act(&views.0, Action::Move(true));
        assert_eq!(views.0.borrow().editor.selected, 0, "只有一项时不越界");
        // 控件信号与用户操作同一路径：查询框改字即边输边查，开关点击改草稿并保存。
        use tigertype_ui_model::layout::{COMMON, QUERY};
        let query = views.0.borrow().lookup.as_ref().unwrap().widget(QUERY);
        let query = query.unwrap().downcast::<gtk::Text>().unwrap();
        query.set_text("hao");
        let (request, reply) = requests.borrow_mut().pop().unwrap();
        assert!(matches!(&request, Request::Lookup { query, page: 0 } if query == "hao"));
        reply.finish(Reply::Lookup(Ok(LookupPage {
            items: vec![],
            has_more: false,
        })));
        drain(|| !views.0.borrow().editor.model.busy);
        assert_eq!(query.text(), "hao", "刷新不能改写正在输入的查询框");
        // 激活查询框等同回车；失败后再次回车应重发同一查询，而不是必须先改字。
        for fail in [true, false] {
            query.emit_activate();
            let (request, reply) = requests.borrow_mut().pop().expect("回车未发查询");
            assert!(matches!(&request, Request::Lookup { query, page: 0 } if query == "hao"));
            reply.finish(Reply::Lookup(if fail {
                Err("合成查询失败".into())
            } else {
                Ok(LookupPage {
                    items: vec![],
                    has_more: false,
                })
            }));
            drain(|| !views.0.borrow().editor.model.busy);
        }
        let common = views.0.borrow().settings.as_ref().unwrap().widget(COMMON);
        let common = common.unwrap().downcast::<gtk::Button>().unwrap();
        common.emit_clicked();
        let (request, reply) = requests.borrow_mut().pop().unwrap();
        let Request::SaveSettings(saved) = request else {
            panic!("开关须立即保存");
        };
        assert!(!saved.common_only, "默认仅常用字，点击后关闭");
        reply.finish(Reply::Saved(Ok(saved)));
        drain(|| !views.0.borrow().editor.model.busy);
        assert!(!views.0.borrow().editor.model.options.common_only);
        // 保存途中关闭并重建窗口，旧回复只能更新模型，不能访问已释放控件。
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
        views.show(Panel::Settings);
        reply.finish(Reply::Saved(Ok(expected)));
        drain(|| !views.0.borrow().editor.model.busy);
        assert_eq!(views.0.borrow().editor.model.options.page_size, 7);
        act(&views.0, Action::Set(Setting::Size(9)));
        let (_, reply) = requests.borrow_mut().pop().unwrap();
        // 在工作线程完成，回复须经主上下文回到界面线程。
        std::thread::spawn(move || reply.fail("合成保存失败".into()))
            .join()
            .unwrap();
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
        // 窗口关闭按钮走关闭请求：隐藏后在空闲回调释放，并丢弃未填完的色值。
        let window = views
            .0
            .borrow()
            .settings
            .as_ref()
            .unwrap()
            .native
            .window
            .clone();
        window.close();
        drain(|| views.0.borrow().settings.is_none());
        views.close();
        println!(
            "GTK 原生断言通过；窗口图像：{}（仍须人工视觉核对）",
            root.display()
        );
    }
}
