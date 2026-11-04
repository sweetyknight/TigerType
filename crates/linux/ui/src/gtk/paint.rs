//! 与 Win32/AppKit 同一布局上的卡片、键帽、预览与按钮外观；只绘图，不处理业务动作。
//! 形状、颜色与尺寸逐项对照 Mac 的 art.rs/controls.rs，文字走 Pango，图标取系统图标主题的单色符号。
use super::*;
use gtk::{graphene, gsk, pango, subclass::prelude::*};
use tigertype_contracts::{CandidateFont, Palette, Theme};
use tigertype_ui_model::{
    layout::*,
    view::{key_parts, keys},
};

mod imp {
    use super::*;
    type Draw = Box<dyn Fn(&super::Paint, &gtk::Snapshot)>;
    #[derive(Default)]
    pub struct Paint {
        pub draw: RefCell<Option<Draw>>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for Paint {
        const NAME: &'static str = "TigerTypePaint";
        type Type = super::Paint;
        type ParentType = gtk::Widget;
    }
    impl ObjectImpl for Paint {}
    impl WidgetImpl for Paint {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let paint = self.obj();
            if let Some(draw) = self.draw.borrow().as_ref() {
                callback(|| draw(&paint, snapshot));
            }
        }
    }
}
glib::wrapper! {
    /// 自绘画布：快照回调读调用方持有的数据；尺寸全由共用布局的框给出，本身不参与测量。
    pub struct Paint(ObjectSubclass<imp::Paint>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
impl Paint {
    pub fn new(role: gtk::AccessibleRole, draw: impl Fn(&Paint, &gtk::Snapshot) + 'static) -> Self {
        let paint: Self = glib::Object::builder()
            .property("accessible-role", role)
            .build();
        *paint.imp().draw.borrow_mut() = Some(Box::new(draw));
        paint
    }
}

pub(super) fn rgba(rgb: u32) -> gdk::RGBA {
    let channel = |shift: u32| ((rgb >> shift) & 255) as f32 / 255.0;
    gdk::RGBA::new(channel(16), channel(8), channel(0), 1.0)
}
pub(super) fn rect(x: f32, y: f32, w: f32, h: f32) -> graphene::Rect {
    graphene::Rect::new(x, y, w, h)
}
pub(super) fn box_rect(b: Box4) -> graphene::Rect {
    rect(b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32)
}
fn inset(r: &graphene::Rect, by: f32) -> graphene::Rect {
    rect(
        r.x() + by,
        r.y() + by,
        r.width() - 2.0 * by,
        r.height() - 2.0 * by,
    )
}
pub(super) fn fill(s: &gtk::Snapshot, r: &graphene::Rect, ink: u32) {
    s.append_color(&rgba(ink), r);
}
pub(super) fn fill_round(s: &gtk::Snapshot, r: &graphene::Rect, radius: f32, ink: u32) {
    s.push_rounded_clip(&gsk::RoundedRect::from_rect(*r, radius));
    s.append_color(&rgba(ink), r);
    s.pop();
}
/// 描边画在框内（同 GDI 的 1px 边与 2px 焦点环），不越出控件边界。
pub(super) fn ring(s: &gtk::Snapshot, r: &graphene::Rect, radius: f32, width: f32, ink: u32) {
    s.append_border(
        &gsk::RoundedRect::from_rect(*r, radius),
        &[width; 4],
        &[rgba(ink); 4],
    );
}

/// 字号与 Windows 字体角色相同（96 DPI 像素即 GTK 逻辑像素）；界面字体取系统默认，中文按语言回退。
pub(super) fn ui(widget: &impl IsA<gtk::Widget>, size: f64) -> pango::FontDescription {
    let mut font = widget
        .pango_context()
        .font_description()
        .unwrap_or_default();
    font.set_absolute_size(size * pango::SCALE as f64);
    font.set_weight(pango::Weight::Normal);
    font
}
fn family(names: &str, size: f64) -> pango::FontDescription {
    let mut font = pango::FontDescription::new();
    font.set_family(names);
    font.set_absolute_size(size * pango::SCALE as f64);
    font
}
/// 编码、色值与键帽的等宽字体：同 Windows 用随包 Cascadia Mono，缺失时用系统等宽。
pub(super) fn mono(size: f64) -> pango::FontDescription {
    family("Cascadia Mono,monospace", size)
}
/// 候选字与查字字形：楷体在 Linux 常见为文鼎楷体；都没有时由 fontconfig 回退系统字体。
pub(super) fn candidate_font(o: Options, size: f64) -> pango::FontDescription {
    family(
        match o.candidate_font {
            CandidateFont::WenKai => "LXGW WenKai GB Screen",
            CandidateFont::KaiTi => "KaiTi,AR PL UKai CN,AR PL KaitiM GB",
        },
        size,
    )
}
/// 拆分与注释只用文楷：字根是私用区字形，替代字体只会显示方块。
fn learning_font(size: f64) -> pango::FontDescription {
    family("LXGW WenKai GB Screen", size)
}
pub(super) fn layout(
    widget: &impl IsA<gtk::Widget>,
    value: &str,
    font: &pango::FontDescription,
) -> pango::Layout {
    let layout = widget.create_pango_layout(Some(value));
    layout.set_font_description(Some(font));
    layout
}
/// 读音行宽：查字布局与绘制同用一种字体，折行处一致。
pub(super) fn text_width(widget: &impl IsA<gtk::Widget>, value: &str, size: f64) -> i32 {
    layout(widget, value, &ui(widget, size)).pixel_size().0
}
/// 文字左起或水平居中，垂直居中（同 DT_VCENTER）。返回所画文字宽。
fn draw_text(
    s: &gtk::Snapshot,
    text: &pango::Layout,
    frame: &graphene::Rect,
    center: bool,
    ink: u32,
) -> f32 {
    let (w, h) = text.pixel_size();
    let (w, h) = (w as f32, h as f32);
    let x = if center {
        frame.x() + (frame.width() - w) / 2.0
    } else {
        frame.x()
    };
    s.save();
    s.translate(&graphene::Point::new(
        x.round(),
        (frame.y() + (frame.height() - h) / 2.0).round(),
    ));
    s.append_layout(text, &rgba(ink));
    s.restore();
    w
}
/// 主题图标的单色符号，按给定色绘制并居中；主题缺少全部候选名时不画，控件名称仍作读屏文字。
fn symbol(
    widget: &impl IsA<gtk::Widget>,
    s: &gtk::Snapshot,
    names: &[&str],
    size: i32,
    ink: u32,
    frame: &graphene::Rect,
) {
    let theme = gtk::IconTheme::for_display(&widget.display());
    let Some(name) = names.iter().find(|name| theme.has_icon(name)) else {
        return;
    };
    let icon = theme.lookup_icon(
        name,
        &[],
        size,
        widget.scale_factor(),
        gtk::TextDirection::None,
        gtk::IconLookupFlags::FORCE_SYMBOLIC,
    );
    s.save();
    s.translate(&graphene::Point::new(
        (frame.x() + (frame.width() - size as f32) / 2.0).round(),
        (frame.y() + (frame.height() - size as f32) / 2.0).round(),
    ));
    icon.snapshot_symbolic(s, size as f64, size as f64, &[rgba(ink)]);
    s.restore();
}
fn bounds(widget: &impl IsA<gtk::Widget>) -> graphene::Rect {
    rect(0.0, 0.0, widget.width() as f32, widget.height() as f32)
}

/// 关于区印章：与 Windows 印章同为琥珀底、米白文楷“虎”，不随主题变色。
const SEAL: u32 = 0xB45309;
const CREAM: u32 = 0xFFF8EE;

/// 父控件要画的底层图形；坐标与 Windows paint_window 相同。
#[derive(Default)]
pub(super) struct Scene {
    pub sidebar: bool,
    pub cards: Vec<Box4>,
    pub lines: Vec<Box4>,
    pub tracks: Vec<Box4>,
    /// 输入框外框与是否正在编辑（编辑中边框改强调色）。
    pub frames: Vec<(Box4, bool)>,
    /// 查字查询框：左侧放大镜；边输边查，不画 Enter 提示。
    pub search: Option<Box4>,
    pub tile: Option<Box4>,
    pub seal: Option<Box4>,
}
/// 窗口底、卡片、分隔线等底层图形；放在同一固定容器最底层，控件叠在其上。
pub(super) struct Canvas {
    pub paint: Paint,
    data: Rc<RefCell<(Palette, Scene)>>,
}
impl Canvas {
    pub fn new(p: Palette) -> Self {
        let data = Rc::new(RefCell::new((p, Scene::default())));
        let source = data.clone();
        let paint = Paint::new(gtk::AccessibleRole::Presentation, move |w, s| {
            paint_scene(w, s, &source.borrow());
        });
        Self { paint, data }
    }
    pub fn update(&self, p: Palette, scene: Scene) {
        *self.data.borrow_mut() = (p, scene);
        self.paint.queue_draw();
    }
}
fn paint_scene(w: &Paint, s: &gtk::Snapshot, (p, scene): &(Palette, Scene)) {
    let p = *p;
    let shade = shades(p);
    let all = bounds(w);
    fill(s, &all, p.window);
    if scene.sidebar {
        fill(
            s,
            &rect(0.0, 0.0, SIDEBAR as f32, all.height()),
            shade.sidebar,
        );
    }
    for b in &scene.cards {
        fill_round(s, &box_rect(*b), 8.0, p.surface);
        ring(s, &box_rect(*b), 8.0, 1.0, p.line);
    }
    for b in &scene.lines {
        fill(s, &box_rect(*b), p.line);
    }
    for b in &scene.tracks {
        fill_round(s, &box_rect(*b), 7.0, shade.track);
    }
    for (b, editing) in &scene.frames {
        fill_round(s, &box_rect(*b), 6.0, p.surface);
        ring(
            s,
            &box_rect(*b),
            6.0,
            1.0,
            if *editing { p.accent } else { p.field },
        );
    }
    if let Some(b) = scene.search {
        let field = box_rect(b);
        let glass = rect(field.x() + 10.0, field.y(), 20.0, field.height());
        symbol(
            w,
            s,
            &["system-search-symbolic", "edit-find-symbolic"],
            16,
            p.muted,
            &glass,
        );
    }
    if let Some(b) = scene.tile {
        fill_round(s, &box_rect(b), 8.0, shade.track);
        symbol(
            w,
            s,
            &["text-x-generic-symbolic", "x-office-document-symbolic"],
            18,
            p.accent,
            &box_rect(b),
        );
    }
    if let Some(b) = scene.seal {
        let r = box_rect(b);
        fill_round(s, &r, r.width() * 0.22, SEAL);
        let glyph = layout(w, "虎", &learning_font((r.width() * 0.72) as f64));
        draw_text(s, &glyph, &r, true, CREAM);
    }
}

/// 静态自绘资料：字形格、编码键帽、读音、拆分、快捷键格与外观预览。
pub(super) struct Art {
    pub paint: Paint,
    data: Rc<RefCell<(Options, String)>>,
}
impl Art {
    pub fn new(id: usize, options: Options) -> Self {
        let data = Rc::new(RefCell::new((options, String::new())));
        let source = data.clone();
        let paint = Paint::new(gtk::AccessibleRole::Label, move |w, s| {
            let (o, value) = &*source.borrow();
            paint_art(w, s, id, *o, value);
        });
        Self { paint, data }
    }
    /// value 同时是读屏文字；键帽格传入去掉括号的朗读文本，绘制仍按 KEYMAP。
    pub fn update(&self, options: Options, value: String) {
        if self.data.borrow().1 != value {
            self.paint
                .update_property(&[gtk::accessible::Property::Label(&value)]);
        }
        *self.data.borrow_mut() = (options, value);
        self.paint.queue_draw();
    }
}
fn paint_art(w: &Paint, s: &gtk::Snapshot, id: usize, o: Options, value: &str) {
    let p = o.palette();
    let r = bounds(w);
    let (width, height) = (r.width(), r.height());
    match id {
        PREVIEW => preview(w, s, o, &r),
        GLYPH => {
            fill_round(s, &r, 8.0, p.surface);
            ring(s, &r, 8.0, 1.0, p.line);
            // 词按共用网格缩排，字号随格子变小，长词也能放全；同 Windows draw_word。
            let count = value.chars().count();
            if count > 1 {
                let (columns, cell) = glyph_grid(count, (width.min(height) - 16.0) as i32);
                let (rows, cell) = (count.div_ceil(columns) as f32, cell as f32);
                let font = candidate_font(o, (cell * 5.0 / 6.0).floor() as f64);
                let left = (width - columns as f32 * cell) / 2.0;
                let top = (height - rows * cell) / 2.0;
                for (index, glyph) in value.chars().enumerate() {
                    let (row, column) = ((index / columns) as f32, (index % columns) as f32);
                    let slot = rect(left + column * cell, top + row * cell, cell, cell);
                    draw_text(
                        s,
                        &layout(w, &glyph.to_string(), &font),
                        &slot,
                        true,
                        p.text,
                    );
                }
                return;
            }
            // 虚线十字格，内缩 8 像素，与 Windows PS_DOT 相同的点距。
            let (cx, cy) = ((width / 2.0).floor(), (height / 2.0).floor());
            let mut t = 8.0;
            while t < width.max(height) - 8.0 {
                if t < height - 8.0 {
                    fill(s, &rect(cx, t, 1.0, 1.0), p.line);
                }
                if t < width - 8.0 {
                    fill(s, &rect(t, cy, 1.0, 1.0), p.line);
                }
                t += 2.0;
            }
            draw_text(
                s,
                &layout(w, value, &candidate_font(o, 76.0)),
                &r,
                true,
                p.text,
            );
        }
        CODES | FULL | READING | PARTS => {
            if value.is_empty() || value == "缺资料" {
                // 与 28 像素高的字段名同一行；拆分格更高，不能按整格居中。
                let line = rect(0.0, 0.0, width, height.min(28.0));
                draw_text(s, &layout(w, value, &ui(w, 12.0)), &line, false, p.muted);
                return;
            }
            // 三项都按共用布局折行，位置与行高同 Windows。
            let columns = width as i32;
            match id {
                // 字根逐个放进 30 像素字块，编码写在字块正下方。
                PARTS => {
                    let font = learning_font(20.0);
                    let small = mono(12.0);
                    for (b, root, code) in tile_boxes(value, columns) {
                        let tile = box_rect(b);
                        ring(s, &tile, 6.0, 1.0, p.line);
                        draw_text(s, &layout(w, &root.to_string(), &font), &tile, true, p.text);
                        let below = box_rect([b[0], b[1] + 32, b[2], 16]);
                        draw_text(s, &layout(w, code, &small), &below, true, p.muted);
                    }
                }
                READING => {
                    let measure = |line: &str| text_width(w, line, 14.0);
                    let font = ui(w, 14.0);
                    for (index, line) in reading_lines(value, columns, measure)
                        .into_iter()
                        .enumerate()
                    {
                        let frame = box_rect(reading_line(index, columns));
                        draw_text(s, &layout(w, line, &font), &frame, false, p.text);
                    }
                }
                // 编码以空格分隔，各成一枚浅底键帽。
                _ => {
                    let font = mono(13.0);
                    for (b, code) in chips(value, columns) {
                        let chip = box_rect(b);
                        fill_round(s, &chip, 5.0, shades(p).track);
                        draw_text(s, &layout(w, code, &font), &chip, true, p.text);
                    }
                }
            }
        }
        id if (KEYS..KEYS + KEYMAP.len()).contains(&id) => {
            let (name, spec) = KEYMAP[id - KEYS];
            let title = rect(15.0, 8.0, width - 23.0, 20.0);
            draw_text(s, &layout(w, name, &ui(w, 14.0)), &title, false, p.text);
            // Linux 键盘的键名与 Windows 相同，只把 N 换成当前每页候选数。
            let mut x = 15.0;
            for (key, part) in key_parts(&keys(spec, o.page_size)) {
                if key {
                    let line = layout(w, part, &mono(13.0));
                    let cap = rect(x, 32.0, (line.pixel_size().0 as f32 + 12.0).max(22.0), 22.0);
                    fill_round(s, &cap, 5.0, p.window);
                    ring(s, &cap, 5.0, 1.0, p.field);
                    draw_text(s, &line, &cap, true, p.text);
                    x += cap.width() + 5.0;
                } else {
                    let line = layout(w, part, &ui(w, 12.0));
                    let room = rect(x, 32.0, line.pixel_size().0 as f32, 22.0);
                    x += draw_text(s, &line, &room, false, p.muted) + 5.0;
                }
            }
        }
        _ => {
            draw_text(s, &layout(w, value, &ui(w, 14.0)), &r, false, p.text);
        }
    }
}
/// 外观区预览：按草稿现画一段带组合的文字和候选窗，尺寸与 Windows draw_preview 相同。
fn preview(w: &Paint, s: &gtk::Snapshot, o: Options, r: &graphene::Rect) {
    let p = o.palette();
    fill_round(s, r, 8.0, shades(p).sidebar);
    ring(s, r, 8.0, 1.0, p.line);
    let body = ui(w, 15.0);
    let line = rect(20.0, 14.0, r.width() - 40.0, 22.0);
    let start = line.x() + draw_text(s, &layout(w, "明天我们一起去", &body), &line, false, p.text);
    let code = rect(start, line.y(), 40.0, 22.0);
    let end = start + draw_text(s, &layout(w, "ui", &body), &code, false, p.text);
    // 组合下的点状下划线与插入符。
    let bottom = line.y() + line.height();
    let mut x = start;
    while x < end {
        fill(s, &rect(x, bottom - 2.0, 1.0, 1.0), p.text);
        x += 3.0;
    }
    fill(
        s,
        &rect(end + 1.0, line.y() + 2.0, 1.0, line.height() - 4.0),
        p.text,
    );
    let rows = [
        ("1", "拉", "〔扌立 · uil〕"),
        ("2", "隹", "〔隹 · ui / uii〕"),
    ];
    let label_font = mono(12.0);
    let glyph_font = candidate_font(o, 19.0);
    let hint_font = learning_font(12.0);
    let measure =
        |text: &str, font: &pango::FontDescription| layout(w, text, font).pixel_size().0 as f32;
    let comment = if o.show_learning {
        rows.iter()
            .map(|(.., hint)| measure(hint, &hint_font))
            .fold(0.0_f32, f32::max)
            + 8.0
    } else {
        0.0
    };
    let width =
        (12.0 + measure("1", &label_font) + 8.0 + measure("隹", &glyph_font) + comment + 14.0)
            .max(132.0)
            .min(r.width() - 12.0 - start);
    let window = rect(start, bottom + 6.0, width, 69.0);
    fill_round(s, &window, 8.0, p.window);
    ring(s, &window, 8.0, 1.0, p.border);
    for (i, (label, glyph, hint)) in rows.into_iter().enumerate() {
        let band = rect(
            window.x() + 4.0,
            window.y() + 4.0 + i as f32 * 31.0,
            width - 8.0,
            30.0,
        );
        let chosen = i == 0;
        if chosen && p.highlight != p.window {
            fill_round(s, &band, 4.0, p.highlight);
        }
        let (label_ink, glyph_ink, hint_ink) = if chosen {
            (p.highlight_text, p.chosen, p.highlight_text)
        } else {
            (p.label, p.text, p.muted)
        };
        let mut x = band.x() + 8.0;
        for (font, ink, words) in [
            (&label_font, label_ink, label),
            (&glyph_font, glyph_ink, glyph),
            (
                &hint_font,
                hint_ink,
                if o.show_learning { hint } else { "" },
            ),
        ] {
            let text = layout(w, words, font);
            let room = rect(x, band.y(), text.pixel_size().0 as f32, 30.0);
            x += draw_text(s, &text, &room, false, ink) + 8.0;
        }
    }
}

/// 自绘按钮外观：开关行、侧栏分区、主题卡、分段单选、色块行、字块与普通按钮；
/// 按钮本身是 GTK 按钮（键盘、读屏与激活语义），外观画在其唯一子控件上。
pub(super) struct FaceState {
    pub style: usize,
    pub title: RefCell<String>,
    pub palette: Cell<Palette>,
    pub selected: Cell<bool>,
    /// 设置控件重新载入期间只画停用、不真的停用，焦点不丢（同 Windows）；查字按钮另用 GTK 停用。
    pub enabled: Cell<bool>,
    pub swatch: Cell<Option<u32>>,
    pub preview: Cell<Option<Palette>>,
    pub font: RefCell<Option<pango::FontDescription>>,
}
pub(super) fn paint_face(button: &gtk::Widget, w: &Paint, s: &gtk::Snapshot, state: &FaceState) {
    let p = state.palette.get();
    let shade = shades(p);
    let r = bounds(w);
    let (width, height) = (r.width(), r.height());
    let id = state.style;
    let chosen = state.selected.get();
    let enabled = button.is_sensitive() && state.enabled.get();
    let flags = button.state_flags();
    // 键盘操作才画焦点环（GTK 的 focus-visible），鼠标点击不显示，同 Windows。
    let focused = flags.contains(gtk::StateFlags::FOCUS_VISIBLE) && button.has_focus();
    let pressed = flags.contains(gtk::StateFlags::ACTIVE);
    let ink = |on: u32| if enabled { on } else { p.faint };
    let title = state.title.borrow();
    let in_range = |start: usize, count: usize| (start..start + count).contains(&id);
    if [AUTO, COMMON, LEARNING].contains(&id) {
        // 开关行：卡片底色上的标题、说明与右侧开关，不另画行框。
        let description = TOGGLES
            .iter()
            .find(|(key, ..)| *key == id)
            .map_or("", |(.., d)| *d);
        let name = rect(15.0, 9.0, width - 95.0, 22.0);
        draw_text(
            s,
            &layout(w, &title, &ui(w, 14.0)),
            &name,
            false,
            ink(p.text),
        );
        let detail = rect(15.0, 32.0, width - 95.0, 18.0);
        draw_text(
            s,
            &layout(w, description, &ui(w, 12.0)),
            &detail,
            false,
            p.muted,
        );
        let track = rect(width - 55.0, ((height - 22.0) / 2.0).floor(), 40.0, 22.0);
        let (x, y) = (track.x(), track.y());
        if chosen {
            fill_round(s, &track, 11.0, ink(p.accent));
            fill_round(s, &rect(x + 22.0, y + 4.0, 14.0, 14.0), 7.0, p.on_accent);
        } else {
            ring(s, &track, 11.0, 2.0, ink(p.field));
            fill_round(s, &rect(x + 5.0, y + 6.0, 10.0, 10.0), 5.0, ink(p.muted));
        }
        if focused {
            ring(s, &inset(&track, -3.0), 14.0, 2.0, p.text);
        }
    } else if in_range(NAV, PAGES.len()) {
        // 侧栏分区：图标 + 文字左对齐，选中只是一块浅色底。
        if chosen {
            fill_round(s, &r, 6.0, shade.nav);
        }
        const ICONS: [&[&str]; 4] = [
            &["insert-text-symbolic", "format-text-plaintext-symbolic"],
            &["applications-graphics-symbolic", "color-select-symbolic"],
            &["input-keyboard-symbolic"],
            &["preferences-other-symbolic", "applications-system-symbolic"],
        ];
        let icon = rect(10.0, 0.0, 20.0, height);
        symbol(
            w,
            s,
            ICONS[id - NAV],
            16,
            if chosen { p.accent } else { p.muted },
            &icon,
        );
        let name = rect(38.0, 0.0, width - 38.0, height);
        draw_text(s, &layout(w, &title, &ui(w, 14.0)), &name, false, p.text);
        if focused {
            ring(s, &r, 6.0, 2.0, p.text);
        }
    } else if in_range(THEME, Theme::ALL.len()) {
        // 主题卡：选中为强调色 2 像素外框，内嵌该主题的迷你候选窗。
        ring(s, &r, 8.0, 2.0, if chosen { p.accent } else { p.line });
        let sample = state.preview.get().unwrap_or_else(|| {
            Options {
                theme: Theme::ALL[id - THEME],
                ..Options::default()
            }
            .palette()
        });
        let mini = rect(8.0, 8.0, width - 16.0, 44.0);
        fill_round(s, &mini, 5.0, sample.window);
        ring(s, &mini, 5.0, 1.0, sample.border);
        let face = candidate_font(Options::default(), 12.0);
        for (row, (words, glyph_ink)) in [("1 拉", sample.chosen), ("2 隹", sample.text)]
            .into_iter()
            .enumerate()
        {
            let band = rect(12.0, 12.0 + row as f32 * 19.0, width - 24.0, 17.0);
            if row == 0 && sample.highlight != sample.window {
                fill_round(s, &band, 3.0, sample.highlight);
            }
            let text = rect(band.x() + 5.0, band.y(), band.width() - 5.0, 17.0);
            draw_text(s, &layout(w, words, &face), &text, false, glyph_ink);
        }
        let name = rect(0.0, 54.0, width, height - 57.0);
        draw_text(s, &layout(w, &title, &ui(w, 14.0)), &name, true, p.text);
        if focused {
            ring(s, &inset(&r, 3.0), 6.0, 2.0, p.text);
        }
    } else if in_range(MODE, 2)
        || in_range(FORM, 3)
        || in_range(FONT, 2)
        || in_range(SIZE_CHOICE, 7)
    {
        // 分段单选：轨道由父控件画；只有选中段是卡片色小块加细边，未选中段灰字无边。
        if chosen {
            fill_round(s, &r, 5.0, p.surface);
            ring(s, &r, 5.0, 1.0, p.line);
        }
        let font = if in_range(FONT, 2) {
            candidate_font(
                Options {
                    candidate_font: CandidateFont::ALL[id - FONT],
                    ..Options::default()
                },
                15.0,
            )
        } else {
            ui(w, 14.0)
        };
        let text = layout(w, &title, &font);
        draw_text(
            s,
            &text,
            &r,
            true,
            ink(if chosen { p.text } else { p.muted }),
        );
        if focused {
            ring(s, &r, 5.0, 2.0, p.text);
        }
    } else if in_range(SWATCH, COLORS.len()) {
        // 自定义四色行：左侧圆角色块，右侧名称与说明。
        let chip = rect(0.0, 5.0, 30.0, 30.0);
        if let Some(value) = state.swatch.get() {
            fill_round(s, &chip, 7.0, value);
        }
        ring(s, &chip, 7.0, 1.0, p.field);
        let (name, description) = COLORS[id - SWATCH];
        let top = rect(42.0, 2.0, width - 42.0, 19.0);
        draw_text(
            s,
            &layout(w, &format!("{name}色"), &ui(w, 14.0)),
            &top,
            false,
            p.text,
        );
        let below = rect(42.0, 21.0, width - 42.0, 17.0);
        draw_text(
            s,
            &layout(w, description, &ui(w, 12.0)),
            &below,
            false,
            p.muted,
        );
        if focused {
            ring(s, &inset(&chip, -3.0), 9.0, 2.0, p.text);
        }
    } else if id == LIST {
        // 查字字块：选中为浅底加强调色描边；键盘焦点另画正文色环。
        let chip = rect(3.0, 2.0, width - 6.0, height - 4.0);
        if chosen {
            fill_round(s, &chip, 7.0, shade.nav);
            ring(s, &chip, 7.0, 2.0, p.accent);
        }
        let font = state.font.borrow().clone().unwrap_or_else(|| ui(w, 20.0));
        draw_text(s, &layout(w, &title, &font), &chip, true, ink(p.text));
        if focused && !chosen {
            ring(s, &chip, 7.0, 2.0, p.text);
        }
    } else if let Some(value) = state.swatch.get() {
        // 色板格：圆角色块；按下或键盘焦点画正文色环，不用系统蓝色高亮。
        let chip = inset(&r, 4.0);
        fill_round(s, &chip, 5.0, value);
        ring(s, &chip, 5.0, 1.0, p.line);
        if focused || pressed {
            ring(s, &inset(&r, 1.0), 7.0, 2.0, p.text);
        }
    } else {
        // 普通按钮：卡片色圆角块加细边；翻页箭头无底无边。
        let flat = matches!(id, PREVIOUS | NEXT);
        if pressed {
            fill_round(s, &r, 6.0, shade.track);
        } else if !flat {
            fill_round(s, &r, 6.0, p.surface);
        }
        if !flat {
            ring(s, &r, 6.0, 1.0, p.field);
        }
        let icon: Option<&[&str]> = match id {
            SETTINGS => Some(&["emblem-system-symbolic", "preferences-system-symbolic"]),
            PREVIOUS => Some(&["go-previous-symbolic", "pan-start-symbolic"]),
            NEXT => Some(&["go-next-symbolic", "pan-end-symbolic"]),
            COPY => Some(&["edit-copy-symbolic"]),
            RELOAD => Some(&["view-refresh-symbolic"]),
            _ => None,
        };
        let label = layout(w, &title, &ui(w, 14.0));
        match icon {
            // 只有图标的按钮：标题留作读屏名称与助记键，不画出来。
            Some(names) if matches!(id, SETTINGS | PREVIOUS | NEXT) => {
                symbol(w, s, names, 16, ink(p.text), &r);
            }
            Some(names) => {
                let words = label.pixel_size().0 as f32;
                let left = ((width - 14.0 - 6.0 - words) / 2.0).floor();
                symbol(w, s, names, 14, ink(p.text), &rect(left, 0.0, 14.0, height));
                let room = rect(left + 20.0, 0.0, words, height);
                draw_text(s, &label, &room, false, ink(p.text));
            }
            None => {
                draw_text(s, &label, &r, true, ink(p.text));
            }
        }
        if focused {
            ring(s, &r, 6.0, 2.0, p.text);
        }
    }
}
