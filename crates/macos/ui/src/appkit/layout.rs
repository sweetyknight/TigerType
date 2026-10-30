//! AppKit 直接消费 Win32 共用布局，不另维护控件位置；只有字体、窗口外框和快捷键写法按平台调整。
use super::art::{Art, Canvas, box_rect, caption};
use super::*;
use tigertype_ui_model::layout::*;

enum Control {
    Button(Retained<Button>),
    Field(Retained<Field>),
    Label(Retained<NSTextField>),
    Art(Retained<Art>),
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
            Self::Label(v) => v.setStringValue(&NSString::from_str(value)),
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
                v.setTextColor(Some(&color(
                    if (ROW_DESC..ROW_DESC + 5).contains(&id) || id == FILE_PATH || id == NOTICE {
                        p.muted
                    } else {
                        p.text
                    },
                )));
            }
            Self::Art(v) => {
                v.update(
                    e.appearance(),
                    if id == PREVIEW {
                        "候选窗预览：明天我们一起去 ui，拉，隹".into()
                    } else if (KEYS..KEYS + KEYMAP.len()).contains(&id) {
                        format!(
                            "{} {}",
                            KEYMAP[id - KEYS].0,
                            caption(KEYMAP[id - KEYS].1).replace("[Ctrl]", "⌃")
                        )
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
            self.title(
                &e.invalid
                    .clone()
                    .unwrap_or_else(|| format!("最低对比度 {:.2}:1", p.min_contrast())),
            );
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
        id if (FONT..FONT + 2).contains(&id) => Setting::Font(if id == FONT {
            CandidateFont::WenKai
        } else {
            CandidateFont::KaiTi
        }),
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
        title,
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
        return Control::Field(Field::new(
            s.mtm,
            COLORS[id - CUSTOM].0,
            frame,
            p,
            |_| {},
            move |v| {
                if let Some(s) = w.upgrade() {
                    act(&s, UiAction::Color(id - CUSTOM, v));
                }
            },
        ));
    }
    let title = if let Some((_, name, _)) = TOGGLES.iter().find(|(key, ..)| *key == id) {
        caption(name)
    } else if (MODE..MODE + 2).contains(&id) {
        TypingMode::ALL[id - MODE].name().into()
    } else if (FORM..FORM + 3).contains(&id) {
        FORMS[id - FORM].1.into()
    } else if (FONT..FONT + 2).contains(&id) {
        if id == FONT { "霞鹜文楷" } else { "楷体" }.into()
    } else if (SIZE_CHOICE..SIZE_CHOICE + 7).contains(&id) {
        (id - SIZE_CHOICE + 3).to_string()
    } else if (THEME..THEME + Theme::ALL.len()).contains(&id) {
        Theme::ALL[id - THEME].name().into()
    } else if (SWATCH..SWATCH + 4).contains(&id) {
        format!("{} · {}", COLORS[id - SWATCH].0, COLORS[id - SWATCH].1)
    } else {
        match id {
            HEADING => PAGES[page].0.into(),
            id if (GROUP..GROUP + GROUPS.len()).contains(&id) => GROUPS[id - GROUP].into(),
            id if (ROW_TITLE..ROW_TITLE + 4).contains(&id) => caption(CHOICES[id - ROW_TITLE].0),
            id if (ROW_DESC..ROW_DESC + 4).contains(&id) => caption(CHOICES[id - ROW_DESC].1),
            id if id == ROW_TITLE + 4 => "重新载入设置".into(),
            id if id == ROW_DESC + 4 => "手工修改文件后点此载入；失败保留当前设置".into(),
            OPEN_CONFIG => "打开".into(),
            RELOAD => "重新载入".into(),
            RESET => "恢复默认四色".into(),
            FILE_NAME => "settings.toml".into(),
            FILE_PATH => s.config.display().to_string(),
            ABOUT_TITLE => format!("TigerType {}", env!("CARGO_PKG_VERSION")),
            ABOUT_DESC => "Rust 专用虎码输入法".into(),
            KEYS_NOTE => "按当前输入状态生效".into(),
            _ => String::new(),
        }
    };
    if setting(id).is_some()
        || (SWATCH..SWATCH + 4).contains(&id)
        || [OPEN_CONFIG, RELOAD, RESET].contains(&id)
    {
        return button(s.mtm, weak, id, &title, frame, p);
    }
    let value = NSTextField::labelWithString(&NSString::from_str(&title), s.mtm);
    value.setFrame(frame);
    value.setMaximumNumberOfLines(0);
    value.setFont(Some(&NSFont::systemFontOfSize(if id == HEADING {
        22.0
    } else if (ROW_DESC..ROW_DESC + 5).contains(&id) || id == FILE_PATH {
        11.0
    } else {
        13.0
    })));
    value.setToolTip(Some(&NSString::from_str(&title)));
    Control::Label(value)
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
    notice: Retained<NSTextField>,
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
        let root = Canvas::new(s.mtm, rect(0.0, 0.0, width as f64, height as f64), p, true);
        native.window.setContentView(Some(&root));
        let mut nav = Vec::new();
        for (id, b) in settings_spec(0, false).fixed {
            if id == NOTICE {
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
        let notice = label(
            s.mtm,
            &root,
            "",
            box_rect(
                settings_spec(0, false)
                    .fixed
                    .into_iter()
                    .find(|(id, _)| *id == NOTICE)
                    .unwrap()
                    .1,
            ),
            p,
        );
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(s.mtm),
            rect(SIDEBAR as f64, 0.0, (width - SIDEBAR) as f64, height as f64),
        );
        scroll.setHasVerticalScroller(true);
        scroll.setScrollerStyle(NSScrollerStyle::Overlay);
        add(&root, &scroll);
        let mut pages = Vec::new();
        for page in 0..4 {
            let spec = settings_spec(page, true);
            let canvas = Canvas::new(
                s.mtm,
                rect(0.0, 0.0, (width - SIDEBAR) as f64, spec.height as f64),
                p,
                false,
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
        let swatches = Canvas::new(s.mtm, rect(230.0, 340.0, 448.0, 128.0), p, false);
        add(&root, &swatches);
        let mut swatch_buttons = Vec::new();
        for (i, c) in SWATCHES.into_iter().enumerate() {
            let b = Button::new(
                s.mtm,
                &format!("#{c:06X}"),
                rect((i % 8) as f64 * 56.0, (i / 8) as f64 * 32.0, 54.0, 30.0),
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
            notice,
            swatches,
            swatch_buttons,
        }
    }
    pub fn update(&self, s: &State) {
        let e = &s.editor;
        let p = e.appearance().palette();
        self.native
            .window
            .setBackgroundColor(Some(&color(p.window)));
        self.root.update(p, vec![], vec![], vec![], vec![], None);
        self.notice
            .setStringValue(&NSString::from_str(&e.model.settings_notice));
        self.notice.setTextColor(Some(&color(p.muted)));
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
        self.scroll.setBackgroundColor(&color(p.window));
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
                    control.view().setFrame(box_rect(shift(*b)));
                    control.update(*id, s);
                    if let Control::Button(b) = control
                        && (SWATCH..SWATCH + 4).contains(id)
                    {
                        let c = e.appearance().custom;
                        b.set_swatch(Some([c.window, c.text, c.highlight, c.accent][id - SWATCH]));
                    }
                }
            }
            page.canvas.update(
                p,
                spec.cards.into_iter().map(shift).collect(),
                spec.lines.into_iter().map(shift).collect(),
                spec.tracks.into_iter().map(shift).collect(),
                spec.frames.into_iter().map(shift).collect(),
                spec.seal.map(shift),
            );
        }
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
        let root = Canvas::new(s.mtm, rect(0.0, 0.0, width as f64, height as f64), p, false);
        native.window.setContentView(Some(&root));
        let mut controls = Vec::new();
        for (id, b) in lookup_spec(width, height).controls {
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
                        "拼音、虎码、汉字或词",
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
                    f.setBordered(false);
                    f.setPlaceholderString(Some(&NSString::from_str("输入拼音、虎码、汉字或词")));
                    f.setStringValue(&NSString::from_str(&s.query));
                    Control::Field(f)
                }
                GLYPH | CODES | FULL | PARTS => {
                    Control::Art(Art::new(s.mtm, id, frame, s.editor.appearance()))
                }
                SETTINGS | PREVIOUS | NEXT | COPY => button(
                    s.mtm,
                    weak,
                    id,
                    match id {
                        SETTINGS => "设置",
                        PREVIOUS => "‹",
                        NEXT => "›",
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
                    Control::Label(label(s.mtm, &root, title, frame, p))
                }
            };
            add(&root, control.view());
            controls.push((id, control));
        }
        let list = NSScrollView::initWithFrame(NSScrollView::alloc(s.mtm), NSRect::ZERO);
        list.setHasHorizontalScroller(true);
        list.setScrollerStyle(NSScrollerStyle::Overlay);
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
        let size = self.root.bounds().size;
        let spec = lookup_spec(size.width as i32, size.height as i32);
        self.root
            .update(p, vec![spec.card], vec![], vec![], vec![spec.field], None);
        let item = e
            .model
            .result
            .as_ref()
            .and_then(|r| r.items.get(e.selected));
        for (id, control) in &self.controls {
            if let Some((_, b)) = spec.controls.iter().find(|(key, _)| key == id) {
                control.view().setFrame(box_rect(*b));
            }
            control.update(*id, s);
            let detail = [GLYPH, CODEPOINT, COPY, CODES, FULL, READING, PARTS].contains(id)
                || (FIELD_LABEL..FIELD_LABEL + 4).contains(id);
            control
                .view()
                .setHidden((detail && item.is_none()) || (*id == STATUS && item.is_some()));
            let join = |items: &[String]| {
                if items.is_empty() {
                    "缺资料".into()
                } else {
                    items.join(" ")
                }
            };
            let value = match *id {
                STATUS => e.model.notice.clone(),
                PAGE => e
                    .model
                    .result
                    .as_ref()
                    .map(|_| (e.model.page + 1).to_string())
                    .unwrap_or_default(),
                GLYPH => item.map(|i| i.text.clone()).unwrap_or_default(),
                CODEPOINT => item
                    .filter(|i| i.text.chars().count() == 1)
                    .map(|i| format!("U+{:04X}", i.text.chars().next().unwrap() as u32))
                    .unwrap_or_default(),
                CODES => item.map(|i| join(&i.learning.codes)).unwrap_or_default(),
                FULL => item
                    .map(|i| join(&i.learning.full_codes))
                    .unwrap_or_default(),
                READING => item.map(|i| join(&i.learning.readings)).unwrap_or_default(),
                PARTS => item
                    .and_then(|i| i.learning.decomposition.clone())
                    .unwrap_or_else(|| "缺资料".into()),
                _ => continue,
            };
            if let Control::Art(a) = control {
                a.update(e.appearance(), value);
            } else {
                control.title(&value);
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
                Control::Field(f) => f.setEnabled(!e.model.busy && !e.fatal),
                _ => {}
            }
        }
        let list = spec.controls.iter().find(|(id, _)| *id == LIST).unwrap().1;
        self.list.setFrame(box_rect(list));
        self.list.setBackgroundColor(&color(p.window));
        let mut x = 0.0;
        for (index, b) in self.items.iter().enumerate() {
            let item = e.model.result.as_ref().and_then(|r| r.items.get(index));
            b.setHidden(item.is_none());
            if let Some(item) = item {
                b.setTitle(&NSString::from_str(&item.text));
                b.setAccessibilityLabel(Some(&NSString::from_str(&item.text)));
                let font = art::candidate_font(e.draft, 26.0);
                let w = string(&item.text, &font, p.text).size().width + 18.0;
                b.setFont(Some(&font));
                b.setFrame(rect(x, 0.0, w.max(42.0), 40.0));
                x += w.max(42.0);
                b.update(p, index == e.selected);
                b.setEnabled(!e.fatal);
            }
        }
        self.strip
            .setFrameSize(NSSize::new(x.max(list[2] as f64), 40.0));
        if let Some(button) = self.items.get(e.selected) {
            button.scrollRectToVisible(button.bounds());
        }
        self.native.window.recalculateKeyViewLoop();
    }
}
