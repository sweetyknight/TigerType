//! 与 Win32 同一布局上的卡片、键帽和预览；只绘图，不处理业务动作。
use super::*;
use crate::layout::*;

pub(super) fn box_rect(b: Box4) -> NSRect {
    rect(b[0] as f64, b[1] as f64, b[2] as f64, b[3] as f64)
}
pub(super) fn caption(value: &str) -> String {
    // Win32 助记键不显示在 macOS；语义与顺序保持一致。
    let mut value = value.to_owned();
    for c in 'A'..='Z' {
        value = value.replace(&format!("(&{c})"), "");
    }
    value
        .replace("Ctrl+", "⌃")
        .replace("任务栏图标菜单", "输入法菜单")
}
fn round(r: NSRect, back: u32, border: u32, radius: f64) {
    color(back).setFill();
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, radius, radius);
    path.fill();
    color(border).setStroke();
    path.setLineWidth(1.0);
    path.stroke();
}
fn words(value: &str, size: f64, ink: u32, x: f64, y: f64) {
    string(value, &NSFont::systemFontOfSize(size), ink).drawAtPoint(NSPoint::new(x, y));
}
pub(super) fn candidate_font(o: Options, size: f64) -> Retained<NSFont> {
    let faces: &[&str] = match o.candidate_font {
        CandidateFont::WenKai => &["LXGW WenKai GB Screen", "PingFang SC"],
        CandidateFont::KaiTi => &["Kaiti SC", "STKaiti", "PingFang SC"],
    };
    faces
        .iter()
        .find_map(|n| NSFont::fontWithName_size(&NSString::from_str(n), size))
        .unwrap_or_else(|| NSFont::systemFontOfSize(size))
}
pub(super) struct CanvasState {
    palette: Cell<Palette>,
    cards: RefCell<Vec<Box4>>,
    lines: RefCell<Vec<Box4>>,
    tracks: RefCell<Vec<Box4>>,
    frames: RefCell<Vec<Box4>>,
    seal: Cell<Option<Box4>>,
    sidebar: bool,
}
define_class!(
    // SAFETY: 绘制数据是本线程拥有的值；不在绘制期间调用业务层。
    #[unsafe(super=NSView)]
    #[name="TigerTypeToolsCanvas"]
    #[thread_kind=MainThreadOnly]
    #[ivars=CanvasState]
    pub(super) struct Canvas;
    // SAFETY: 沿用 NSObject 语义。
    unsafe impl NSObjectProtocol for Canvas {}
    impl Canvas {
        #[unsafe(method(isFlipped))] fn flipped(&self)->bool{true}
        #[unsafe(method(drawRect:))] fn draw(&self,_:NSRect){callback(||self.paint());}
    }
);
impl Canvas {
    pub fn new(mtm: MainThreadMarker, frame: NSRect, p: Palette, sidebar: bool) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(CanvasState {
            palette: Cell::new(p),
            cards: RefCell::default(),
            lines: RefCell::default(),
            tracks: RefCell::default(),
            frames: RefCell::default(),
            seal: Cell::new(None),
            sidebar,
        });
        // SAFETY: NSView 指定初始化器，ivars 已初始化。
        unsafe { msg_send![super(this),initWithFrame:frame] }
    }
    pub fn update(
        &self,
        p: Palette,
        cards: Vec<Box4>,
        lines: Vec<Box4>,
        tracks: Vec<Box4>,
        frames: Vec<Box4>,
        seal: Option<Box4>,
    ) {
        self.ivars().palette.set(p);
        *self.ivars().cards.borrow_mut() = cards;
        *self.ivars().lines.borrow_mut() = lines;
        *self.ivars().tracks.borrow_mut() = tracks;
        *self.ivars().frames.borrow_mut() = frames;
        self.ivars().seal.set(seal);
        self.setNeedsDisplay(true);
    }
    fn paint(&self) {
        let s = self.ivars();
        let p = s.palette.get();
        color(p.window).setFill();
        NSBezierPath::bezierPathWithRect(self.bounds()).fill();
        if s.sidebar {
            color(p.surface).setFill();
            NSBezierPath::bezierPathWithRect(rect(
                0.0,
                0.0,
                SIDEBAR as f64,
                self.bounds().size.height,
            ))
            .fill();
        }
        for b in s.cards.borrow().iter() {
            round(box_rect(*b), p.surface, p.line, 8.0);
        }
        for b in s.tracks.borrow().iter() {
            round(box_rect(*b), p.field, p.field, 6.0);
        }
        for b in s.frames.borrow().iter() {
            round(box_rect(*b), p.field, p.border, 6.0);
        }
        for b in s.lines.borrow().iter() {
            color(p.line).setFill();
            NSBezierPath::bezierPathWithRect(box_rect(*b)).fill();
        }
        if let Some(b) = s.seal.get() {
            let r = box_rect(b);
            round(r, p.accent, p.accent, 6.0);
            let f = NSFont::fontWithName_size(&NSString::from_str("Kaiti SC"), 28.0)
                .unwrap_or_else(|| NSFont::systemFontOfSize(28.0));
            string("虎", &f, p.on_accent)
                .drawAtPoint(NSPoint::new(r.origin.x + 5.0, r.origin.y + 3.0));
        }
    }
}
pub(super) struct ArtState {
    id: usize,
    options: Cell<Options>,
    value: RefCell<String>,
}
define_class!(
    // SAFETY: 静态展示的 NSView；每次更新提供无所有权外借的文本与设置。
    #[unsafe(super=NSView)]
    #[name="TigerTypeToolsArtwork"]
    #[thread_kind=MainThreadOnly]
    #[ivars=ArtState]
    pub(super) struct Art;
    // SAFETY: 沿用 NSObject 语义。
    unsafe impl NSObjectProtocol for Art {}
    impl Art{
        #[unsafe(method(isFlipped))] fn flipped(&self)->bool{true}
        #[unsafe(method(drawRect:))] fn draw(&self,_:NSRect){callback(||self.paint());}
    }
);
impl Art {
    pub fn new(
        mtm: MainThreadMarker,
        id: usize,
        frame: NSRect,
        options: Options,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ArtState {
            id,
            options: Cell::new(options),
            value: RefCell::default(),
        });
        // SAFETY: NSView 指定初始化器。
        unsafe { msg_send![super(this),initWithFrame:frame] }
    }
    pub fn update(&self, options: Options, value: String) {
        self.ivars().options.set(options);
        self.setAccessibilityLabel(Some(&NSString::from_str(&value)));
        self.setAccessibilityElement(true);
        // SAFETY: 系统静态文字 role 常量，确保自绘资料能被读屏器访问。
        unsafe {
            self.setAccessibilityRole(Some(NSAccessibilityStaticTextRole));
        }
        *self.ivars().value.borrow_mut() = value;
        self.setNeedsDisplay(true);
    }
    fn paint(&self) {
        let s = self.ivars();
        let o = s.options.get();
        let p = o.palette();
        let r = self.bounds();
        let value = s.value.borrow();
        match s.id {
            PREVIEW => {
                round(r, p.surface, p.line, 8.0);
                let lead = string("明天我们一起去", &NSFont::systemFontOfSize(15.0), p.text);
                lead.drawAtPoint(NSPoint::new(20.0, 14.0));
                let x = 20.0 + lead.size().width;
                words("ui", 15.0, p.text, x, 14.0);
                color(p.text).setFill();
                for n in 0..7 {
                    NSBezierPath::bezierPathWithRect(rect(x + n as f64 * 3.0, 34.0, 1.0, 1.0))
                        .fill();
                }
                NSBezierPath::bezierPathWithRect(rect(x + 19.0, 16.0, 1.0, 18.0)).fill();
                let width = if o.show_learning { 282.0 } else { 132.0 };
                round(rect(x, 42.0, width, 69.0), p.window, p.border, 8.0);
                for (i, (label, value, hint)) in [
                    ("1", "拉", "〔扌立 · uil〕"),
                    ("2", "隹", "〔隹 · ui / uii〕"),
                ]
                .into_iter()
                .enumerate()
                {
                    let y = 46.0 + i as f64 * 31.0;
                    if i == 0 {
                        round(
                            rect(x + 4.0, y, width - 8.0, 30.0),
                            p.highlight,
                            p.highlight,
                            4.0,
                        );
                    }
                    words(
                        label,
                        12.0,
                        if i == 0 { p.highlight_text } else { p.label },
                        x + 12.0,
                        y + 7.0,
                    );
                    let glyph = string(
                        value,
                        &candidate_font(o, 19.0),
                        if i == 0 { p.chosen } else { p.text },
                    );
                    glyph.drawAtPoint(NSPoint::new(x + 30.0, y + 3.0));
                    if o.show_learning {
                        words(
                            hint,
                            12.0,
                            if i == 0 { p.highlight_text } else { p.muted },
                            x + 58.0,
                            y + 7.0,
                        );
                    }
                }
            }
            GLYPH => {
                round(r, p.field, p.line, 8.0);
                let font = candidate_font(
                    o,
                    if value.chars().count() > 1 {
                        28.0
                    } else {
                        64.0
                    },
                );
                let glyph = string(&value, &font, p.text);
                let size = glyph.size();
                glyph.drawAtPoint(NSPoint::new(
                    (r.size.width - size.width) / 2.0,
                    (r.size.height - size.height) / 2.0,
                ));
            }
            CODES | FULL | PARTS => {
                let mut x = 0.0;
                let mut y = 2.0;
                let font = if s.id == PARTS {
                    NSFont::fontWithName_size(&NSString::from_str("TumanPUA"), 18.0)
                        .unwrap_or_else(|| candidate_font(o, 18.0))
                } else {
                    NSFont::fontWithName_size(&NSString::from_str("Cascadia Mono"), 13.0)
                        .unwrap_or_else(|| NSFont::systemFontOfSize(13.0))
                };
                let chunks: Vec<String> = if s.id == PARTS && *value != "缺资料" {
                    value.chars().map(|c| c.to_string()).collect()
                } else {
                    value.split_whitespace().map(str::to_owned).collect()
                };
                for part in chunks {
                    let line = string(&part, &font, p.text);
                    let w = line.size().width + 12.0;
                    if x + w > r.size.width && x > 0.0 {
                        x = 0.0;
                        y += 28.0;
                    }
                    round(rect(x, y, w, 24.0), p.field, p.line, 4.0);
                    line.drawAtPoint(NSPoint::new(x + 6.0, y + 3.0));
                    x += w + 5.0;
                }
            }
            id if (KEYS..KEYS + KEYMAP.len()).contains(&id) => {
                let (name, keys) = KEYMAP[id - KEYS];
                words(name, 12.0, p.label, 14.0, 7.0);
                let keys = caption(keys)
                    .replace("[Ctrl]", "[⌃]")
                    .replace("[N]", &format!("[{}]", o.page_size));
                let mut x = 14.0;
                for part in keys.split_whitespace() {
                    let word = part.trim_matches(['[', ']']);
                    let line = string(word, &NSFont::systemFontOfSize(12.0), p.text);
                    let width = line.size().width + 10.0;
                    if part.starts_with('[') {
                        round(rect(x, 29.0, width, 20.0), p.field, p.line, 3.0);
                    }
                    line.drawAtPoint(NSPoint::new(x + 5.0, 31.0));
                    x += width + 4.0;
                }
            }
            _ => words(&value, 13.0, p.text, 0.0, 2.0),
        }
    }
}
