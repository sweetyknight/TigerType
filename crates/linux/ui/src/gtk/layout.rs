//! GTK 直接消费共用布局：控件放在固定容器里共用布局给出的坐标上，不另维护位置；
//! 只有字体、标题栏与助记键写法按平台调整。
use super::paint::{Art, Canvas, FaceState, Paint, Scene, candidate_font, paint_face, text_width};
use super::*;
use gtk::pango;
use tigertype_contracts::{Palette, Theme};
use tigertype_ui_model::{
    editor::Setting,
    layout::*,
    view::{self, Tone},
};

/// 按共用布局的框放置控件：固定容器按控件请求尺寸分配，请求即布局框。
fn put(fixed: &gtk::Fixed, widget: &impl IsA<gtk::Widget>, b: Box4) {
    widget.set_size_request(b[2].max(0), b[3].max(0));
    if widget.parent().is_some() {
        fixed.move_(widget, b[0] as f64, b[1] as f64);
    } else {
        fixed.put(widget, b[0] as f64, b[1] as f64);
    }
}
/// Alt+字母助记键：交给窗口统一处理，与 GTK 标签助记键同一机制。
fn mnemonic(widget: &impl IsA<gtk::Widget>, key: char, action: gtk::ShortcutAction) {
    let Some(key) = gdk::Key::from_name(key.to_string()) else {
        return;
    };
    let shortcuts = gtk::ShortcutController::new();
    shortcuts.set_scope(gtk::ShortcutScope::Managed);
    shortcuts.add_shortcut(gtk::Shortcut::new(
        Some(gtk::MnemonicTrigger::new(key)),
        Some(action),
    ));
    widget.add_controller(shortcuts);
}
/// 焦点进出输入框时重画外框；状态正被借用（本次更新途中）则跳过，随后的更新会画出正确外框。
fn redraw(weak: &WeakState) -> impl Fn() + Clone + 'static {
    let weak = weak.clone();
    move || {
        if let Some(shared) = weak.upgrade()
            && shared.try_borrow_mut().is_ok()
        {
            refresh(&shared);
        }
    }
}

/// 自绘按钮：GTK 按钮负责键盘、激活与读屏，外观画在唯一的子画布上。
pub(super) struct Face {
    pub button: gtk::Button,
    paint: Paint,
    state: Rc<FaceState>,
}
impl Face {
    fn new(
        id: usize,
        text: &str,
        p: Palette,
        swatch: Option<u32>,
        click: impl Fn() + 'static,
    ) -> Self {
        let radio = [
            (MODE, 2),
            (FORM, 3),
            (FONT, 2),
            (SIZE_CHOICE, 7),
            (THEME, Theme::ALL.len()),
            (NAV, PAGES.len()),
            // 九个查字字块同用 LIST 一个编号；其后紧接着是翻页、复制、设置按钮的编号。
            (LIST, 1),
        ]
        .iter()
        .any(|(start, count)| (*start..*start + count).contains(&id));
        // 保留系统可访问性中的开关/单选语义，像素仍由画布绘制。
        let role = if [AUTO, COMMON, LEARNING].contains(&id) {
            gtk::AccessibleRole::Switch
        } else if radio {
            gtk::AccessibleRole::Radio
        } else {
            gtk::AccessibleRole::Button
        };
        let button: gtk::Button = glib::Object::builder()
            .property("accessible-role", role)
            .build();
        button.add_css_class("tt");
        // 鼠标点击不抢键盘焦点，焦点环只随键盘操作出现。
        button.set_focus_on_click(false);
        let (shown, key) = style::caption(id, text);
        let state = Rc::new(FaceState {
            style: id,
            title: RefCell::new(shown),
            palette: Cell::new(p),
            selected: Cell::new(false),
            enabled: Cell::new(true),
            swatch: Cell::new(swatch),
            preview: Cell::new(None),
            font: RefCell::new(None),
        });
        let source = state.clone();
        let paint = Paint::new(gtk::AccessibleRole::Presentation, move |w, s| {
            if let Some(button) = WidgetExt::parent(w) {
                paint_face(&button, w, s, &source);
            }
        });
        button.set_child(Some(&paint));
        button.update_property(&[gtk::accessible::Property::Label(&view::strip_mnemonic(
            text,
        ))]);
        if let Some((.., description)) = TOGGLES.iter().find(|(key, ..)| *key == id) {
            button.update_property(&[gtk::accessible::Property::Description(description)]);
        }
        let gate = state.clone();
        button.connect_clicked(move |_| {
            if gate.enabled.get() {
                callback(&click);
            }
        });
        let face = paint.clone();
        button.connect_state_flags_changed(move |_, _| face.queue_draw());
        if let Some(key) = key {
            mnemonic(&button, key, gtk::ActivateAction::get().upcast());
        }
        Self {
            button,
            paint,
            state,
        }
    }
    /// soft：只画停用并告知读屏，不真的停用（用于设置控件，重新载入时焦点不丢）。
    fn update(&self, p: Palette, selected: bool, enabled: bool, soft: bool) {
        self.state.palette.set(p);
        self.state.selected.set(selected);
        self.state.enabled.set(enabled || !soft);
        if !soft {
            self.button.set_sensitive(enabled);
        }
        self.button
            .update_state(&[gtk::accessible::State::Disabled(!enabled)]);
        if self.button.accessible_role() != gtk::AccessibleRole::Button {
            self.button
                .update_state(&[gtk::accessible::State::Checked(if selected {
                    gtk::AccessibleTristate::True
                } else {
                    gtk::AccessibleTristate::False
                })]);
        }
        self.paint.queue_draw();
    }
    fn set_title(&self, value: &str) {
        if *self.state.title.borrow() != value {
            *self.state.title.borrow_mut() = value.into();
            self.button
                .update_property(&[gtk::accessible::Property::Label(value)]);
        }
    }
}

/// 文本框：外框由父画布画，GTK 文本控件只管编辑、输入法与选区；程序改写文字时不回传为用户编辑。
pub(super) struct Field {
    pub text: gtk::Text,
    changed: glib::SignalHandlerId,
}
impl Field {
    fn new(name: &str, mono: bool, weak: &WeakState, change: impl Fn(String) + 'static) -> Self {
        // GTK 文本控件默认不进读屏树（原由外层 Entry 代表），构造时声明为文本框。
        let text: gtk::Text = glib::Object::builder()
            .property("accessible-role", gtk::AccessibleRole::TextBox)
            .build();
        text.add_css_class("tt");
        if mono {
            text.add_css_class("mono");
        }
        // 宽度只由布局框决定，不按默认字符数撑大固定容器。
        text.set_width_chars(1);
        text.set_max_width_chars(1);
        text.update_property(&[gtk::accessible::Property::Label(name)]);
        let changed = text.connect_changed(move |text| {
            let value = text.text().to_string();
            callback(|| change(value));
        });
        let focus = gtk::EventControllerFocus::new();
        let enter = redraw(weak);
        let leave = enter.clone();
        focus.connect_enter(move |_| {
            callback(&enter);
        });
        focus.connect_leave(move |_| {
            callback(&leave);
        });
        text.add_controller(focus);
        Self { text, changed }
    }
    fn set(&self, value: &str) {
        if self.text.text() != value {
            self.text.block_signal(&self.changed);
            self.text.set_text(value);
            self.text.unblock_signal(&self.changed);
        }
    }
    fn editing(&self) -> bool {
        self.text.has_focus()
    }
}

enum Control {
    Button(Face),
    Field(Field),
    Label(gtk::Label),
    Art(Art),
}
/// 静态文字：字号、粗细与颜色角色取自共用样式表，颜色随主题换在样式表里。
fn label(id: usize, value: &str) -> gtk::Label {
    let t = view::text_style(id);
    let label = gtk::Label::new(Some(value));
    label.add_css_class("tt");
    label.add_css_class(&format!("size-{}", t.size as i32));
    match t.tone {
        Tone::Text => {}
        Tone::Label => label.add_css_class("label"),
        Tone::Muted => label.add_css_class("muted"),
    }
    if t.strong {
        label.add_css_class("strong");
    }
    if t.mono {
        label.add_css_class("mono");
    }
    label.set_xalign(if t.center { 0.5 } else { 0.0 });
    label.set_yalign(if t.middle { 0.5 } else { 0.0 });
    // 宽度只由布局框决定：折行按框宽折，单行超长时省略（路径省略中段，保留文件名，同 SS_PATHELLIPSIS）。
    label.set_max_width_chars(1);
    if t.wrap {
        label.set_wrap(true);
        label.set_wrap_mode(pango::WrapMode::WordChar);
    } else {
        label.set_ellipsize(if id == FILE_PATH {
            pango::EllipsizeMode::Middle
        } else {
            pango::EllipsizeMode::End
        });
    }
    if id == FILE_PATH {
        label.set_tooltip_text(Some(value));
    }
    label.set_overflow(gtk::Overflow::Hidden);
    label
}
/// 预览与快捷键格的读屏文字；查字资料由查字窗按结果另给。
fn art_value(id: usize, e: &Editor) -> Option<String> {
    if id == PREVIEW {
        return Some("候选窗预览：明天我们一起去 ui，拉，隹".into());
    }
    let (name, spec) = KEYMAP.get(id.checked_sub(KEYS)?)?;
    Some(view::key_speech(
        name,
        &view::keys(spec, e.appearance().page_size),
    ))
}
impl Control {
    fn widget(&self) -> gtk::Widget {
        match self {
            Self::Button(v) => v.button.clone().upcast(),
            Self::Field(v) => v.text.clone().upcast(),
            Self::Label(v) => v.clone().upcast(),
            Self::Art(v) => v.paint.clone().upcast(),
        }
    }
    fn editing(&self) -> bool {
        matches!(self, Self::Field(f) if f.editing())
    }
    fn title(&self, value: &str) {
        match self {
            Self::Button(v) => v.set_title(value),
            Self::Field(v) => v.set(value),
            Self::Label(v) => {
                if v.text() != value {
                    v.set_text(value);
                }
            }
            Self::Art(_) => {}
        }
    }
    /// soft 见 [`Face::update`]：设置窗控件软停用，查字窗控件真停用。
    fn update(&self, id: usize, s: &State, soft: bool) {
        let e = &s.editor;
        let p = e.appearance().palette();
        let enabled = view::enabled(e, id);
        match self {
            Self::Button(b) => {
                if (THEME..THEME + Theme::ALL.len()).contains(&id) {
                    b.state.preview.set(Some(
                        Options {
                            theme: Theme::ALL[id - THEME],
                            ..e.appearance()
                        }
                        .palette(),
                    ));
                }
                b.update(p, view::selected(e, id), enabled, soft);
            }
            Self::Field(f) => {
                // 边输边查：查询在途时查询框仍可编辑，否则正在打字的焦点会丢。
                if soft {
                    f.text.set_editable(enabled);
                } else {
                    f.text.set_sensitive(enabled);
                }
            }
            Self::Label(_) => {}
            Self::Art(a) => {
                if let Some(value) = art_value(id, e) {
                    a.update(e.appearance(), value);
                }
            }
        }
        if (CUSTOM..CUSTOM + 4).contains(&id) {
            self.title(&e.colors[id - CUSTOM]);
        }
        if id == NOTICE {
            self.title(&e.model.settings_notice);
        }
        if id == CUSTOM_STATUS {
            self.title(&view::custom_status(e));
        }
    }
}
fn button(weak: &WeakState, id: usize, title: &str, p: Palette) -> Control {
    Control::Button(Face::new(
        id,
        title,
        p,
        None,
        handler(weak, move || match id {
            OPEN_CONFIG => Action::Open,
            RELOAD => Action::Reload,
            RESET => Action::Reset,
            SETTINGS => Action::Settings,
            PREVIOUS => Action::Previous,
            NEXT => Action::Next,
            COPY => Action::Copy,
            id if (SWATCH..SWATCH + 4).contains(&id) => Action::Palette(id - SWATCH),
            _ => Action::Set(Setting::of(id).expect("布局只创建已知动作")),
        }),
    ))
}
fn tool_control(s: &State, weak: &WeakState, page: usize, id: usize) -> Control {
    let p = s.editor.appearance().palette();
    if id == PREVIEW || (KEYS..KEYS + KEYMAP.len()).contains(&id) {
        return Control::Art(Art::new(id, s.editor.appearance()));
    }
    if (CUSTOM..CUSTOM + 4).contains(&id) {
        let w = weak.clone();
        let field = Field::new(
            &format!("{}色值", COLORS[id - CUSTOM].0),
            true,
            weak,
            move |v| {
                if let Some(s) = w.upgrade() {
                    act(&s, Action::Color(id - CUSTOM, v));
                }
            },
        );
        field.set(&s.editor.colors[id - CUSTOM]);
        return Control::Field(field);
    }
    if Setting::of(id).is_some()
        || (SWATCH..SWATCH + 4).contains(&id)
        || [OPEN_CONFIG, RELOAD, RESET].contains(&id)
    {
        return button(weak, id, &view::text(id, page), p);
    }
    if (ROW_TITLE..ROW_TITLE + CHOICES.len()).contains(&id) {
        // 分段行标题同 Windows 静态文字助记键：Alt+字母把焦点移到该行当前选中的一段。
        let (shown, key) = style::caption(id, &view::text(id, page));
        let title = label(id, &shown);
        if let Some(key) = key {
            let w = weak.clone();
            let focus = gtk::CallbackAction::new(move |_, _| {
                callback(|| focus_choice(&w, id - ROW_TITLE));
                glib::Propagation::Stop
            });
            mnemonic(&title, key, focus.upcast());
        }
        return Control::Label(title);
    }
    let title = match id {
        FILE_PATH => s.config.display().to_string(),
        _ => view::strip_mnemonic(&view::text(id, page)),
    };
    Control::Label(label(id, &title))
}
fn focus_choice(weak: &WeakState, choice: usize) {
    let Some(shared) = weak.upgrade() else {
        return;
    };
    let s = shared.borrow();
    let (_, _, first, count, _) = CHOICES[choice];
    let target = s.settings.as_ref().and_then(|w| {
        w.pages
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|(id, c)| match c {
                Control::Button(b)
                    if (first..first + count).contains(id) && view::selected(&s.editor, *id) =>
                {
                    Some(b.button.clone())
                }
                _ => None,
            })
    });
    drop(s);
    if let Some(button) = target {
        button.grab_focus();
    }
}

struct Page {
    fixed: gtk::Fixed,
    canvas: Canvas,
    controls: Vec<(usize, Control)>,
}
pub(super) struct SettingsWidgets {
    pub native: NativeWindow,
    sidebar: Canvas,
    scroll: gtk::ScrolledWindow,
    pages: Vec<Page>,
    nav: Vec<Face>,
    notice: Control,
    swatches: gtk::Fixed,
    swatch_card: Canvas,
    swatch_buttons: Vec<Face>,
    /// 当前显示的分区；换分区时滚回顶部（同 Windows select_page）。
    shown: Cell<usize>,
}
impl SettingsWidgets {
    /// 测试按控件编号取原生控件，直接发出与用户操作相同的 GTK 信号。
    #[cfg(test)]
    pub fn widget(&self, id: usize) -> Option<gtk::Widget> {
        self.pages
            .iter()
            .flat_map(|p| &p.controls)
            .find(|(key, _)| *key == id)
            .map(|(_, c)| c.widget())
    }
    pub fn new(s: &State, weak: &WeakState, token: u64) -> Self {
        let (width, height) = SETTINGS_SIZE;
        let p = s.editor.appearance().palette();
        let native = NativeWindow::new(weak, Panel::Settings, token);
        native.window.set_resizable(false);
        let root = gtk::Fixed::new();
        native.window.set_child(Some(&root));
        let sidebar = Canvas::new(p);
        put(&root, &sidebar.paint, [0, 0, width, height]);
        let mut nav = Vec::new();
        let mut notice = None;
        for (id, b) in settings_spec(0, false).fixed {
            if id == NOTICE {
                let control = Control::Label(label(NOTICE, &s.editor.model.settings_notice));
                put(&root, &control.widget(), b);
                notice = Some(control);
                continue;
            }
            let i = id - NAV;
            let face = Face::new(
                id,
                PAGES[i].0,
                p,
                None,
                handler(weak, move || Action::Page(i)),
            );
            put(&root, &face.button, b);
            nav.push(face);
        }
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        put(&root, &scroll, [SIDEBAR, 0, width - SIDEBAR, height]);
        let content = gtk::Fixed::new();
        scroll.set_child(Some(&content));
        let mut pages = Vec::new();
        for page in 0..4 {
            let fixed = gtk::Fixed::new();
            content.put(&fixed, 0.0, 0.0);
            let canvas = Canvas::new(p);
            let spec = settings_spec(page, true);
            put(
                &fixed,
                &canvas.paint,
                [0, 0, width - SIDEBAR, spec.height.max(height)],
            );
            let controls = spec
                .controls
                .into_iter()
                .map(|(id, mut b)| {
                    b[0] -= SIDEBAR;
                    let control = tool_control(s, weak, page, id);
                    put(&fixed, &control.widget(), b);
                    (id, control)
                })
                .collect();
            pages.push(Page {
                fixed,
                canvas,
                controls,
            });
        }
        // 色板覆盖在设置窗内，不用同步弹出菜单，Esc 先收起它。
        let swatches = gtk::Fixed::new();
        put(&root, &swatches, [230, 340, 448, 128]);
        let swatch_card = Canvas::new(p);
        put(&swatches, &swatch_card.paint, [0, 0, 448, 128]);
        let mut swatch_buttons = Vec::new();
        for (i, c) in SWATCHES.into_iter().enumerate() {
            // 编号 0 不属于任何控件，按色板格绘制（同 Mac 未设样式的色板按钮）。
            let face = Face::new(
                0,
                &format!("#{c:06X}"),
                p,
                Some(c),
                handler(weak, move || Action::Swatch(c)),
            );
            let (x, y) = (8 + (i % 8) as i32 * 54, 8 + (i / 8) as i32 * 28);
            put(&swatches, &face.button, [x, y, 54, 28]);
            swatch_buttons.push(face);
        }
        Self {
            native,
            sidebar,
            scroll,
            pages,
            nav,
            notice: notice.expect("共用布局含侧栏状态"),
            swatches,
            swatch_card,
            swatch_buttons,
            shown: Cell::new(usize::MAX),
        }
    }
    pub fn update(&self, s: &State) {
        let e = &s.editor;
        let p = e.appearance().palette();
        self.sidebar.update(
            p,
            Scene {
                sidebar: true,
                ..Scene::default()
            },
        );
        self.notice.update(NOTICE, s, true);
        for (i, b) in self.nav.iter().enumerate() {
            b.update(p, i == e.page, !e.fatal, true);
        }
        if self.shown.replace(e.page) != e.page {
            self.scroll.vadjustment().set_value(0.0);
        }
        for (index, page) in self.pages.iter().enumerate() {
            page.fixed.set_visible(index == e.page);
            let spec = settings_spec(index, e.draft.theme == Theme::Custom);
            let shift = |mut b: Box4| {
                b[0] -= SIDEBAR;
                b
            };
            page.canvas
                .paint
                .set_size_request(SETTINGS_SIZE.0 - SIDEBAR, spec.height.max(SETTINGS_SIZE.1));
            for (id, control) in &page.controls {
                let place = spec.controls.iter().find(|(key, _)| key == id);
                control.widget().set_visible(place.is_some());
                if let Some((_, b)) = place {
                    control.update(*id, s, true);
                    put(&page.fixed, &control.widget(), shift(*b));
                    if let Control::Button(b) = control
                        && (SWATCH..SWATCH + 4).contains(id)
                    {
                        let c = e.appearance().custom;
                        b.state
                            .swatch
                            .set(Some([c.window, c.text, c.highlight, c.accent][id - SWATCH]));
                    }
                }
            }
            let editing = |k: usize| {
                page.controls
                    .iter()
                    .any(|(id, c)| *id == CUSTOM + k && c.editing())
            };
            page.canvas.update(
                p,
                Scene {
                    cards: spec.cards.into_iter().map(shift).collect(),
                    lines: spec.lines.into_iter().map(shift).collect(),
                    tracks: spec.tracks.into_iter().map(shift).collect(),
                    frames: spec
                        .frames
                        .into_iter()
                        .enumerate()
                        .map(|(k, b)| (shift(b), editing(k)))
                        .collect(),
                    tile: spec.tile.map(shift),
                    seal: spec.seal.map(shift),
                    ..Scene::default()
                },
            );
        }
        self.swatch_card.update(
            p,
            Scene {
                cards: vec![[0, 0, 448, 128]],
                ..Scene::default()
            },
        );
        self.swatches.set_visible(e.swatch_open());
        for b in &self.swatch_buttons {
            b.update(p, false, !e.fatal, true);
        }
    }
}

pub(super) struct LookupWidgets {
    pub native: NativeWindow,
    /// 内容视图：长词资料高于窗口时整窗纵向滚动，同 Windows。
    scroll: gtk::ScrolledWindow,
    root: gtk::Fixed,
    canvas: Canvas,
    controls: Vec<(usize, Control)>,
    list: gtk::ScrolledWindow,
    strip: gtk::Fixed,
    items: Vec<Face>,
}
impl LookupWidgets {
    pub fn selected_button(&self, index: usize) -> Option<gtk::Button> {
        self.items.get(index).map(|f| f.button.clone())
    }
    #[cfg(test)]
    pub fn widget(&self, id: usize) -> Option<gtk::Widget> {
        self.controls
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, c)| c.widget())
    }
    pub fn new(s: &State, weak: &WeakState, token: u64) -> Self {
        let (width, height) = LOOKUP_SIZE;
        let p = s.editor.appearance().palette();
        let native = NativeWindow::new(weak, Panel::Lookup, token);
        // 默认尺寸含 46 像素标题栏，客户区与 Windows 520×440 相同。
        native.window.set_default_size(width, height + 46);
        let scroll = gtk::ScrolledWindow::new();
        // 宽度跟随窗口重排、不出横向滚动条；高度超出时纵向滚动。
        scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Automatic);
        scroll.set_size_request(LOOKUP_MIN.0, LOOKUP_MIN.1);
        native.window.set_child(Some(&scroll));
        let root = gtk::Fixed::new();
        scroll.set_child(Some(&root));
        let canvas = Canvas::new(p);
        put(&root, &canvas.paint, [0, 0, width, height]);
        let mut controls = Vec::new();
        for (id, b) in lookup_spec(width, height, [""; 4], |_| 0).controls {
            if [QUERY_LABEL, CLIP, LIST].contains(&id) {
                continue;
            }
            let control = match id {
                QUERY => {
                    let w = weak.clone();
                    let name = view::strip_mnemonic(&view::text(QUERY_LABEL, 0));
                    let field = Field::new(&name, false, weak, move |q| {
                        if let Some(s) = w.upgrade() {
                            act(&s, Action::Query(q));
                        }
                    });
                    field.text.set_placeholder_text(Some(QUERY_HINT));
                    field.set(&s.editor.query);
                    // 回车与改字走同一查询路径，失败后不用改动文字也能重试。
                    let w = weak.clone();
                    field.text.connect_activate(move |text| {
                        callback(|| {
                            if let Some(s) = w.upgrade() {
                                act(&s, Action::Query(text.text().to_string()));
                            }
                        });
                    });
                    let focus = gtk::CallbackAction::new(|widget, _| {
                        widget.grab_focus();
                        glib::Propagation::Stop
                    });
                    mnemonic(&field.text, 'q', focus.upcast());
                    Control::Field(field)
                }
                GLYPH | CODES | FULL | READING | PARTS => {
                    Control::Art(Art::new(id, s.editor.appearance()))
                }
                SETTINGS | PREVIOUS | NEXT | COPY => button(weak, id, &view::text(id, 0), p),
                _ => Control::Label(label(id, &view::text(id, 0))),
            };
            put(&root, &control.widget(), b);
            controls.push((id, control));
        }
        let list = gtk::ScrolledWindow::new();
        list.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
        root.put(&list, 0.0, 0.0);
        let strip = gtk::Fixed::new();
        list.set_child(Some(&strip));
        let mut items = Vec::new();
        for i in 0..9 {
            let face = Face::new(LIST, "", p, None, handler(weak, move || Action::Select(i)));
            let keys = gtk::EventControllerKey::new();
            let w = weak.clone();
            // 左右方向键在字块间移动选中（同 Windows 多列列表），不交给默认焦点移动。
            keys.connect_key_pressed(move |_, key, _, modifiers| {
                let next = match key {
                    gdk::Key::Left => false,
                    gdk::Key::Right => true,
                    _ => return glib::Propagation::Proceed,
                };
                if !modifiers.is_empty() {
                    return glib::Propagation::Proceed;
                }
                callback(|| {
                    if let Some(s) = w.upgrade() {
                        act(&s, Action::Move(next));
                    }
                });
                glib::Propagation::Stop
            });
            face.button.add_controller(keys);
            strip.put(&face.button, 0.0, 0.0);
            items.push(face);
        }
        // 窗口缩放改变可见区后按新尺寸重排；放进空闲回调，不在尺寸分配途中改布局。
        let pending = Rc::new(Cell::new(false));
        for adjustment in [scroll.hadjustment(), scroll.vadjustment()] {
            let (weak, pending) = (weak.clone(), pending.clone());
            adjustment.connect_page_size_notify(move |_| {
                if pending.replace(true) {
                    return;
                }
                let (weak, pending) = (weak.clone(), pending.clone());
                glib::idle_add_local_once(move || {
                    pending.set(false);
                    callback(redraw(&weak));
                });
            });
        }
        Self {
            native,
            scroll,
            root,
            canvas,
            controls,
            list,
            strip,
            items,
        }
    }
    /// 可见区尺寸；首次显示前尚未分配时用默认客户区。
    fn size(&self) -> (i32, i32) {
        let (w, h) = (
            self.scroll.hadjustment().page_size() as i32,
            self.scroll.vadjustment().page_size() as i32,
        );
        if w <= 0 || h <= 0 {
            LOOKUP_SIZE
        } else {
            (w.max(LOOKUP_MIN.0), h.max(LOOKUP_MIN.1))
        }
    }
    pub fn update(&self, s: &State) {
        let e = &s.editor;
        let p = e.appearance().palette();
        // 四行资料先算出来：行高按其折行现排，绘制再用同一份文字。
        let texts: [String; 4] = e.item().map_or_else(Default::default, view::detail_texts);
        let (width, height) = self.size();
        let spec = lookup_spec(
            width,
            height,
            texts.each_ref().map(String::as_str),
            |line| text_width(&self.root, line, 14.0),
        );
        // 画布至少与可见区等高；更高时出现纵向滚动条。
        self.canvas
            .paint
            .set_size_request(width, spec.height.max(height));
        let editing = self
            .controls
            .iter()
            .any(|(id, c)| *id == QUERY && c.editing());
        self.canvas.update(
            p,
            Scene {
                cards: vec![spec.card],
                frames: vec![(spec.field, editing)],
                search: Some(spec.field),
                ..Scene::default()
            },
        );
        for (id, control) in &self.controls {
            control.update(*id, s, false);
            control.widget().set_visible(view::lookup_visible(e, *id));
            if let Some(value) = view::lookup_value(e, *id, &texts) {
                if let Control::Art(a) = control {
                    a.update(e.appearance(), value);
                } else {
                    control.title(&value);
                }
            }
            if let Some((_, b)) = spec.controls.iter().find(|(key, _)| key == id) {
                put(&self.root, &control.widget(), *b);
            }
        }
        let list = spec.controls.iter().find(|(id, _)| *id == LIST).unwrap().1;
        put(&self.root, &self.list, list);
        // 同 Windows 多列列表：各字块等宽，取最宽字加 18 像素与 42 像素中的较大者，高 40。
        let font = candidate_font(e.draft, 20.0);
        let items = e.model.result.as_ref().map_or(&[][..], |r| &r.items[..]);
        let column = items
            .iter()
            .map(|item| {
                super::paint::layout(&self.root, &item.text, &font)
                    .pixel_size()
                    .0
                    + 18
            })
            .fold(42, i32::max);
        for (index, face) in self.items.iter().enumerate() {
            let item = items.get(index);
            face.button.set_visible(item.is_some());
            if let Some(item) = item {
                face.set_title(&item.text);
                *face.state.font.borrow_mut() = Some(font.clone());
                put(
                    &self.strip,
                    &face.button,
                    [index as i32 * column, 0, column, 40],
                );
                face.update(p, index == e.selected, !e.fatal, false);
            }
        }
        self.strip
            .set_size_request((items.len() as i32 * column).max(list[2]), 40);
        // 选中字块滚进可见区。
        let x = (e.selected as i32 * column) as f64;
        self.list.hadjustment().clamp_page(x, x + column as f64);
    }
}
