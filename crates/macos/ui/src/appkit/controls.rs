//! 原生控件保留文本编辑和无障碍；按钮不进入同步跟踪循环。外观按 Windows draw_button 逐项还原：
//! 开关行、侧栏分区、主题卡、分段单选、色块行、字块与普通按钮；键盘焦点才画焦点环，鼠标点击不抢焦点。
use super::art::{caption, draw_text, fill_round, inset, ring, symbol};
use super::*;
use objc2::{AnyThread, runtime::AnyObject, sel};
use objc2_foundation::{NSAttributedString, NSDictionary};

pub(super) fn color(rgb: u32) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        ((rgb >> 16) & 255) as f64 / 255.0,
        ((rgb >> 8) & 255) as f64 / 255.0,
        (rgb & 255) as f64 / 255.0,
        1.0,
    )
}
pub(super) fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}
pub(super) fn string(text: &str, font: &NSFont, ink: u32) -> Retained<NSAttributedString> {
    let ink = color(ink);
    // SAFETY: 字典键与值为系统约定的字体和颜色类型。
    unsafe {
        let attrs = NSDictionary::from_slices(
            &[NSFontAttributeName, NSForegroundColorAttributeName],
            &[font.as_ref(), ink.as_ref()],
        );
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(text),
            Some(&attrs),
        )
    }
}
/// 字号与 Windows 字体角色相同（96 DPI 像素约等于 AppKit 点）；系统字体遇中文自动回退苹方。
pub(super) fn ui(size: f64) -> Retained<NSFont> {
    NSFont::systemFontOfSize(size)
}
pub(super) fn regular() -> NSFontWeight {
    // SAFETY: AppKit 导出的只读字重常量。
    unsafe { NSFontWeightRegular }
}
/// 标题与分组名的半粗体。
pub(super) fn strong(size: f64) -> Retained<NSFont> {
    // SAFETY: AppKit 导出的只读字重常量。
    NSFont::systemFontOfSize_weight(size, unsafe { NSFontWeightSemibold })
}
/// 编码、色值与键帽的等宽字体：同 Windows 用随包 Cascadia Mono（产品启动时已私有加载），缺失时用系统等宽。
pub(super) fn mono(size: f64) -> Retained<NSFont> {
    NSFont::fontWithName_size(&NSString::from_str("Cascadia Mono"), size)
        .unwrap_or_else(|| NSFont::monospacedSystemFontOfSize_weight(size, regular()))
}
pub(super) fn add(parent: &NSView, child: &NSView) {
    // 两者均在主线程创建，父视图持有子视图，无环。
    parent.addSubview(child);
}
define_class!(
    // SAFETY: 只更改坐标方向，不改变 NSView 的持有规则。
    #[unsafe(super = NSView)]
    #[name = "TigerTypeToolsFlippedView"]
    #[thread_kind = MainThreadOnly]
    pub(super) struct Flipped;
    // SAFETY: 沿用 NSObject 语义。
    unsafe impl NSObjectProtocol for Flipped {}
    impl Flipped { #[unsafe(method(isFlipped))] fn flipped(&self)->bool {true} }
);
impl Flipped {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        // SAFETY: NSView 指定初始化器，frame 已由固定布局生成。
        unsafe { msg_send![Self::alloc(mtm),initWithFrame:frame] }
    }
}

type Navigation = Box<dyn Fn(bool)>;
pub(super) struct ButtonState {
    palette: Cell<Palette>,
    selected: Cell<bool>,
    pressed: Cell<bool>,
    swatch: Cell<Option<u32>>,
    preview: Cell<Option<Palette>>,
    style: Cell<usize>,
    navigation: RefCell<Option<Navigation>>,
    click: Box<dyn Fn()>,
}
define_class!(
    // SAFETY: NSButton 仅主线程使用；回调捕获 panic，闭包只能弱持有界面状态。
    #[unsafe(super = NSButton)]
    #[name = "TigerTypeThemeButton"]
    #[thread_kind = MainThreadOnly]
    #[ivars = ButtonState]
    pub(super) struct Button;
    // SAFETY: 保留 NSObject 身份。
    unsafe impl NSObjectProtocol for Button {}
    impl Button {
        #[unsafe(method(invoke:))]
        fn invoke(&self,_:Option<&AnyObject>){callback(||{if self.isEnabled(){(self.ivars().click)();}});}
        #[unsafe(method(drawRect:))]
        fn draw(&self,_:NSRect) { callback(||self.paint()); }
        #[unsafe(method(acceptsFirstResponder))]
        fn focusable(&self)->bool {true}
        #[unsafe(method(becomeFirstResponder))]
        fn focus(&self)->bool { callback(||NSView::setNeedsDisplay(self,true)); true }
        #[unsafe(method(resignFirstResponder))]
        fn blur(&self)->bool { callback(||NSView::setNeedsDisplay(self,true)); true }
        // 鼠标按下只画按下底色，不夺键盘焦点；焦点环与 Windows 一样只在键盘操作时出现。
        #[unsafe(method(mouseDown:))]
        fn down(&self,_:&NSEvent) { callback(|| {
            if self.isEnabled() { self.ivars().pressed.set(true); NSView::setNeedsDisplay(self,true); }
        }); }
        #[unsafe(method(mouseUp:))]
        fn up(&self,event:&NSEvent) { callback(|| {
            let p=self.convertPoint_fromView(event.locationInWindow(),None); let r=self.bounds();
            let pressed=self.ivars().pressed.replace(false); NSView::setNeedsDisplay(self,true);
            if pressed && self.isEnabled() && p.x>=0.0 && p.y>=0.0 && p.x<r.size.width && p.y<r.size.height { (self.ivars().click)(); }
        }); }
        #[unsafe(method(keyDown:))]
        fn key(&self,event:&NSEvent) { callback(|| {
            let plain=!event.modifierFlags().intersects(NSEventModifierFlags::Command|NSEventModifierFlags::Control|NSEventModifierFlags::Option);
            if plain && matches!(event.keyCode(),123|124) && let Some(navigate)=self.ivars().navigation.borrow().as_ref(){navigate(event.keyCode()==124);}
            else if plain && matches!(event.keyCode(),36|49|76) && self.isEnabled() { (self.ivars().click)(); }
            else {
                // SAFETY: 未消费的 Tab、方向键和快捷键交回 NSButton/响应链。
                unsafe { let _:()=msg_send![super(self),keyDown:event]; }
            }
        }); }
        #[unsafe(method(accessibilityPerformPress))]
        fn press_accessible(&self)->bool { callback(|| {
            if !self.isEnabled() {return false;} (self.ivars().click)(); true
        }).unwrap_or(false) }
    }
);
impl Button {
    pub fn new(
        mtm: MainThreadMarker,
        title: &str,
        frame: NSRect,
        p: Palette,
        swatch: Option<u32>,
        click: impl Fn() + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ButtonState {
            palette: Cell::new(p),
            selected: Cell::new(false),
            pressed: Cell::new(false),
            swatch: Cell::new(swatch),
            preview: Cell::new(None),
            style: Cell::new(0),
            navigation: RefCell::new(None),
            click: Box::new(click),
        });
        // SAFETY: 指定初始化器；不用原生强调色边框，焦点由 paint 绘制。
        let button: Retained<Self> = unsafe { msg_send![super(this),initWithFrame:frame] };
        // SAFETY: 原生键盘/辅助功能触发 performClick 时也走同一动作；target 为自身，不形成强持有环。
        unsafe {
            button.setTarget(Some(&button));
            button.setAction(Some(sel!(invoke:)));
        }
        button.setTitle(&NSString::from_str(title));
        button.setBordered(false);
        button.setFocusRingType(NSFocusRingType::None);
        button.setAccessibilityLabel(Some(&NSString::from_str(title)));
        button
    }
    pub fn update(&self, p: Palette, selected: bool) {
        self.ivars().palette.set(p);
        self.ivars().selected.set(selected);
        self.setState(if selected {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        NSView::setNeedsDisplay(self, true);
    }
    pub fn style(&self, id: usize) {
        self.ivars().style.set(id);
        use tigertype_ui_model::layout::*;
        // 保留系统可访问性中的开关/单选语义，像素仍由 paint 绘制。
        if [AUTO, COMMON, LEARNING].contains(&id) {
            self.setButtonType(NSButtonType::Switch);
        } else if [
            (MODE, 2),
            (FORM, 3),
            (FONT, 2),
            (SIZE_CHOICE, 7),
            (THEME, 5),
            (NAV, 4),
        ]
        .iter()
        .any(|(start, count)| (*start..*start + count).contains(&id))
        {
            self.setButtonType(NSButtonType::Radio);
        }
    }
    pub fn set_swatch(&self, value: Option<u32>) {
        self.ivars().swatch.set(value);
    }
    /// 自定义主题缩略图也必须使用当前合法草稿，而不是初始默认四色。
    pub fn set_preview(&self, palette: Palette) {
        self.ivars().preview.set(Some(palette));
    }
    pub fn navigation(&self, callback: impl Fn(bool) + 'static) {
        *self.ivars().navigation.borrow_mut() = Some(Box::new(callback));
    }
    fn paint(&self) {
        use tigertype_ui_model::layout::*;
        let state = self.ivars();
        let p = state.palette.get();
        let s = shades(p);
        let r = self.bounds();
        let (w, h) = (r.size.width, r.size.height);
        let id = state.style.get();
        let chosen = state.selected.get();
        let enabled = self.isEnabled();
        let ink = |on: u32| if enabled { on } else { p.faint };
        let responder: &NSResponder = self;
        let focused = self
            .window()
            .and_then(|w| w.firstResponder())
            .is_some_and(|r| std::ptr::eq(&*r, responder));
        let title = caption(&self.title().to_string());
        let in_range = |start: usize, count: usize| (start..start + count).contains(&id);
        if [AUTO, COMMON, LEARNING].contains(&id) {
            // 开关行：卡片底色上的标题、说明与右侧开关，不另画行框。
            let description = TOGGLES
                .iter()
                .find(|(key, ..)| *key == id)
                .map_or("", |(.., d)| *d);
            draw_text(
                &string(&title, &ui(14.0), ink(p.text)),
                rect(15.0, 9.0, w - 95.0, 22.0),
                false,
            );
            draw_text(
                &string(&caption(description), &ui(12.0), p.muted),
                rect(15.0, 32.0, w - 95.0, 18.0),
                false,
            );
            let track = rect(w - 55.0, ((h - 22.0) / 2.0).floor(), 40.0, 22.0);
            let (x, y) = (track.origin.x, track.origin.y);
            if chosen {
                fill_round(track, 11.0, ink(p.accent));
                fill_round(rect(x + 22.0, y + 4.0, 14.0, 14.0), 7.0, p.on_accent);
            } else {
                ring(track, 11.0, 2.0, ink(p.field));
                fill_round(rect(x + 5.0, y + 6.0, 10.0, 10.0), 5.0, ink(p.muted));
            }
            if focused {
                ring(inset(track, -3.0), 14.0, 2.0, p.text);
            }
        } else if in_range(NAV, PAGES.len()) {
            // 侧栏分区：图标 + 文字左对齐，选中只是一块浅色底。
            if chosen {
                fill_round(r, 6.0, s.nav);
            }
            const ICONS: [&[&str]; 4] = [
                &["character.cursor.ibeam", "textformat"],
                &["paintpalette"],
                &["keyboard"],
                &["slider.horizontal.3"],
            ];
            symbol(
                ICONS[id - NAV],
                14.0,
                if chosen { p.accent } else { p.muted },
                rect(10.0, 0.0, 20.0, h),
            );
            draw_text(
                &string(&title, &ui(14.0), p.text),
                rect(38.0, 0.0, w - 38.0, h),
                false,
            );
            if focused {
                ring(r, 6.0, 2.0, p.text);
            }
        } else if in_range(THEME, Theme::ALL.len()) {
            // 主题卡：选中为强调色 2 点外框，内嵌该主题的迷你候选窗。
            ring(r, 8.0, 2.0, if chosen { p.accent } else { p.line });
            let sample = state.preview.get().unwrap_or_else(|| {
                Options {
                    theme: Theme::ALL[id - THEME],
                    ..Options::default()
                }
                .palette()
            });
            let mini = rect(8.0, 8.0, w - 16.0, 44.0);
            fill_round(mini, 5.0, sample.window);
            ring(mini, 5.0, 1.0, sample.border);
            let face = art::candidate_font(Options::default(), 12.0);
            for (row, (words, glyph_ink)) in [("1 拉", sample.chosen), ("2 隹", sample.text)]
                .into_iter()
                .enumerate()
            {
                let band = rect(12.0, 12.0 + row as f64 * 19.0, w - 24.0, 17.0);
                if row == 0 && sample.highlight != sample.window {
                    fill_round(band, 3.0, sample.highlight);
                }
                draw_text(
                    &string(words, &face, glyph_ink),
                    rect(
                        band.origin.x + 5.0,
                        band.origin.y,
                        band.size.width - 5.0,
                        17.0,
                    ),
                    false,
                );
            }
            draw_text(
                &string(&title, &ui(14.0), p.text),
                rect(0.0, 54.0, w, h - 57.0),
                true,
            );
            if focused {
                ring(inset(r, 3.0), 6.0, 2.0, p.text);
            }
        } else if in_range(MODE, 2)
            || in_range(FORM, 3)
            || in_range(FONT, 2)
            || in_range(SIZE_CHOICE, 7)
        {
            // 分段单选：轨道由父视图画；只有选中段是卡片色小块加细边，未选中段灰字无边。
            if chosen {
                fill_round(r, 5.0, p.surface);
                ring(r, 5.0, 1.0, p.line);
            }
            let font = if in_range(FONT, 2) {
                art::candidate_font(
                    Options {
                        candidate_font: CandidateFont::ALL[id - FONT],
                        ..Options::default()
                    },
                    15.0,
                )
            } else {
                ui(14.0)
            };
            draw_text(
                &string(&title, &font, ink(if chosen { p.text } else { p.muted })),
                r,
                true,
            );
            if focused {
                ring(r, 5.0, 2.0, p.text);
            }
        } else if in_range(SWATCH, COLORS.len()) {
            // 自定义四色行：左侧圆角色块，右侧名称与说明。
            let chip = rect(0.0, 5.0, 30.0, 30.0);
            if let Some(value) = state.swatch.get() {
                fill_round(chip, 7.0, value);
            }
            ring(chip, 7.0, 1.0, p.field);
            let (name, description) = COLORS[id - SWATCH];
            draw_text(
                &string(&format!("{name}色"), &ui(14.0), p.text),
                rect(42.0, 2.0, w - 42.0, 19.0),
                false,
            );
            draw_text(
                &string(description, &ui(12.0), p.muted),
                rect(42.0, 21.0, w - 42.0, 17.0),
                false,
            );
            if focused {
                ring(inset(chip, -3.0), 9.0, 2.0, p.text);
            }
        } else if id == LIST {
            // 查字字块：选中为浅底加强调色描边；键盘焦点另画正文色环。
            let chip = rect(3.0, 2.0, w - 6.0, h - 4.0);
            if chosen {
                fill_round(chip, 7.0, s.nav);
                ring(chip, 7.0, 2.0, p.accent);
            }
            let font = self.font().unwrap_or_else(|| ui(20.0));
            draw_text(&string(&title, &font, ink(p.text)), chip, true);
            if focused && !chosen {
                ring(chip, 7.0, 2.0, p.text);
            }
        } else if let Some(value) = state.swatch.get() {
            // 色板格：圆角色块；按下或键盘焦点画正文色环，不用系统蓝色高亮。
            let chip = inset(r, 4.0);
            fill_round(chip, 5.0, value);
            ring(chip, 5.0, 1.0, p.line);
            if focused || state.pressed.get() {
                ring(inset(r, 1.0), 7.0, 2.0, p.text);
            }
        } else {
            // 普通按钮：卡片色圆角块加细边；翻页箭头无底无边。
            let flat = matches!(id, PREVIOUS | NEXT);
            if state.pressed.get() {
                fill_round(r, 6.0, s.track);
            } else if !flat {
                fill_round(r, 6.0, p.surface);
            }
            if !flat {
                ring(r, 6.0, 1.0, p.field);
            }
            let icon: Option<&[&str]> = match id {
                SETTINGS => Some(&["gearshape"]),
                PREVIOUS => Some(&["chevron.left"]),
                NEXT => Some(&["chevron.right"]),
                COPY => Some(&["doc.on.doc"]),
                RELOAD => Some(&["arrow.clockwise"]),
                _ => None,
            };
            let label = string(&title, &ui(14.0), ink(p.text));
            match icon {
                // 只有图标的按钮：标题留作读屏名称，不画出来。
                Some(names) if matches!(id, SETTINGS | PREVIOUS | NEXT) => {
                    symbol(names, 14.0, ink(p.text), r);
                }
                Some(names) => {
                    let width = 14.0 + 6.0 + label.size().width;
                    let left = ((w - width) / 2.0).floor();
                    symbol(names, 12.0, ink(p.text), rect(left, 0.0, 14.0, h));
                    draw_text(&label, rect(left + 20.0, 0.0, label.size().width, h), false);
                }
                None => draw_text(&label, r, true),
            }
            if focused {
                ring(r, 6.0, 2.0, p.text);
            }
        }
    }
}

pub(super) struct FieldState {
    palette: Cell<Palette>,
    submit: Box<dyn Fn(String)>,
    change: Box<dyn Fn(String)>,
    /// 取得或失去键盘焦点后通知父窗口重画外框（编辑中外框改强调色）。
    focus: RefCell<Option<Box<dyn Fn()>>>,
}
define_class!(
    // SAFETY: 文本编辑继续交给 NSTextField，delegate 与 target 都是自身且不形成强引用环。
    #[unsafe(super = NSTextField)]
    #[name = "TigerTypeThemeField"]
    #[thread_kind = MainThreadOnly]
    #[ivars = FieldState]
    pub(super) struct Field;
    // SAFETY: 保留原生对象身份。
    unsafe impl NSObjectProtocol for Field {}
    // SAFETY: 所有 delegate 回调在主线程执行且捕获 panic。
    unsafe impl NSControlTextEditingDelegate for Field {
        #[unsafe(method(controlTextDidChange:))]
        fn changed(&self,_:&NSNotification) { callback(|| (self.ivars().change)(self.stringValue().to_string())); }
        #[unsafe(method(controlTextDidBeginEditing:))]
        fn began(&self,_:&NSNotification) { callback(||self.editor_colors()); }
    }
    // SAFETY: NSTextFieldDelegate 沿用 NSControlTextEditingDelegate 的约定。
    unsafe impl NSTextFieldDelegate for Field {}
    impl Field {
        #[unsafe(method(becomeFirstResponder))]
        fn focus(&self)->bool{
            // SAFETY: 先交原生实现装入字段编辑器，再通知外框与选区配色。
            let became:bool=unsafe{msg_send![super(self),becomeFirstResponder]};
            if became { callback(||{self.editor_colors();self.notify();}); }
            became
        }
        #[unsafe(method(textDidEndEditing:))]
        fn ended(&self,notification:&NSNotification){
            // SAFETY: 原生实现先结束字段编辑器会话；随后 currentEditor 为空，外框恢复常态。
            unsafe{let _:()=msg_send![super(self),textDidEndEditing:notification];}
            callback(||self.notify());
        }
        #[unsafe(method(submit:))]
        fn submit(&self,_:Option<&AnyObject>) { callback(|| (self.ivars().submit)(self.stringValue().to_string())); }
    }
);
impl Field {
    pub fn new(
        mtm: MainThreadMarker,
        title: &str,
        frame: NSRect,
        p: Palette,
        submit: impl Fn(String) + 'static,
        change: impl Fn(String) + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(FieldState {
            palette: Cell::new(p),
            submit: Box::new(submit),
            change: Box::new(change),
            focus: RefCell::new(None),
        });
        // SAFETY: 指定初始化器和与 submit: 对应的 selector；控件自己持有闭包。
        let field: Retained<Self> = unsafe {
            let field: Retained<Self> = msg_send![super(this),initWithFrame:frame];
            field.setTarget(Some(&field));
            field.setAction(Some(sel!(submit:)));
            field
        };
        // SAFETY: delegate 为控件自身；原生 delegate 弱持有，生命周期与控件完全一致。
        unsafe {
            field.setDelegate(Some(ProtocolObject::from_ref(&*field)));
        }
        // 外框由父视图按 Windows 画法绘制，原生凹边与焦点环都关掉。
        field.setBezeled(false);
        field.setBordered(false);
        field.setDrawsBackground(true);
        field.setFocusRingType(NSFocusRingType::None);
        field.setFont(Some(&ui(14.0)));
        field.setAccessibilityLabel(Some(&NSString::from_str(title)));
        field.update(p);
        field
    }
    /// 回调只应弱持有界面状态，并在状态未被借用时才重画。
    pub fn on_focus(&self, callback: impl Fn() + 'static) {
        *self.ivars().focus.borrow_mut() = Some(Box::new(callback));
    }
    pub fn editing(&self) -> bool {
        self.currentEditor().is_some()
    }
    fn notify(&self) {
        if let Some(callback) = self.ivars().focus.borrow().as_ref() {
            callback();
        }
    }
    pub fn update(&self, p: Palette) {
        self.ivars().palette.set(p);
        self.setTextColor(Some(&color(p.text)));
        self.setBackgroundColor(Some(&color(p.surface)));
        // 系统占位色随深浅外观变化，不能用于固定主题底色；同 Windows 用次要色。
        if let Some(value) = self
            .placeholderAttributedString()
            .map(|value| value.string())
            .or_else(|| self.placeholderString())
        {
            self.setPlaceholderAttributedString(Some(&string(
                &value.to_string(),
                &self.font().unwrap_or_else(|| ui(14.0)),
                p.muted,
            )));
        }
        self.editor_colors();
    }
    fn editor_colors(&self) {
        NSView::setNeedsDisplay(self, true);
        if let Some(editor) = self
            .currentEditor()
            .and_then(|e| e.downcast::<NSTextView>().ok())
        {
            let p = self.ivars().palette.get();
            let ink = color(p.text);
            let back = color(p.selection);
            // SAFETY: 字段编辑器的选中文字属性只包含 NSColor，不影响 IME 标记文本。
            unsafe {
                let attrs = NSDictionary::from_slices(
                    &[
                        NSForegroundColorAttributeName,
                        NSBackgroundColorAttributeName,
                    ],
                    &[ink.as_ref(), back.as_ref()],
                );
                editor.setSelectedTextAttributes(&attrs);
            }
            editor.setInsertionPointColor(Some(&color(p.accent)));
            editor.setBackgroundColor(&color(p.surface));
        }
    }
}
