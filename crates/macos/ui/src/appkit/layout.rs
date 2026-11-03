//! AppKit 直接消费 Win32 共用布局，不另维护控件位置；只有字体、窗口外框和快捷键写法按平台调整。
use super::art::{Art, Canvas, Scene, box_rect, caption, key_parts, mac_keys, reading_width};
use super::*;
use tigertype_ui_model::layout::*;

enum Control {
    Button(Retained<Button>),
    Field(Retained<Field>),
    Label(Retained<NSTextField>),
    Art(Retained<Art>),
}
/// 静态文字的颜色角色，对应 Windows WM_CTLCOLORSTATIC 的正文、分组名与次要色。
#[derive(Clone, Copy)]
enum Tone {
    Text,
    Label,
    Muted,
}
/// 静态文字样式：字体角色、颜色、是否垂直居中（SS_CENTERIMAGE）与水平居中（SS_CENTER）、能否折行。
struct Role {
    font: Retained<NSFont>,
    tone: Tone,
    middle: bool,
    center: bool,
    wrap: bool,
}
fn role(id: usize) -> Role {
    let (font, tone, middle) = match id {
        HEADING => (strong(20.0), Tone::Text, false),
        _ if (GROUP..GROUP + GROUPS.len()).contains(&id) => (strong(13.0), Tone::Label, false),
        FILE_NAME | ABOUT_TITLE => (strong(13.0), Tone::Text, false),
        FILE_PATH | CODEPOINT => (mono(13.0), Tone::Muted, false),
        PAGE => (mono(13.0), Tone::Muted, true),
        CUSTOM_STATUS => (ui(12.0), Tone::Muted, true),
        _ if (ROW_TITLE..ROW_TITLE + 5).contains(&id) => (ui(14.0), Tone::Text, true),
        _ if (FIELD_LABEL..FIELD_LABEL + 4).contains(&id) => (ui(12.0), Tone::Muted, true),
        _ if (ROW_DESC..ROW_DESC + 5).contains(&id)
            || matches!(id, NOTICE | KEYS_NOTE | ABOUT_DESC | STATUS) =>
        {
            (ui(12.0), Tone::Muted, false)
        }
        _ => (ui(14.0), Tone::Text, false),
    };
    Role {
        font,
        tone,
        middle,
        center: matches!(id, CODEPOINT | PAGE | STATUS),
        wrap: matches!(id, NOTICE | STATUS | CUSTOM_STATUS) || id == ROW_DESC + 4,
    }
}
fn text(mtm: MainThreadMarker, id: usize, value: &str) -> Retained<NSTextField> {
    let r = role(id);
    let value = NSString::from_str(value);
    let label = if r.wrap {
        NSTextField::wrappingLabelWithString(&value, mtm)
    } else {
        NSTextField::labelWithString(&value, mtm)
    };
    label.setFont(Some(&r.font));
    if r.center {
        label.setAlignment(NSTextAlignment::Center);
    }
    if id == FILE_PATH {
        // 同 SS_PATHELLIPSIS：路径过长时省略中段，保留文件名。
        label.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    }
    label.setToolTip(Some(&value));
    label
}
/// 焦点进出输入框时重画外框；状态正被借用（本次更新途中）则跳过，随后的更新会画出正确外框。
fn redraw(weak: &WeakState) -> impl Fn() + 'static {
    let weak = weak.clone();
    move || {
        if let Some(shared) = weak.upgrade()
            && shared.try_borrow_mut().is_ok()
        {
            refresh(&shared);
        }
    }
}
impl Control {
    fn view(&self) -> &NSView {
        match self {
            Self::Button(v) => v,
            Self::Field(v) => v,
            Self::Label(v) => v,
            Self::Art(v) => v,
        }
    }
    /// 垂直居中的单行标签按实际行高放到框中线，其余控件直接用共用布局的框。
    fn place(&self, id: usize, frame: NSRect) {
        let Self::Label(label) = self else {
            self.view().setFrame(frame);
            return;
        };
        if !role(id).middle {
            label.setFrame(frame);
            return;
        }
        let natural = label
            .sizeThatFits(NSSize::new(frame.size.width, frame.size.height))
            .height;
        label.setFrame(rect(
            frame.origin.x,
            frame.origin.y + ((frame.size.height - natural) / 2.0).floor(),
            frame.size.width,
            natural,
        ));
    }
    fn editing(&self) -> bool {
        matches!(self, Self::Field(f) if f.editing())
    }
    fn title(&self, value: &str) {
        match self {
            Self::Button(v) => {
                v.setTitle(&NSString::from_str(value));
                v.setAccessibilityLabel(Some(&NSString::from_str(value)));
            }
            Self::Field(v) => {
                let value = NSString::from_str(value);
                if v.stringValue() != value {
                    v.setStringValue(&value);
                }
            }
            Self::Label(v) => {
                let value = NSString::from_str(value);
                v.setStringValue(&value);
                v.setToolTip(Some(&value));
            }
            Self::Art(_) => {}
        }
    }
    fn update(&self, id: usize, s: &State) {
        let e = &s.editor;
        let p = e.appearance().palette();
        match self {
            Self::Button(b) => {
                b.update(p, setting(id).is_some_and(|v| v.selected(e.draft)));
                if (THEME..THEME + Theme::ALL.len()).contains(&id) {
                    b.set_preview(
                        Options {
                            theme: Theme::ALL[id - THEME],
                            ..e.appearance()
                        }
                        .palette(),
                    );
                }
                let edits =
                    setting(id).is_some() || (SWATCH..SWATCH + 4).contains(&id) || id == RESET;
                b.setEnabled(
                    !e.fatal && (!edits || !e.model.reloading()) && (id != RELOAD || !e.model.busy),
                );
            }
            Self::Field(f) => {
                f.update(p);
                f.setEnabled(!e.fatal && !e.model.reloading());
            }
            Self::Label(v) => {
                v.setTextColor(Some(&color(match role(id).tone {
                    Tone::Text => p.text,
                    Tone::Label => p.label,
                    Tone::Muted => p.muted,
                })));
            }
            Self::Art(v) => {
                v.update(
                    e.appearance(),
                    if id == PREVIEW {
                        "候选窗预览：明天我们一起去 ui，拉，隹".into()
                    } else if (KEYS..KEYS + KEYMAP.len()).contains(&id) {
                        // 读屏朗读去掉键帽括号的文字，画面仍由 KEYMAP 自绘。
                        let (name, keys) = KEYMAP[id - KEYS];
                        let keys = mac_keys(keys, e.appearance().page_size);
                        let spoken: Vec<&str> =
                            key_parts(&keys).into_iter().map(|(_, part)| part).collect();
                        format!("{name}：{}", spoken.join(" "))
                    } else {
                        String::new()
                    },
                );
            }
        }
        if (CUSTOM..CUSTOM + 4).contains(&id) {
            self.title(&e.colors[id - CUSTOM]);
        }
        if id == NOTICE {
            self.title(&e.model.settings_notice);
        }
        if id == CUSTOM_STATUS {
            // 与 Windows custom_status 同一措辞：可读时报最低对比度，否则说明原因。
            self.title(&match &e.invalid {
                Some(error) => format!("✗ {error}；修正后自动保存"),
                None => format!("✓ 全部文字可读，最低对比度 {:.1} : 1", p.min_contrast()),
            });
        }
    }
}
fn setting(id: usize) -> Option<Setting> {
    Some(match id {
        AUTO => Setting::Auto,
        COMMON => Setting::Common,
        LEARNING => Setting::Learning,
        id if (MODE..MODE + 2).contains(&id) => Setting::Mode(TypingMode::ALL[id - MODE]),
        id if (FORM..FORM + 3).contains(&id) => Setting::Form(FORMS[id - FORM].0),
        id if (FONT..FONT + 2).contains(&id) => Setting::Font(CandidateFont::ALL[id - FONT]),
        id if (SIZE_CHOICE..SIZE_CHOICE + 7).contains(&id) => {
            Setting::Size((id - SIZE_CHOICE + 3) as u8)
        }
        id if (THEME..THEME + Theme::ALL.len()).contains(&id) => {
            Setting::Theme(Theme::ALL[id - THEME])
        }
        _ => return None,
    })
}
fn button(
    mtm: MainThreadMarker,
    weak: &WeakState,
    id: usize,
    title: &str,
    frame: NSRect,
    p: Palette,
) -> Control {
    let b = Button::new(
        mtm,
        &caption(title),
        frame,
        p,
        None,
        handler(weak, move || match id {
            OPEN_CONFIG => UiAction::Open,
            RELOAD => UiAction::Reload,
            RESET => UiAction::Reset,
            SETTINGS => UiAction::Settings,
            PREVIOUS => UiAction::Previous,
            NEXT => UiAction::Next,
            COPY => UiAction::Copy,
            id if (SWATCH..SWATCH + 4).contains(&id) => UiAction::Palette(id - SWATCH),
            _ => UiAction::Set(setting(id).expect("布局只创建已知动作")),
        }),
    );
    b.style(id);
    Control::Button(b)
}
fn tool_control(s: &State, weak: &WeakState, page: usize, id: usize, frame: NSRect) -> Control {
    let p = s.editor.appearance().palette();
    if id == PREVIEW || (KEYS..KEYS + KEYMAP.len()).contains(&id) {
        return Control::Art(Art::new(s.mtm, id, frame, s.editor.appearance()));
    }
    if (CUSTOM..CUSTOM + 4).contains(&id) {
        let w = weak.clone();
        let field = Field::new(
            s.mtm,
            &format!("{}色", COLORS[id - CUSTOM].0),
            frame,
            p,
            |_| {},
            move |v| {
                if let Some(s) = w.upgrade() {
                    act(&s, UiAction::Color(id - CUSTOM, v));
                }
            },
        );
        field.setFont(Some(&mono(13.0)));
        field.on_focus(redraw(weak));
        return Control::Field(field);
    }
    let title = if let Some((_, name, _)) = TOGGLES.iter().find(|(key, ..)| *key == id) {
        caption(name)
    } else if (MODE..MODE + 2).contains(&id) {
        TypingMode::ALL[id - MODE].name().into()
    } else if (FORM..FORM + 3).contains(&id) {
        FORMS[id - FORM].1.into()
    } else if (FONT..FONT + 2).contains(&id) {
        CandidateFont::ALL[id - FONT].name().into()
    } else if (SIZE_CHOICE..SIZE_CHOICE + 7).contains(&id) {
        (id - SIZE_CHOICE + 3).to_string()
    } else if (THEME..THEME + Theme::ALL.len()).contains(&id) {
        Theme::ALL[id - THEME].name().into()
    } else if (SWATCH..SWATCH + 4).contains(&id) {
        format!("{}色", COLORS[id - SWATCH].0)
    } else {
        match id {
            HEADING => PAGES[page].0.into(),
            id if (GROUP..GROUP + GROUPS.len()).contains(&id) => GROUPS[id - GROUP].into(),
            id if (ROW_TITLE..ROW_TITLE + 4).contains(&id) => caption(CHOICES[id - ROW_TITLE].0),
            id if (ROW_DESC..ROW_DESC + 4).contains(&id) => caption(CHOICES[id - ROW_DESC].1),
            id if id == ROW_TITLE + 4 => RELOAD_TITLE.into(),
            id if id == ROW_DESC + 4 => RELOAD_DESC.into(),
            OPEN_CONFIG => OPEN_TEXT.into(),
            RELOAD => RELOAD_TEXT.into(),
            RESET => RESET_TEXT.into(),
            FILE_NAME => "settings.toml".into(),
            FILE_PATH => s.config.display().to_string(),
            ABOUT_TITLE => ABOUT_NAME.into(),
            ABOUT_DESC => about_description(),
            KEYS_NOTE => KEYS_NOTE_TEXT.into(),
            _ => String::new(),
        }
    };
    if setting(id).is_some()
        || (SWATCH..SWATCH + 4).contains(&id)
        || [OPEN_CONFIG, RELOAD, RESET].contains(&id)
    {
        return button(s.mtm, weak, id, &title, frame, p);
    }
    Control::Label(text(s.mtm, id, &title))
}

struct Page {
    canvas: Retained<Canvas>,
    controls: Vec<(usize, Control)>,
}
pub(super) struct SettingsWidgets {
    pub native: NativeWindow,
    root: Retained<Canvas>,
    scroll: Retained<NSScrollView>,
    pages: Vec<Page>,
    nav: Vec<Retained<Button>>,
    notice: Control,
    swatches: Retained<Canvas>,
    swatch_buttons: Vec<Retained<Button>>,
}
impl SettingsWidgets {
    pub fn new(s: &State, weak: &WeakState, token: u64) -> Self {
        let (width, height) = SETTINGS_SIZE;
        let p = s.editor.appearance().palette();
        let native = NativeWindow::new(
            s,
            weak,
            Panel::Settings,
            token,
            (width as f64, height as f64),
        );
        let root = Canvas::new(s.mtm, rect(0.0, 0.0, width as f64, height as f64), p);
        native.window.setContentView(Some(&root));
        let mut nav = Vec::new();
        let mut notice = None;
        for (id, b) in settings_spec(0, false).fixed {
            if id == NOTICE {
                let label = Control::Label(text(s.mtm, NOTICE, &s.editor.model.settings_notice));
                label.place(NOTICE, box_rect(b));
                add(&root, label.view());
                notice = Some(label);
                continue;
            }
            let i = id - NAV;
            let control = Button::new(
                s.mtm,
                PAGES[i].0,
                box_rect(b),
                p,
                None,
                handler(weak, move || UiAction::Page(i)),
            );
            control.style(id);
            add(&root, &control);
            nav.push(control);
        }
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(s.mtm),
            rect(SIDEBAR as f64, 0.0, (width - SIDEBAR) as f64, height as f64),
        );
        scroll.setHasVerticalScroller(true);
        scroll.setScrollerStyle(NSScrollerStyle::Overlay);
        scroll.setDrawsBackground(false);
        add(&root, &scroll);
        let mut pages = Vec::new();
        for page in 0..4 {
            let spec = settings_spec(page, true);
            let canvas = Canvas::new(
                s.mtm,
                rect(0.0, 0.0, (width - SIDEBAR) as f64, spec.height as f64),
                p,
            );
            let controls = spec
                .controls
                .into_iter()
                .map(|(id, mut b)| {
                    b[0] -= SIDEBAR;
                    let widget = tool_control(s, weak, page, id, box_rect(b));
                    add(&canvas, widget.view());
                    (id, widget)
                })
                .collect();
            pages.push(Page { canvas, controls });
        }
        // 色板覆盖在设置窗内；不使用同步菜单，避免 M1 的主线程消息被嵌套循环阻塞。
        let swatches = Canvas::new(s.mtm, rect(230.0, 340.0, 448.0, 128.0), p);
        add(&root, &swatches);
        let mut swatch_buttons = Vec::new();
        for (i, c) in SWATCHES.into_iter().enumerate() {
            let b = Button::new(
                s.mtm,
                &format!("#{c:06X}"),
                rect(
                    8.0 + (i % 8) as f64 * 54.0,
                    8.0 + (i / 8) as f64 * 28.0,
                    54.0,
                    28.0,
                ),
                p,
                Some(c),
                handler(weak, move || UiAction::Swatch(c)),
            );
            add(&swatches, &b);
            swatch_buttons.push(b);
        }
        Self {
            native,
            root,
            scroll,
            pages,
            nav,
            notice: notice.expect("共用布局含侧栏状态"),
            swatches,
            swatch_buttons,
        }
    }
    pub fn update(&self, s: &State) {
        let e = &s.editor;
        let p = e.appearance().palette();
        // 标题栏同 Windows 设置窗：侧栏色。
        self.native.decorate(p, shades(p).sidebar);
        self.root.update(
            p,
            Scene {
                sidebar: true,
                ..Scene::default()
            },
        );
        self.notice.update(NOTICE, s);
        for (i, b) in self.nav.iter().enumerate() {
            b.update(p, i == s.page);
        }
        let view: &NSView = &self.pages[s.page].canvas;
        if self
            .scroll
            .documentView()
            .is_none_or(|v| !std::ptr::eq(&*v, view))
        {
            self.scroll.setDocumentView(Some(view));
        }
        for (index, page) in self.pages.iter().enumerate() {
            let spec = settings_spec(index, e.draft.theme == Theme::Custom);
            let shift = |mut b: Box4| {
                b[0] -= SIDEBAR;
                b
            };
            page.canvas.setFrameSize(NSSize::new(
                (SETTINGS_SIZE.0 - SIDEBAR) as f64,
                spec.height.max(SETTINGS_SIZE.1) as f64,
            ));
            for (id, control) in &page.controls {
                let place = spec.controls.iter().find(|(key, _)| key == id);
                control.view().setHidden(place.is_none());
                if let Some((_, b)) = place {
                    control.update(*id, s);
                    control.place(*id, box_rect(shift(*b)));
                    if let Control::Button(b) = control
                        && (SWATCH..SWATCH + 4).contains(id)
                    {
                        let c = e.appearance().custom;
                        b.set_swatch(Some([c.window, c.text, c.highlight, c.accent][id - SWATCH]));
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
        self.swatches.update(
            p,
            Scene {
                cards: vec![[0, 0, 448, 128]],
                ..Scene::default()
            },
        );
        self.swatches.setHidden(
            s.swatch.is_none()
                || s.page != 1
                || e.draft.theme != Theme::Custom
                || e.model.reloading(),
        );
        for b in &self.swatch_buttons {
            b.update(p, false);
        }
        self.native.window.recalculateKeyViewLoop();
    }
}

pub(super) struct LookupWidgets {
    pub native: NativeWindow,
    /// 内容视图：长词资料高于窗口时整窗纵向滚动，同 Windows。
    scroll: Retained<NSScrollView>,
    root: Retained<Canvas>,
    controls: Vec<(usize, Control)>,
    list: Retained<NSScrollView>,
    strip: Retained<Flipped>,
    items: Vec<Retained<Button>>,
}
impl LookupWidgets {
    pub fn selected_button(&self, index: usize) -> Option<Retained<Button>> {
        self.items.get(index).cloned()
    }
    pub fn new(s: &State, weak: &WeakState, token: u64) -> Self {
        let (width, height) = LOOKUP_SIZE;
        let p = s.editor.appearance().palette();
        let native =
            NativeWindow::new(s, weak, Panel::Lookup, token, (width as f64, height as f64));
        native
            .window
            .setStyleMask(native.window.styleMask() | NSWindowStyleMask::Resizable);
        // 合法正尺寸，保证共用查字布局的边界。
        native
            .window
            .setContentMinSize(NSSize::new(LOOKUP_MIN.0 as f64, LOOKUP_MIN.1 as f64));
        let root = Canvas::new(s.mtm, rect(0.0, 0.0, width as f64, height as f64), p);
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(s.mtm),
            rect(0.0, 0.0, width as f64, height as f64),
        );
        scroll.setHasVerticalScroller(true);
        scroll.setScrollerStyle(NSScrollerStyle::Overlay);
        scroll.setDrawsBackground(false);
        scroll.setDocumentView(Some(&root));
        native.window.setContentView(Some(&scroll));
        let mut controls = Vec::new();
        for (id, b) in lookup_spec(width, height, [""; 4], |_| 0).controls {
            if [QUERY_LABEL, CLIP, LIST].contains(&id) {
                continue;
            }
            let frame = box_rect(b);
            let control = match id {
                QUERY => {
                    let w = weak.clone();
                    let changed = weak.clone();
                    let f = Field::new(
                        s.mtm,
                        "汉字或拼音",
                        frame,
                        p,
                        move |q| {
                            if let Some(s) = w.upgrade() {
                                act(&s, UiAction::Search(q));
                            }
                        },
                        move |q| {
                            if let Some(s) = changed.upgrade() {
                                act(&s, UiAction::Query(q));
                            }
                        },
                    );
                    f.setPlaceholderString(Some(&NSString::from_str(QUERY_HINT)));
                    f.setStringValue(&NSString::from_str(&s.query));
                    f.on_focus(redraw(weak));
                    Control::Field(f)
                }
                GLYPH | CODES | FULL | READING | PARTS => {
                    Control::Art(Art::new(s.mtm, id, frame, s.editor.appearance()))
                }
                SETTINGS | PREVIOUS | NEXT | COPY => button(
                    s.mtm,
                    weak,
                    id,
                    match id {
                        SETTINGS => "设置",
                        PREVIOUS => "上一页",
                        NEXT => "下一页",
                        _ => "复制",
                    },
                    frame,
                    p,
                ),
                _ => {
                    let title = if (FIELD_LABEL..FIELD_LABEL + 4).contains(&id) {
                        ["可用编码", "完整编码", "读音", "拆分"][id - FIELD_LABEL]
                    } else {
                        ""
                    };
                    Control::Label(text(s.mtm, id, title))
                }
            };
            add(&root, control.view());
            controls.push((id, control));
        }
        let list = NSScrollView::initWithFrame(NSScrollView::alloc(s.mtm), NSRect::ZERO);
        list.setHasHorizontalScroller(true);
        list.setScrollerStyle(NSScrollerStyle::Overlay);
        list.setDrawsBackground(false);
        let strip = Flipped::new(s.mtm, NSRect::ZERO);
        list.setDocumentView(Some(&strip));
        add(&root, &list);
        let mut items = Vec::new();
        for i in 0..9 {
            let b = Button::new(
                s.mtm,
                "",
                NSRect::ZERO,
                p,
                None,
                handler(weak, move || UiAction::Select(i)),
            );
            b.style(LIST);
            let w = weak.clone();
            b.navigation(move |next| {
                if let Some(s) = w.upgrade() {
                    act(&s, UiAction::Move(next));
                }
            });
            add(&strip, &b);
            items.push(b);
        }
        Self {
            native,
            scroll,
            root,
            controls,
            list,
            strip,
            items,
        }
    }
    pub fn update(&self, s: &State) {
        let e = &s.editor;
        let p = e.appearance().palette();
        self.native.decorate(p, p.window);
        let item = e
            .model
            .result
            .as_ref()
            .and_then(|r| r.items.get(e.selected));
        let join = |items: &[String], separator: &str| {
            if items.is_empty() {
                "缺资料".into()
            } else {
                items.join(separator)
            }
        };
        // 四行资料先算出来：行高按其折行现排，绘制再用同一份文字。
        let texts: [String; 4] = item.map_or_else(Default::default, |i| {
            [
                join(&i.learning.codes, " "),
                join(&i.learning.full_codes, " "),
                join(&i.learning.readings, "、"),
                tigertype_ui_model::model::parts(i),
            ]
        });
        let size = self.scroll.contentSize();
        let font = ui(14.0);
        let spec = lookup_spec(
            size.width as i32,
            size.height as i32,
            texts.each_ref().map(String::as_str),
            |line| reading_width(line, &font),
        );
        // 文档视图至少与可见区等高；更高时滚动条出现。
        self.root.setFrameSize(NSSize::new(
            size.width,
            spec.height.max(size.height as i32) as f64,
        ));
        let editing = self
            .controls
            .iter()
            .any(|(id, c)| *id == QUERY && c.editing());
        self.root.update(
            p,
            Scene {
                cards: vec![spec.card],
                frames: vec![(spec.field, editing)],
                search: Some(spec.field),
                ..Scene::default()
            },
        );
        for (id, control) in &self.controls {
            control.update(*id, s);
            let detail = [GLYPH, CODEPOINT, COPY, CODES, FULL, READING, PARTS].contains(id)
                || (FIELD_LABEL..FIELD_LABEL + 4).contains(id);
            control
                .view()
                .setHidden((detail && item.is_none()) || (*id == STATUS && item.is_some()));
            let value = match *id {
                STATUS => Some(e.model.notice.clone()),
                PAGE => Some(
                    e.model
                        .result
                        .as_ref()
                        .map(|_| (e.model.page + 1).to_string())
                        .unwrap_or_default(),
                ),
                GLYPH => Some(item.map(|i| i.text.clone()).unwrap_or_default()),
                CODEPOINT => Some(
                    item.filter(|i| i.text.chars().count() == 1)
                        .map(|i| format!("U+{:04X}", i.text.chars().next().unwrap() as u32))
                        .unwrap_or_default(),
                ),
                CODES => Some(texts[0].clone()),
                FULL => Some(texts[1].clone()),
                READING => Some(texts[2].clone()),
                PARTS => Some(texts[3].clone()),
                _ => None,
            };
            if let Some(value) = value {
                if let Control::Art(a) = control {
                    a.update(e.appearance(), value);
                } else {
                    control.title(&value);
                }
            }
            // 文字先更新再定位，垂直居中按新内容的行高计算。
            if let Some((_, b)) = spec.controls.iter().find(|(key, _)| key == id) {
                control.place(*id, box_rect(*b));
            }
        }
        for (id, c) in &self.controls {
            match c {
                Control::Button(b) => b.setEnabled(
                    !e.fatal
                        && match *id {
                            PREVIOUS => !e.model.busy && e.model.page > 0,
                            NEXT => {
                                !e.model.busy && e.model.result.as_ref().is_some_and(|r| r.has_more)
                            }
                            COPY => item.is_some(),
                            _ => true,
                        },
                ),
                // 边输边查：查询在途时仍可编辑，否则正在打字的焦点会丢。
                Control::Field(f) => f.setEnabled(!e.fatal),
                _ => {}
            }
        }
        let list = spec.controls.iter().find(|(id, _)| *id == LIST).unwrap().1;
        self.list.setFrame(box_rect(list));
        // 同 Windows 多列列表：各字块等宽，取最宽字加 18 点与 42 点中的较大者，高 40 点。
        let font = art::candidate_font(e.draft, 20.0);
        let items = e.model.result.as_ref().map_or(&[][..], |r| &r.items[..]);
        let column = items
            .iter()
            .map(|item| string(&item.text, &font, p.text).size().width + 18.0)
            .fold(42.0_f64, f64::max)
            .ceil();
        for (index, b) in self.items.iter().enumerate() {
            let item = items.get(index);
            b.setHidden(item.is_none());
            if let Some(item) = item {
                b.setTitle(&NSString::from_str(&item.text));
                b.setAccessibilityLabel(Some(&NSString::from_str(&item.text)));
                b.setFont(Some(&font));
                b.setFrame(rect(index as f64 * column, 0.0, column, 40.0));
                b.update(p, index == e.selected);
                b.setEnabled(!e.fatal);
            }
        }
        self.strip.setFrameSize(NSSize::new(
            (items.len() as f64 * column).max(list[2] as f64),
            40.0,
        ));
        if let Some(button) = self.items.get(e.selected) {
            button.scrollRectToVisible(button.bounds());
        }
        self.native.window.recalculateKeyViewLoop();
    }
}
