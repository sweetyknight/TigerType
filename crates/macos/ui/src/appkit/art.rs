//! 与 Win32 同一布局上的卡片、键帽和预览；只绘图，不处理业务动作。
//! 形状、颜色与尺寸逐项对照 Windows 的 drawing.rs/controls.rs，字体与图标换成系统字体和 SF Symbols。
use super::*;
use objc2_foundation::{NSArray, NSAttributedString};
use tigertype_ui_model::layout::*;

pub(super) fn box_rect(b: Box4) -> NSRect {
    rect(b[0] as f64, b[1] as f64, b[2] as f64, b[3] as f64)
}
pub(super) fn inset(r: NSRect, by: f64) -> NSRect {
    rect(
        r.origin.x + by,
        r.origin.y + by,
        r.size.width - 2.0 * by,
        r.size.height - 2.0 * by,
    )
}
pub(super) fn caption(value: &str) -> String {
    // Win32 助记键不显示在 macOS；语义与顺序保持一致。
    let mut value = value.to_owned();
    for c in 'A'..='Z' {
        value = value.replace(&format!("(&{c})"), "");
    }
    // ⌃ 在苹方里像脱字符，Mac 键盘上印的是 control，说明文字写全称。
    value.replace("Ctrl+", "Control+")
}
/// 快捷键键帽改用 Mac 键名：笔记本没有翻页键，以 fn+方向键代替；N 按当前每页候选数显示。
pub(super) fn mac_keys(spec: &str, page_size: u8) -> String {
    caption(spec)
        .replace("[Ctrl]", "[Control]")
        .replace("[Enter]", "[Return]")
        .replace("[PgUp]", "[fn ↑]")
        .replace("[PgDn]", "[fn ↓]")
        .replace("[N]", &format!("[{page_size}]"))
}
/// 按方括号切成键帽与说明文字（键名可含空格，如 fn ↑），与 Windows 同一切法。
pub(super) fn key_parts(spec: &str) -> Vec<(bool, &str)> {
    spec.split_inclusive(']')
        .flat_map(|part| match part.find('[') {
            Some(at) => vec![&part[..at], &part[at..]],
            None => vec![part],
        })
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(
            |part| match part.strip_prefix('[').and_then(|k| k.strip_suffix(']')) {
                Some(key) => (true, key),
                None => (false, part),
            },
        )
        .collect()
}
pub(super) fn fill(r: NSRect, ink: u32) {
    color(ink).setFill();
    NSBezierPath::bezierPathWithRect(r).fill();
}
pub(super) fn fill_round(r: NSRect, radius: f64, ink: u32) {
    color(ink).setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, radius, radius).fill();
}
/// 描边画在框内（同 GDI 的 1px 边与 2px 焦点环），不越出控件边界。
pub(super) fn ring(r: NSRect, radius: f64, width: f64, ink: u32) {
    color(ink).setStroke();
    let half = width / 2.0;
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        inset(r, half),
        (radius - half).max(0.0),
        (radius - half).max(0.0),
    );
    path.setLineWidth(width);
    path.stroke();
}
/// 文字左起或水平居中，垂直居中（同 DT_VCENTER）。
pub(super) fn draw_text(text: &NSAttributedString, frame: NSRect, center: bool) {
    let size = text.size();
    let x = if center {
        frame.origin.x + (frame.size.width - size.width) / 2.0
    } else {
        frame.origin.x
    };
    text.drawAtPoint(NSPoint::new(
        x,
        frame.origin.y + (frame.size.height - size.height) / 2.0,
    ));
}
/// SF Symbols 以主题色单色绘制并居中；系统缺少该符号时不画，控件文字仍作读屏名称。
pub(super) fn symbol(names: &[&str], size: f64, ink: u32, frame: NSRect) {
    let Some(image) = names.iter().find_map(|name| {
        NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
    }) else {
        return;
    };
    let tint = NSImageSymbolConfiguration::configurationWithPaletteColors(
        &NSArray::from_retained_slice(&[color(ink)]),
    );
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(size, regular())
        .configurationByApplyingConfiguration(&tint);
    let Some(image) = image.imageWithSymbolConfiguration(&config) else {
        return;
    };
    let s = image.size();
    let target = rect(
        frame.origin.x + (frame.size.width - s.width) / 2.0,
        frame.origin.y + (frame.size.height - s.height) / 2.0,
        s.width,
        s.height,
    );
    // SAFETY: 零源矩形表示整幅图，hints 为空；respectFlipped 让翻转视图里图标不倒置。
    unsafe {
        image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
            target,
            NSRect::ZERO,
            NSCompositingOperation::SourceOver,
            1.0,
            true,
            None,
        );
    }
}
pub(super) fn candidate_font(o: Options, size: f64) -> Retained<NSFont> {
    let faces: &[&str] = match o.candidate_font {
        CandidateFont::WenKai => &["LXGW WenKai GB Screen", "PingFang SC"],
        CandidateFont::KaiTi => &["Kaiti SC", "STKaiti", "PingFang SC"],
    };
    faces
        .iter()
        .find_map(|n| NSFont::fontWithName_size(&NSString::from_str(n), size))
        .unwrap_or_else(|| ui(size))
}
/// 组装层启动时已核对系统文楷与字根，拆分禁止静默采用其他字体。
fn learning_font(size: f64) -> Retained<NSFont> {
    NSFont::fontWithName_size(&NSString::from_str("LXGW WenKai GB Screen"), size)
        .expect("系统文楷不可用")
}
/// 关于区印章：与 Windows 印章同为琥珀底、米白文楷“虎”，40 点以下不画虎纹，不随主题变色。
const SEAL: u32 = 0xB45309;
const CREAM: u32 = 0xFFF8EE;

/// 父视图要画的底层图形；坐标与 Windows paint_window 相同。
#[derive(Default)]
pub(super) struct Scene {
    pub sidebar: bool,
    pub cards: Vec<Box4>,
    pub lines: Vec<Box4>,
    pub tracks: Vec<Box4>,
    /// 输入框外框与是否正在编辑（编辑中边框改强调色）。
    pub frames: Vec<(Box4, bool)>,
    /// 查字查询框：左侧放大镜、右侧 Enter 键帽提示（同 Windows paint_window）。
    pub search: Option<Box4>,
    pub tile: Option<Box4>,
    pub seal: Option<Box4>,
}
pub(super) struct CanvasState {
    palette: Cell<Palette>,
    scene: RefCell<Scene>,
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
    pub fn new(mtm: MainThreadMarker, frame: NSRect, p: Palette) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(CanvasState {
            palette: Cell::new(p),
            scene: RefCell::default(),
        });
        // SAFETY: NSView 指定初始化器，ivars 已初始化。
        unsafe { msg_send![super(this),initWithFrame:frame] }
    }
    pub fn update(&self, p: Palette, scene: Scene) {
        self.ivars().palette.set(p);
        *self.ivars().scene.borrow_mut() = scene;
        self.setNeedsDisplay(true);
    }
    fn paint(&self) {
        let p = self.ivars().palette.get();
        let s = shades(p);
        let scene = self.ivars().scene.borrow();
        fill(self.bounds(), p.window);
        if scene.sidebar {
            fill(
                rect(0.0, 0.0, SIDEBAR as f64, self.bounds().size.height),
                s.sidebar,
            );
        }
        for b in &scene.cards {
            fill_round(box_rect(*b), 8.0, p.surface);
            ring(box_rect(*b), 8.0, 1.0, p.line);
        }
        for b in &scene.lines {
            fill(box_rect(*b), p.line);
        }
        for b in &scene.tracks {
            fill_round(box_rect(*b), 7.0, s.track);
        }
        for (b, editing) in &scene.frames {
            fill_round(box_rect(*b), 6.0, p.surface);
            ring(
                box_rect(*b),
                6.0,
                1.0,
                if *editing { p.accent } else { p.field },
            );
        }
        if let Some(b) = scene.search {
            let field = box_rect(b);
            let (x, y, w, h) = (
                field.origin.x,
                field.origin.y,
                field.size.width,
                field.size.height,
            );
            symbol(
                &["magnifyingglass"],
                14.0,
                p.muted,
                rect(x + 10.0, y, 20.0, h),
            );
            let hint = string("Enter", &mono(11.0), p.muted);
            let width = hint.size().width + 12.0;
            let cap = rect(x + w - 8.0 - width, y + 9.0, width, h - 18.0);
            fill_round(cap, 4.0, s.track);
            draw_text(&hint, cap, true);
        }
        if let Some(b) = scene.tile {
            fill_round(box_rect(b), 8.0, s.track);
            symbol(&["doc.text"], 17.0, p.accent, box_rect(b));
        }
        if let Some(b) = scene.seal {
            let r = box_rect(b);
            fill_round(r, r.size.width * 0.22, SEAL);
            let glyph = string(
                "虎",
                &NSFont::fontWithName_size(
                    &NSString::from_str("LXGW WenKai GB Screen"),
                    r.size.width * 0.72,
                )
                .unwrap_or_else(|| ui(r.size.width * 0.72)),
                CREAM,
            );
            draw_text(&glyph, r, true);
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
    /// value 同时是读屏文字；键帽格传入去掉括号的朗读文本，绘制仍按 KEYMAP。
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
        let (w, h) = (r.size.width, r.size.height);
        let value = s.value.borrow();
        match s.id {
            PREVIEW => preview(o, r),
            GLYPH => {
                fill_round(r, 8.0, p.surface);
                ring(r, 8.0, 1.0, p.line);
                // 虚线十字格，内缩 8 点，与 Windows PS_DOT 相同的点距。
                let (cx, cy) = ((w / 2.0).floor(), (h / 2.0).floor());
                let mut t = 8.0;
                while t < w.max(h) - 8.0 {
                    if t < h - 8.0 {
                        fill(rect(cx, t, 1.0, 1.0), p.line);
                    }
                    if t < w - 8.0 {
                        fill(rect(t, cy, 1.0, 1.0), p.line);
                    }
                    t += 2.0;
                }
                // 词组放不进字形格，改用字块字号。
                let size = if value.chars().count() > 1 {
                    20.0
                } else {
                    76.0
                };
                draw_text(&string(&value, &candidate_font(o, size), p.text), r, true);
            }
            CODES | FULL | PARTS => {
                if value.is_empty() || *value == "缺资料" {
                    draw_text(&string(&value, &ui(12.0), p.muted), r, false);
                    return;
                }
                let mut x = 0.0;
                if s.id == PARTS {
                    // 拆分字根逐个放进 30 点字块；放不下即止，同 Windows。
                    let font = learning_font(20.0);
                    for part in value.chars() {
                        let tile = rect(x, 0.0, 30.0, 30.0);
                        if x + 30.0 > w {
                            break;
                        }
                        ring(tile, 6.0, 1.0, p.line);
                        draw_text(&string(&part.to_string(), &font, p.text), tile, true);
                        x += 36.0;
                    }
                    return;
                }
                // 编码以空格分隔，各成一枚浅底键帽。
                let font = mono(13.0);
                for code in value.split(' ').filter(|c| !c.is_empty()) {
                    let line = string(code, &font, p.text);
                    let chip = rect(x, (h - 26.0) / 2.0, line.size().width + 18.0, 26.0);
                    if chip.origin.x + chip.size.width > w {
                        break;
                    }
                    fill_round(chip, 5.0, shades(p).track);
                    draw_text(&line, chip, true);
                    x += chip.size.width + 6.0;
                }
            }
            id if (KEYS..KEYS + KEYMAP.len()).contains(&id) => {
                let (name, keys) = KEYMAP[id - KEYS];
                draw_text(
                    &string(name, &ui(14.0), p.text),
                    rect(15.0, 8.0, w - 23.0, 20.0),
                    false,
                );
                let spec = mac_keys(keys, o.page_size);
                let mut x = 15.0;
                for (key, part) in key_parts(&spec) {
                    if key {
                        let line = string(part, &mono(13.0), p.text);
                        let cap = rect(x, 32.0, (line.size().width + 12.0).max(22.0), 22.0);
                        fill_round(cap, 5.0, p.window);
                        ring(cap, 5.0, 1.0, p.field);
                        draw_text(&line, cap, true);
                        x += cap.size.width + 5.0;
                    } else {
                        let line = string(part, &ui(12.0), p.muted);
                        let width = line.size().width;
                        draw_text(&line, rect(x, 32.0, width, 22.0), false);
                        x += width + 5.0;
                    }
                }
            }
            _ => draw_text(&string(&value, &ui(14.0), p.text), r, false),
        }
    }
}
/// 外观区预览：按草稿现画一段带组合的文字和候选窗，尺寸与 Windows draw_preview 相同。
fn preview(o: Options, r: NSRect) {
    let p = o.palette();
    let s = shades(p);
    fill_round(r, 8.0, s.sidebar);
    ring(r, 8.0, 1.0, p.line);
    let body = ui(15.0);
    let line = rect(20.0, 14.0, r.size.width - 40.0, 22.0);
    let lead = string("明天我们一起去", &body, p.text);
    draw_text(&lead, line, false);
    let start = line.origin.x + lead.size().width;
    let code = string("ui", &body, p.text);
    draw_text(&code, rect(start, line.origin.y, 40.0, 22.0), false);
    let end = start + code.size().width;
    // 组合下的点状下划线与插入符。
    let bottom = line.origin.y + line.size.height;
    let mut x = start;
    while x < end {
        fill(rect(x, bottom - 2.0, 1.0, 1.0), p.text);
        x += 3.0;
    }
    fill(
        rect(end + 1.0, line.origin.y + 2.0, 1.0, line.size.height - 4.0),
        p.text,
    );
    let rows = [
        ("1", "拉", "〔扌立 · uil〕"),
        ("2", "隹", "〔隹 · ui / uii〕"),
    ];
    let label_font = mono(12.0);
    let glyph_font = candidate_font(o, 19.0);
    let hint_font = learning_font(12.0);
    let label_w = string("1", &label_font, p.label).size().width;
    let text_w = string("隹", &glyph_font, p.text).size().width;
    let comment_w = if o.show_learning {
        rows.iter()
            .map(|(.., hint)| string(hint, &hint_font, p.text).size().width)
            .fold(0.0_f64, f64::max)
            + 8.0
    } else {
        0.0
    };
    let width = (12.0 + label_w + 8.0 + text_w + comment_w + 14.0)
        .max(132.0)
        .min(r.size.width - 12.0 - start);
    let window = rect(start, bottom + 6.0, width, 69.0);
    fill_round(window, 8.0, p.window);
    ring(window, 8.0, 1.0, p.border);
    for (i, (label, glyph, hint)) in rows.into_iter().enumerate() {
        let band = rect(
            window.origin.x + 4.0,
            window.origin.y + 4.0 + i as f64 * 31.0,
            width - 8.0,
            30.0,
        );
        let chosen = i == 0;
        if chosen && p.highlight != p.window {
            fill_round(band, 4.0, p.highlight);
        }
        let (label_ink, glyph_ink, hint_ink) = if chosen {
            (p.highlight_text, p.chosen, p.highlight_text)
        } else {
            (p.label, p.text, p.muted)
        };
        let mut x = band.origin.x + 8.0;
        for (font, ink, words) in [
            (&label_font, label_ink, label),
            (&glyph_font, glyph_ink, glyph),
            (
                &hint_font,
                hint_ink,
                if o.show_learning { hint } else { "" },
            ),
        ] {
            let text = string(words, font, ink);
            draw_text(
                &text,
                rect(x, band.origin.y, text.size().width, 30.0),
                false,
            );
            x += text.size().width + 8.0;
        }
    }
}

#[cfg(test)]
mod tests {
    // 自定义 harness 的原生测试也编译本文件但不生成 #[test] 函数，导入在那里未使用。
    #[allow(unused_imports)]
    use super::{caption, key_parts, mac_keys};
    #[test]
    fn mac_key_names_replace_windows_only_keys_and_keep_spaced_keycaps_whole() {
        assert_eq!(
            key_parts(&mac_keys("[-] [=] [PgUp] [PgDn]", 5)),
            [(true, "-"), (true, "="), (true, "fn ↑"), (true, "fn ↓")]
        );
        assert_eq!(
            key_parts(&mac_keys("[Ctrl] + [.]", 5)),
            [(true, "Control"), (false, "+"), (true, ".")]
        );
        assert_eq!(
            key_parts(&mac_keys("[1] – [N]", 7)),
            [(true, "1"), (false, "–"), (true, "7")]
        );
        assert_eq!(
            key_parts(&mac_keys("[Shift] 单击", 5)),
            [(true, "Shift"), (false, "单击")]
        );
        assert_eq!(mac_keys("[Enter]", 5), "[Return]");
        assert_eq!(caption("仅常用字(&C)"), "仅常用字");
        assert_eq!(caption("按 Ctrl+L 切换"), "按 Control+L 切换");
    }
}
