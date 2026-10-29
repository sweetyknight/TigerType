//! 候选面板的 AppKit 边界。主线程拥有所有对象；不激活应用、不读取宿主或词表。
use crate::{Action, Location, NativeRect, Presentation, Screen, Selection};
use objc2::rc::Retained;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::*;
use objc2_foundation::{
    NSArray, NSAttributedString, NSDictionary, NSNotification, NSObject,
    NSObjectNSDelayedPerforming, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};
use tigertype_contracts::{CandidateFont, InputState, Options};
use tigertype_frontend::metrics as m;
use tigertype_frontend::{footer, rows};
#[path = "appkit/menu.rs"]
mod menu;
pub use menu::Menu;

/// UI 回调中的 Rust panic 不得穿过 Objective-C 栈；失败时不产生用户动作。
pub(super) fn callback<T>(f: impl FnOnce() -> T) -> Option<T> {
    catch_unwind(AssertUnwindSafe(f)).ok()
}
fn color(rgb: u32) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        ((rgb >> 16) & 255) as f64 / 255.0,
        ((rgb >> 8) & 255) as f64 / 255.0,
        (rgb & 255) as f64 / 255.0,
        1.0,
    )
}
fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}
fn native(r: NSRect) -> NativeRect {
    NativeRect {
        x: r.origin.x,
        y: r.origin.y,
        width: r.size.width,
        height: r.size.height,
    }
}
fn text(value: &str, font: &NSFont, color: &NSColor) -> Retained<NSAttributedString> {
    // SAFETY: 属性键对应 NSFont/NSColor，字典被字符串复制持有。
    unsafe {
        let attrs = NSDictionary::from_slices(
            &[NSFontAttributeName, NSForegroundColorAttributeName],
            &[font.as_ref(), color.as_ref()],
        );
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(value),
            Some(&attrs),
        )
    }
}
fn font(names: &[&str], size: f64) -> Retained<NSFont> {
    names
        .iter()
        .find_map(|name| NSFont::fontWithName_size(&NSString::from_str(name), size))
        .unwrap_or_else(|| NSFont::systemFontOfSize(size))
}
struct Line {
    label: Retained<NSAttributedString>,
    word: Retained<NSAttributedString>,
    comment: Retained<NSAttributedString>,
}
struct Drawing {
    lines: Vec<Line>,
    footer: Option<Retained<NSAttributedString>>,
    width: f64,
    height: f64,
    background: Retained<NSColor>,
    highlight: Retained<NSColor>,
    border: Retained<NSColor>,
    text_x: f64,
    comment_x: f64,
    row_height: f64,
}
impl Drawing {
    fn new(state: &InputState, fonts: &Fonts) -> Self {
        let p = state.options.palette();
        let contrast = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast();
        let (background, foreground, highlight, chosen, border) = if contrast {
            (
                NSColor::windowBackgroundColor(),
                NSColor::labelColor(),
                NSColor::selectedTextBackgroundColor(),
                NSColor::selectedTextColor(),
                NSColor::labelColor(),
            )
        } else {
            (
                color(p.window),
                color(p.text),
                color(p.highlight),
                color(p.highlight_text),
                color(p.border),
            )
        };
        let mut label_width: f64 = 0.0;
        let mut word_width: f64 = 0.0;
        let mut comment_width: f64 = 0.0;
        let lines = rows(state)
            .map(|row| {
                let ink = if row.chosen { &chosen } else { &foreground };
                let label_ink = if contrast {
                    ink.clone()
                } else {
                    color(if row.chosen {
                        p.highlight_text
                    } else {
                        p.label
                    })
                };
                let word_ink = if contrast {
                    ink.clone()
                } else {
                    color(if row.chosen { p.chosen } else { p.text })
                };
                let comment_ink = if contrast {
                    ink.clone()
                } else {
                    color(if row.chosen {
                        p.highlight_text
                    } else {
                        p.muted
                    })
                };
                let label = text(row.label, &fonts.mono, &label_ink);
                let word = text(row.text, &fonts.candidate, &word_ink);
                let comment = text(row.comment, &fonts.comment, &comment_ink);
                label_width = label_width.max(label.size().width);
                word_width = word_width.max(word.size().width);
                comment_width = comment_width.max(comment.size().width);
                Line {
                    label,
                    word,
                    comment,
                }
            })
            .collect::<Vec<_>>();
        let footer = footer(state).map(|f| {
            let label = format!(
                "{}{}",
                if f.pinyin { "拼音反查  " } else { "" },
                f.pager
                    .map(|p| format!(
                        "{}  {}  {}",
                        if p.previous { "‹" } else { "" },
                        p.page,
                        if p.next { "›" } else { "" }
                    ))
                    .unwrap_or_default()
            );
            text(&label, &fonts.mono, &foreground)
        });
        let text_x = (m::PAD + m::ROW_LEFT + m::GAP) as f64 + label_width;
        let comment_x = text_x + word_width + m::GAP as f64;
        let mut width = (if comment_width > 0.0 {
            comment_x + comment_width
        } else {
            text_x + word_width
        }) + (m::ROW_RIGHT + m::PAD) as f64;
        if let Some(f) = &footer {
            width = width.max(f.size().width + 24.0);
        }
        let row_height = (m::ROW as f64)
            .max(fonts.candidate.ascender() - fonts.candidate.descender() + 4.0)
            .ceil();
        let height = (2 * m::PAD) as f64
            + lines.len() as f64 * row_height
            + if footer.is_some() {
                (m::FOOTER + 4) as f64
            } else {
                0.0
            };
        Self {
            lines,
            footer,
            width: width.ceil().clamp(m::MIN_WIDTH as f64, m::MAX_WIDTH as f64),
            height,
            background,
            highlight,
            border,
            text_x,
            comment_x,
            row_height,
        }
    }
}
struct Fonts {
    kind: CandidateFont,
    candidate: Retained<NSFont>,
    mono: Retained<NSFont>,
    comment: Retained<NSFont>,
}
impl Fonts {
    fn new(kind: CandidateFont) -> Self {
        let names: &[&str] = match kind {
            CandidateFont::WenKai => &["LXGW WenKai GB Screen", "PingFang SC"],
            CandidateFont::KaiTi => &["Kaiti SC", "STKaiti", "PingFang SC"],
        };
        Self {
            kind,
            candidate: font(names, m::TEXT_POINT as f64 * 4.0 / 3.0),
            mono: font(
                &["Cascadia Mono", "Menlo"],
                m::SMALL_POINT as f64 * 4.0 / 3.0,
            ),
            comment: font(
                &["TumanPUA", "PingFang SC"],
                m::SMALL_POINT as f64 * 4.0 / 3.0,
            ),
        }
    }
}
struct ViewState {
    presentation: RefCell<Presentation>,
    drawing: RefCell<Option<Drawing>>,
    fonts: RefCell<Fonts>,
    action: Rc<dyn Fn(Action)>,
}
define_class!(
    // SAFETY: NSView 子类仅主线程使用，所有入口捕获 Rust panic，ivars 持有对象而非裸指针。
    #[unsafe(super = NSView)]
    #[name = "TigerTypeCandidateView"]
    #[thread_kind = MainThreadOnly]
    #[ivars = ViewState]
    struct CandidateView;
    // SAFETY: 不改变 NSObject 的身份与生命周期约定。
    unsafe impl NSObjectProtocol for CandidateView {}
    impl CandidateView {
        #[unsafe(method(dismissToast:))]
        fn dismiss_toast(&self,_:Option<&NSObject>){callback(||{if let Some(window)=self.window(){window.orderOut(None);}*self.ivars().drawing.borrow_mut()=None;});}
        #[unsafe(method(displayOptionsChanged:))]
        fn display_options(&self,_:&NSNotification){callback(||{
            if let Some(state)=self.ivars().presentation.borrow().state(){*self.ivars().drawing.borrow_mut()=Some(Drawing::new(state,&self.ivars().fonts.borrow()));}
            self.setNeedsDisplay(true);self.accessibility(false);
        });}
        #[unsafe(method(isFlipped))]
        fn flipped(&self) -> bool { true }
        #[unsafe(method(acceptsFirstMouse:))]
        fn first_mouse(&self, _: Option<&NSEvent>) -> bool { true }
        #[unsafe(method(drawRect:))]
        fn draw(&self, _: NSRect) { callback(|| self.paint()); }
        #[unsafe(method(mouseDown:))]
        fn down(&self, event: &NSEvent) { callback(|| {
            let row = self.hit(event); self.ivars().presentation.borrow_mut().press(row);
        }); }
        #[unsafe(method(mouseUp:))]
        fn up(&self, event: &NSEvent) { callback(|| {
            let selected = self.ivars().presentation.borrow_mut().release(self.hit(event));
            // 先释放 RefCell，再调用组装方；上屏可能同步触发面板更新或失焦。
            if let Some(selected) = selected { (self.ivars().action)(Action::Select(selected)); }
        }); }
    }
);
impl CandidateView {
    fn new(mtm: MainThreadMarker, action: Rc<dyn Fn(Action)>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewState {
            presentation: RefCell::new(Presentation::default()),
            drawing: RefCell::new(None),
            fonts: RefCell::new(Fonts::new(CandidateFont::WenKai)),
            action,
        });
        // SAFETY: NSView 的指定初始化器；调用前已安装全部 Rust ivars。
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
    fn hit(&self, event: &NSEvent) -> Option<usize> {
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        let d = self.ivars().drawing.borrow();
        let d = d.as_ref()?;
        if p.x < m::PAD as f64
            || p.x >= self.bounds().size.width - m::PAD as f64
            || p.y < m::PAD as f64
            || p.y >= m::PAD as f64 + d.lines.len() as f64 * d.row_height
        {
            return None;
        }
        Some(((p.y - m::PAD as f64) / d.row_height) as usize)
    }
    fn paint(&self) {
        let d = self.ivars().drawing.borrow();
        let Some(d) = d.as_ref() else {
            return;
        };
        d.background.setFill();
        let outline =
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.bounds(), 8.0, 8.0);
        outline.fill();
        d.border.setStroke();
        outline.setLineWidth(1.0);
        outline.stroke();
        let model = self.ivars().presentation.borrow();
        for (index, line) in d.lines.iter().enumerate() {
            let y = m::PAD as f64 + index as f64 * d.row_height;
            if model.state().is_some_and(|s| s.highlighted == Some(index)) {
                d.highlight.setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    rect(
                        m::PAD as f64,
                        y,
                        self.bounds().size.width - (2 * m::PAD) as f64,
                        d.row_height,
                    ),
                    5.0,
                    5.0,
                )
                .fill();
            }
            for (line, x) in [
                (&line.label, (m::PAD + m::ROW_LEFT) as f64),
                (&line.word, d.text_x),
                (&line.comment, d.comment_x),
            ] {
                line.drawAtPoint(NSPoint::new(
                    x,
                    y + (d.row_height - line.size().height) / 2.0,
                ));
            }
        }
        if let Some(footer) = &d.footer {
            footer.drawAtPoint(NSPoint::new(
                (m::PAD + 8) as f64,
                (m::PAD + 4) as f64 + d.lines.len() as f64 * d.row_height,
            ));
        }
    }
    fn accessibility(&self, changed: bool) {
        let workspace = NSWorkspace::sharedWorkspace();
        // SAFETY: AppKit 常量和数组元素类型匹配；view/子元素由 Cocoa 保持有效。
        unsafe {
            self.setAccessibilityRole(Some(NSAccessibilityListRole));
            self.setAccessibilityLabel(Some(&NSString::from_str("虎码候选")));
            self.setAccessibilityElement(true);
            if !workspace.isVoiceOverEnabled() {
                self.setAccessibilityChildren(None);
                self.setAccessibilitySelectedChildren(None);
                return;
            }
            let state = self.ivars().presentation.borrow().state().cloned();
            let Some(state) = state else {
                self.setAccessibilityChildren(None);
                self.setAccessibilitySelectedChildren(None);
                return;
            };
            let mut children = Vec::new();
            let row_height = self
                .ivars()
                .drawing
                .borrow()
                .as_ref()
                .map_or(m::ROW as f64, |d| d.row_height);
            for (index, row) in rows(&state).enumerate() {
                let child = NSAccessibilityElement::new();
                child.setAccessibilityRole(Some(NSAccessibilityStaticTextRole));
                child.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                    "{} {} {}",
                    row.label, row.text, row.comment
                ))));
                child.setAccessibilitySelected(row.chosen);
                child.setAccessibilityParent(Some(self.as_ref()));
                child.setAccessibilityFrameInParentSpace(rect(
                    m::PAD as f64,
                    m::PAD as f64 + index as f64 * row_height,
                    self.bounds().size.width - (2 * m::PAD) as f64,
                    row_height,
                ));
                children.push(child);
            }
            let objects: Vec<&objc2::runtime::AnyObject> =
                children.iter().map(|c| (**c).as_ref()).collect();
            let all = NSArray::from_slice(&objects);
            self.setAccessibilityChildren(Some(&all));
            let selected: Vec<_> = state
                .highlighted
                .and_then(|i| objects.get(i).copied())
                .into_iter()
                .collect();
            self.setAccessibilitySelectedChildren(Some(&NSArray::from_slice(&selected)));
            if changed {
                NSAccessibilityPostNotification(
                    self,
                    NSAccessibilitySelectedChildrenChangedNotification,
                );
                if let Some(row) = state.highlighted.and_then(|i| state.candidates.get(i)) {
                    let announcement = NSString::from_str(&format!("{} {}", row.text, row.comment));
                    let info = NSDictionary::from_slices(
                        &[NSAccessibilityAnnouncementKey],
                        &[announcement.as_ref()],
                    );
                    NSAccessibilityPostNotificationWithUserInfo(
                        self,
                        NSAccessibilityAnnouncementRequestedNotification,
                        Some(&info),
                    );
                }
            }
        }
    }
}
define_class!(
    // SAFETY: 面板明确不获取 key/main 状态，所有权沿用 NSPanel。
    #[unsafe(super = NSPanel)]
    #[name = "TigerTypeCandidatePanel"]
    #[thread_kind = MainThreadOnly]
    struct PassivePanel;
    // SAFETY: 不改变 NSObject 协议约定。
    unsafe impl NSObjectProtocol for PassivePanel {}
    impl PassivePanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn key(&self) -> bool {false}
        #[unsafe(method(canBecomeMainWindow))]
        fn main_window(&self) -> bool {false}
    }
);

/// 由 M1 组装方每进程创建一份。只在主线程调用；Select 回调仍须由 Host 核对快照再上屏。
pub struct CandidatePanel {
    window: Retained<PassivePanel>,
    view: Retained<CandidateView>,
}
impl CandidatePanel {
    pub fn new(mtm: MainThreadMarker, action: Rc<dyn Fn(Action)>) -> Self {
        let view = CandidateView::new(mtm, action);
        // SAFETY: 指定初始化器，关闭自动释放，Rust Retained 是唯一释放策略。
        let window: Retained<PassivePanel> = unsafe {
            let window: Retained<PassivePanel> = msg_send![PassivePanel::alloc(mtm), initWithContentRect: NSRect::ZERO,
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered, defer: false];
            window.setReleasedWhenClosed(false);
            window
        };
        window.setContentView(Some(&view));
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(true);
        window.setHidesOnDeactivate(false);
        window.setBecomesKeyOnlyIfNeeded(true);
        window.setLevel(NSPopUpMenuWindowLevel);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        // SAFETY: 通知仅发给本主线程对象；Drop 在释放 view 前配对移除观察者。
        unsafe {
            NSWorkspace::sharedWorkspace()
                .notificationCenter()
                .addObserver_selector_name_object(
                    &view,
                    sel!(displayOptionsChanged:),
                    Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                    None,
                );
        }
        Self { window, view }
    }
    /// composition/caret 由控制器查询；无坐标时使用同控制器历史，绝不访问其他进程界面。
    pub fn show(
        &self,
        owner: u64,
        state: InputState,
        location: &mut Location,
        composition: Option<NativeRect>,
        caret: Option<NativeRect>,
    ) -> Result<bool, &'static str> {
        self.cancel_toast();
        if !state.composing || state.candidates.is_empty() {
            self.hide();
            return Ok(false);
        }
        let updated = self
            .view
            .ivars()
            .presentation
            .borrow_mut()
            .update(owner, state);
        // 先释放可变借用，再执行完整隐藏；非法快照也须清理旧无障碍节点与绘制缓存。
        let changed = match updated {
            Ok(changed) => changed,
            Err(e) => {
                self.hide();
                return Err(e);
            }
        };
        let model = self.view.ivars().presentation.borrow();
        let state = model.state().ok_or("候选已隐藏")?;
        if changed {
            let mut fonts = self.view.ivars().fonts.borrow_mut();
            if fonts.kind != state.options.candidate_font {
                *fonts = Fonts::new(state.options.candidate_font);
            }
            *self.view.ivars().drawing.borrow_mut() = Some(Drawing::new(state, &fonts));
        }
        drop(model);
        let drawing = self.view.ivars().drawing.borrow();
        let d = drawing.as_ref().ok_or("候选绘制缓存不存在")?;
        let screens: Vec<_> = NSScreen::screens(self.view.mtm())
            .iter()
            .map(|s| Screen {
                bounds: native(s.frame()),
                visible: native(s.visibleFrame()),
            })
            .collect();
        // 面板坐标不再当作锚点传回，位置退路由每控制器 Location 保存，避免漂移。
        let placed = location.place(
            composition,
            caret,
            &screens,
            (d.width as i32, d.height as i32),
        );
        drop(drawing);
        let Some(placed) = placed else {
            self.view.ivars().presentation.borrow_mut().press(None);
            self.window.orderOut(None);
            return Ok(false);
        };
        self.window
            .setFrame_display(rect(placed.x, placed.y, placed.width, placed.height), false);
        if changed || !self.window.isVisible() {
            self.view.setNeedsDisplay(true);
        }
        self.view.accessibility(changed);
        self.window.orderFrontRegardless();
        Ok(true)
    }
    pub fn hide(&self) {
        self.cancel_toast();
        self.view.ivars().presentation.borrow_mut().hide();
        *self.view.ivars().drawing.borrow_mut() = None;
        self.window.orderOut(None);
        self.view.accessibility(false);
    }
    pub fn accepts(&self, selection: Selection) -> bool {
        self.window.isVisible() && self.view.ivars().presentation.borrow().accepts(selection)
    }
    fn cancel_toast(&self) {
        // SAFETY: 只取消本 view 的一次性隐藏，不影响其他 UI 或输入定时行为。
        unsafe {
            NSObject::cancelPreviousPerformRequestsWithTarget_selector_object(
                &self.view,
                sel!(dismissToast:),
                None,
            );
        }
    }
    /// 中英切换提示使用同一非激活面板；新快照/失焦立即取消一次性隐藏，空闲没有周期唤醒。
    pub fn status(
        &self,
        ascii: bool,
        options: Options,
        location: &mut Location,
        anchor: Option<NativeRect>,
    ) -> Result<bool, String> {
        self.hide();
        options.validate()?;
        let fonts = self.view.ivars().fonts.borrow();
        let p = options.palette();
        *self.view.ivars().drawing.borrow_mut() = Some(Drawing {
            lines: vec![],
            footer: Some(text(
                if ascii { "英" } else { "中" },
                &fonts.candidate,
                &color(p.text),
            )),
            width: 48.0,
            height: 40.0,
            background: color(p.window),
            highlight: color(p.highlight),
            border: color(p.border),
            text_x: 0.0,
            comment_x: 0.0,
            row_height: 0.0,
        });
        let screens: Vec<_> = NSScreen::screens(self.view.mtm())
            .iter()
            .map(|s| Screen {
                bounds: native(s.frame()),
                visible: native(s.visibleFrame()),
            })
            .collect();
        let Some(r) = location.place(anchor, None, &screens, (48, 40)) else {
            return Ok(false);
        };
        self.window
            .setFrame_display(rect(r.x, r.y, r.width, r.height), false);
        self.view.setNeedsDisplay(true);
        self.window.orderFrontRegardless();
        // SAFETY: selector 对应 dismissToast:；系统持有 target 到触发或取消，无循环定时器。
        unsafe {
            self.view
                .performSelector_withObject_afterDelay(sel!(dismissToast:), None, 0.8);
        }
        Ok(true)
    }
}
impl Drop for CandidatePanel {
    fn drop(&mut self) {
        self.hide();
        // SAFETY: 与 new 的观察者注册配对，释放 view 前先取消所有系统持有。
        unsafe {
            NSWorkspace::sharedWorkspace()
                .notificationCenter()
                .removeObserver(&self.view);
        }
        self.window.close();
    }
}

#[cfg(test)]
pub mod tests {
    //! 本机隐藏窗口验证渲染、非激活属性和原生鼠标路径；不使用真实宿主。
    use super::*;
    pub fn run() {
        let mtm = MainThreadMarker::new().expect("面板检查必须在主线程");
        let _app = NSApplication::sharedApplication(mtm);
        let clicks = Rc::new(RefCell::new(Vec::new()));
        let sink = clicks.clone();
        let panel = CandidatePanel::new(mtm, Rc::new(move |a| sink.borrow_mut().push(a)));
        let mut state = InputState {
            composing: true,
            highlighted: Some(0),
            revision: 9,
            candidates: vec![tigertype_contracts::Candidate {
                text: "𠀀".into(),
                label: "1".into(),
                comment: "〔测试〕".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(!panel.window.canBecomeKeyWindow());
        assert!(!panel.window.canBecomeMainWindow());
        assert!(!panel.window.isVisible());
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "tigertype-candidate-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        for theme in tigertype_contracts::Theme::ALL {
            state.options.theme = theme;
            panel
                .view
                .ivars()
                .presentation
                .borrow_mut()
                .update(1, state.clone())
                .unwrap();
            let d = Drawing::new(&state, &panel.view.ivars().fonts.borrow());
            panel
                .window
                .setFrame_display(rect(0.0, 0.0, d.width, d.height), false);
            *panel.view.ivars().drawing.borrow_mut() = Some(d);
            let bitmap = panel
                .view
                .bitmapImageRepForCachingDisplayInRect(panel.view.bounds())
                .unwrap();
            panel
                .view
                .cacheDisplayInRect_toBitmapImageRep(panel.view.bounds(), &bitmap);
            // SAFETY: 空属性字典使用系统默认 PNG 编码参数。
            let png = unsafe {
                bitmap.representationUsingType_properties(
                    NSBitmapImageFileType::PNG,
                    &NSDictionary::new(),
                )
            }
            .unwrap();
            assert!(png.len() > 100);
            std::fs::write(root.join(format!("candidate-{theme:?}.png")), png.to_vec()).unwrap();
        }
        let event = |kind| {
            // 合成鼠标事件绑定本隐藏窗口，坐标在第一行；不向系统事件队列注入。
            NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(kind,NSPoint::new(20.0,panel.view.bounds().size.height-12.0),NSEventModifierFlags::empty(),0.0,panel.window.windowNumber(),None,1,1,1.0).unwrap()
        };
        panel.view.mouseDown(&event(NSEventType::LeftMouseDown));
        panel.view.mouseUp(&event(NSEventType::LeftMouseUp));
        assert_eq!(clicks.borrow().len(), 1);
        panel.view.mouseDown(&event(NSEventType::LeftMouseDown));
        state.revision += 1;
        panel
            .view
            .ivars()
            .presentation
            .borrow_mut()
            .update(1, state.clone())
            .unwrap();
        panel.view.mouseUp(&event(NSEventType::LeftMouseUp));
        assert_eq!(clicks.borrow().len(), 1);
        let menu = Menu::new(mtm, 1, &state, Some("合成不可用"), Rc::new(|_| {}));
        assert_eq!(menu.native().numberOfItems(), 7);
        let _status = CandidatePanel::status;
        let _accepts = CandidatePanel::accepts;
        assert!(
            !panel
                .show(
                    1,
                    InputState::default(),
                    &mut Location::default(),
                    None,
                    None
                )
                .unwrap()
        );
        assert!(!panel.window.isVisible());
        panel.hide();
        println!(
            "候选面板原生断言通过；离屏图像：{}（仍须人工视觉核对）",
            root.display()
        );
    }
}
