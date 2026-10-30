//! 原生控件保留文本编辑和无障碍；按钮不进入同步跟踪循环，焦点环与配色由主题绘制。
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
pub(super) fn add(parent: &NSView, child: &NSView) {
    // 两者均在主线程创建，父视图持有子视图，无环。
    parent.addSubview(child);
}
pub(super) fn label(
    mtm: MainThreadMarker,
    parent: &NSView,
    value: &str,
    frame: NSRect,
    p: Palette,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(value), mtm);
    label.setFrame(frame);
    label.setTextColor(Some(&color(p.text)));
    label.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    label.setMaximumNumberOfLines(0);
    add(parent, &label);
    label
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
        #[unsafe(method(mouseDown:))]
        fn down(&self,_:&NSEvent) { callback(|| {
            if self.isEnabled() { self.ivars().pressed.set(true); if let Some(w)=self.window() { w.makeFirstResponder(Some(self)); } NSView::setNeedsDisplay(self,true); }
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
        let p = self.ivars().palette.get();
        let chosen = self.ivars().selected.get();
        let r = self.bounds();
        let style = self.ivars().style.get();
        // 开关行沿用 Windows 的标题/说明/右侧开关，而不是另一套大按钮布局。
        if let Some((_, title, description)) = TOGGLES.iter().find(|(id, ..)| *id == style) {
            let title = art::caption(title);
            let description = art::caption(description);
            string(&title, &NSFont::systemFontOfSize(13.0), p.text)
                .drawAtPoint(NSPoint::new(16.0, r.size.height - 28.0));
            string(&description, &NSFont::systemFontOfSize(11.0), p.muted).drawInRect(rect(
                16.0,
                8.0,
                r.size.width - 84.0,
                18.0,
            ));
            color(if chosen { p.accent } else { p.border }).setFill();
            let x = r.size.width - 54.0;
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                rect(x, 20.0, 36.0, 20.0),
                10.0,
                10.0,
            )
            .fill();
            color(if chosen { p.on_accent } else { p.surface }).setFill();
            NSBezierPath::bezierPathWithOvalInRect(rect(
                x + if chosen { 18.0 } else { 2.0 },
                22.0,
                16.0,
                16.0,
            ))
            .fill();
        }
        let outline = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            rect(2.0, 2.0, r.size.width - 4.0, r.size.height - 4.0),
            5.0,
            5.0,
        );
        // 四色编辑行只有左侧小色块使用基色；整行染色会破坏说明文字对比度。
        let swatch = if (SWATCH..SWATCH + 4).contains(&style) {
            None
        } else {
            self.ivars().swatch.get()
        };
        color(swatch.unwrap_or(if chosen || self.ivars().pressed.get() {
            p.highlight
        } else {
            p.surface
        }))
        .setFill();
        if ![AUTO, COMMON, LEARNING].contains(&style) {
            outline.fill();
        }
        let responder: &NSResponder = self;
        let focused = self
            .window()
            .and_then(|w| w.firstResponder())
            .is_some_and(|r| std::ptr::eq(&*r, responder));
        color(if focused { p.accent } else { p.border }).setStroke();
        outline.setLineWidth(if focused { 2.0 } else { 1.0 });
        outline.stroke();
        if (THEME..THEME + Theme::ALL.len()).contains(&style) {
            let sample = self.ivars().preview.get().unwrap_or_else(|| {
                Options {
                    theme: Theme::ALL[style - THEME],
                    ..Options::default()
                }
                .palette()
            });
            color(sample.window).setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                rect(8.0, 28.0, r.size.width - 16.0, 40.0),
                5.0,
                5.0,
            )
            .fill();
            string("虎", &NSFont::systemFontOfSize(18.0), sample.text)
                .drawAtPoint(NSPoint::new(16.0, 38.0));
            color(sample.accent).setFill();
            NSBezierPath::bezierPathWithOvalInRect(rect(r.size.width - 26.0, 38.0, 12.0, 12.0))
                .fill();
            string(
                &self.title().to_string(),
                &NSFont::systemFontOfSize(12.0),
                p.text,
            )
            .drawAtPoint(NSPoint::new(10.0, 7.0));
        } else if (SWATCH..SWATCH + 4).contains(&style) {
            if let Some(value) = self.ivars().swatch.get() {
                color(value).setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    rect(8.0, 8.0, 24.0, 24.0),
                    4.0,
                    4.0,
                )
                .fill();
            }
            let (title, description) = COLORS[style - SWATCH];
            string(title, &NSFont::systemFontOfSize(12.0), p.text)
                .drawAtPoint(NSPoint::new(44.0, 21.0));
            string(description, &NSFont::systemFontOfSize(10.0), p.muted)
                .drawAtPoint(NSPoint::new(44.0, 5.0));
        } else if self.ivars().swatch.get().is_none() && ![AUTO, COMMON, LEARNING].contains(&style)
        {
            let title = string(
                &self.title().to_string(),
                &self
                    .font()
                    .unwrap_or_else(|| NSFont::systemFontOfSize(13.0)),
                if !self.isEnabled() {
                    p.muted
                } else if chosen {
                    p.highlight_text
                } else {
                    p.text
                },
            );
            let size = title.size();
            title.drawAtPoint(NSPoint::new(
                (r.size.width - size.width) / 2.0,
                (r.size.height - size.height) / 2.0,
            ));
        }
    }
}

pub(super) struct FieldState {
    palette: Cell<Palette>,
    submit: Box<dyn Fn(String)>,
    change: Box<dyn Fn(String)>,
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
        fn editing(&self,_:&NSNotification) { callback(||self.editor_colors()); }
        #[unsafe(method(controlTextDidEndEditing:))]
        fn ended(&self,_:&NSNotification){callback(||NSView::setNeedsDisplay(self,true));}
    }
    // SAFETY: NSTextFieldDelegate 沿用 NSControlTextEditingDelegate 的约定。
    unsafe impl NSTextFieldDelegate for Field {}
    impl Field {
        #[unsafe(method(drawRect:))]
        fn draw_field(&self,r:NSRect){callback(||{
            // SAFETY: 先交回原生文本绘制，再覆盖主题色焦点边框，不更改标记文本。
            unsafe{let _:()=msg_send![super(self),drawRect:r];}
            if self.currentEditor().is_some(){color(self.ivars().palette.get().accent).setStroke();let b=self.bounds();let ring=NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect(1.0,1.0,b.size.width-2.0,b.size.height-2.0),3.0,3.0);ring.setLineWidth(1.0);ring.stroke();}
        });}
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
        field.setFocusRingType(NSFocusRingType::None);
        field.setAccessibilityLabel(Some(&NSString::from_str(title)));
        field.update(p);
        field
    }
    pub fn update(&self, p: Palette) {
        self.ivars().palette.set(p);
        self.setTextColor(Some(&color(p.text)));
        self.setBackgroundColor(Some(&color(p.field)));
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
        }
    }
}
