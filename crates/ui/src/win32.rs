//! Win32 标准控件提供键盘导航和系统可访问性；不启用系统控件主题（其悬停、焦点、勾选都是蓝色），
//! 外观由父窗口与子类化按钮按主题自绘。回调无业务状态、无分配、无可展开代码：
//! 配色取本 UI 线程的绘制配色，字体取控件自身，布局按客户区尺寸与控件勾选状态现算，
//! 窗口数据只存设置窗的滚动位置（整数），不存指针。
//! 设置窗分“输入/外观/快捷键/高级”四区，修改即时保存；查字窗为紧凑的字块行加详情卡。
use crate::{
    Inbox, Latest,
    model::{Effect, Model, details},
};
use std::{
    cell::Cell,
    ffi::c_void,
    io,
    os::windows::ffi::OsStrExt,
    os::windows::io::AsRawHandle,
    path::Path,
    ptr::{null, null_mut},
    sync::mpsc::{SyncSender, TryRecvError},
};
use tigertype_contracts::{
    CandidateFont, CustomColors, Options, OutputForm, Palette, Theme, TypingMode, mix,
    protocol::Panel,
    ui::{Reply, Request},
};
use windows_sys::Win32::{
    Foundation::{
        COLORREF, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, POINT, RECT, SIZE,
        WAIT_FAILED, WPARAM,
    },
    Graphics::{
        Dwm::{
            DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
            DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
        },
        Gdi::*,
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        SystemServices::{SS_CENTER, SS_CENTERIMAGE, SS_NOPREFIX, SS_OWNERDRAW, SS_PATHELLIPSIS},
        Threading::{
            INFINITE, RegisterWaitForSingleObject, SetEvent, UnregisterWaitEx, WT_EXECUTEONLYONCE,
        },
    },
    UI::{
        Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
        Controls::{
            BST_CHECKED, BST_UNCHECKED, DRAWITEMSTRUCT, EM_GETLINE, EM_GETRECT, EM_GETSEL,
            EM_LINEFROMCHAR, EM_LINEINDEX, EM_LINELENGTH, EM_POSFROMCHAR, EM_SETLIMITTEXT,
            EM_SETMARGINS, EM_SETSEL, MEASUREITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_NOFOCUSRECT,
            ODS_SELECTED, ODT_LISTBOX, ODT_MENU, ODT_STATIC, SetScrollInfo,
        },
        HiDpi::*,
        Input::{
            Ime::{GCS_COMPSTR, ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext},
            KeyboardAndMouse::{
                EnableWindow, GetFocus, GetKeyState, IsWindowEnabled, SetFocus, VK_CONTROL,
                VK_ESCAPE, VK_MENU, VK_RETURN,
            },
        },
        Shell::ShellExecuteW,
        WindowsAndMessaging::*,
    },
};

mod seal;
mod tray;
pub use seal::ico_file;

const ACTION: u32 = WM_APP + 1;
const CLOSE: u32 = WM_APP + 2;
const LAYOUT: u32 = WM_APP + 3;
/// 收起系统模态循环：唤起邮箱在模态期间有信号（含后台停止）时由线程池回调投递给进入模态的窗口。
const UNBLOCK: u32 = WM_APP + 6;

// 查字窗控件。
const QUERY: usize = 101;
/// 回车查询的动作编号；查字窗没有单独的查询按钮。
const SEARCH: usize = 102;
/// 横排字块：单行多列自绘列表，方向键左右换字。
const LIST: usize = 103;
const PREVIOUS: usize = 104;
const NEXT: usize = 105;
const COPY: usize = 106;
const SETTINGS: usize = 107;
/// 详情卡里的提示：无结果、查询中或出错时代替资料显示。
const STATUS: usize = 108;
const PAGE: usize = 109;
const GLYPH: usize = 110;
const CODEPOINT: usize = 111;
/// 零尺寸标签：给查询框提供读屏名称与 Alt+Q 助记键。
const QUERY_LABEL: usize = 112;
/// 四项资料：编码以键帽、拆分以字块自绘，读音为普通文字；标签为 STATIC。
const CODES: usize = 113;
const FULL: usize = 114;
const READING: usize = 115;
const PARTS: usize = 116;
const FIELD_LABEL: usize = 117;
/// 隐藏的多行 EDIT，只用来把当前字的资料交给标准复制。
const CLIP: usize = 121;

// 设置窗控件。
const AUTO: usize = 201;
const COMMON: usize = 202;
const LEARNING: usize = 203;
const OPEN_CONFIG: usize = 207;
const RELOAD: usize = 208;
/// 侧栏底部的保存状态。
const NOTICE: usize = 211;
const HEADING: usize = 212;
const FORM: usize = 220;
const FONT: usize = 230;
const THEME: usize = 240;
/// 自定义四基色（背景、文字、高亮、强调）的十六进制输入框。
const CUSTOM: usize = 250;
const ROW_TITLE: usize = 260;
const ROW_DESC: usize = 270;
const MODE: usize = 280;
/// 每页候选数 3–9 的分段单选。
const SIZE_CHOICE: usize = 290;
/// 侧栏分区：单选组，选中项即当前分区。
const NAV: usize = 300;
const GROUP: usize = 310;
/// 自定义配色每行的色块按钮，点开无蓝色板。
const SWATCH: usize = 320;
const CUSTOM_STATUS: usize = 324;
const RESET: usize = 325;
/// 外观区顶部的候选窗预览（按草稿现画）。
const PREVIEW: usize = 326;
const KEYS: usize = 330;
const FILE_NAME: usize = 342;
const FILE_PATH: usize = 343;
const ABOUT_TITLE: usize = 344;
const ABOUT_DESC: usize = 345;
const KEYS_NOTE: usize = 346;

const FORMS: [(OutputForm, &str); 3] = [
    (OutputForm::Original, "原字"),
    (OutputForm::Simplified, "简体"),
    (OutputForm::Traditional, "繁体"),
];
const TOGGLES: [(usize, &str, &str); 3] = [
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
const CHOICES: [(&str, &str, usize, usize, i32); 4] = [
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
const PAGES: [(&str, u16); 4] = [
    ("输入", 0xE8D2),
    ("外观", 0xE790),
    ("快捷键", 0xE765),
    ("高级", 0xE9E9),
];
const GROUPS: [&str; 8] = [
    "码表",
    "上屏",
    "候选窗",
    "配色",
    "任何时候",
    "输入编码时",
    "配置文件",
    "关于",
];
const COLORS: [(&str, &str); 4] = [
    ("背景", "窗口与候选窗底色"),
    ("文字", "正文与候选字"),
    ("高亮", "当前候选的底色"),
    ("强调", "开关、按钮与选中标记"),
];
/// 自定义配色色板：8 列色相（红、玫红、橙、琥珀、黄、绿、暖灰、中性灰）× 4 档明度，全部无蓝。
const SWATCHES: [u32; 32] = [
    0xFEF2F2, 0xFFF1F3, 0xFFF7ED, 0xFFFBEB, 0xFEFCE8, 0xF7FEE7, 0xF9F8F5, 0xFAFAFA, //
    0xFEE2E2, 0xFFE4E6, 0xFFEDD5, 0xFEF3C7, 0xFEF9C3, 0xECFCCB, 0xEDE9E3, 0xEBEBEB, //
    0xDC2626, 0xE11D48, 0xEA580C, 0xB45309, 0xA16207, 0x4D7C0F, 0x78716C, 0x525252, //
    0x7F1D1D, 0x881337, 0x7C2D12, 0x78350F, 0x713F12, 0x365314, 0x1C1A17, 0x1A1A1A,
];
/// 快捷键说明：方括号内为键帽；“N”按当前每页候选数显示。前四项为“任何时候”，其余为“输入编码时”。
const KEYMAP: [(&str, &str); 12] = [
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
const UI_FACE: &str = "Microsoft YaHei UI";
/// 候选字体的 Windows 字体名；未安装时由 create_font 回退到界面字体。
const fn font_face(font: CandidateFont) -> &'static str {
    match font {
        CandidateFont::WenKai => "LXGW WenKai GB Screen",
        CandidateFont::KaiTi => "KaiTi",
    }
}
const MONO_FACES: [&str; 2] = ["Cascadia Mono", "Consolas"];
/// 图标字体：Win11 为 Fluent，Win10 为 MDL2，常用码位相同。
const ICON_FACES: [&str; 2] = ["Segoe Fluent Icons", "Segoe MDL2 Assets"];
/// 字块列表的行高与最小列宽（96 DPI）。
const ITEM_HEIGHT: u32 = 40;
const CHIP: i32 = 42;
/// 设置窗固定客户区；内容超出时右侧出现滚动条。
const SETTINGS_SIZE: (i32, i32) = (720, 520);
const SIDEBAR: i32 = 184;
/// 内容区左边与宽度、行高（96 DPI）。
const CX: i32 = 212;
const CW: i32 = 480;
const ROW: i32 = 62;
/// 查字窗默认与最小客户区。
const LOOKUP_SIZE: (i32, i32) = (520, 400);
const LOOKUP_MIN: (i32, i32) = (440, 340);

thread_local! {
    // 本 UI 线程的绘制配色：只在确认设置后由本线程写入，系统回调只读；不是业务状态。
    static PALETTE: Cell<Palette> = Cell::new(Options::default().palette());
    // 唤起邮箱事件与模态期间登记的线程池等待；只在本 UI 线程读写，是基础设施而非业务状态。
    static WAKE: Cell<HANDLE> = const { Cell::new(null_mut()) };
    static WAIT: Cell<HANDLE> = const { Cell::new(null_mut()) };
}

/// 登记唤起邮箱：菜单（托盘右键、窗口系统菜单、色板）与移动/缩放都是系统模态循环，期间事件循环不运行；
/// 来了唤起或后台停止就收起模态交回事件循环，否则停止时 UI 线程会一直等用户操作，join 不返回。
/// 邮箱须活过本线程的全部窗口；传 None 取消登记。
fn watch(show: Option<&Inbox<Panel>>) {
    WAKE.set(show.map_or(null_mut(), |show| show.event.as_raw_handle()));
}
// SAFETY: 线程池线程调用；上下文是进入模态的窗口句柄（值），只投递消息，不碰 UI 状态。
unsafe extern "system" fn woken(context: *mut c_void, _: bool) {
    // SAFETY: 同上；窗口已销毁时投递只返回失败。
    unsafe { PostMessageW(context as HWND, UNBLOCK, 0, 0) };
}
/// WM_ENTERMENULOOP/WM_ENTERSIZEMOVE：登记一次性等待；已登记（嵌套）或未登记邮箱时不动。登记失败只是退回旧行为。
/// 返回本次是否登记，嵌套的调用方据此只注销自己登记的等待。
fn modal_enter(hwnd: HWND) -> bool {
    let event = WAKE.get();
    if event.is_null() || !WAIT.get().is_null() {
        return false;
    }
    let mut wait = null_mut();
    // SAFETY: 事件由登记的邮箱持有；上下文按值传窗口句柄，不指向栈或堆对象。
    if unsafe {
        RegisterWaitForSingleObject(
            &mut wait,
            event,
            Some(woken),
            hwnd as *const c_void,
            INFINITE,
            WT_EXECUTEONLYONCE,
        )
    } != 0
    {
        WAIT.set(wait);
        return true;
    }
    false
}
/// WM_EXITMENULOOP/WM_EXITSIZEMOVE：阻塞注销到回调结束（若已开始）。
fn modal_exit() {
    let wait = WAIT.replace(null_mut());
    if !wait.is_null() {
        // SAFETY: wait 由 modal_enter 登记，只注销一次；回调只投递消息，阻塞等待不会死锁。
        unsafe { UnregisterWaitEx(wait, INVALID_HANDLE_VALUE) };
    }
}
/// UNBLOCK：自动复位事件已被等待消耗，先重新置上让事件循环照常取唤起或发现停止；
/// 仍在模态中才收起菜单并取消移动/缩放（迟到的 UNBLOCK 不打断之后的操作）。
fn unblock(hwnd: HWND) {
    // SAFETY: 事件由登记的邮箱持有；EndMenu 与 WM_CANCELMODE 只作用于本线程的模态循环与鼠标捕获。
    unsafe {
        SetEvent(WAKE.get());
        if !WAIT.get().is_null() {
            EndMenu();
            SendMessageW(hwnd, WM_CANCELMODE, 0, 0);
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
/// 不带结尾 NUL 的 UTF-16，供 DrawTextW 与测量使用。
fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}
fn error() -> String {
    io::Error::last_os_error().to_string()
}

fn post(hwnd: HWND, message: u32, value: WPARAM) {
    // SAFETY: 只排队值类型通知；队列满时退出并报告故障，不能悄悄丢失用户操作。
    unsafe {
        if PostMessageW(hwnd, message, value, 0) == 0 {
            PostQuitMessage(1);
        }
    }
}

fn high_contrast() -> bool {
    let mut contrast = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        dwFlags: 0,
        lpszDefaultScheme: null_mut(),
    };
    // SAFETY: 输出结构大小已填写，只读系统设置。
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            (&mut contrast as *mut HIGHCONTRASTW).cast(),
            0,
        ) != 0
            && contrast.dwFlags & HCF_HIGHCONTRASTON != 0
    }
}

/// 绘制用颜色（GDI 字节序）。产品要求界面不得出现蓝色，所以不用系统选中色与强调色；
/// 只在用户自己开启高对比度时沿用系统颜色保证可读。侧栏、分段轨道等派生色由主题色混出。
#[derive(Clone, Copy, PartialEq, Debug)]
struct Colors {
    window: COLORREF,
    surface: COLORREF,
    line: COLORREF,
    field: COLORREF,
    text: COLORREF,
    label: COLORREF,
    muted: COLORREF,
    faint: COLORREF,
    highlight: COLORREF,
    highlight_text: COLORREF,
    accent: COLORREF,
    on_accent: COLORREF,
    selection: COLORREF,
    /// 侧栏与预览台面：窗口色向文字色靠 4%。
    sidebar: COLORREF,
    /// 分段单选轨道、键帽底与字块选中外的浅底：靠 6%。
    track: COLORREF,
    /// 侧栏选中项与字块选中底：有高亮底用高亮底，玫红这类无高亮底的主题用选区色。
    nav: COLORREF,
}
/// 主题颜色为 0xRRGGBB，GDI 为 0x00BBGGRR。
const fn rgb(value: u32) -> COLORREF {
    (value & 0xFF) << 16 | (value & 0xFF00) | (value >> 16 & 0xFF)
}
const fn themed(p: Palette) -> Colors {
    Colors {
        window: rgb(p.window),
        surface: rgb(p.surface),
        line: rgb(p.line),
        field: rgb(p.field),
        text: rgb(p.text),
        label: rgb(p.label),
        muted: rgb(p.muted),
        faint: rgb(p.faint),
        highlight: rgb(p.highlight),
        highlight_text: rgb(p.highlight_text),
        accent: rgb(p.accent),
        on_accent: rgb(p.on_accent),
        selection: rgb(p.selection),
        sidebar: rgb(mix(p.window, p.text, 4)),
        track: rgb(mix(p.window, p.text, 6)),
        nav: rgb(if p.highlight != p.window {
            p.highlight
        } else {
            p.selection
        }),
    }
}
fn colors() -> Colors {
    if !high_contrast() {
        return themed(PALETTE.with(Cell::get));
    }
    // SAFETY: 只读系统颜色。
    let system = |index| unsafe { GetSysColor(index) };
    let (window, text) = (system(COLOR_WINDOW), system(COLOR_WINDOWTEXT));
    Colors {
        window,
        surface: window,
        line: text,
        field: text,
        text,
        label: text,
        muted: text,
        faint: system(COLOR_GRAYTEXT),
        highlight: system(COLOR_HIGHLIGHT),
        highlight_text: system(COLOR_HIGHLIGHTTEXT),
        accent: system(COLOR_HIGHLIGHT),
        on_accent: system(COLOR_HIGHLIGHTTEXT),
        selection: system(COLOR_HIGHLIGHT),
        sidebar: window,
        track: window,
        nav: system(COLOR_HIGHLIGHT),
    }
}

/// 用系统 DC 画刷/画笔改色绘制，重绘不新建 GDI 对象。
fn fill(dc: HDC, rect: &RECT, color: COLORREF) {
    // SAFETY: DC 画刷为库存对象，无需释放。
    unsafe {
        SetDCBrushColor(dc, color);
        FillRect(dc, rect, GetStockObject(DC_BRUSH));
    }
}
/// 圆角矩形：fill 为 Some 时实心填充，否则只描 1px 边。
fn round(dc: HDC, rect: &RECT, radius: i32, fill: Option<COLORREF>, line: COLORREF) {
    // SAFETY: 只选入库存画刷/画笔，绘制后还原。
    unsafe {
        let (brush, pen) = match fill {
            Some(color) => {
                SetDCBrushColor(dc, color);
                (GetStockObject(DC_BRUSH), GetStockObject(NULL_PEN))
            }
            None => {
                SetDCPenColor(dc, line);
                (GetStockObject(NULL_BRUSH), GetStockObject(DC_PEN))
            }
        };
        let old_brush = SelectObject(dc, brush);
        let old_pen = SelectObject(dc, pen);
        // 无画笔时 GDI 填充少画右/下一像素，补齐。
        let extra = fill.is_some() as i32;
        RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right + extra,
            rect.bottom + extra,
            radius * 2,
            radius * 2,
        );
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
    }
}
/// 2px 描边：键盘焦点环、选中卡片外框，替代系统虚线框。
fn ring(dc: HDC, rect: &RECT, radius: i32, color: COLORREF) {
    for inset in [0, 1] {
        let inner = RECT {
            left: rect.left + inset,
            top: rect.top + inset,
            right: rect.right - inset - 1,
            bottom: rect.bottom - inset - 1,
        };
        round(dc, &inner, (radius - inset).max(0), None, color);
    }
}
fn inset(rect: &RECT, by: i32) -> RECT {
    RECT {
        left: rect.left + by,
        top: rect.top + by,
        right: rect.right - by,
        bottom: rect.bottom - by,
    }
}
fn draw_text(dc: HDC, font: HFONT, color: COLORREF, text: &[u16], rect: RECT, format: u32) {
    // 长度为 0 时 DrawTextW 仍会读首字符，空 Vec 的悬空指针（0x2）会使后台崩溃。
    if text.is_empty() {
        return;
    }
    let mut rect = rect;
    // SAFETY: 文本缓冲区在同步调用内有效；字体选入后还原。
    unsafe {
        let old = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as _);
        SetTextColor(dc, color);
        DrawTextW(
            dc,
            text.as_ptr(),
            text.len() as i32,
            &mut rect,
            DT_SINGLELINE | DT_VCENTER | format,
        );
        SelectObject(dc, old);
    }
}
/// 按字体测量单行文字宽度。
fn extent(dc: HDC, font: HFONT, text: &[u16]) -> i32 {
    let mut size = SIZE::default();
    // SAFETY: 字体临时选入后还原；文字缓冲在调用期间有效。
    unsafe {
        let old = SelectObject(dc, font);
        GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32, &mut size);
        SelectObject(dc, old);
    }
    size.cx
}
/// 临时字体：按 96 DPI 像素与设备 DPI 新建，用完由 Drop 删除；建不出时为空，绘制跳过。
struct TempFont(HFONT);
impl TempFont {
    fn new(dc: HDC, faces: &[&str], pixels: i32, weight: i32) -> Self {
        // SAFETY: 只读绘制 DC 的 DPI。
        let dpi = unsafe { GetDeviceCaps(dc, LOGPIXELSY as i32) }.max(96) as u32;
        Self(create_font(dc, faces, pixels, dpi, weight).unwrap_or(null_mut()))
    }
}
impl Drop for TempFont {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: 本对象新建的字体，绘制结束后已不再选在 DC 里。
            unsafe { DeleteObject(self.0) };
        }
    }
}
/// 画一个图标字体字形（居中）。
fn draw_icon(dc: HDC, code: u16, pixels: i32, color: COLORREF, rect: RECT) {
    let font = TempFont::new(dc, &ICON_FACES, pixels, 400);
    if !font.0.is_null() {
        draw_text(dc, font.0, color, &[code], rect, DT_CENTER | DT_NOPREFIX);
    }
}
fn window_text(hwnd: HWND, buffer: &mut [u16; 64]) -> usize {
    // SAFETY: 缓冲区长度与传入容量相同。
    unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32).max(0) as usize }
}
fn font_of(hwnd: HWND) -> HFONT {
    // SAFETY: 同步查询控件字体；字体由所属 Window 持有。
    unsafe { SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT }
}
fn parent(hwnd: HWND) -> HWND {
    // SAFETY: 只查询本线程控件的父窗口。
    unsafe { GetParent(hwnd) }
}
fn sibling(hwnd: HWND, id: usize) -> HWND {
    // SAFETY: 只在同一父窗口内按编号找控件。
    unsafe { GetDlgItem(GetParent(hwnd), id as i32) }
}
fn sibling_font(hwnd: HWND, id: usize) -> HFONT {
    font_of(sibling(hwnd, id))
}
fn scale(hwnd: HWND, value: i32) -> i32 {
    // SAFETY: 只读窗口 DPI。
    value * unsafe { GetDpiForWindow(hwnd) }.max(96) as i32 / 96
}
/// 用户未用键盘时按系统约定隐藏焦点环与助记下划线。
fn cues(hwnd: HWND) -> (bool, u32) {
    // SAFETY: 同步查询本控件界面提示状态。
    let state = unsafe { SendMessageW(hwnd, WM_QUERYUISTATE, 0, 0) } as u32;
    (
        state & UISF_HIDEFOCUS == 0,
        if state & UISF_HIDEACCEL != 0 {
            DT_HIDEPREFIX
        } else {
            0
        },
    )
}

/// 按优先顺序建字体：GDI 找不到字体会静默替换，所以核对实际字体名，不符换下一个。
fn create_font(
    dc: HDC,
    faces: &[&str],
    pixels: i32,
    dpi: u32,
    weight: i32,
) -> Result<HFONT, String> {
    for (index, face) in faces.iter().enumerate() {
        // SAFETY: 名称 NUL 结尾；新建字体由调用方持有并负责释放。
        let font = unsafe {
            CreateFontW(
                -(pixels * dpi as i32 / 96),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                CLEARTYPE_QUALITY as u32,
                0,
                wide(face).as_ptr(),
            )
        };
        if font.is_null() {
            continue;
        }
        if index + 1 == faces.len() || face_is(dc, font, face) {
            return Ok(font);
        }
        // SAFETY: 字体只在 face_is 内临时选入并已还原。
        unsafe {
            DeleteObject(font);
        }
    }
    Err(error())
}
fn face_is(dc: HDC, font: HFONT, face: &str) -> bool {
    let mut buffer = [0u16; 64];
    // SAFETY: 临时选入字体读取实际字体名后还原；缓冲区为本地值。
    let length = unsafe {
        let old = SelectObject(dc, font);
        let length = GetTextFaceW(dc, buffer.len() as i32, buffer.as_mut_ptr());
        SelectObject(dc, old);
        length
    };
    String::from_utf16_lossy(&buffer[..(length.max(1) as usize - 1).min(63)])
        .eq_ignore_ascii_case(face)
}

#[cfg(target_pointer_width = "64")]
fn class_procedure(hwnd: HWND) -> WNDPROC {
    // SAFETY: 读取控件类登记的原窗口过程（不受本窗口子类化影响），与 WNDPROC 同宽。
    unsafe { std::mem::transmute::<usize, WNDPROC>(GetClassLongPtrW(hwnd, GCLP_WNDPROC)) }
}
#[cfg(target_pointer_width = "32")]
fn class_procedure(hwnd: HWND) -> WNDPROC {
    // SAFETY: x86 没有 Ptr 版本，类长整数与函数指针同宽。
    unsafe { std::mem::transmute::<usize, WNDPROC>(GetClassLongW(hwnd, GCLP_WNDPROC) as usize) }
}

// SAFETY: 系统在所属 UI 线程调用；先交 EDIT 原过程处理，再在本次回调内按主题重画可见选区与空查询框提示；
// 不保存指针、不分配，选区相关以外的消息原样返回（UNBLOCK 由本过程处理，不交 EDIT）。
unsafe extern "system" fn edit_procedure(
    hwnd: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    if message == UNBLOCK {
        unblock(hwnd);
        return 0;
    }
    // EDIT 自带的右键菜单不发 WM_ENTERMENULOOP，在原过程弹菜单前后自行登记/注销模态等待。
    let menu = message == WM_CONTEXTMENU && modal_enter(hwnd);
    // SAFETY: 参数原样交给 EDIT 类的原窗口过程。
    let result = unsafe { CallWindowProcW(class_procedure(hwnd), hwnd, message, w, l) };
    if menu {
        modal_exit();
    }
    if matches!(message, WM_SETFOCUS | WM_KILLFOCUS) {
        // 输入框外框随焦点变色，由父窗口重画。
        // SAFETY: 只使父窗口失效，绘制在其 WM_PAINT 中进行。
        unsafe {
            InvalidateRect(GetParent(hwnd), null(), 0);
        }
    }
    if message == WM_PAINT {
        paint_cue(hwnd);
    }
    if matches!(
        message,
        WM_PAINT
            | WM_SETFOCUS
            | WM_KEYDOWN
            | WM_CHAR
            | WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_LBUTTONDBLCLK
            | WM_MOUSEMOVE
            | WM_MOUSEWHEEL
            | WM_VSCROLL
            | WM_HSCROLL
            | WM_SIZE
            | WM_IME_ENDCOMPOSITION
            | EM_SETSEL
    ) {
        repaint_selection(hwnd);
    }
    result
}

/// 查询框为空时画占位提示（不启用通用控件 6 版，EM_SETCUEBANNER 不可用）。
fn paint_cue(hwnd: HWND) {
    // SAFETY: 只读本控件编号与文字长度；取本窗口 DC 并在结束时归还，插入符先隐藏避免残影。
    unsafe {
        if GetDlgCtrlID(hwnd) as usize != QUERY || GetWindowTextLengthW(hwnd) != 0 {
            return;
        }
        let dc = GetDC(hwnd);
        if dc.is_null() {
            return;
        }
        let mut format = RECT::default();
        SendMessageW(hwnd, EM_GETRECT, 0, &mut format as *mut RECT as LPARAM);
        HideCaret(hwnd);
        draw_text(
            dc,
            font_of(hwnd),
            colors().muted,
            &units("汉字或拼音，按 Enter 查询"),
            format,
            DT_NOPREFIX,
        );
        ShowCaret(hwnd);
        ReleaseDC(hwnd, dc);
    }
}

/// EDIT 只能用系统选中色（蓝）画选区；原过程画完后，按字符位置用主题选区色重画仍可见的选区，
/// 选择与复制行为不变。选区只在拥有焦点时可见（未设 ES_NOHIDESEL）。
fn repaint_selection(hwnd: HWND) {
    // SAFETY: 只读本线程焦点。
    if unsafe { GetFocus() } != hwnd || high_contrast() {
        return;
    }
    // SAFETY: 取本窗口 DC 并在结束时归还；先隐藏插入符，避免异或插入符在选区色上留下残影。
    unsafe {
        let dc = GetDC(hwnd);
        if dc.is_null() {
            return;
        }
        HideCaret(hwnd);
        paint_selection(hwnd, dc);
        ShowCaret(hwnd);
        ReleaseDC(hwnd, dc);
    }
}
fn paint_selection(hwnd: HWND, dc: HDC) {
    // SAFETY: 只向本 EDIT 同步发送查询消息，输出参数均为本栈上的值。
    let send = |message, w: WPARAM, l: LPARAM| unsafe { SendMessageW(hwnd, message, w, l) };
    let (mut start, mut end) = (0u32, 0u32);
    send(
        EM_GETSEL,
        &mut start as *mut u32 as WPARAM,
        &mut end as *mut u32 as LPARAM,
    );
    if start >= end {
        return;
    }
    let c = colors();
    let mut format = RECT::default();
    send(EM_GETRECT, 0, &mut format as *mut RECT as LPARAM);
    let first = send(EM_LINEFROMCHAR, start as usize, 0).max(0) as usize;
    let last = send(EM_LINEFROMCHAR, end as usize, 0).max(0) as usize;
    // SAFETY: 选入本控件字体测量与绘制，结束前还原；文字缓冲区在调用期间存活。
    unsafe {
        let old = SelectObject(dc, send(WM_GETFONT, 0, 0) as HFONT);
        let mut metrics: TEXTMETRICW = std::mem::zeroed();
        GetTextMetricsW(dc, &mut metrics);
        SetTextColor(dc, c.text);
        SetBkColor(dc, c.selection);
        // 编辑框都是单行短文本；上限只防异常输入拖慢绘制。
        for line in first..=last.min(first + 4096) {
            let line_start = send(EM_LINEINDEX, line, 0);
            if line_start < 0 {
                break;
            }
            let line_start = line_start as u32;
            let length = send(EM_LINELENGTH, line_start as usize, 0).max(0) as u32;
            let (from, to) = (start.max(line_start), end.min(line_start + length));
            if from >= to {
                continue;
            }
            let mut buffer = [0u16; 1024];
            buffer[0] = buffer.len() as u16;
            let copied = send(EM_GETLINE, line, buffer.as_mut_ptr() as LPARAM).max(0) as usize;
            let range = (from - line_start) as usize..((to - line_start) as usize).min(copied);
            let Some(segment) = buffer.get(range).filter(|s| !s.is_empty()) else {
                continue;
            };
            let position = send(EM_POSFROMCHAR, from as usize, 0);
            if position == -1 {
                continue;
            }
            let x = (position & 0xffff) as i16 as i32;
            let y = ((position >> 16) & 0xffff) as i16 as i32;
            let mut size = SIZE::default();
            GetTextExtentPoint32W(dc, segment.as_ptr(), segment.len() as i32, &mut size);
            let cell = RECT {
                left: x.max(format.left),
                top: y.max(format.top),
                right: (x + size.cx).min(format.right),
                bottom: (y + metrics.tmHeight).min(format.bottom),
            };
            if cell.left < cell.right && cell.top < cell.bottom {
                ExtTextOutW(
                    dc,
                    x,
                    y,
                    ETO_OPAQUE | ETO_CLIPPED,
                    &cell,
                    segment.as_ptr(),
                    segment.len() as u32,
                    null(),
                );
            }
        }
        SelectObject(dc, old);
    }
}

// SAFETY: 系统在所属 UI 线程调用；WM_PAINT 完全自绘，其余消息先交 BUTTON 原过程，
// 原过程会在状态变化时直接画经典外观，随后立即按主题覆盖；不保存指针、不分配。
unsafe extern "system" fn button_procedure(
    hwnd: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    match message {
        WM_ERASEBKGND => return 1,
        WM_PAINT => {
            // SAFETY: WM_PAINT 内配对 BeginPaint/EndPaint。
            unsafe {
                let mut paint: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(hwnd, &mut paint);
                draw_button(hwnd, dc);
                EndPaint(hwnd, &paint);
            }
            return 0;
        }
        _ => {}
    }
    // SAFETY: 参数原样交给 BUTTON 类的原窗口过程。
    let result = unsafe { CallWindowProcW(class_procedure(hwnd), hwnd, message, w, l) };
    if matches!(
        message,
        BM_SETCHECK
            | BM_SETSTATE
            | BM_CLICK
            | WM_SETFOCUS
            | WM_KILLFOCUS
            | WM_ENABLE
            | WM_SETTEXT
            | WM_UPDATEUISTATE
            | WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_LBUTTONDBLCLK
            | WM_KEYDOWN
            | WM_KEYUP
            | WM_CAPTURECHANGED
    ) {
        // SAFETY: 取本控件 DC 覆盖原过程直接画出的经典外观，结束时归还。
        unsafe {
            let dc = GetDC(hwnd);
            if !dc.is_null() {
                draw_button(hwnd, dc);
                ReleaseDC(hwnd, dc);
            }
        }
    }
    result
}

fn is_checked(control: HWND) -> bool {
    // SAFETY: 同步查询本线程按钮的勾选状态；空句柄返回 0。
    unsafe { SendMessageW(control, BM_GETCHECK, 0, 0) == BST_CHECKED as isize }
}
fn checked_in(parent: HWND, id: usize) -> bool {
    // SAFETY: 只在本窗口内按编号找控件。
    is_checked(unsafe { GetDlgItem(parent, id as i32) })
}
/// 一组分段单选中被选中的序号。
fn pick(parent: HWND, first: usize, count: usize) -> Option<usize> {
    (0..count).find(|k| checked_in(parent, first + k))
}
fn page_size_in(parent: HWND) -> u8 {
    pick(parent, SIZE_CHOICE, 7).map_or(Options::default().page_size, |k| k as u8 + 3)
}
/// 读一个十六进制色值框：可带或不带“#”，须恰为 6 位。
fn hex_in(parent: HWND, k: usize) -> Result<u32, String> {
    let mut buffer = [0u16; 64];
    // SAFETY: 只在本窗口内按编号找控件。
    let length = window_text(
        unsafe { GetDlgItem(parent, (CUSTOM + k) as i32) },
        &mut buffer,
    );
    let text = String::from_utf16_lossy(&buffer[..length]);
    let hex = text.trim().trim_start_matches('#');
    u32::from_str_radix(hex, 16)
        .ok()
        .filter(|_| hex.len() == 6)
        .ok_or_else(|| format!("{}色“{text}”须为 6 位十六进制，如 #F9F8F5", COLORS[k].0))
}
fn custom_in(parent: HWND) -> Result<CustomColors, String> {
    Ok(CustomColors {
        window: hex_in(parent, 0)?,
        text: hex_in(parent, 1)?,
        highlight: hex_in(parent, 2)?,
        accent: hex_in(parent, 3)?,
    })
}
/// 设置窗控件即草稿：读出并在本地校验，偏蓝、不可读或格式不对的不发往后台。
fn read_options(parent: HWND) -> Result<Options, String> {
    let choice = |first: usize, count: usize, name: &str| {
        pick(parent, first, count).ok_or_else(|| format!("必须选择{name}"))
    };
    let options = Options {
        auto_commit_four: checked_in(parent, AUTO),
        common_only: checked_in(parent, COMMON),
        show_learning: checked_in(parent, LEARNING),
        output_form: FORMS[choice(FORM, FORMS.len(), "输出字形")?].0,
        candidate_font: CandidateFont::ALL[choice(FONT, CandidateFont::ALL.len(), "候选字体")?],
        theme: Theme::ALL[choice(THEME, Theme::ALL.len(), "配色主题")?],
        typing_mode: TypingMode::ALL[choice(MODE, TypingMode::ALL.len(), "输入模式")?],
        page_size: choice(SIZE_CHOICE, 7, "每页候选数")? as u8 + 3,
        custom: custom_in(parent)?,
    };
    options.validate()?;
    Ok(options)
}
/// 自定义配色行下方的提示：可读时报最低对比度，否则说明原因。
fn custom_status(parent: HWND) -> String {
    let check = custom_in(parent).and_then(|custom| {
        let options = Options {
            theme: Theme::Custom,
            custom,
            ..Options::default()
        };
        options
            .validate()
            .map(|()| options.palette().min_contrast())
    });
    match check {
        Ok(contrast) => format!("✓ 全部文字可读，最低对比度 {contrast:.1} : 1"),
        Err(error) => format!("✗ {error}；修正后自动保存"),
    }
}
/// 主题卡片与预览所用配色：预设取固定值，自定义取输入框里的草稿，草稿无效时退回当前配色。
fn theme_palette(parent: HWND, k: usize) -> Palette {
    match Theme::ALL[k].preset() {
        Some(palette) => palette,
        None => custom_in(parent).map_or_else(|_| PALETTE.with(Cell::get), CustomColors::palette),
    }
}

/// 按钮外观：开关行（复选框）、侧栏分区、主题卡片、分段单选、色块行与普通按钮。
fn draw_button(hwnd: HWND, dc: HDC) {
    let c = colors();
    let s = |value| scale(hwnd, value);
    let mut rect = RECT::default();
    let mut buffer = [0u16; 64];
    let length = window_text(hwnd, &mut buffer);
    let text = &buffer[..length];
    // SAFETY: 只读本控件的样式、状态与焦点。
    let (kind, state, id, enabled, focused) = unsafe {
        GetClientRect(hwnd, &mut rect);
        (
            GetWindowLongW(hwnd, GWL_STYLE) & BS_TYPEMASK,
            SendMessageW(hwnd, BM_GETSTATE, 0, 0) as u32,
            GetDlgCtrlID(hwnd) as usize,
            IsWindowEnabled(hwnd) != 0,
            GetFocus() == hwnd,
        )
    };
    let (show_focus, prefix) = cues(hwnd);
    let focused = focused && show_focus;
    let (checked, pushed) = (state & BST_CHECKED != 0, state & BST_PUSHED != 0);
    let font = font_of(hwnd);
    let color = |on: COLORREF| if enabled { on } else { c.faint };
    let small = sibling_font(hwnd, NOTICE);
    match kind {
        BS_AUTOCHECKBOX => {
            fill(dc, &rect, c.surface);
            let description = TOGGLES
                .iter()
                .find(|(key, ..)| *key == id)
                .map_or("", |(.., description)| *description);
            let title = RECT {
                left: rect.left + s(15),
                top: rect.top + s(9),
                right: rect.right - s(80),
                bottom: rect.top + s(31),
            };
            draw_text(dc, font, color(c.text), text, title, prefix);
            let line = RECT {
                top: rect.top + s(32),
                bottom: rect.top + s(50),
                ..title
            };
            draw_text(dc, small, c.muted, &units(description), line, DT_NOPREFIX);
            let top = (rect.top + rect.bottom - s(22)) / 2;
            let track = RECT {
                left: rect.right - s(15 + 40),
                top,
                right: rect.right - s(15),
                bottom: top + s(22),
            };
            if checked {
                round(dc, &track, s(11), Some(color(c.accent)), c.accent);
                let knob = RECT {
                    left: track.right - s(18),
                    top: top + s(4),
                    right: track.right - s(4),
                    bottom: top + s(18),
                };
                round(dc, &knob, s(7), Some(c.on_accent), c.on_accent);
            } else {
                round(dc, &track, s(11), None, color(c.field));
                round(dc, &inset(&track, 1), s(10), None, color(c.field));
                let knob = RECT {
                    left: track.left + s(5),
                    top: top + s(6),
                    right: track.left + s(15),
                    bottom: top + s(16),
                };
                round(dc, &knob, s(5), Some(color(c.muted)), c.muted);
            }
            if focused {
                ring(dc, &inset(&track, -s(3)), s(14), c.text);
            }
        }
        BS_AUTORADIOBUTTON if (NAV..NAV + PAGES.len()).contains(&id) => {
            fill(dc, &rect, c.sidebar);
            if checked {
                round(dc, &rect, s(6), Some(c.nav), c.nav);
            }
            let icon = RECT {
                left: rect.left + s(10),
                right: rect.left + s(30),
                ..rect
            };
            let tint = if checked { c.accent } else { c.muted };
            draw_icon(dc, PAGES[id - NAV].1, 16, tint, icon);
            let label = RECT {
                left: rect.left + s(38),
                ..rect
            };
            draw_text(dc, font, c.text, text, label, prefix);
            if focused {
                ring(dc, &rect, s(6), c.text);
            }
        }
        BS_AUTORADIOBUTTON if (THEME..THEME + Theme::ALL.len()).contains(&id) => {
            fill(dc, &rect, c.surface);
            let p = theme_palette(parent(hwnd), id - THEME);
            ring(dc, &rect, s(8), if checked { c.accent } else { c.line });
            let mini = RECT {
                left: rect.left + s(8),
                top: rect.top + s(8),
                right: rect.right - s(8),
                bottom: rect.top + s(52),
            };
            round(dc, &mini, s(5), Some(rgb(p.window)), rgb(p.window));
            round(dc, &mini, s(5), None, rgb(p.border));
            let glyphs = TempFont::new(dc, &[font_face(CandidateFont::WenKai), UI_FACE], 12, 400);
            for (row, (label, ink)) in [("1 拉", p.chosen), ("2 隹", p.text)]
                .into_iter()
                .enumerate()
            {
                let band = RECT {
                    left: mini.left + s(4),
                    top: mini.top + s(4) + row as i32 * s(19),
                    right: mini.right - s(4),
                    bottom: mini.top + s(21) + row as i32 * s(19),
                };
                if row == 0 && p.highlight != p.window {
                    round(dc, &band, s(3), Some(rgb(p.highlight)), rgb(p.highlight));
                }
                let words = RECT {
                    left: band.left + s(5),
                    ..band
                };
                draw_text(dc, glyphs.0, rgb(ink), &units(label), words, DT_NOPREFIX);
            }
            let name = RECT {
                top: mini.bottom + s(2),
                bottom: rect.bottom - s(3),
                ..rect
            };
            draw_text(dc, font, c.text, text, name, DT_CENTER | prefix);
            if focused {
                ring(dc, &inset(&rect, s(3)), s(6), c.text);
            }
        }
        BS_AUTORADIOBUTTON => {
            // 轨道由父窗口画；选中段为卡片色小块加细边，不用整块强调色。
            fill(dc, &rect, c.track);
            if checked {
                round(dc, &rect, s(5), Some(c.surface), c.surface);
                round(dc, &rect, s(5), None, c.line);
            }
            let ink = color(if checked { c.text } else { c.muted });
            let face = (FONT..FONT + CandidateFont::ALL.len())
                .contains(&id)
                .then(|| {
                    TempFont::new(
                        dc,
                        &[font_face(CandidateFont::ALL[id - FONT]), UI_FACE],
                        15,
                        400,
                    )
                });
            let font = face.as_ref().map_or(font, |face| face.0);
            draw_text(dc, font, ink, text, rect, DT_CENTER | prefix);
            if focused {
                ring(dc, &rect, s(5), c.text);
            }
        }
        _ if (SWATCH..SWATCH + COLORS.len()).contains(&id) => {
            let k = id - SWATCH;
            fill(dc, &rect, c.surface);
            let value = hex_in(parent(hwnd), k).unwrap_or(match k {
                0 => PALETTE.with(Cell::get).window,
                1 => PALETTE.with(Cell::get).text,
                2 => PALETTE.with(Cell::get).highlight,
                _ => PALETTE.with(Cell::get).accent,
            });
            let chip = RECT {
                left: rect.left,
                top: rect.top + s(5),
                right: rect.left + s(30),
                bottom: rect.top + s(35),
            };
            round(dc, &chip, s(7), Some(rgb(value)), rgb(value));
            round(dc, &chip, s(7), None, c.field);
            let title = RECT {
                left: chip.right + s(12),
                top: rect.top + s(2),
                right: rect.right,
                bottom: rect.top + s(21),
            };
            draw_text(dc, font, c.text, text, title, prefix);
            let line = RECT {
                top: rect.top + s(21),
                bottom: rect.top + s(38),
                ..title
            };
            draw_text(dc, small, c.muted, &units(COLORS[k].1), line, DT_NOPREFIX);
            if focused {
                ring(dc, &inset(&chip, -s(3)), s(9), c.text);
            }
        }
        _ => {
            let flat = matches!(id, PREVIOUS | NEXT);
            let back = if on_surface(id) { c.surface } else { c.window };
            fill(dc, &rect, back);
            if pushed {
                round(dc, &rect, s(6), Some(c.track), c.track);
            } else if !flat {
                round(dc, &rect, s(6), Some(c.surface), c.surface);
            }
            if !flat {
                round(dc, &rect, s(6), None, c.field);
            }
            let ink = color(c.text);
            let icon = match id {
                SETTINGS => Some(0xE713),
                PREVIOUS => Some(0xE76B),
                NEXT => Some(0xE76C),
                COPY => Some(0xE8C8),
                RELOAD => Some(0xE72C),
                _ => None,
            };
            match icon {
                // 只有图标的按钮：文字留作读屏名称与助记键，不画出来。
                Some(code) if matches!(id, SETTINGS | PREVIOUS | NEXT) => {
                    draw_icon(dc, code, 16, ink, rect);
                }
                Some(code) => {
                    let label = &text[..text
                        .iter()
                        .position(|&unit| unit == u16::from(b'('))
                        .unwrap_or(text.len())];
                    let width = s(14) + s(6) + extent(dc, font, label);
                    let left = (rect.left + rect.right - width) / 2;
                    let glyph = RECT {
                        left,
                        right: left + s(14),
                        ..rect
                    };
                    draw_icon(dc, code, 13, ink, glyph);
                    let words = RECT {
                        left: glyph.right + s(6),
                        ..rect
                    };
                    draw_text(dc, font, ink, label, words, DT_NOPREFIX);
                }
                None => draw_text(dc, font, ink, text, rect, DT_CENTER | prefix),
            }
            if focused {
                ring(dc, &rect, s(6), c.text);
            }
        }
    }
}

/// 自绘静态控件与字块列表：字形格、预览、快捷键格、编码键帽、拆分字块、查字字块。
fn draw_item(item: &DRAWITEMSTRUCT) {
    let c = colors();
    let s = |value| scale(item.hwndItem, value);
    let rect = item.rcItem;
    let dc = item.hDC;
    if item.CtlType == ODT_LISTBOX {
        draw_chip(item, &c);
        return;
    }
    let id = item.CtlID as usize;
    match id {
        PREVIEW => return draw_preview(item, &c),
        _ if (KEYS..KEYS + KEYMAP.len()).contains(&id) => return draw_keys(item, &c, id - KEYS),
        _ => {}
    }
    let mut buffer = [0u16; 64];
    let length = window_text(item.hwndItem, &mut buffer);
    let text = &buffer[..length];
    fill(dc, &rect, c.surface);
    if id == GLYPH {
        round(dc, &rect, s(8), None, c.line);
        // SAFETY: 新建虚线画笔用后即删；直线端点为本地值。
        unsafe {
            let pen = CreatePen(PS_DOT, 1, c.line);
            let old = SelectObject(dc, pen);
            let (x, y) = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
            MoveToEx(dc, x, rect.top + s(8), null_mut());
            LineTo(dc, x, rect.bottom - s(8));
            MoveToEx(dc, rect.left + s(8), y, null_mut());
            LineTo(dc, rect.right - s(8), y);
            SelectObject(dc, old);
            DeleteObject(pen);
        }
        // 词组放不进字形格，改用字块字号。
        let single = String::from_utf16_lossy(text).chars().count() <= 1;
        let font = if single {
            font_of(item.hwndItem)
        } else {
            sibling_font(item.hwndItem, LIST)
        };
        draw_text(dc, font, c.text, text, rect, DT_CENTER | DT_NOPREFIX);
        return;
    }
    let missing = String::from_utf16_lossy(text) == "缺资料";
    if missing || text.is_empty() {
        draw_text(
            dc,
            sibling_font(item.hwndItem, FIELD_LABEL),
            c.muted,
            text,
            rect,
            DT_NOPREFIX,
        );
        return;
    }
    let font = font_of(item.hwndItem);
    let mut left = rect.left;
    if id == PARTS {
        // 拆分字根逐个放进字块，字根可能是扩展区字符，按 Unicode 标量切分。
        for part in String::from_utf16_lossy(text).chars() {
            let tile = RECT {
                left,
                top: rect.top,
                right: left + s(30),
                bottom: rect.top + s(30),
            };
            if tile.right > rect.right {
                break;
            }
            round(dc, &tile, s(6), None, c.line);
            let mut code = [0u16; 2];
            draw_text(
                dc,
                font,
                c.text,
                part.encode_utf16(&mut code),
                tile,
                DT_CENTER | DT_NOPREFIX,
            );
            left = tile.right + s(6);
        }
        return;
    }
    // 编码以空格分隔，各成一枚键帽。
    for code in text
        .split(|&unit| unit == u16::from(b' '))
        .filter(|code| !code.is_empty())
    {
        let chip = RECT {
            left,
            top: rect.top + (rect.bottom - rect.top - s(26)) / 2,
            right: left + extent(dc, font, code) + s(18),
            bottom: rect.top + (rect.bottom - rect.top + s(26)) / 2,
        };
        if chip.right > rect.right {
            break;
        }
        round(dc, &chip, s(5), Some(c.track), c.track);
        draw_text(dc, font, c.text, code, chip, DT_CENTER | DT_NOPREFIX);
        left = chip.right + s(6);
    }
}

/// 查字字块：选中为浅底加强调色描边；键盘焦点另画正文色环。
fn draw_chip(item: &DRAWITEMSTRUCT, c: &Colors) {
    let s = |value| scale(item.hwndItem, value);
    let rect = item.rcItem;
    let dc = item.hDC;
    fill(dc, &rect, c.window);
    if item.itemID == u32::MAX {
        return;
    }
    let mut buffer = [0u16; 64];
    // SAFETY: 先核对长度不超过缓冲区，再取本控件自己的字符串。
    let length = unsafe {
        let length = SendMessageW(item.hwndItem, LB_GETTEXTLEN, item.itemID as usize, 0);
        if !(0..buffer.len() as isize).contains(&length)
            || SendMessageW(
                item.hwndItem,
                LB_GETTEXT,
                item.itemID as usize,
                buffer.as_mut_ptr() as LPARAM,
            ) < 0
        {
            return;
        }
        length as usize
    };
    let chip = RECT {
        left: rect.left + s(3),
        top: rect.top + s(2),
        right: rect.right - s(3),
        bottom: rect.bottom - s(2),
    };
    let chosen = item.itemState & ODS_SELECTED != 0;
    if chosen {
        round(dc, &chip, s(7), Some(c.nav), c.nav);
        ring(dc, &chip, s(7), c.accent);
    }
    let disabled = item.itemState & ODS_DISABLED != 0;
    draw_text(
        dc,
        font_of(item.hwndItem),
        if disabled { c.faint } else { c.text },
        &buffer[..length],
        chip,
        DT_CENTER | DT_NOPREFIX,
    );
    if item.itemState & ODS_FOCUS != 0 && item.itemState & ODS_NOFOCUSRECT == 0 && !chosen {
        ring(dc, &chip, s(7), c.text);
    }
}

/// 外观区预览：按设置窗草稿现画一段带组合的文字和候选窗；草稿无效时用当前配色。
fn draw_preview(item: &DRAWITEMSTRUCT, c: &Colors) {
    let s = |value| scale(item.hwndItem, value);
    let (dc, rect) = (item.hDC, item.rcItem);
    let parent = parent(item.hwndItem);
    let draft = read_options(parent).ok();
    let palette = draft.map_or_else(|| PALETTE.with(Cell::get), |o| o.palette());
    let face = draft.map_or(CandidateFont::default(), |o| o.candidate_font);
    let learning = checked_in(parent, LEARNING);
    fill(dc, &rect, c.window);
    round(dc, &rect, s(8), Some(c.sidebar), c.sidebar);
    round(dc, &rect, s(8), None, c.line);
    let body = TempFont::new(dc, &[UI_FACE], 15, 400);
    let mono = TempFont::new(dc, &MONO_FACES, 12, 400);
    let glyphs = TempFont::new(dc, &[font_face(face), UI_FACE], 19, 400);
    let lead = units("明天我们一起去");
    let line = RECT {
        left: rect.left + s(20),
        top: rect.top + s(14),
        right: rect.right - s(20),
        bottom: rect.top + s(36),
    };
    draw_text(dc, body.0, c.text, &lead, line, DT_NOPREFIX);
    let start = line.left + extent(dc, body.0, &lead);
    let code = units("ui");
    let composition = RECT {
        left: start,
        ..line
    };
    draw_text(dc, body.0, c.text, &code, composition, DT_NOPREFIX);
    let end = start + extent(dc, body.0, &code);
    // 组合下的点状下划线与插入符。
    let mut x = start;
    while x < end {
        let dot = RECT {
            left: x,
            top: line.bottom - s(2),
            right: x + s(1).max(1),
            bottom: line.bottom - s(1),
        };
        fill(dc, &dot, c.text);
        x += s(3);
    }
    let caret = RECT {
        left: end + s(1),
        top: line.top + s(2),
        right: end + s(1) + 1,
        bottom: line.bottom - s(2),
    };
    fill(dc, &caret, c.text);
    let rows = [
        ("1", "拉", "〔扌立 · uil〕"),
        ("2", "隹", "〔隹 · ui / uii〕"),
    ];
    let label_w = extent(dc, mono.0, &units("1"));
    let text_w = extent(dc, glyphs.0, &units("隹"));
    let comment_w = if learning {
        rows.iter()
            .map(|(.., comment)| extent(dc, mono.0, &units(comment)))
            .max()
            .unwrap_or(0)
            + s(8)
    } else {
        0
    };
    let window = RECT {
        left: start,
        top: line.bottom + s(6),
        right: (start + s(4 + 8) + label_w + s(8) + text_w + comment_w + s(14))
            .max(start + s(132))
            .min(rect.right - s(12)),
        bottom: line.bottom + s(6 + 4 + 31 * 2 + 3),
    };
    round(
        dc,
        &window,
        s(8),
        Some(rgb(palette.window)),
        rgb(palette.window),
    );
    round(dc, &window, s(8), None, rgb(palette.border));
    for (index, (label, text, comment)) in rows.into_iter().enumerate() {
        let band = RECT {
            left: window.left + s(4),
            top: window.top + s(4) + index as i32 * s(31),
            right: window.right - s(4),
            bottom: window.top + s(34) + index as i32 * s(31),
        };
        let chosen = index == 0;
        if chosen && palette.highlight != palette.window {
            round(
                dc,
                &band,
                s(4),
                Some(rgb(palette.highlight)),
                rgb(palette.highlight),
            );
        }
        let (label_ink, text_ink, hint_ink) = if chosen {
            (
                palette.highlight_text,
                palette.chosen,
                palette.highlight_text,
            )
        } else {
            (palette.label, palette.text, palette.muted)
        };
        let mut left = band.left + s(8);
        for (font, ink, words) in [
            (mono.0, label_ink, label),
            (glyphs.0, text_ink, text),
            (mono.0, hint_ink, if learning { comment } else { "" }),
        ] {
            let words = units(words);
            let cell = RECT { left, ..band };
            draw_text(dc, font, rgb(ink), &words, cell, DT_NOPREFIX);
            left += extent(dc, font, &words) + s(8);
        }
    }
}

/// 快捷键格：上为说明，下为键帽；“N”换成当前每页候选数。
fn draw_keys(item: &DRAWITEMSTRUCT, c: &Colors, index: usize) {
    let s = |value| scale(item.hwndItem, value);
    let (dc, rect) = (item.hDC, item.rcItem);
    fill(dc, &rect, c.surface);
    let (label, spec) = KEYMAP[index];
    let title = RECT {
        left: rect.left + s(15),
        top: rect.top + s(8),
        right: rect.right - s(8),
        bottom: rect.top + s(28),
    };
    draw_text(
        dc,
        font_of(item.hwndItem),
        c.text,
        &units(label),
        title,
        DT_NOPREFIX,
    );
    let size = page_size_in(parent(item.hwndItem)).to_string();
    let spec = spec.replace("[N]", &format!("[{size}]"));
    let mono = sibling_font(item.hwndItem, CUSTOM);
    let small = sibling_font(item.hwndItem, NOTICE);
    let mut left = title.left;
    let (top, bottom) = (rect.top + s(32), rect.top + s(54));
    for part in spec
        .split_inclusive(']')
        .flat_map(|part| match part.find('[') {
            Some(at) => vec![&part[..at], &part[at..]],
            None => vec![part],
        })
    {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(key) = part.strip_prefix('[').and_then(|key| key.strip_suffix(']')) {
            let key = units(key);
            let cap = RECT {
                left,
                top,
                right: left + (extent(dc, mono, &key) + s(12)).max(s(22)),
                bottom,
            };
            round(dc, &cap, s(5), Some(c.window), c.window);
            round(dc, &cap, s(5), None, c.field);
            draw_text(dc, mono, c.text, &key, cap, DT_CENTER | DT_NOPREFIX);
            left = cap.right + s(5);
        } else {
            let words = units(part);
            let cell = RECT {
                left,
                top,
                right: rect.right,
                bottom,
            };
            draw_text(dc, small, c.muted, &words, cell, DT_NOPREFIX);
            left += extent(dc, small, &words) + s(5);
        }
    }
}

/// 色板菜单项：圆角色块，悬停时画正文色环；底色同卡片，不用系统蓝色高亮。
fn draw_swatch(item: &DRAWITEMSTRUCT) {
    let c = colors();
    let dc = item.hDC;
    // SAFETY: 只读绘制 DC 的 DPI。
    let dpi = unsafe { GetDeviceCaps(dc, LOGPIXELSY as i32) }.max(96);
    let s = |value: i32| value * dpi / 96;
    let rect = item.rcItem;
    fill(dc, &rect, c.surface);
    let chip = inset(&rect, s(4));
    let value = rgb(item.itemData as u32);
    round(dc, &chip, s(5), Some(value), value);
    round(dc, &chip, s(5), None, c.line);
    if item.itemState & ODS_SELECTED != 0 {
        ring(dc, &inset(&rect, s(1)), s(7), c.text);
    }
}

type Box4 = [i32; 4];
/// 查字窗布局（96 DPI 逻辑坐标，按客户区尺寸现算）：控件位置与需由父窗口画的查询框、详情卡。
struct LookupSpec {
    controls: Vec<(usize, Box4)>,
    field: Box4,
    card: Box4,
}
fn lookup_spec(width: i32, height: i32) -> LookupSpec {
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
struct SettingsSpec {
    fixed: Vec<(usize, Box4)>,
    controls: Vec<(usize, Box4)>,
    cards: Vec<Box4>,
    lines: Vec<Box4>,
    tracks: Vec<Box4>,
    frames: Vec<Box4>,
    /// 配置文件图标底与“关于”印章的位置。
    tile: Option<Box4>,
    seal: Option<Box4>,
    height: i32,
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
fn settings_spec(page: usize, custom: bool) -> SettingsSpec {
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
/// 当前分区：侧栏单选中被选中的一项。
fn current_page(hwnd: HWND) -> usize {
    pick(hwnd, NAV, PAGES.len()).unwrap_or(0)
}
/// 设置窗滚动位置（96 DPI 逻辑像素），存于窗口数据：值类型，不是指针。
fn scroll_offset(hwnd: HWND) -> i32 {
    // SAFETY: 只读本线程窗口的用户数据整数。
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as i32 }
}
fn set_scroll_offset(hwnd: HWND, offset: i32) {
    // SAFETY: 只写本线程窗口的用户数据整数。
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, offset as _) };
}
/// 滚动到 position（逻辑像素）：按滚动条记录的范围夹紧，改了才重新布局。
fn scroll_to(hwnd: HWND, position: i32) {
    let mut info = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL,
        ..Default::default()
    };
    // SAFETY: 输出结构大小已填，只读本窗口滚动条。
    if unsafe { GetScrollInfo(hwnd, SB_VERT, &mut info) } == 0 {
        return;
    }
    let max = (info.nMax - info.nPage as i32 + 1).max(0);
    let position = position.clamp(0, max);
    if position != scroll_offset(hwnd) {
        set_scroll_offset(hwnd, position);
        post(hwnd, LAYOUT, 0);
    }
}

/// 卡片内的控件用卡片底色，说明类文字用次要色。
fn on_surface(id: usize) -> bool {
    matches!(
        id,
        QUERY | GLYPH | CODEPOINT | COPY | STATUS | CODES
            ..=PARTS
                | OPEN_CONFIG
                | RELOAD
                | RESET
                | CUSTOM_STATUS
                | FILE_NAME
                | FILE_PATH
                | ABOUT_TITLE
                | ABOUT_DESC
    ) || (FIELD_LABEL..FIELD_LABEL + 4).contains(&id)
        || (ROW_TITLE..ROW_TITLE + 5).contains(&id)
        || (ROW_DESC..ROW_DESC + 5).contains(&id)
        || (CUSTOM..CUSTOM + 4).contains(&id)
}
fn muted(id: usize) -> bool {
    matches!(
        id,
        QUERY_LABEL
            | STATUS
            | PAGE
            | CODEPOINT
            | NOTICE
            | CUSTOM_STATUS
            | FILE_PATH
            | ABOUT_DESC
            | KEYS_NOTE
    ) || (FIELD_LABEL..FIELD_LABEL + 4).contains(&id)
        || (ROW_DESC..ROW_DESC + 5).contains(&id)
        || (GROUP..GROUP + GROUPS.len()).contains(&id)
}

/// 父窗口自绘底色、侧栏、卡片、轨道、输入框外框与分隔线；全部由客户区尺寸与控件状态现算。
fn paint_window(hwnd: HWND) {
    let c = colors();
    // SAFETY: WM_PAINT 内配对 BeginPaint/EndPaint；只查询本窗口子控件。
    unsafe {
        let mut paint: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(hwnd, &mut paint);
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client);
        fill(dc, &client, c.window);
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        let s = |value: i32| value * dpi / 96;
        let query = GetDlgItem(hwnd, QUERY as i32);
        if !query.is_null() {
            let r = |[x, y, w, h]: Box4| RECT {
                left: s(x),
                top: s(y),
                right: s(x + w),
                bottom: s(y + h),
            };
            let spec = lookup_spec(client.right * 96 / dpi, client.bottom * 96 / dpi);
            let card = r(spec.card);
            round(dc, &card, s(10), Some(c.surface), c.surface);
            round(dc, &card, s(10), None, c.line);
            let field = r(spec.field);
            round(dc, &field, s(8), Some(c.surface), c.surface);
            round(
                dc,
                &field,
                s(8),
                None,
                if GetFocus() == query {
                    c.accent
                } else {
                    c.field
                },
            );
            let icon = RECT {
                left: field.left + s(10),
                right: field.left + s(30),
                ..field
            };
            draw_icon(dc, 0xE721, 15, c.muted, icon);
            let mono = TempFont::new(dc, &MONO_FACES, 11, 400);
            let hint = units("Enter");
            let width = extent(dc, mono.0, &hint) + s(12);
            let cap = RECT {
                left: field.right - s(8) - width,
                top: field.top + s(9),
                right: field.right - s(8),
                bottom: field.bottom - s(9),
            };
            round(dc, &cap, s(4), Some(c.track), c.track);
            draw_text(dc, mono.0, c.muted, &hint, cap, DT_CENTER | DT_NOPREFIX);
        } else {
            let sidebar = RECT {
                right: s(SIDEBAR),
                ..client
            };
            fill(dc, &sidebar, c.sidebar);
            let offset = scroll_offset(hwnd);
            let r = |[x, y, w, h]: Box4| RECT {
                left: s(x),
                top: s(y - offset),
                right: s(x + w),
                bottom: s(y - offset + h),
            };
            let spec = settings_spec(current_page(hwnd), checked_in(hwnd, THEME + 4));
            for card in &spec.cards {
                round(dc, &r(*card), s(8), Some(c.surface), c.surface);
                round(dc, &r(*card), s(8), None, c.line);
            }
            for line in &spec.lines {
                let mut line = r(*line);
                line.right = line.right.max(line.left + 1);
                line.bottom = line.bottom.max(line.top + 1);
                fill(dc, &line, c.line);
            }
            for track in &spec.tracks {
                round(dc, &r(*track), s(7), Some(c.track), c.track);
            }
            let focus = GetFocus();
            for (k, frame) in spec.frames.iter().enumerate() {
                let edit = GetDlgItem(hwnd, (CUSTOM + k) as i32);
                round(dc, &r(*frame), s(6), Some(c.surface), c.surface);
                round(
                    dc,
                    &r(*frame),
                    s(6),
                    None,
                    if focus == edit { c.accent } else { c.field },
                );
            }
            if let Some(tile) = spec.tile {
                let tile = r(tile);
                round(dc, &tile, s(8), Some(c.track), c.track);
                draw_icon(dc, 0xE8A5, 18, c.accent, tile);
            }
            if let Some(place) = spec.seal {
                let place = r(place);
                let side = place.right - place.left;
                if let Some(icon) = seal::icon(side) {
                    DrawIconEx(
                        dc,
                        place.left,
                        place.top,
                        icon,
                        side,
                        side,
                        0,
                        null_mut(),
                        DI_NORMAL,
                    );
                    DestroyIcon(icon);
                }
            }
        }
        EndPaint(hwnd, &paint);
    }
}

// SAFETY: Windows 在所属 UI 线程调用；不解引用业务指针、不使用业务状态，不执行可能 panic 的 Rust 操作。
unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: 只转发值类型通知；WM_DPICHANGED 的 RECT、自绘结构仅在本次系统回调内读取，绝不排队裸指针。
    unsafe {
        match message {
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                paint_window(hwnd);
                0
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
                let c = colors();
                let id = GetDlgCtrlID(l as HWND) as usize;
                let back = if (NAV..NAV + PAGES.len()).contains(&id) || id == NOTICE {
                    c.sidebar
                } else if on_surface(id) {
                    c.surface
                } else {
                    c.window
                };
                let dc = w as HDC;
                let fore = if (GROUP..GROUP + GROUPS.len()).contains(&id) {
                    c.label
                } else if muted(id) {
                    c.muted
                } else {
                    c.text
                };
                SetTextColor(dc, fore);
                SetBkColor(dc, back);
                SetDCBrushColor(dc, back);
                GetStockObject(DC_BRUSH) as LRESULT
            }
            WM_MEASUREITEM if l != 0 => {
                let item = &mut *(l as *mut MEASUREITEMSTRUCT);
                let dpi = GetDpiForWindow(hwnd).max(96);
                match item.CtlType {
                    ODT_LISTBOX => item.itemHeight = ITEM_HEIGHT * dpi / 96,
                    ODT_MENU => {
                        item.itemWidth = 30 * dpi / 96;
                        item.itemHeight = 30 * dpi / 96;
                    }
                    _ => return DefWindowProcW(hwnd, message, w, l),
                }
                1
            }
            WM_DRAWITEM if l != 0 => {
                let item = &*(l as *const DRAWITEMSTRUCT);
                match item.CtlType {
                    ODT_LISTBOX | ODT_STATIC => draw_item(item),
                    ODT_MENU => draw_swatch(item),
                    _ => return DefWindowProcW(hwnd, message, w, l),
                }
                1
            }
            WM_COMMAND => {
                post(hwnd, ACTION, w);
                0
            }
            WM_CLOSE => {
                post(hwnd, CLOSE, 0);
                0
            }
            WM_ENTERMENULOOP | WM_ENTERSIZEMOVE => {
                let _ = modal_enter(hwnd);
                0
            }
            WM_EXITMENULOOP => {
                modal_exit();
                0
            }
            UNBLOCK => {
                unblock(hwnd);
                0
            }
            // 设置窗内容超出时纵向滚动；滚动条与滚轮都只改滚动位置，再交给事件循环重新布局。
            WM_VSCROLL => {
                let mut info = SCROLLINFO {
                    cbSize: size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_ALL,
                    ..Default::default()
                };
                GetScrollInfo(hwnd, SB_VERT, &mut info);
                let position = match (w & 0xFFFF) as i32 {
                    SB_LINEUP => info.nPos - 40,
                    SB_LINEDOWN => info.nPos + 40,
                    SB_PAGEUP => info.nPos - info.nPage as i32,
                    SB_PAGEDOWN => info.nPos + info.nPage as i32,
                    SB_THUMBTRACK | SB_THUMBPOSITION => info.nTrackPos,
                    SB_TOP => 0,
                    SB_BOTTOM => info.nMax,
                    _ => info.nPos,
                };
                scroll_to(hwnd, position);
                0
            }
            WM_MOUSEWHEEL => {
                let delta = (w >> 16 & 0xFFFF) as u16 as i16 as i32;
                scroll_to(hwnd, scroll_offset(hwnd) - delta * 48 / 120);
                0
            }
            // 拖动缩放期间系统模态循环会吞掉投递的布局消息，结束时补发一次。
            WM_SIZE | WM_EXITSIZEMOVE => {
                if message == WM_EXITSIZEMOVE {
                    modal_exit();
                }
                post(hwnd, LAYOUT, 0);
                0
            }
            WM_DPICHANGED => {
                if l != 0 {
                    let rect = &*(l as *const RECT);
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                post(hwnd, LAYOUT, 0);
                0
            }
            // 只限制可缩放的查字窗；固定尺寸的设置窗不受此下限撑宽。
            WM_GETMINMAXINFO => {
                if l != 0 && GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_THICKFRAME != 0 {
                    let info = &mut *(l as *mut MINMAXINFO);
                    let dpi = GetDpiForWindow(hwnd) as i32;
                    let (width, height) = LOOKUP_MIN;
                    info.ptMinTrackSize.x = (width + 16) * dpi / 96;
                    info.ptMinTrackSize.y = (height + 39) * dpi / 96;
                }
                0
            }
            _ => DefWindowProcW(hwnd, message, w, l),
        }
    }
}

/// 控件字体角色；字号为 96 DPI 像素，与设计稿一致。
#[derive(Clone, Copy)]
enum Role {
    Body,
    Small,
    Heading,
    Mono,
    List,
    Glyph,
    Title,
}
const ROLES: usize = 7;
fn role(id: usize) -> Role {
    match id {
        HEADING => Role::Heading,
        CODES | FULL | PAGE | CODEPOINT | FILE_PATH | CUSTOM..=253 => Role::Mono,
        LIST | PARTS => Role::List,
        GLYPH => Role::Glyph,
        FILE_NAME | ABOUT_TITLE => Role::Title,
        _ if (GROUP..GROUP + GROUPS.len()).contains(&id) => Role::Title,
        _ if muted(id) => Role::Small,
        _ => Role::Body,
    }
}

struct Window {
    handle: HWND,
    controls: Vec<(usize, HWND)>,
    fonts: [HFONT; ROLES],
    icons: [HICON; 2],
    dpi: u32,
    face: CandidateFont,
}
impl Window {
    fn create(class: &[u16], title: &str, owner: HWND, style: u32) -> Result<Self, String> {
        // 设置窗可在查字窗隐藏时单独打开，被拥有窗口默认无任务栏按钮，须显式要一个，免得被挡住后找不回。
        let extra = if owner.is_null() { 0 } else { WS_EX_APPWINDOW };
        // SAFETY: 字符串 NUL 终止；窗口归当前线程所有，没有传入业务指针。
        let handle = unsafe {
            CreateWindowExW(
                WS_EX_CONTROLPARENT | extra,
                class.as_ptr(),
                wide(title).as_ptr(),
                style | WS_CLIPCHILDREN,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                LOOKUP_SIZE.0,
                LOOKUP_SIZE.1,
                owner,
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            )
        };
        if handle.is_null() {
            return Err(error());
        }
        Ok(Self {
            handle,
            controls: Vec::new(),
            fonts: [null_mut(); ROLES],
            icons: [null_mut(); 2],
            dpi: 0,
            face: CandidateFont::default(),
        })
    }
    fn add(&mut self, id: usize, class: &str, title: &str, style: u32) -> Result<(), String> {
        // SAFETY: 父窗口仍有效；子控件由父窗口销毁，id 仅作整数控件标识。
        let handle = unsafe {
            CreateWindowExW(
                0,
                wide(class).as_ptr(),
                wide(title).as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                0,
                0,
                self.handle,
                id as _,
                GetModuleHandleW(null()),
                null(),
            )
        };
        if handle.is_null() {
            return Err(error());
        }
        let procedure: usize = match class {
            "EDIT" => edit_procedure as *const () as usize,
            "BUTTON" => button_procedure as *const () as usize,
            _ => 0,
        };
        if procedure != 0 {
            // SAFETY: 本线程拥有的控件，新过程以类登记的原过程处理其余消息；控件随父窗口销毁，无需还原。
            unsafe {
                SetWindowLongPtrW(handle, GWLP_WNDPROC, procedure as _);
            }
        }
        self.controls.push((id, handle));
        Ok(())
    }
    fn control(&self, id: usize) -> HWND {
        self.controls
            .iter()
            .find(|(key, _)| *key == id)
            .expect("已创建的控件")
            .1
    }
    fn text(&self, id: usize, text: &str) {
        // SAFETY: 控件归本视图持有，文本在同步调用期间存活；自绘静态控件改字后重画。
        unsafe {
            SetWindowTextW(self.control(id), wide(text).as_ptr());
            InvalidateRect(self.control(id), null(), 0);
        }
    }
    fn enable(&self, id: usize, enabled: bool) {
        // SAFETY: 控件只在创建和销毁之间由所属 UI 线程访问。
        unsafe {
            EnableWindow(self.control(id), enabled.into());
        }
    }
    fn show(&self, id: usize, visible: bool) {
        let control = self.control(id);
        // 看控件自身的可见样式：父窗口隐藏时 IsWindowVisible 恒为假，不能据此判断。
        // SAFETY: 本线程拥有的子控件；只在显隐确实变化时调用，避免闪烁。
        unsafe {
            if shown(control) != visible {
                ShowWindow(control, if visible { SW_SHOWNA } else { SW_HIDE });
            }
        }
    }
    fn focused_control(&self) -> Option<usize> {
        // SAFETY: 只查询当前 UI 线程焦点，返回控件编号而非跨线程裸句柄。
        let focus = unsafe { GetFocus() };
        self.controls
            .iter()
            .find(|(_, hwnd)| *hwnd == focus)
            .map(|(id, _)| *id)
    }
    fn restore_focus(&self, id: usize) {
        let control = self.control(id);
        // SAFETY: 两个窗口在整个消息循环内存活；只恢复本前台窗口因禁用而丢失的焦点。
        unsafe {
            let focus = GetFocus();
            if GetForegroundWindow() == self.handle
                && (focus.is_null() || focus == self.handle || focus == control)
                && IsWindowVisible(control) != 0
                && IsWindowEnabled(control) != 0
            {
                SetFocus(control);
            }
        }
    }
    fn send(&self, id: usize, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        // SAFETY: 本模块只发送已匹配控件类型的同步消息；字符串参数在调用方保持存活。
        unsafe { SendMessageW(self.control(id), message, w, l) }
    }
    fn place(&self, id: usize, [x, y, width, height]: Box4) {
        let scale = |value| value * self.dpi as i32 / 96;
        // SAFETY: 有效子控件；尺寸按当前窗口 DPI 转换，不传递业务状态。
        unsafe {
            MoveWindow(
                self.control(id),
                scale(x),
                scale(y),
                scale(width),
                scale(height),
                1,
            );
        }
    }
    /// 字体随 DPI 与候选字体变化重建，控件全部换上新字体后才释放旧字体。
    fn ensure_fonts(&mut self, face: CandidateFont) -> Result<(), String> {
        // SAFETY: 查询本线程拥有窗口的当前 DPI。
        let dpi = unsafe { GetDpiForWindow(self.handle) }.max(96);
        if self.dpi == dpi && self.face == face {
            return Ok(());
        }
        // SAFETY: 取本窗口 DC 只用于核对字体名，结束前归还。
        let dc = unsafe { GetDC(self.handle) };
        let mut fonts = [null_mut(); ROLES];
        let specs: [(&[&str], i32, i32); ROLES] = [
            (&[UI_FACE], 14, 400),
            (&[UI_FACE], 12, 400),
            (&[UI_FACE], 20, 600),
            (&MONO_FACES, 13, 400),
            (&[font_face(face), UI_FACE], 20, 400),
            (&[font_face(face), UI_FACE], 76, 400),
            (&[UI_FACE], 13, 600),
        ];
        let mut result = Ok(());
        for (slot, (faces, pixels, weight)) in fonts.iter_mut().zip(specs) {
            match create_font(dc, faces, pixels, dpi, weight) {
                Ok(font) => *slot = font,
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        // SAFETY: 归还上面取得的 DC。
        unsafe {
            ReleaseDC(self.handle, dc);
        }
        if let Err(error) = result {
            release(fonts);
            return Err(error);
        }
        for (id, control) in &self.controls {
            // SAFETY: 新字体在整个控件使用期保持存活，消息同步处理。
            unsafe {
                SendMessageW(*control, WM_SETFONT, fonts[role(*id) as usize] as usize, 1);
            }
        }
        release(std::mem::replace(&mut self.fonts, fonts));
        self.dpi = dpi;
        self.face = face;
        if let Some((_, list)) = self.controls.iter().find(|(id, _)| *id == LIST) {
            // SAFETY: 本线程拥有的列表框，值类型参数。
            unsafe {
                SendMessageW(
                    *list,
                    LB_SETITEMHEIGHT,
                    0,
                    (ITEM_HEIGHT * dpi / 96) as LPARAM,
                );
            }
        }
        Ok(())
    }
    fn layout(&mut self, face: CandidateFont) -> Result<(), String> {
        self.ensure_fonts(face)?;
        // EDIT 换字体会按字号重设左右边距；色值框与查询框统一清零，位置由外框决定。
        for (id, control) in &self.controls {
            if *id == QUERY || (CUSTOM..CUSTOM + 4).contains(id) {
                // SAFETY: 本线程拥有的 EDIT，值类型参数。
                unsafe {
                    SendMessageW(
                        *control,
                        EM_SETMARGINS,
                        (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
                        0,
                    );
                }
            }
        }
        let mut rect = RECT::default();
        // SAFETY: 输出 RECT 由栈分配且窗口有效。
        if unsafe { GetClientRect(self.handle, &mut rect) } == 0 {
            return Err(error());
        }
        let dpi = self.dpi as i32;
        if self.controls.iter().any(|(id, _)| *id == QUERY) {
            for (id, place) in lookup_spec(rect.right * 96 / dpi, rect.bottom * 96 / dpi).controls {
                self.place(id, place);
            }
        } else {
            self.layout_settings(rect.bottom * 96 / dpi);
        }
        // SAFETY: 重画本窗口及其子控件，使卡片与新位置一致。
        unsafe {
            RedrawWindow(
                self.handle,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_ALLCHILDREN,
            );
        }
        Ok(())
    }
    /// 设置窗：按当前分区与是否自定义配色排布，隐藏其余控件；内容高于窗口时更新滚动条并按滚动位置平移。
    fn layout_settings(&self, view: i32) {
        let spec = settings_spec(
            current_page(self.handle),
            checked_in(self.handle, THEME + 4),
        );
        let offset = scroll_offset(self.handle).clamp(0, (spec.height - view).max(0));
        set_scroll_offset(self.handle, offset);
        let info = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: spec.height - 1,
            nPage: view.max(0) as u32,
            nPos: offset,
            nTrackPos: 0,
        };
        // SAFETY: 结构大小已填，只设置本窗口滚动条；页长不小于范围时系统自动隐藏滚动条。
        unsafe { SetScrollInfo(self.handle, SB_VERT, &info, 1) };
        for (id, place) in &spec.fixed {
            self.place(*id, *place);
        }
        for (id, [x, y, width, height]) in &spec.controls {
            self.place(*id, [*x, y - offset, *width, *height]);
        }
        for (id, _) in &self.controls {
            let visible = spec
                .fixed
                .iter()
                .chain(&spec.controls)
                .any(|(key, _)| key == id);
            self.show(*id, visible);
        }
    }
    /// Win11 标题栏与边框改用主题色，不随系统强调色；Win10 不支持时保持系统默认。
    fn decorate(&self, palette: Palette, caption: u32) {
        if high_contrast() {
            return;
        }
        for (attribute, value) in [
            (DWMWA_CAPTION_COLOR, rgb(caption)),
            (DWMWA_BORDER_COLOR, rgb(palette.border)),
            (DWMWA_TEXT_COLOR, rgb(palette.text)),
            (DWMWA_USE_IMMERSIVE_DARK_MODE, palette.dark() as u32),
        ] {
            // SAFETY: 只设置本窗口属性，参数为本地 4 字节值；失败不影响使用。
            unsafe {
                DwmSetWindowAttribute(
                    self.handle,
                    attribute as _,
                    (&value as *const u32).cast(),
                    size_of::<u32>() as u32,
                );
            }
        }
    }
    /// 标题栏与任务栏按钮用虎码印章；图标随窗口保留，Drop 时释放。
    fn set_icons(&mut self) {
        for (slot, (kind, metric)) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)]
            .into_iter()
            .enumerate()
        {
            // SAFETY: 系统度量查询无副作用；图标在窗口存活期间有效，换下的旧图标随即释放。
            unsafe {
                let side = GetSystemMetricsForDpi(metric, GetDpiForWindow(self.handle).max(96));
                if let Some(icon) = seal::icon(side) {
                    SendMessageW(self.handle, WM_SETICON, kind as usize, icon as LPARAM);
                    let old = std::mem::replace(&mut self.icons[slot], icon);
                    if !old.is_null() {
                        DestroyIcon(old);
                    }
                }
            }
        }
    }
}
fn release(fonts: [HFONT; ROLES]) {
    for font in fonts {
        if !font.is_null() {
            // SAFETY: 控件已改用其他字体或已销毁，旧 GDI 对象不再被引用。
            unsafe {
                DeleteObject(font);
            }
        }
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: 子控件先随窗口销毁，之后释放字体与图标；均在创建它们的 UI 线程执行。
        unsafe {
            DestroyWindow(self.handle);
            for icon in self.icons {
                if !icon.is_null() {
                    DestroyIcon(icon);
                }
            }
        }
        release(self.fonts);
    }
}

struct Class(Vec<u16>);
impl Class {
    fn register() -> Result<Self, String> {
        let name = wide("TigerType.Lookup.Native.v3");
        // SAFETY: 仅注册无状态系统回调；不设背景画刷，底色由 WM_PAINT 按主题绘制。
        let registered = unsafe {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: GetModuleHandleW(null()),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                lpszClassName: name.as_ptr(),
                ..Default::default()
            })
        };
        if registered == 0 {
            return Err(error());
        }
        Ok(Self(name))
    }
}
impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: run 中窗口晚于类创建，先于类销毁；名称在注销调用期间存活。
        unsafe {
            UnregisterClassW(self.0.as_ptr(), GetModuleHandleW(null()));
        }
    }
}
pub(crate) struct DpiContext(DPI_AWARENESS_CONTEXT);
impl DpiContext {
    pub(crate) fn enter() -> Result<Self, String> {
        // SAFETY: 只改变本 UI 线程，不修改宿主或全进程默认值，退出时恢复。
        let old =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if old.is_null() {
            return Err(error());
        }
        Ok(Self(old))
    }
}
impl Drop for DpiContext {
    fn drop(&mut self) {
        // SAFETY: 恢复进入时系统返回的线程上下文。
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}

/// 按客户区尺寸设定窗口外框（按窗口 DPI 换算），不移动、不激活。
fn size_client(window: &Window, (width, height): (i32, i32), style: u32) {
    let dpi = window.dpi;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: width * dpi as i32 / 96,
        bottom: height * dpi as i32 / 96,
    };
    // SAFETY: 输出 RECT 为本地值；只调整本窗口尺寸，不激活。
    unsafe {
        AdjustWindowRectExForDpi(&mut rect, style, 0, WS_EX_CONTROLPARENT, dpi);
        SetWindowPos(
            window.handle,
            null_mut(),
            0,
            0,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn lookup_window(class: &Class, options: &Options) -> Result<Window, String> {
    let style = WS_OVERLAPPEDWINDOW & !WS_MAXIMIZEBOX;
    let mut window = Window::create(&class.0, "虎码 查字", null_mut(), style)?;
    // 零尺寸标签只为读屏名称与 Alt+Q；查询框里另画占位提示。
    window.add(QUERY_LABEL, "STATIC", "汉字或拼音(&Q)", 0)?;
    window.add(QUERY, "EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL as u32)?;
    // 粘贴上限保留足够余量；超过业务上限明确报错，不把截断文本当成有效查询。
    window.send(QUERY, EM_SETLIMITTEXT, 4096, 0);
    window.add(
        LIST,
        "LISTBOX",
        "",
        WS_TABSTOP
            | (LBS_OWNERDRAWFIXED
                | LBS_HASSTRINGS
                | LBS_NOTIFY
                | LBS_NOINTEGRALHEIGHT
                | LBS_MULTICOLUMN) as u32,
    )?;
    window.add(PREVIOUS, "BUTTON", "上一页(&P)", WS_TABSTOP)?;
    window.add(PAGE, "STATIC", "", SS_CENTER | SS_CENTERIMAGE)?;
    window.add(NEXT, "BUTTON", "下一页(&N)", WS_TABSTOP)?;
    window.add(GLYPH, "STATIC", "", SS_OWNERDRAW)?;
    window.add(CODEPOINT, "STATIC", "", SS_CENTER)?;
    window.add(COPY, "BUTTON", "复制(&C)", WS_TABSTOP)?;
    for (index, (value, label)) in [
        (CODES, "可用编码"),
        (FULL, "完整编码"),
        (READING, "读音"),
        (PARTS, "拆分"),
    ]
    .into_iter()
    .enumerate()
    {
        window.add(
            FIELD_LABEL + index,
            "STATIC",
            label,
            SS_CENTERIMAGE | SS_NOPREFIX,
        )?;
        let style = if value == READING {
            SS_CENTERIMAGE | SS_NOPREFIX
        } else {
            SS_OWNERDRAW
        };
        window.add(value, "STATIC", "", style)?;
    }
    window.add(STATUS, "STATIC", "", SS_CENTER | SS_NOPREFIX)?;
    window.add(SETTINGS, "BUTTON", "设置(&S)", WS_TABSTOP)?;
    // 隐藏控件不显示、不可聚焦，只承载当前字的资料供标准复制。
    window.add(CLIP, "EDIT", "", ES_MULTILINE as u32 | ES_READONLY as u32)?;
    window.show(CLIP, false);
    window.decorate(options.palette(), options.palette().window);
    window.layout(options.candidate_font)?;
    window.set_icons();
    size_client(&window, LOOKUP_SIZE, style);
    initialize_cues(window.handle);
    Ok(window)
}
/// 按系统约定初始化焦点环与助记下划线：用户未用键盘时先隐藏，按 Alt/Tab 后再显示。
fn initialize_cues(hwnd: HWND) {
    // SAFETY: 发给本线程拥有的顶层窗口，值类型参数。
    unsafe {
        // 高字指定受影响的提示；为 0 时什么都不改。
        let flags = (UISF_HIDEACCEL | UISF_HIDEFOCUS) << 16;
        SendMessageW(hwnd, WM_CHANGEUISTATE, (UIS_INITIALIZE | flags) as usize, 0);
    }
}
/// 设置窗标题栏与侧栏同色。
fn caption(palette: Palette) -> u32 {
    mix(palette.window, palette.text, 4)
}
fn settings_window(
    class: &Class,
    owner: HWND,
    options: &Options,
    settings_path: &Path,
) -> Result<Window, String> {
    let style = WS_CAPTION | WS_SYSMENU | WS_VSCROLL;
    let mut window = Window::create(&class.0, "虎码 设置", owner, style)?;
    for (k, (name, _)) in PAGES.iter().enumerate() {
        let group = if k == 0 { WS_GROUP | WS_TABSTOP } else { 0 };
        window.add(NAV + k, "BUTTON", name, group | BS_AUTORADIOBUTTON as u32)?;
    }
    window.add(NOTICE, "STATIC", crate::model::SETTINGS_IDLE, SS_NOPREFIX)?;
    window.add(HEADING, "STATIC", PAGES[0].0, SS_NOPREFIX)?;
    for (k, name) in GROUPS.iter().enumerate() {
        window.add(GROUP + k, "STATIC", name, SS_NOPREFIX)?;
    }
    let choice = |window: &mut Window, row: usize| -> Result<(), String> {
        let (title, description, first, count, _) = CHOICES[row];
        window.add(ROW_TITLE + row, "STATIC", title, SS_CENTERIMAGE)?;
        window.add(ROW_DESC + row, "STATIC", description, SS_NOPREFIX)?;
        let names: Vec<String> = match first {
            MODE => TypingMode::ALL.iter().map(|m| m.name().into()).collect(),
            FORM => FORMS.iter().map(|(_, name)| (*name).into()).collect(),
            FONT => CandidateFont::ALL.iter().map(|f| f.name().into()).collect(),
            _ => Options::PAGE_SIZES.map(|n| n.to_string()).collect(),
        };
        debug_assert_eq!(names.len(), count);
        for (k, name) in names.iter().enumerate() {
            // 每组首项开新组并作 Tab 停靠；组内方向键切换，选中项随之勾选。
            let style = if k == 0 { WS_GROUP | WS_TABSTOP } else { 0 };
            window.add(first + k, "BUTTON", name, style | BS_AUTORADIOBUTTON as u32)?;
        }
        Ok(())
    };
    let toggle = |window: &mut Window, id: usize| -> Result<(), String> {
        let title = TOGGLES
            .iter()
            .find(|(key, ..)| *key == id)
            .map_or("", |(_, t, _)| *t);
        window.add(
            id,
            "BUTTON",
            title,
            WS_TABSTOP | WS_GROUP | BS_AUTOCHECKBOX as u32,
        )
    };
    // 输入
    choice(&mut window, 0)?;
    toggle(&mut window, COMMON)?;
    toggle(&mut window, AUTO)?;
    choice(&mut window, 1)?;
    // 外观
    window.add(PREVIEW, "STATIC", "候选窗预览", SS_OWNERDRAW)?;
    choice(&mut window, 2)?;
    choice(&mut window, 3)?;
    toggle(&mut window, LEARNING)?;
    for (k, theme) in Theme::ALL.iter().enumerate() {
        let style = if k == 0 { WS_GROUP | WS_TABSTOP } else { 0 };
        window.add(
            THEME + k,
            "BUTTON",
            theme.name(),
            style | BS_AUTORADIOBUTTON as u32,
        )?;
    }
    for (k, (name, _)) in COLORS.iter().enumerate() {
        window.add(
            SWATCH + k,
            "BUTTON",
            &format!("{name}色"),
            WS_GROUP | WS_TABSTOP,
        )?;
        window.add(
            CUSTOM + k,
            "EDIT",
            "",
            WS_GROUP | WS_TABSTOP | ES_UPPERCASE as u32,
        )?;
        window.send(CUSTOM + k, EM_SETLIMITTEXT, 7, 0);
    }
    window.add(CUSTOM_STATUS, "STATIC", "", SS_CENTERIMAGE | SS_NOPREFIX)?;
    window.add(RESET, "BUTTON", "从琥珀重新开始", WS_GROUP | WS_TABSTOP)?;
    // 快捷键
    window.add(KEYS_NOTE, "STATIC", "暂不支持自定义", SS_NOPREFIX)?;
    for (k, (label, spec)) in KEYMAP.iter().enumerate() {
        // 读屏朗读去掉键帽括号的文字，画面由自绘给出。
        let spoken = format!("{label}：{}", spec.replace(['[', ']'], ""));
        window.add(KEYS + k, "STATIC", &spoken, SS_OWNERDRAW)?;
    }
    // 高级
    window.add(FILE_NAME, "STATIC", "settings.toml", SS_NOPREFIX)?;
    window.add(
        FILE_PATH,
        "STATIC",
        &settings_path.display().to_string(),
        SS_NOPREFIX | SS_PATHELLIPSIS,
    )?;
    window.add(OPEN_CONFIG, "BUTTON", "打开(&O)", WS_GROUP | WS_TABSTOP)?;
    window.add(ROW_TITLE + 4, "STATIC", "载入手工修改", SS_NOPREFIX)?;
    window.add(
        ROW_DESC + 4,
        "STATIC",
        "编辑并保存文件后点“重新载入”；格式有误时保留当前设置，并指出出错的行",
        SS_NOPREFIX,
    )?;
    window.add(RELOAD, "BUTTON", "重新载入(&R)", WS_GROUP | WS_TABSTOP)?;
    window.add(ABOUT_TITLE, "STATIC", "虎码 TigerType", SS_NOPREFIX)?;
    window.add(
        ABOUT_DESC,
        "STATIC",
        &format!(
            "版本 {} · 本地运行，不联网、不收集数据",
            env!("CARGO_PKG_VERSION")
        ),
        SS_NOPREFIX,
    )?;
    check(&window, NAV, true);
    window.decorate(options.palette(), caption(options.palette()));
    window.layout(options.candidate_font)?;
    window.set_icons();
    size_client(&window, SETTINGS_SIZE, style & !WS_VSCROLL);
    initialize_cues(window.handle);
    Ok(window)
}

/// 用系统默认编辑器打开受管配置文件；文件不存在时返回可读错误，不创建或覆盖配置。
fn open_config(path: &Path) -> Result<(), String> {
    let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    let target: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: 两个 UTF-16 缓冲区在调用期间有效且以 NUL 结尾；ShellExecuteW 不转移其所有权。
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    } as usize;
    if result <= 32 {
        Err("无法打开 settings.toml，请确认系统有可用的文本编辑器".into())
    } else {
        Ok(())
    }
}
fn check(window: &Window, id: usize, on: bool) {
    window.send(
        id,
        BM_SETCHECK,
        if on { BST_CHECKED } else { BST_UNCHECKED } as usize,
        0,
    );
}
fn set_custom(window: &Window, custom: CustomColors) {
    for (k, color) in [custom.window, custom.text, custom.highlight, custom.accent]
        .into_iter()
        .enumerate()
    {
        window.text(CUSTOM + k, &format!("#{color:06X}"));
    }
}
fn set_options(window: &Window, options: Options) {
    check(window, AUTO, options.auto_commit_four);
    check(window, COMMON, options.common_only);
    check(window, LEARNING, options.show_learning);
    for (k, (form, _)) in FORMS.iter().enumerate() {
        check(window, FORM + k, *form == options.output_form);
    }
    for (k, font) in CandidateFont::ALL.iter().enumerate() {
        check(window, FONT + k, *font == options.candidate_font);
    }
    for (k, theme) in Theme::ALL.iter().enumerate() {
        check(window, THEME + k, *theme == options.theme);
    }
    for (k, mode) in TypingMode::ALL.iter().enumerate() {
        check(window, MODE + k, *mode == options.typing_mode);
    }
    for (k, size) in Options::PAGE_SIZES.enumerate() {
        check(window, SIZE_CHOICE + k, size == options.page_size);
    }
    set_custom(window, options.custom);
}
fn query(window: &Window) -> Result<String, String> {
    // SAFETY: 文本最多读取 128 个 UTF-16 单元，足够容纳 64 个非 BMP 字符；绝不接受截断查询。
    let length = unsafe { GetWindowTextLengthW(window.control(QUERY)) };
    if length > 128 {
        return Err("查字文本超过 64 个字符".into());
    }
    let mut buffer = [0u16; 129];
    // SAFETY: 缓冲区长度与传入容量相同；控件和读取都在同一线程。
    let read = unsafe { GetWindowTextW(window.control(QUERY), buffer.as_mut_ptr(), 129) };
    String::from_utf16(&buffer[..read as usize]).map_err(|_| "查询包含无效 Unicode".into())
}
fn selected<'a>(main: &Window, model: &'a Model) -> Option<&'a tigertype_contracts::LookupItem> {
    let index = main.send(LIST, LB_GETCURSEL, 0, 0);
    model
        .result
        .as_ref()
        .and_then(|page| page.items.get(usize::try_from(index).ok()?))
}
/// 换上新查询页：字块逐个列出，列宽容得下最长的词，默认选中首项。
fn show_page(main: &Window, model: &Model) {
    main.send(LIST, LB_RESETCONTENT, 0, 0);
    let items = model
        .result
        .as_ref()
        .map_or(&[][..], |page| &page.items[..]);
    let list = main.control(LIST);
    let mut widest = 0;
    // SAFETY: 取列表 DC 只用于测量，结束时归还。
    let dc = unsafe { GetDC(list) };
    for item in items {
        let text = wide(&item.text);
        main.send(LIST, LB_ADDSTRING, 0, text.as_ptr() as LPARAM);
        widest = widest.max(extent(dc, font_of(list), &units(&item.text)));
    }
    // SAFETY: 归还上面取得的 DC。
    unsafe { ReleaseDC(list, dc) };
    let dpi = main.dpi.max(96) as i32;
    let column = (CHIP * dpi / 96).max(widest + 18 * dpi / 96);
    main.send(LIST, LB_SETCOLUMNWIDTH, column as usize, 0);
    main.send(
        LIST,
        LB_SETCURSEL,
        if items.is_empty() { usize::MAX } else { 0 },
        0,
    );
    show_detail(main, model);
}
/// 详情卡：有选中字时显示资料，否则显示提示（空态、无结果或出错）。
fn show_detail(main: &Window, model: &Model) {
    let item = selected(main, model);
    if let Some(item) = item {
        let join = |items: &[String], separator: &str| {
            if items.is_empty() {
                "缺资料".to_owned()
            } else {
                items.join(separator)
            }
        };
        let mut chars = item.text.chars();
        let point = match (chars.next(), chars.next()) {
            (Some(c), None) => format!("U+{:04X}", c as u32),
            _ => String::new(),
        };
        main.text(GLYPH, &item.text);
        main.text(CODEPOINT, &point);
        main.text(CODES, &join(&item.learning.codes, " "));
        main.text(FULL, &join(&item.learning.full_codes, " "));
        main.text(READING, &join(&item.learning.readings, "、"));
        main.text(
            PARTS,
            item.learning.decomposition.as_deref().unwrap_or("缺资料"),
        );
        main.text(CLIP, &details(item));
    }
    for id in [GLYPH, CODEPOINT, COPY, CODES, FULL, READING, PARTS]
        .into_iter()
        .chain(FIELD_LABEL..FIELD_LABEL + 4)
    {
        main.show(id, item.is_some());
    }
    main.show(STATUS, item.is_none());
}
fn refresh(main: &Window, settings: &Window, model: &Model) {
    main.enable(QUERY, !model.busy);
    main.enable(
        PREVIOUS,
        !model.busy && model.page > 0 && model.result.is_some(),
    );
    main.enable(
        NEXT,
        !model.busy && model.result.as_ref().is_some_and(|p| p.has_more),
    );
    main.enable(COPY, selected(main, model).is_some());
    main.text(STATUS, &model.notice);
    let page = model.result.as_ref().map(|_| (model.page + 1).to_string());
    main.text(PAGE, &page.unwrap_or_default());
    settings.text(NOTICE, &model.settings_notice);
    settings.text(CUSTOM_STATUS, &custom_status(settings.handle));
}
/// 确认的配色/字体用于两个窗口：换绘制配色、标题栏与字体后整体重画。
fn apply_look(main: &mut Window, settings: &mut Window, options: &Options) -> Result<(), String> {
    let palette = options.palette();
    PALETTE.with(|current| current.set(palette));
    main.decorate(palette, palette.window);
    settings.decorate(palette, caption(palette));
    main.layout(options.candidate_font)?;
    settings.layout(options.candidate_font)
}
/// 切换设置分区：回到顶部重新布局；焦点若落在已隐藏的控件上，移到侧栏当前项。
fn select_page(settings: &mut Window, page: usize, face: CandidateFont) -> Result<(), String> {
    for k in 0..PAGES.len() {
        check(settings, NAV + k, k == page);
    }
    settings.text(HEADING, PAGES[page].0);
    set_scroll_offset(settings.handle, 0);
    settings.layout(face)?;
    let nav = settings.control(NAV + page);
    // SAFETY: 只查询并改变本线程窗口焦点。
    unsafe {
        let focus = GetFocus();
        if !focus.is_null() && GetParent(focus) == settings.handle && IsWindowVisible(focus) == 0 {
            SetFocus(nav);
        }
    }
    Ok(())
}
fn repaint(window: &Window) {
    // SAFETY: 只使本窗口及子控件失效，绘制在各自 WM_PAINT 中进行。
    unsafe {
        RedrawWindow(
            window.handle,
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ALLCHILDREN,
        );
    }
}
/// 色块按钮下方弹出无蓝色板（8 列色相 × 4 档明度的自绘菜单），返回所选颜色；取消为 None。
fn palette_menu(settings: &Window, k: usize) -> Option<u32> {
    let current = hex_in(settings.handle, k).ok();
    // SAFETY: 菜单与画刷在本函数内创建并释放；文字缓冲在插入调用期间有效（菜单复制文字）。
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return None;
        }
        let mut position = 0;
        for column in 0..8 {
            for row in 0..4 {
                let index = row * 8 + column;
                let color = SWATCHES[index];
                let mut text = wide(&format!("#{color:06X}"));
                let item = MENUITEMINFOW {
                    cbSize: size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_ID | MIIM_STRING | MIIM_FTYPE | MIIM_DATA | MIIM_STATE,
                    fType: MFT_OWNERDRAW
                        | if row == 0 && column > 0 {
                            MFT_MENUBREAK
                        } else {
                            0
                        },
                    fState: if current == Some(color) {
                        MFS_CHECKED
                    } else {
                        0
                    },
                    wID: index as u32 + 1,
                    dwItemData: color as usize,
                    dwTypeData: text.as_mut_ptr(),
                    ..Default::default()
                };
                InsertMenuItemW(menu, position, 1, &item);
                position += 1;
            }
        }
        let brush = CreateSolidBrush(colors().surface);
        SetMenuInfo(
            menu,
            &MENUINFO {
                cbSize: size_of::<MENUINFO>() as u32,
                fMask: MIM_BACKGROUND | MIM_STYLE,
                dwStyle: MNS_NOCHECK,
                hbrBack: brush,
                ..Default::default()
            },
        );
        let mut rect = RECT::default();
        GetWindowRect(settings.control(SWATCH + k), &mut rect);
        let chosen = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
            rect.left,
            rect.bottom,
            settings.handle,
            null(),
        ) as usize;
        DestroyMenu(menu);
        DeleteObject(brush);
        chosen
            .checked_sub(1)
            .and_then(|index| SWATCHES.get(index).copied())
    }
}
/// 即时生效：草稿合法且与已确认设置不同才保存；在途时不排队，回复后由事件循环再比一次。
/// 草稿不合法（色值未填完、偏蓝或不可读）时不保存，侧栏写明原因，在其他分区改设置时也看得到；
/// 草稿回到已确认设置后清掉这条提示。
fn commit(settings: &Window, model: &mut Model) -> Option<Request> {
    match read_options(settings.handle) {
        Ok(options) if options == model.options => {
            if model.settings_notice.starts_with('✗') {
                model.settings_notice = crate::model::SETTINGS_IDLE.into();
            }
            None
        }
        Ok(options) if !model.busy => model.save(options).ok(),
        Ok(_) => None,
        Err(error) => {
            model.settings_notice = format!("✗ 未保存：{error}");
            None
        }
    }
}
/// 控件自身是否带可见样式（不看父窗口）。
fn shown(control: HWND) -> bool {
    // SAFETY: 只读本线程控件的样式位。
    unsafe { GetWindowLongW(control, GWL_STYLE) as u32 & WS_VISIBLE != 0 }
}
fn visible(window: &Window) -> bool {
    // SAFETY: 只查询本线程窗口显隐。
    unsafe { IsWindowVisible(window.handle) != 0 }
}
fn send(requests: &SyncSender<Request>, model: &mut Model, request: Request) -> bool {
    match requests.try_send(request) {
        Ok(()) => true,
        Err(error) => {
            model.fail(format!("操作未送达：{error}"));
            false
        }
    }
}
/// 草稿里会影响设置的控件：点击后即保存。
fn is_option(id: usize) -> bool {
    matches!(id, AUTO | COMMON | LEARNING)
        || (MODE..MODE + TypingMode::ALL.len()).contains(&id)
        || (FORM..FORM + FORMS.len()).contains(&id)
        || (FONT..FONT + CandidateFont::ALL.len()).contains(&id)
        || (SIZE_CHOICE..SIZE_CHOICE + Options::PAGE_SIZES.count()).contains(&id)
}
/// 设置窗的命令：切换分区、即时保存、色板、打开与重新载入配置文件。
fn settings_action(
    settings: &mut Window,
    model: &mut Model,
    id: usize,
    notification: u32,
    settings_path: &Path,
) -> Result<Option<Request>, String> {
    let face = model.options.candidate_font;
    Ok(match notification {
        EN_CHANGE if (CUSTOM..CUSTOM + COLORS.len()).contains(&id) => {
            repaint(settings);
            commit(settings, model)
        }
        BN_CLICKED if (NAV..NAV + PAGES.len()).contains(&id) => {
            select_page(settings, id - NAV, face)?;
            None
        }
        BN_CLICKED if (SWATCH..SWATCH + COLORS.len()).contains(&id) => {
            // 改写色值框会触发 EN_CHANGE，随后照常校验并保存。
            if let Some(color) = palette_menu(settings, id - SWATCH) {
                settings.text(CUSTOM + id - SWATCH, &format!("#{color:06X}"));
            }
            None
        }
        BN_CLICKED if (THEME..THEME + Theme::ALL.len()).contains(&id) => {
            // 离开自定义时，未填完或不合格的四色退回已保存的值，免得挡住换预设主题的保存。
            if id != THEME + Theme::ALL.len() - 1 && read_options(settings.handle).is_err() {
                set_custom(settings, model.options.custom);
            }
            // 选不选自定义决定是否露出四色编辑行。
            settings.layout(face)?;
            commit(settings, model)
        }
        BN_CLICKED if is_option(id) => {
            repaint(settings);
            commit(settings, model)
        }
        BN_CLICKED => match id {
            RESET => {
                set_custom(settings, CustomColors::default());
                None
            }
            OPEN_CONFIG => {
                model.settings_notice = match open_config(settings_path) {
                    Ok(()) => "已用默认编辑器打开 settings.toml".into(),
                    Err(error) => error,
                };
                None
            }
            RELOAD => match model.reload() {
                Ok(request) => Some(request),
                Err(error) => {
                    model.settings_notice = error.into();
                    None
                }
            },
            _ => None,
        },
        _ => None,
    })
}

fn composing(hwnd: HWND) -> bool {
    // SAFETY: 仅借用当前编辑控件的输入上下文，立即配对归还；不切换或配置用户输入法。
    unsafe {
        let context = ImmGetContext(hwnd);
        if context.is_null() {
            return false;
        }
        let length = ImmGetCompositionStringW(context, GCS_COMPSTR, null_mut(), 0);
        ImmReleaseContext(hwnd, context);
        length > 0
    }
}

fn key_command(settings: bool, id: usize, key: usize, ime: bool) -> Option<(u32, usize)> {
    // IME 先处理确认/取消；开关、单选与色值框上 Enter 不触发别的动作。
    if ime {
        return None;
    }
    if settings {
        if key == VK_ESCAPE as usize {
            return Some((CLOSE, 0));
        }
        if key == VK_RETURN as usize
            && (matches!(id, OPEN_CONFIG | RELOAD | RESET)
                || (SWATCH..SWATCH + COLORS.len()).contains(&id))
        {
            return Some((ACTION, id));
        }
    } else if key == VK_RETURN as usize {
        return match id {
            QUERY => Some((ACTION, SEARCH)),
            PREVIOUS | NEXT | COPY | SETTINGS => Some((ACTION, id)),
            _ => None,
        };
    }
    None
}

/// Tab 移到滚出视野的控件时把它滚回来。
fn ensure_visible(settings: &Window) {
    let Some(id) = settings.focused_control() else {
        return;
    };
    if (NAV..NAV + PAGES.len()).contains(&id) {
        return;
    }
    let mut rect = RECT::default();
    let mut client = RECT::default();
    // SAFETY: 只读本线程窗口位置并换算到设置窗客户区坐标；输出为本地值。
    unsafe {
        GetWindowRect(settings.control(id), &mut rect);
        MapWindowPoints(
            null_mut(),
            settings.handle,
            (&mut rect as *mut RECT).cast(),
            2,
        );
        GetClientRect(settings.handle, &mut client);
    }
    let dpi = settings.dpi.max(96) as i32;
    let (top, bottom, view) = (
        rect.top * 96 / dpi,
        rect.bottom * 96 / dpi,
        client.bottom * 96 / dpi,
    );
    let offset = scroll_offset(settings.handle);
    if top < 12 {
        scroll_to(settings.handle, offset + top - 24);
    } else if bottom > view - 12 {
        scroll_to(settings.handle, offset + bottom - view + 24);
    }
}

fn keyboard(message: &MSG, main: &Window, settings: &Window) {
    // SAFETY: 消息源自本线程队列，仅本程序的顶层窗使用对话框导航。
    let root = unsafe { GetAncestor(message.hwnd, GA_ROOT) };
    let dialog = if root == settings.handle {
        Some(settings)
    } else if root == main.handle {
        Some(main)
    } else {
        None
    };
    if let Some(dialog) = dialog {
        // SAFETY: 查询当前线程按键状态，不注入全局键盘事件。
        let modified =
            unsafe { GetKeyState(VK_CONTROL as i32) < 0 || GetKeyState(VK_MENU as i32) < 0 };
        if message.message == WM_KEYDOWN && !modified {
            // SAFETY: hwnd 来自系统消息；无效/非控件句柄返回值不会匹配已知动作。
            let id = unsafe { GetDlgCtrlID(message.hwnd) } as usize;
            let is_settings = root == settings.handle;
            let ime = message.hwnd == main.control(QUERY)
                && matches!(message.wParam, key if key == VK_RETURN as usize || key == VK_ESCAPE as usize)
                && composing(message.hwnd);
            if ime {
                // SAFETY: IME 确认/取消必须直接交编辑控件，不能再被对话框默认按钮截获。
                unsafe {
                    TranslateMessage(message);
                    DispatchMessageW(message);
                }
                return;
            }
            if let Some((event, action)) = key_command(is_settings, id, message.wParam, ime) {
                post(root, event, action);
                return;
            }
        }
        // SAFETY: 使用系统导航处理 Tab/方向键/助记键，消息及窗口都在同一线程存活。
        if unsafe { IsDialogMessageW(dialog.handle, message) } != 0 {
            if root == settings.handle {
                ensure_visible(settings);
            }
            return;
        }
    }
    // SAFETY: 未接管的输入原样交回原生控件，包括 IME。
    unsafe {
        TranslateMessage(message);
        DispatchMessageW(message);
    }
}

/// 当前线程专用于原生 UI；事件唤醒回复、系统消息驱动操作，不使用定时轮询。
/// 后台进程内的固定 UI 线程：启动即挂通知区图标，查字/设置窗等首次唤起（唤起信号或图标操作）才建；
/// 关闭主窗只隐藏，再次唤起即前置目标窗口；show 断开（后台停止）才退出并删除图标。
/// taskbar 为后台给出的通知区图标显隐（前台程序正用本输入法时隐藏），全程随到随用。
pub fn run(
    requests: SyncSender<Request>,
    inbox: Inbox<Reply>,
    show: Inbox<Panel>,
    taskbar: &Latest<bool>,
    settings_path: &Path,
    options: Options,
) -> Result<(), String> {
    let _dpi = DpiContext::enter()?;
    PALETTE.with(|palette| palette.set(options.palette()));
    watch(Some(&show));
    // 邮箱比所有窗口活得久；退出时先撤销登记，不留悬空事件句柄。
    let result = serve(requests, inbox, &show, taskbar, settings_path, options);
    watch(None);
    result
}
fn serve(
    requests: SyncSender<Request>,
    inbox: Inbox<Reply>,
    show: &Inbox<Panel>,
    taskbar: &Latest<bool>,
    settings_path: &Path,
    options: Options,
) -> Result<(), String> {
    let tray = tray::Tray::new()?;
    let Some(panel) = tray::first(show, &tray, taskbar)? else {
        return Ok(());
    };
    let class = Class::register()?;
    let mut main = lookup_window(&class, &options)?;
    let mut settings = settings_window(&class, main.handle, &options, settings_path)?;
    let mut model = Model::new(options);
    set_options(&settings, options);
    settings.layout(options.candidate_font)?;
    show_page(&main, &model);
    refresh(&main, &settings, &model);
    present(&main, &settings, &mut model, panel);
    event_loop(
        &mut main,
        &mut settings,
        &mut model,
        &requests,
        &inbox,
        show,
        (settings_path, Some((&tray, taskbar))),
    )
}

/// 显示并前置目标窗口，最小化时先还原。唤起方已放行前台切换；通知区图标被点击时本进程获准切前台。
/// 设置页可单独打开（输入法图标或通知区图标），不连带显示查字窗；隐藏后重开时控件恢复为已确认设置。
fn present(main: &Window, settings: &Window, model: &mut Model, panel: Panel) {
    let (window, focus) = match panel {
        Panel::Lookup => (main, main.control(QUERY)),
        Panel::Settings => {
            if !visible(settings) {
                set_options(settings, model.options);
                post(settings.handle, LAYOUT, 0);
            }
            (
                settings,
                settings.control(NAV + current_page(settings.handle)),
            )
        }
    };
    // SAFETY: 窗口与控件由调用方持有，只在本线程显示与激活，不注册或切换输入法。
    unsafe {
        let mode = if IsIconic(window.handle) != 0 {
            SW_RESTORE
        } else {
            SW_SHOW
        };
        ShowWindow(window.handle, mode);
        SetForegroundWindow(window.handle);
        SetFocus(focus);
    }
}

fn event_loop(
    main: &mut Window,
    settings: &mut Window,
    model: &mut Model,
    requests: &SyncSender<Request>,
    inbox: &Inbox<Reply>,
    show: &Inbox<Panel>,
    // 设置文件路径与通知区图标；测试不登记图标时为 None。
    context: (&Path, Option<(&tray::Tray, &Latest<bool>)>),
) -> Result<(), String> {
    let (settings_path, icon) = context;
    let mut events = vec![inbox.event.as_raw_handle(), show.event.as_raw_handle()];
    if let Some((_, taskbar)) = icon {
        events.push(taskbar.event().as_raw_handle());
    }
    let mut main_focus = None;
    loop {
        // SAFETY: Inbox 与交接槽持有事件直到循环结束；消息、回复、唤起与显隐共享一次阻塞等待，空闲不耗 CPU。
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(
                events.len() as u32,
                events.as_ptr(),
                INFINITE,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if wait == WAIT_FAILED {
            return Err(error());
        }
        if let Some((tray, taskbar)) = icon {
            tray.sync(taskbar)?;
        }
        match show.receiver.try_recv() {
            Ok(panel) => present(main, settings, model, panel),
            Err(TryRecvError::Empty) => {}
            // 后台停止：放弃窗口，线程退出由调用方 join。
            Err(TryRecvError::Disconnected) => return Ok(()),
        }
        loop {
            match inbox.receiver.try_recv() {
                Ok(reply) => {
                    let before = model.options;
                    match model.receive(reply)? {
                        Effect::Page => show_page(main, model),
                        Effect::Controls => {
                            set_options(settings, model.options);
                            settings.layout(model.options.candidate_font)?;
                        }
                        Effect::Nothing => {}
                    }
                    if model.options != before {
                        apply_look(main, settings, &model.options)?;
                    }
                    // 在途期间又改过的设置：回复后按最新草稿补存一次。
                    if visible(settings)
                        && let Some(request) = commit(settings, model)
                    {
                        send(requests, model, request);
                    }
                    refresh(main, settings, model);
                    if let Some(id) = main_focus.take() {
                        main.restore_focus(id);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err("查字工作线程已退出，请重新启动工具".into());
                }
            }
        }
        let mut message = MSG::default();
        // 每轮最多处理 64 条系统消息，避免持续消息流饿死工作线程回复。
        for _ in 0..64 {
            // SAFETY: MSG 栈对象完整有效，消息仅从当前线程提取。
            if unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } == 0 {
                break;
            }
            match message.message {
                WM_QUIT => {
                    return if message.wParam == 0 {
                        Ok(())
                    } else {
                        Err("窗口消息投递失败".into())
                    };
                }
                // 后台常驻：关闭主窗只隐藏（设置窗一并收起，未填完的色值丢弃），保留查询结果，等下次唤起。
                CLOSE if message.hwnd == main.handle => {
                    main_focus = None;
                    set_options(settings, model.options);
                    // SAFETY: 两个窗口仅隐藏，所有权继续由本函数持有。
                    unsafe {
                        ShowWindow(settings.handle, SW_HIDE);
                        ShowWindow(main.handle, SW_HIDE);
                    }
                }
                CLOSE if message.hwnd == settings.handle => {
                    set_options(settings, model.options);
                    post(settings.handle, LAYOUT, 0);
                    // SAFETY: 设置窗仅隐藏，所有权继续由本函数持有；单独打开时查字窗仍隐藏，不去激活它。
                    unsafe {
                        ShowWindow(settings.handle, SW_HIDE);
                        if IsWindowVisible(main.handle) != 0 {
                            SetFocus(main.control(QUERY));
                        }
                    }
                }
                LAYOUT => {
                    let face = model.options.candidate_font;
                    if message.hwnd == main.handle {
                        main.layout(face)?;
                    } else if message.hwnd == settings.handle {
                        settings.layout(face)?;
                    }
                }
                ACTION if message.hwnd == settings.handle => {
                    let id = message.wParam & 0xffff;
                    let notification = (message.wParam >> 16) as u32;
                    if let Some(request) =
                        settings_action(settings, model, id, notification, settings_path)?
                    {
                        send(requests, model, request);
                    }
                    refresh(main, settings, model);
                }
                ACTION => {
                    let id = message.wParam & 0xffff;
                    let notification = (message.wParam >> 16) as u32;
                    if id == QUERY && notification == EN_CHANGE && !model.busy {
                        model.result = None;
                        model.page = 0;
                        model.notice = "按 Enter 查询".into();
                        show_page(main, model);
                        refresh(main, settings, model);
                        continue;
                    }
                    if id == LIST && notification == LBN_SELCHANGE {
                        show_detail(main, model);
                        refresh(main, settings, model);
                        continue;
                    }
                    if notification != BN_CLICKED {
                        continue;
                    }
                    let request = match id {
                        SEARCH if !model.busy => Some(
                            query(main).and_then(|q| model.lookup(q, 0).map_err(str::to_owned)),
                        ),
                        PREVIOUS | NEXT if !model.busy => {
                            let page = if id == NEXT {
                                model.page + 1
                            } else {
                                model.page.saturating_sub(1)
                            };
                            Some(
                                query(main)
                                    .and_then(|q| model.lookup(q, page).map_err(str::to_owned)),
                            )
                        }
                        SETTINGS => {
                            present(main, settings, model, Panel::Settings);
                            None
                        }
                        COPY if selected(main, model).is_some() => {
                            // 只在用户点击时委托标准 EDIT 复制当前字资料，不读取或监视剪贴板。
                            main.send(CLIP, EM_SETSEL, 0, -1);
                            main.send(CLIP, WM_COPY, 0, 0);
                            None
                        }
                        _ => None,
                    };
                    match request {
                        Some(Ok(request)) => {
                            if send(requests, model, request) {
                                main_focus = main.focused_control();
                            }
                        }
                        Some(Err(error)) => model.notice = error,
                        None => {}
                    }
                    refresh(main, settings, model);
                }
                // 通知区图标的单击或菜单选择。
                tray::OPEN => present(main, settings, model, tray::decode(message.wParam)),
                _ => keyboard(&message, main, settings),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在内存位图上作画并读像素，不显示窗口、不碰桌面。
    struct Canvas(HDC, HBITMAP, HGDIOBJ);
    impl Canvas {
        fn new(width: i32, height: i32) -> Self {
            // SAFETY: 创建与屏幕兼容的内存 DC 和位图，Drop 时配对释放；先铺白底。
            unsafe {
                let screen = GetDC(null_mut());
                let dc = CreateCompatibleDC(screen);
                let bitmap = CreateCompatibleBitmap(screen, width, height);
                ReleaseDC(null_mut(), screen);
                let old = SelectObject(dc, bitmap);
                fill(
                    dc,
                    &RECT {
                        left: 0,
                        top: 0,
                        right: width,
                        bottom: height,
                    },
                    0x00FF_FFFF,
                );
                Self(dc, bitmap, old)
            }
        }
        fn pixel(&self, x: i32, y: i32) -> COLORREF {
            // SAFETY: 读本内存 DC 的像素。
            unsafe { GetPixel(self.0, x, y) }
        }
    }
    impl Drop for Canvas {
        fn drop(&mut self) {
            // SAFETY: 还原原位图后删除自建位图与 DC。
            unsafe {
                SelectObject(self.0, self.2);
                DeleteObject(self.1);
                DeleteDC(self.0);
            }
        }
    }
    fn client(window: &Window, id: usize) -> RECT {
        let mut rect = RECT::default();
        // SAFETY: 只读本测试窗口控件的客户区。
        unsafe { GetClientRect(window.control(id), &mut rect) };
        rect
    }

    /// 由隐藏窗口测试调用（窗口类同名，不能与之并行注册）：开关、分段与侧栏按主题绘制而非系统蓝。
    fn assert_themed_drawing(main: &Window, settings: &Window) {
        // SAFETY: 只读本测试窗口的窗口过程。
        let current = unsafe { GetWindowLongPtrW(main.control(QUERY), GWLP_WNDPROC) };
        assert_eq!(current as usize, edit_procedure as *const () as usize);
        // SAFETY: 同上。
        let current = unsafe { GetWindowLongPtrW(settings.control(RELOAD), GWLP_WNDPROC) };
        assert_eq!(current as usize, button_procedure as *const () as usize);
        // 用户开启高对比度时沿用系统色，不核对主题像素。
        if high_contrast() {
            return;
        }
        let c = themed(PALETTE.with(Cell::get));
        assert_eq!(rgb(0xB45309), 0x0009_53B4);
        // 打开的开关轨道为强调色，关着的开关行为卡片底色；选中的分段为卡片色小块，未选中为轨道色。
        check(settings, AUTO, true);
        check(settings, COMMON, false);
        check(settings, FORM + 1, true);
        check(settings, FORM, false);
        for (id, on) in [(AUTO, true), (COMMON, false)] {
            let rect = client(settings, id);
            let canvas = Canvas::new(rect.right, rect.bottom);
            draw_button(settings.control(id), canvas.0);
            let track = canvas.pixel(
                rect.right - scale(settings.handle, 15 + 30),
                rect.bottom / 2,
            );
            if on {
                assert_eq!(track, c.accent, "控件 {id}");
            } else {
                assert_eq!(canvas.pixel(4, 4), c.surface, "控件 {id}");
            }
        }
        for (id, expected) in [(FORM + 1, c.surface), (FORM, c.track)] {
            let rect = client(settings, id);
            let canvas = Canvas::new(rect.right, rect.bottom);
            draw_button(settings.control(id), canvas.0);
            assert_eq!(canvas.pixel(rect.right / 2, 3), expected, "控件 {id}");
        }
        // 侧栏选中项为主题高亮底（琥珀为浅杏色），不是系统蓝。
        let rect = client(settings, NAV);
        let canvas = Canvas::new(rect.right, rect.bottom);
        draw_button(settings.control(NAV), canvas.0);
        assert_eq!(canvas.pixel(rect.right - 6, rect.bottom / 2), c.nav);
        // 关闭〔拆分 · 编码〕时预览的提示列为空串：曾把空 Vec 的悬空指针交给 DrawTextW，打开外观区即令后台崩溃。
        for learning in [false, true] {
            check(settings, LEARNING, learning);
            let rect = client(settings, PREVIEW);
            let canvas = Canvas::new(rect.right.max(1), rect.bottom.max(1));
            let item = DRAWITEMSTRUCT {
                hwndItem: settings.control(PREVIEW),
                hDC: canvas.0,
                rcItem: rect,
                ..Default::default()
            };
            draw_preview(&item, &c);
        }
        check(settings, LEARNING, false);
        check(settings, AUTO, false);
        check(settings, COMMON, true);
        check(settings, FORM + 1, false);
        check(settings, FORM, true);
    }

    #[test]
    fn keyboard_commands_preserve_ime_handling_and_escape_closes_settings() {
        assert_eq!(
            key_command(false, QUERY, VK_RETURN as usize, false),
            Some((ACTION, SEARCH))
        );
        for id in [PREVIOUS, NEXT, COPY, SETTINGS] {
            assert_eq!(
                key_command(false, id, VK_RETURN as usize, false),
                Some((ACTION, id))
            );
        }
        for id in [LIST, CODES] {
            assert_eq!(key_command(false, id, VK_RETURN as usize, false), None);
        }
        assert_eq!(key_command(false, QUERY, VK_ESCAPE as usize, false), None);
        for id in [
            AUTO,
            COMMON,
            LEARNING,
            FORM,
            FONT + 1,
            THEME + 3,
            CUSTOM,
            NAV + 2,
        ] {
            // 即时生效：开关与单选上 Enter 不再触发保存。
            assert_eq!(key_command(true, id, VK_RETURN as usize, false), None);
            assert_eq!(
                key_command(true, id, VK_ESCAPE as usize, false),
                Some((CLOSE, 0))
            );
        }
        for id in [OPEN_CONFIG, RELOAD, RESET, SWATCH + 1] {
            assert_eq!(
                key_command(true, id, VK_RETURN as usize, false),
                Some((ACTION, id))
            );
        }
        for key in [VK_RETURN, VK_ESCAPE] {
            assert_eq!(key_command(false, QUERY, key as usize, true), None);
        }
    }
    #[test]
    fn layouts_keep_controls_inside_their_areas() {
        let (min_width, min_height) = LOOKUP_MIN;
        for (width, height) in [LOOKUP_MIN, LOOKUP_SIZE] {
            let spec = lookup_spec(width, height);
            for (id, [x, y, w, h]) in &spec.controls {
                if !matches!(*id, CLIP | QUERY_LABEL) {
                    assert!(*x >= 0 && *y >= 0 && *w > 0 && *h > 0, "控件 {id}");
                    assert!(
                        x + w <= width && y + h <= height,
                        "控件 {id} {width}×{height}"
                    );
                }
            }
            assert!(spec.card[1] + spec.card[3] <= height.max(min_height));
        }
        assert!(lookup_spec(min_width, min_height).card[2] > 0);
        let (width, height) = SETTINGS_SIZE;
        for page in 0..PAGES.len() {
            for custom in [false, true] {
                let spec = settings_spec(page, custom);
                let mut ids: Vec<_> = spec
                    .fixed
                    .iter()
                    .chain(&spec.controls)
                    .map(|(id, _)| *id)
                    .collect();
                let count = ids.len();
                ids.sort_unstable();
                ids.dedup();
                assert_eq!(ids.len(), count, "分区 {page} 控件重复");
                for (id, [x, y, w, h]) in &spec.fixed {
                    assert!(
                        *x >= 0 && x + w <= SIDEBAR && *y >= 0 && y + h <= height,
                        "控件 {id}"
                    );
                }
                for (id, [x, y, w, h]) in &spec.controls {
                    assert!(
                        *x >= CX && x + w <= CX + CW && *y >= 0,
                        "分区 {page} 控件 {id}"
                    );
                    assert!(y + h <= spec.height, "分区 {page} 控件 {id}");
                }
                // 滚动条出现时客户区变窄约 17px，内容仍须放得下。
                assert!(CX + CW <= width - 20);
                // 只有外观区会超出窗口高度并滚动。
                assert_eq!(spec.height > height, page == 1, "分区 {page}");
                let shows_editor = spec.controls.iter().any(|(id, _)| *id == CUSTOM);
                assert_eq!(shows_editor, page == 1 && custom);
            }
        }
    }
    // SAFETY: 系统在本线程菜单模态循环内调用；只收起本线程菜单。
    unsafe extern "system" fn give_up(_: HWND, _: u32, _: usize, _: u32) {
        // SAFETY: 同上。
        unsafe { EndMenu() };
    }
    // 系统同一时刻只允许一个弹出菜单：别的线程弹菜单会立即关掉本线程的，弹真实菜单的测试经此串行。
    static MENU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    /// 在本线程运行一个模态循环，取得锁后才起表：约 300ms 时另一线程断开唤起邮箱（即后台停止）。
    /// 修复失效时由 5 秒定时器收起菜单，让测试以超时失败而不是挂住；模态须在 3 秒内被收起，
    /// 不足 200ms 说明模态根本没展开（例如被别处的菜单顶掉）。
    pub(super) fn bounded(sender: crate::ReplySender<Panel>, modal: impl FnOnce()) {
        let _only = MENU.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(sender);
        });
        let started = std::time::Instant::now();
        // SAFETY: 定时器属于本线程，用后注销。
        let timer = unsafe { SetTimer(null_mut(), 0, 5000, Some(give_up)) };
        modal();
        // 只计模态本身：回收停止线程要等满 300ms，计入会让没展开的菜单也显得够久。
        let elapsed = started.elapsed();
        // SAFETY: 同上。
        unsafe { KillTimer(null_mut(), timer) };
        stopper.join().unwrap();
        assert!(elapsed < std::time::Duration::from_secs(3), "{elapsed:?}");
        assert!(
            elapsed > std::time::Duration::from_millis(200),
            "模态未展开：{elapsed:?}"
        );
    }
    /// 查字/设置窗的菜单模态展开期间后台停止：窗口菜单、设置窗色板（同经 WM_ENTERMENULOOP）
    /// 与文本框自带右键菜单（不发该通知）都须立即收起，停止信号留给事件循环，UI 线程随即退出。
    /// 菜单在屏幕上一闪而过，不移动鼠标。
    fn window_menu_closes_when_the_backend_stops(
        main: &mut Window,
        settings: &mut Window,
        model: &mut Model,
    ) {
        for kind in 0..3 {
            menu_closes_when_the_backend_stops(main, settings, model, kind);
        }
    }
    fn menu_closes_when_the_backend_stops(
        main: &mut Window,
        settings: &mut Window,
        model: &mut Model,
        kind: usize,
    ) {
        let (sender, show) = crate::mailbox::<Panel>().unwrap();
        watch(Some(&show));
        let query = main.control(QUERY);
        // SAFETY: 菜单在本函数内创建并销毁，所有者为本测试线程的窗口；文本框右键菜单按键盘方式（lParam -1）在插入符处弹出。
        bounded(sender, || unsafe {
            match kind {
                0 => {
                    let menu = CreatePopupMenu();
                    AppendMenuW(menu, MF_STRING, 1, wide("测试").as_ptr());
                    TrackPopupMenuEx(menu, TPM_RETURNCMD, 0, 0, main.handle, null());
                    DestroyMenu(menu);
                }
                1 => {
                    SendMessageW(query, WM_CONTEXTMENU, query as WPARAM, -1);
                }
                _ => {
                    assert_eq!(palette_menu(settings, 3), None);
                }
            }
        });
        let (requests, _receiver) = std::sync::mpsc::sync_channel(1);
        let (_replies, inbox) = crate::mailbox().unwrap();
        assert!(
            event_loop(
                main,
                settings,
                model,
                &requests,
                &inbox,
                &show,
                (Path::new("settings.toml"), None),
            )
            .is_ok()
        );
        watch(None);
    }
    /// 投递一次控件命令（与系统 WM_COMMAND 经窗口过程转成的 ACTION 相同）。
    fn command(window: &Window, id: usize, notification: u32) {
        post(window.handle, ACTION, (notification as usize) << 16 | id);
    }
    #[test]
    fn hidden_native_controls_roundtrip_unicode_settings_and_cleanup() {
        // 合成控件测试不显示窗口、不碰剪贴板、不加载个人资源。
        let _dpi = DpiContext::enter().unwrap();
        for _ in 0..2 {
            let class = Class::register().unwrap();
            let defaults = Options::default();
            let mut main = lookup_window(&class, &defaults).unwrap();
            let mut settings =
                settings_window(&class, main.handle, &defaults, Path::new("settings.toml"))
                    .unwrap();
            // 控件编号重复会让布局、助记键与字体角色落到别的控件上。
            for window in [&main, &settings] {
                let mut ids: Vec<_> = window.controls.iter().map(|(id, _)| *id).collect();
                ids.sort_unstable();
                let count = ids.len();
                ids.dedup();
                assert_eq!(ids.len(), count, "控件编号重复");
            }
            set_options(&settings, defaults);
            assert_themed_drawing(&main, &settings);
            // 隐藏窗收到迟到回复也不能从用户桌面抢走焦点。
            // SAFETY: 仅观察本测试线程焦点，测试不激活或显示窗口。
            let original_focus = unsafe { GetFocus() };
            main.restore_focus(QUERY);
            // SAFETY: GetFocus 无输出指针，前后观察同一测试线程。
            assert_eq!(unsafe { GetFocus() }, original_focus);
            main.text(QUERY, "𠀀ü漢");
            assert_eq!(query(&main).unwrap(), "𠀀ü漢");
            main.text(QUERY, &"𠀀".repeat(65));
            assert!(query(&main).is_err());
            let options = Options {
                auto_commit_four: true,
                common_only: false,
                show_learning: true,
                output_form: OutputForm::Traditional,
                theme: Theme::Ink,
                candidate_font: CandidateFont::KaiTi,
                page_size: 7,
                ..defaults
            };
            set_options(&settings, options);
            assert_eq!(read_options(settings.handle).unwrap(), options);
            // 偏蓝或格式不对的自定义基色在本地即被拒绝，不发往后台，提示写明原因。
            settings.text(CUSTOM + 3, "#0078D4");
            assert!(read_options(settings.handle).unwrap_err().contains("偏蓝"));
            assert!(custom_status(settings.handle).starts_with('✗'));
            settings.text(CUSTOM + 3, "12345");
            assert!(read_options(settings.handle).is_err());
            set_options(&settings, options);
            assert!(custom_status(settings.handle).starts_with('✓'));
            // 分区切换只显示该区控件；选了自定义才露出四色编辑行。
            select_page(&mut settings, 1, defaults.candidate_font).unwrap();
            let on = |window: &Window, id| shown(window.control(id));
            assert!(on(&settings, PREVIEW) && !on(&settings, AUTO) && !on(&settings, CUSTOM));
            check(&settings, THEME + 1, false);
            check(&settings, THEME + 4, true);
            settings.layout(defaults.candidate_font).unwrap();
            assert!(on(&settings, CUSTOM) && on(&settings, SWATCH + 3));
            select_page(&mut settings, 0, defaults.candidate_font).unwrap();
            assert!(on(&settings, AUTO) && !on(&settings, CUSTOM) && !on(&settings, PREVIEW));
            set_options(&settings, defaults);
            settings.layout(defaults.candidate_font).unwrap();

            // 隐藏窗口仍走真实消息泵和事件等待，证明查询回复与工作线程退出都能被观察到。
            main.text(QUERY, "𠀀");
            let mut model = Model::new(defaults);
            // 唤起邮箱保持连接，代表后台仍在运行。
            let (show_sender, show) = crate::mailbox::<Panel>().unwrap();
            let (requests, receiver) = std::sync::mpsc::sync_channel(1);
            let (replies, inbox) = crate::mailbox().unwrap();
            let worker = std::thread::spawn(move || {
                let request = receiver
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
                assert!(matches!(request, Request::Lookup { query, page: 0 } if query == "𠀀"));
                replies
                    .send(Reply::Lookup(Ok(tigertype_contracts::LookupPage {
                        items: vec![tigertype_contracts::LookupItem {
                            text: "𠀀".into(),
                            learning: tigertype_contracts::LearningInfo {
                                codes: vec!["ab".into(), "abc".into()],
                                ..Default::default()
                            },
                        }],
                        has_more: false,
                    })))
                    .unwrap();
            });
            post(main.handle, ACTION, SEARCH);
            let run = |main: &mut Window,
                       settings: &mut Window,
                       model: &mut Model,
                       requests: &SyncSender<Request>,
                       inbox: &Inbox<Reply>| {
                event_loop(
                    main,
                    settings,
                    model,
                    requests,
                    inbox,
                    &show,
                    (Path::new("settings.toml"), None),
                )
            };
            assert!(
                run(&mut main, &mut settings, &mut model, &requests, &inbox)
                    .unwrap_err()
                    .contains("工作线程已退出")
            );
            worker.join().unwrap();
            assert_eq!(model.result.as_ref().unwrap().items[0].text, "𠀀");
            assert!(!model.busy);
            assert_eq!(main.send(LIST, LB_GETCOUNT, 0, 0), 1);
            let mut buffer = [0u16; 64];
            let length = window_text(main.control(GLYPH), &mut buffer);
            assert_eq!(String::from_utf16_lossy(&buffer[..length]), "𠀀");
            let length = window_text(main.control(CODES), &mut buffer);
            assert_eq!(String::from_utf16_lossy(&buffer[..length]), "ab abc");
            let length = window_text(main.control(PARTS), &mut buffer);
            assert_eq!(String::from_utf16_lossy(&buffer[..length]), "缺资料");
            assert!(!shown(main.control(STATUS)));

            // 即时生效：点一下开关就发出保存，回复后换上新设置，并不改动查字结果与选择。
            let (requests, receiver) = std::sync::mpsc::sync_channel(1);
            let (replies, inbox) = crate::mailbox().unwrap();
            check(&settings, AUTO, true);
            command(&settings, AUTO, BN_CLICKED);
            let worker = std::thread::spawn(move || {
                let request = receiver
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
                let Request::SaveSettings(saved) = request else {
                    panic!("开关须立即保存");
                };
                assert!(saved.auto_commit_four);
                replies.send(Reply::Saved(Ok(saved))).unwrap();
            });
            assert!(run(&mut main, &mut settings, &mut model, &requests, &inbox).is_err());
            worker.join().unwrap();
            assert!(model.options.auto_commit_four);
            assert_eq!(main.send(LIST, LB_GETCURSEL, 0, 0), 0);

            // 换主题：回复后整套界面换上新配色与字体。
            let (requests, receiver) = std::sync::mpsc::sync_channel(1);
            let (replies, inbox) = crate::mailbox().unwrap();
            check(&settings, THEME, false);
            check(&settings, THEME + 1, true);
            check(&settings, FONT, false);
            check(&settings, FONT + 1, true);
            command(&settings, THEME + 1, BN_CLICKED);
            let worker = std::thread::spawn(move || {
                let Request::SaveSettings(saved) = receiver
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap()
                else {
                    panic!("换主题须立即保存");
                };
                replies.send(Reply::Saved(Ok(saved))).unwrap();
            });
            assert!(run(&mut main, &mut settings, &mut model, &requests, &inbox).is_err());
            worker.join().unwrap();
            assert_eq!(model.options.theme, Theme::Ink);
            assert_eq!(PALETTE.with(Cell::get), model.options.palette());
            assert_eq!(main.face, CandidateFont::KaiTi);

            // 保存失败：控件撤回到已确认设置，提示写明原因。
            let (requests, receiver) = std::sync::mpsc::sync_channel(1);
            let (replies, inbox) = crate::mailbox().unwrap();
            check(&settings, COMMON, false);
            command(&settings, COMMON, BN_CLICKED);
            let worker = std::thread::spawn(move || {
                receiver
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
                replies
                    .send(Reply::Saved(Err("settings.toml 已在外部修改".into())))
                    .unwrap();
            });
            assert!(run(&mut main, &mut settings, &mut model, &requests, &inbox).is_err());
            worker.join().unwrap();
            assert!(checked_in(settings.handle, COMMON));
            assert_eq!(read_options(settings.handle).unwrap(), model.options);
            assert!(model.settings_notice.contains("外部修改"));

            // 重新载入：控件换成文件里的设置。
            let (requests, receiver) = std::sync::mpsc::sync_channel(1);
            let (replies, inbox) = crate::mailbox().unwrap();
            let edited = Options {
                page_size: 9,
                ..model.options
            };
            command(&settings, RELOAD, BN_CLICKED);
            let worker = std::thread::spawn(move || {
                assert!(matches!(
                    receiver
                        .recv_timeout(std::time::Duration::from_secs(2))
                        .unwrap(),
                    Request::ReloadSettings
                ));
                replies.send(Reply::Saved(Ok(edited))).unwrap();
            });
            assert!(run(&mut main, &mut settings, &mut model, &requests, &inbox).is_err());
            worker.join().unwrap();
            assert_eq!(model.options, edited);
            assert_eq!(read_options(settings.handle).unwrap(), edited);

            // 自定义色值不合格时侧栏写明未保存；换回预设主题时四色退回已保存的值，换主题照常保存。
            let (requests, receiver) = std::sync::mpsc::sync_channel(1);
            let (replies, inbox) = crate::mailbox().unwrap();
            for k in 0..Theme::ALL.len() {
                check(&settings, THEME + k, k == 4);
            }
            settings.text(CUSTOM + 3, "#0078D4");
            assert!(commit(&settings, &mut model).is_none());
            assert!(model.settings_notice.starts_with('✗'));
            for k in 0..Theme::ALL.len() {
                check(&settings, THEME + k, k == 0);
            }
            command(&settings, THEME, BN_CLICKED);
            let kept = model.options.custom;
            let worker = std::thread::spawn(move || {
                let Request::SaveSettings(saved) = receiver
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap()
                else {
                    panic!("换预设主题须保存");
                };
                assert_eq!((saved.theme, saved.custom), (Theme::Amber, kept));
                replies.send(Reply::Saved(Ok(saved))).unwrap();
            });
            assert!(run(&mut main, &mut settings, &mut model, &requests, &inbox).is_err());
            worker.join().unwrap();
            assert_eq!(model.options.theme, Theme::Amber);
            assert_eq!(read_options(settings.handle).unwrap(), model.options);

            // 后台常驻：关闭主窗只收起（连同设置窗），循环继续；本例由随后的 WM_QUIT 结束。
            let (requests, _receiver) = std::sync::mpsc::sync_channel(1);
            let (_replies, inbox) = crate::mailbox().unwrap();
            post(main.handle, CLOSE, 0);
            // SAFETY: 只向本测试线程的消息队列投递退出消息，排在 CLOSE 之后取出。
            unsafe { PostQuitMessage(0) };
            assert!(run(&mut main, &mut settings, &mut model, &requests, &inbox).is_ok());
            let mut message = MSG::default();
            // 确认退出消息已被循环取走，而不是 CLOSE 提前返回后残留在队列里。
            // SAFETY: MSG 栈对象完整有效，只查看本线程队列。
            let left =
                unsafe { PeekMessageW(&mut message, null_mut(), WM_QUIT, WM_QUIT, PM_REMOVE) };
            assert_eq!(left, 0);
            // 唤起邮箱断开即后台停止，UI 线程正常退出。
            drop(show_sender);
            assert!(run(&mut main, &mut settings, &mut model, &requests, &inbox).is_ok());
            window_menu_closes_when_the_backend_stops(&mut main, &mut settings, &mut model);
            PALETTE.with(|palette| palette.set(Options::default().palette()));
            // 反复创建可发现遗漏的窗口类注销，避免把线程退出误当资源已释放。
            drop(settings);
            drop(main);
            drop(class);
        }
    }
}
