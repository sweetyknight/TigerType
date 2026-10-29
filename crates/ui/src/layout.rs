//! Win32 与 AppKit 共用的控件标识、内容与逻辑尺寸；平台层仅创建原生控件和绘制。
use tigertype_contracts::{OutputForm, Theme};

// 查字窗控件。
pub(crate) const QUERY: usize = 101;
/// 回车查询的动作编号；查字窗没有单独的查询按钮。
#[cfg(windows)]
pub(crate) const SEARCH: usize = 102;
/// 横排字块：单行多列自绘列表，方向键左右换字。
pub(crate) const LIST: usize = 103;
pub(crate) const PREVIOUS: usize = 104;
pub(crate) const NEXT: usize = 105;
pub(crate) const COPY: usize = 106;
pub(crate) const SETTINGS: usize = 107;
/// 详情卡里的提示：无结果、查询中或出错时代替资料显示。
pub(crate) const STATUS: usize = 108;
pub(crate) const PAGE: usize = 109;
pub(crate) const GLYPH: usize = 110;
pub(crate) const CODEPOINT: usize = 111;
/// 零尺寸标签：给查询框提供读屏名称与 Alt+Q 助记键。
pub(crate) const QUERY_LABEL: usize = 112;
/// 四项资料：编码以键帽、拆分以字块自绘，读音为普通文字；标签为 STATIC。
pub(crate) const CODES: usize = 113;
pub(crate) const FULL: usize = 114;
pub(crate) const READING: usize = 115;
pub(crate) const PARTS: usize = 116;
pub(crate) const FIELD_LABEL: usize = 117;
/// 隐藏的多行 EDIT，只用来把当前字的资料交给标准复制。
pub(crate) const CLIP: usize = 121;

// 设置窗控件。
pub(crate) const AUTO: usize = 201;
pub(crate) const COMMON: usize = 202;
pub(crate) const LEARNING: usize = 203;
pub(crate) const OPEN_CONFIG: usize = 207;
pub(crate) const RELOAD: usize = 208;
/// 侧栏底部的保存状态。
pub(crate) const NOTICE: usize = 211;
pub(crate) const HEADING: usize = 212;
pub(crate) const FORM: usize = 220;
pub(crate) const FONT: usize = 230;
pub(crate) const THEME: usize = 240;
/// 自定义四基色（背景、文字、高亮、强调）的十六进制输入框。
pub(crate) const CUSTOM: usize = 250;
pub(crate) const ROW_TITLE: usize = 260;
pub(crate) const ROW_DESC: usize = 270;
pub(crate) const MODE: usize = 280;
/// 每页候选数 3–9 的分段单选。
pub(crate) const SIZE_CHOICE: usize = 290;
/// 侧栏分区：单选组，选中项即当前分区。
pub(crate) const NAV: usize = 300;
pub(crate) const GROUP: usize = 310;
/// 自定义配色每行的色块按钮，点开无蓝色板。
pub(crate) const SWATCH: usize = 320;
pub(crate) const CUSTOM_STATUS: usize = 324;
pub(crate) const RESET: usize = 325;
/// 外观区顶部的候选窗预览（按草稿现画）。
pub(crate) const PREVIEW: usize = 326;
pub(crate) const KEYS: usize = 330;
pub(crate) const FILE_NAME: usize = 342;
pub(crate) const FILE_PATH: usize = 343;
pub(crate) const ABOUT_TITLE: usize = 344;
pub(crate) const ABOUT_DESC: usize = 345;
pub(crate) const KEYS_NOTE: usize = 346;

pub(crate) const FORMS: [(OutputForm, &str); 3] = [
    (OutputForm::Original, "原字"),
    (OutputForm::Simplified, "简体"),
    (OutputForm::Traditional, "繁体"),
];
pub(crate) const TOGGLES: [(usize, &str, &str); 3] = [
    (
        AUTO,
        "四码唯一时自动上屏(&A)",
        "编码满四码且只有一个候选时直接上屏",
    ),
    (
        COMMON,
        "仅常用字(&C)",
        "关闭后普通输入也使用全字集；查字始终使用全字集",
    ),
    (
        LEARNING,
        "显示拆分与编码(&L)",
        "候选旁显示〔拆分 · 编码〕；组合中按 Ctrl+L 临时切换",
    ),
];
/// 分段单选行：标题、说明、首个控件编号、段数、段宽（96 DPI）；行号即下标。
pub(crate) const CHOICES: [(&str, &str, usize, usize, i32); 4] = [
    ("输入模式(&M)", "普通输入使用的码表", MODE, 2, 64),
    ("输出字形(&O)", "上屏前转换为所选字形", FORM, 3, 56),
    (
        "候选字体(&F)",
        "候选窗与查字字形；未安装时回退到系统字体",
        FONT,
        2,
        84,
    ),
    (
        "每页候选数(&N)",
        "候选多于一页时才显示翻页",
        SIZE_CHOICE,
        7,
        30,
    ),
];
/// 侧栏分区：名称与图标字体码位（Segoe Fluent Icons / MDL2 Assets 共有）。
pub(crate) const PAGES: [(&str, u16); 4] = [
    ("输入", 0xE8D2),
    ("外观", 0xE790),
    ("快捷键", 0xE765),
    ("高级", 0xE9E9),
];
pub(crate) const GROUPS: [&str; 8] = [
    "码表",
    "上屏",
    "候选窗",
    "配色",
    "任何时候",
    "输入编码时",
    "配置文件",
    "关于",
];
pub(crate) const COLORS: [(&str, &str); 4] = [
    ("背景", "窗口与候选窗底色"),
    ("文字", "正文与候选字"),
    ("高亮", "当前候选的底色"),
    ("强调", "开关、按钮与选中标记"),
];
/// 自定义配色色板：8 列色相（红、玫红、橙、琥珀、黄、绿、暖灰、中性灰）× 4 档明度，全部无蓝。
pub(crate) const SWATCHES: [u32; 32] = [
    0xFEF2F2, 0xFFF1F3, 0xFFF7ED, 0xFFFBEB, 0xFEFCE8, 0xF7FEE7, 0xF9F8F5, 0xFAFAFA, //
    0xFEE2E2, 0xFFE4E6, 0xFFEDD5, 0xFEF3C7, 0xFEF9C3, 0xECFCCB, 0xEDE9E3, 0xEBEBEB, //
    0xDC2626, 0xE11D48, 0xEA580C, 0xB45309, 0xA16207, 0x4D7C0F, 0x78716C, 0x525252, //
    0x7F1D1D, 0x881337, 0x7C2D12, 0x78350F, 0x713F12, 0x365314, 0x1C1A17, 0x1A1A1A,
];
/// 快捷键说明：方括号内为键帽；“N”按当前每页候选数显示。前四项为“任何时候”，其余为“输入编码时”。
pub(crate) const KEYMAP: [(&str, &str); 12] = [
    ("切换中英文", "[Shift] 单击"),
    ("切换中英文标点", "[Ctrl] + [.]"),
    ("拼音反查", "[`] 后输入拼音"),
    ("全角与半角", "任务栏图标菜单"),
    ("选择候选", "[1] – [N]"),
    ("第二、三候选", "[;] [']"),
    ("移动高亮", "[↑] [↓] [←] [→]"),
    ("翻页", "[-] [=] [PgUp] [PgDn]"),
    ("首选上屏", "[Space]"),
    ("编码原样上屏", "[Enter]"),
    ("取消输入", "[Esc]"),
    ("显示拆分与编码", "[Ctrl] + [L]"),
];
/// 设置窗固定客户区；内容超出时右侧出现滚动条。
pub(crate) const SETTINGS_SIZE: (i32, i32) = (720, 520);
pub(crate) const SIDEBAR: i32 = 184;
/// 内容区左边与宽度、行高（96 DPI）。
pub(crate) const CX: i32 = 212;
pub(crate) const CW: i32 = 480;
pub(crate) const ROW: i32 = 62;
/// 查字窗默认与最小客户区。
pub(crate) const LOOKUP_SIZE: (i32, i32) = (520, 400);
pub(crate) const LOOKUP_MIN: (i32, i32) = (440, 340);

pub(crate) type Box4 = [i32; 4];
/// 查字窗布局（96 DPI 逻辑坐标，按客户区尺寸现算）：控件位置与需由父窗口画的查询框、详情卡。
pub(crate) struct LookupSpec {
    pub controls: Vec<(usize, Box4)>,
    pub field: Box4,
    pub card: Box4,
}
pub(crate) fn lookup_spec(width: i32, height: i32) -> LookupSpec {
    const M: i32 = 20;
    let field = [M, 12, (width - 2 * M - 46).max(160), 38];
    let card = [M, 112, width - 2 * M, (height - 112 - 18).max(170)];
    let (x0, y0) = (card[0] + 16, card[1] + 16);
    let x1 = x0 + 112 + 20;
    let value_x = x1 + 68;
    let value_w = (card[0] + card[2] - 16 - value_x).max(60);
    let mut controls = vec![
        (QUERY_LABEL, [0, 0, 0, 0]),
        (
            QUERY,
            [field[0] + 36, field[1] + 10, field[2] - 36 - 62, 20],
        ),
        (SETTINGS, [width - M - 38, 12, 38, 38]),
        (LIST, [M, 62, (width - 2 * M - 100).max(120), 40]),
        (PREVIOUS, [width - M - 92, 66, 30, 32]),
        (PAGE, [width - M - 62, 66, 32, 32]),
        (NEXT, [width - M - 30, 66, 30, 32]),
        (GLYPH, [x0, y0, 112, 112]),
        (CODEPOINT, [x0, y0 + 118, 112, 18]),
        (COPY, [card[0] + card[2] - 16 - 80, y0 - 4, 80, 28]),
        (STATUS, [x0, card[1] + card[3] / 2 - 24, card[2] - 32, 48]),
        (CLIP, [0, 0, 0, 0]),
    ];
    for (index, value) in [CODES, FULL, READING, PARTS].into_iter().enumerate() {
        let top = y0 + index as i32 * 38;
        controls.push((FIELD_LABEL + index, [x1, top, 64, 28]));
        // 首行右侧让出“复制”按钮。
        let width = if index == 0 { value_w - 88 } else { value_w };
        controls.push((
            value,
            [
                value_x,
                top,
                width.max(40),
                if value == PARTS { 30 } else { 28 },
            ],
        ));
    }
    LookupSpec {
        controls,
        field,
        card,
    }
}

/// 设置窗一个分区的布局：固定的侧栏控件、随滚动移动的内容控件与父窗口要画的卡片等；
/// 只列当前分区可见的控件，其余由 layout 隐藏。
#[derive(Default)]
pub(crate) struct SettingsSpec {
    pub fixed: Vec<(usize, Box4)>,
    pub controls: Vec<(usize, Box4)>,
    pub cards: Vec<Box4>,
    pub lines: Vec<Box4>,
    pub tracks: Vec<Box4>,
    pub frames: Vec<Box4>,
    /// 配置文件图标底与“关于”印章的位置。
    pub tile: Option<Box4>,
    pub seal: Option<Box4>,
    pub height: i32,
}
enum Row {
    Toggle(usize),
    Choice(usize),
}
impl SettingsSpec {
    fn group(&mut self, index: usize, y: i32) -> i32 {
        self.controls.push((GROUP + index, [CX + 2, y, CW - 2, 20]));
        y + 26
    }
    fn card(&mut self, y: i32, rows: &[Row]) -> i32 {
        self.cards.push([CX, y, CW, rows.len() as i32 * ROW]);
        for (index, row) in rows.iter().enumerate() {
            let top = y + index as i32 * ROW;
            if index > 0 {
                self.lines.push([CX + 1, top, CW - 2, 1]);
            }
            match *row {
                Row::Toggle(id) => self.controls.push((id, [CX + 1, top + 1, CW - 2, ROW - 2])),
                Row::Choice(choice) => {
                    let (_, _, first, count, segment) = CHOICES[choice];
                    let count = count as i32;
                    let total = count * segment + (count - 1) * 2 + 4;
                    let track = [CX + CW - 16 - total, top + (ROW - 32) / 2, total, 32];
                    self.tracks.push(track);
                    for k in 0..count {
                        self.controls.push((
                            first + k as usize,
                            [track[0] + 2 + k * (segment + 2), track[1] + 2, segment, 28],
                        ));
                    }
                    let width = CW - 32 - total - 16;
                    self.controls
                        .push((ROW_TITLE + choice, [CX + 16, top + 10, width, 22]));
                    self.controls
                        .push((ROW_DESC + choice, [CX + 16, top + 33, width, 18]));
                }
            }
        }
        y + rows.len() as i32 * ROW
    }
    fn palette(&mut self, y: i32, custom: bool) -> i32 {
        let count = Theme::ALL.len() as i32;
        let width = (CW - 28 - (count - 1) * 10) / count;
        for k in 0..count {
            self.controls.push((
                THEME + k as usize,
                [CX + 14 + k * (width + 10), y + 14, width, 78],
            ));
        }
        let mut bottom = y + 106;
        if custom {
            self.lines.push([CX + 14, bottom, CW - 28, 1]);
            let start = bottom + 8;
            for k in 0..COLORS.len() as i32 {
                let top = start + k * 42;
                self.controls
                    .push((SWATCH + k as usize, [CX + 14, top, CW - 28 - 110, 40]));
                let frame = [CX + CW - 14 - 96, top + 5, 96, 30];
                self.frames.push(frame);
                self.controls
                    .push((CUSTOM + k as usize, [frame[0] + 9, top + 11, 78, 18]));
            }
            let status = start + COLORS.len() as i32 * 42 + 4;
            self.controls
                .push((CUSTOM_STATUS, [CX + 14, status, CW - 28 - 140, 28]));
            self.controls
                .push((RESET, [CX + CW - 14 - 128, status, 128, 28]));
            bottom = status + 28 + 14;
        }
        self.cards.push([CX, y, CW, bottom - y]);
        bottom
    }
    fn keys(&mut self, y: i32, range: std::ops::Range<usize>) -> i32 {
        const CELL: i32 = 58;
        let rows = range.len().div_ceil(2) as i32;
        self.cards.push([CX, y, CW, rows * CELL]);
        self.lines.push([CX + CW / 2, y + 1, 1, rows * CELL - 2]);
        for row in 1..rows {
            self.lines.push([CX + 1, y + row * CELL, CW - 2, 1]);
        }
        for (index, key) in range.enumerate() {
            let (row, column) = ((index / 2) as i32, (index % 2) as i32);
            self.controls.push((
                KEYS + key,
                [
                    CX + 1 + column * CW / 2,
                    y + 1 + row * CELL,
                    CW / 2 - 2,
                    CELL - 2,
                ],
            ));
        }
        y + rows * CELL
    }
    fn config(&mut self, y: i32) -> i32 {
        self.cards.push([CX, y, CW, 64 + 72]);
        self.tile = Some([CX + 16, y + 14, 36, 36]);
        self.controls
            .push((FILE_NAME, [CX + 64, y + 12, CW - 64 - 16 - 76, 20]));
        self.controls
            .push((FILE_PATH, [CX + 64, y + 34, CW - 64 - 16 - 76, 18]));
        self.controls
            .push((OPEN_CONFIG, [CX + CW - 16 - 64, y + 17, 64, 30]));
        self.lines.push([CX + 1, y + 64, CW - 2, 1]);
        self.controls
            .push((ROW_TITLE + 4, [CX + 16, y + 74, CW - 32 - 116, 22]));
        self.controls
            .push((ROW_DESC + 4, [CX + 16, y + 96, CW - 32 - 116, 34]));
        self.controls
            .push((RELOAD, [CX + CW - 16 - 104, y + 85, 104, 30]));
        y + 136
    }
    fn about(&mut self, y: i32) -> i32 {
        self.cards.push([CX, y, CW, 72]);
        self.seal = Some([CX + 16, y + 16, 40, 40]);
        self.controls
            .push((ABOUT_TITLE, [CX + 70, y + 15, CW - 86, 22]));
        self.controls
            .push((ABOUT_DESC, [CX + 70, y + 38, CW - 86, 18]));
        y + 72
    }
}
pub(crate) fn settings_spec(page: usize, custom: bool) -> SettingsSpec {
    let (_, height) = SETTINGS_SIZE;
    let mut s = SettingsSpec::default();
    for k in 0..PAGES.len() {
        s.fixed
            .push((NAV + k, [10, 8 + k as i32 * 38, SIDEBAR - 20, 36]));
    }
    s.fixed
        .push((NOTICE, [20, height - 16 - 52, SIDEBAR - 36, 52]));
    s.controls.push((HEADING, [CX, 20, CW, 30]));
    let mut y = 66;
    match page {
        0 => {
            y = s.group(0, y);
            y = s.card(y, &[Row::Choice(0), Row::Toggle(COMMON)]);
            y = s.group(1, y + 18);
            y = s.card(y, &[Row::Toggle(AUTO), Row::Choice(1)]);
        }
        1 => {
            s.controls.push((PREVIEW, [CX, y, CW, 120]));
            y = s.group(2, y + 120 + 18);
            y = s.card(y, &[Row::Choice(2), Row::Choice(3), Row::Toggle(LEARNING)]);
            y = s.group(3, y + 18);
            y = s.palette(y, custom);
        }
        2 => {
            s.controls.push((KEYS_NOTE, [CX + 80, 26, 240, 20]));
            y = s.group(4, y);
            y = s.keys(y, 0..4);
            y = s.group(5, y + 18);
            y = s.keys(y, 4..KEYMAP.len());
        }
        _ => {
            y = s.group(6, y);
            y = s.config(y);
            y = s.group(7, y + 18);
            y = s.about(y);
        }
    }
    s.height = y + 24;
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn both_platforms_share_the_same_sections_and_bounded_layout() {
        use super::*;
        for size in [LOOKUP_MIN, LOOKUP_SIZE] {
            let spec = lookup_spec(size.0, size.1);
            for (_, b) in spec.controls {
                assert!(b[0] >= 0 && b[1] >= 0 && b[2] >= 0 && b[3] >= 0);
                assert!(b[0] + b[2] <= size.0 && b[1] + b[3] <= size.1);
            }
        }
        for page in 0..4 {
            for custom in [false, true] {
                let spec = settings_spec(page, custom);
                let mut ids = std::collections::BTreeSet::new();
                for (id, b) in spec.controls {
                    assert!(ids.insert(id), "控件编号重复 {id}");
                    assert!(b[0] >= SIDEBAR && b[0] + b[2] <= SETTINGS_SIZE.0);
                    assert!(b[1] >= 0 && b[1] + b[3] <= spec.height);
                }
                assert_eq!(ids.contains(&PREVIEW), page == 1);
                assert_eq!(ids.contains(&CUSTOM), page == 1 && custom);
            }
        }
        assert!(SWATCHES.into_iter().all(tigertype_contracts::without_blue));
    }
}
