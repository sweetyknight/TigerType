//! Win32 标准控件提供键盘导航和系统可访问性；不启用系统控件主题（其悬停、焦点、勾选都是蓝色），
//! 外观由父窗口与子类化按钮按主题自绘。回调无业务状态、无分配、无可展开代码：
//! 配色取本 UI 线程的绘制配色，字体取控件自身，布局按客户区尺寸现算，不在窗口数据里存指针。
use crate::{
    Inbox, Latest,
    model::{Model, details},
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
    CandidateFont, CustomColors, Options, OutputForm, Palette, Theme, TypingMode,
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
        SystemServices::{SS_CENTER, SS_CENTERIMAGE, SS_NOPREFIX, SS_OWNERDRAW},
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
            ODS_SELECTED, ODT_LISTBOX, ODT_STATIC,
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

mod tray;

const ACTION: u32 = WM_APP + 1;
const CLOSE: u32 = WM_APP + 2;
const LAYOUT: u32 = WM_APP + 3;
const QUERY: usize = 101;
const SEARCH: usize = 102;
const LIST: usize = 103;
const PREVIOUS: usize = 104;
const NEXT: usize = 105;
const COPY: usize = 106;
const SETTINGS: usize = 107;
const STATUS: usize = 108;
const PAGE: usize = 109;
const GLYPH: usize = 110;
const CODEPOINT: usize = 111;
const QUERY_LABEL: usize = 112;
/// 四项资料：值为无边框只读 EDIT（可选中复制），标签为 STATIC。
const CODES: usize = 113;
const FULL: usize = 114;
const READING: usize = 115;
const PARTS: usize = 116;
const FIELD_LABEL: usize = 117;
/// 隐藏的多行 EDIT，只用来把整页资料交给标准复制。
const CLIP: usize = 121;
const AUTO: usize = 201;
const COMMON: usize = 202;
const LEARNING: usize = 203;
const SAVE: usize = 205;
const CANCEL: usize = 206;
const OPEN_CONFIG: usize = 207;
const NOTICE: usize = 211;
const HEADING: usize = 212;
const SUB: usize = 213;
const ROW_TITLE: usize = 260;
const ROW_DESC: usize = 270;
const FORM: usize = 220;
const FONT: usize = 230;
const THEME: usize = 240;
/// 自定义四基色（背景、文字、高亮、强调）的十六进制输入框。
const CUSTOM: usize = 250;
const MODE: usize = 280;

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
    (COMMON, "普通输入仅常用字(&C)", "关闭后普通输入也使用全字集"),
    (
        LEARNING,
        "显示拆分与编码(&L)",
        "在候选旁显示〔拆分 · 编码〕，如〔白勹丶 · unid〕",
    ),
];
/// 三组分段单选：行标题、说明、首个控件编号、各段宽度（96 DPI）。
const CHOICES: [(&str, &str, usize, i32); 4] = [
    ("输出字形(&O)", "上屏前转换为所选字形", FORM, 52),
    ("候选字体(&F)", "候选窗与查字字形所用字体", FONT, 80),
    ("配色主题(&T)", "全部界面所用配色", THEME, 48),
    ("输入模式(&M)", "普通输入使用单字或词码表", MODE, 64),
];
const UI_FACE: &str = "Microsoft YaHei UI";
/// 列表每项高度（96 DPI）。
const ITEM_HEIGHT: u32 = 44;

/// 收起系统模态循环：唤起邮箱在模态期间有信号（含后台停止）时由线程池回调投递给进入模态的窗口。
const UNBLOCK: u32 = WM_APP + 6;

thread_local! {
    // 本 UI 线程的绘制配色：只在确认设置后由本线程写入，系统回调只读；不是业务状态。
    static PALETTE: Cell<Palette> = Cell::new(Options::default().palette());
    // 唤起邮箱事件与模态期间登记的线程池等待；只在本 UI 线程读写，是基础设施而非业务状态。
    static WAKE: Cell<HANDLE> = const { Cell::new(null_mut()) };
    static WAIT: Cell<HANDLE> = const { Cell::new(null_mut()) };
}

/// 登记唤起邮箱：菜单（托盘右键、窗口系统菜单）与移动/缩放都是系统模态循环，期间事件循环不运行；
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
/// 只在用户自己开启高对比度时沿用系统颜色保证可读。
#[derive(Clone, Copy, PartialEq, Debug)]
struct Colors {
    window: COLORREF,
    surface: COLORREF,
    line: COLORREF,
    field: COLORREF,
    text: COLORREF,
    muted: COLORREF,
    faint: COLORREF,
    highlight: COLORREF,
    highlight_text: COLORREF,
    accent: COLORREF,
    on_accent: COLORREF,
    selection: COLORREF,
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
        muted: rgb(p.muted),
        faint: rgb(p.faint),
        highlight: rgb(p.highlight),
        highlight_text: rgb(p.highlight_text),
        accent: rgb(p.accent),
        on_accent: rgb(p.on_accent),
        selection: rgb(p.selection),
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
        muted: text,
        faint: system(COLOR_GRAYTEXT),
        highlight: system(COLOR_HIGHLIGHT),
        highlight_text: system(COLOR_HIGHLIGHTTEXT),
        accent: system(COLOR_HIGHLIGHT),
        on_accent: system(COLOR_HIGHLIGHTTEXT),
        selection: system(COLOR_HIGHLIGHT),
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
/// 键盘焦点环：2px 正文色描边，替代系统虚线框。
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
fn draw_text(dc: HDC, font: HFONT, color: COLORREF, text: &[u16], rect: RECT, format: u32) {
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
fn window_text(hwnd: HWND, buffer: &mut [u16; 64]) -> usize {
    // SAFETY: 缓冲区长度与传入容量相同。
    unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32).max(0) as usize }
}
fn font_of(hwnd: HWND) -> HFONT {
    // SAFETY: 同步查询控件字体；字体由所属 Window 持有。
    unsafe { SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT }
}
fn sibling_font(hwnd: HWND, id: usize) -> HFONT {
    // SAFETY: 只在同一父窗口内按编号找控件。
    font_of(unsafe { GetDlgItem(GetParent(hwnd), id as i32) })
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

// SAFETY: 系统在所属 UI 线程调用；先交 EDIT 原过程处理，再在本次回调内按主题重画可见选区；
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
        // 输入框外框的强调底边随焦点变化，由父窗口重画。
        // SAFETY: 只使父窗口失效，绘制在其 WM_PAINT 中进行。
        unsafe {
            InvalidateRect(GetParent(hwnd), null(), 0);
        }
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
        // 资料框一页至多九字、数十行；上限只防异常输入拖慢绘制。
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

/// 按钮三种外观：普通按钮（主/次/无框）、开关行（复选框）、分段单选。
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
    match kind {
        BS_AUTOCHECKBOX => {
            fill(dc, &rect, c.surface);
            let description = TOGGLES
                .iter()
                .find(|(key, ..)| *key == id)
                .map_or("", |(.., description)| *description);
            let left = rect.left + s(16);
            let title = RECT {
                left,
                top: rect.top + s(9),
                right: rect.right - s(120),
                bottom: rect.top + s(31),
            };
            draw_text(dc, font, color(c.text), text, title, prefix);
            let small = sibling_font(hwnd, SUB);
            let line = RECT {
                top: rect.top + s(31),
                bottom: rect.top + s(51),
                ..title
            };
            draw_text(
                dc,
                small,
                c.muted,
                &wide(description)[..description.encode_utf16().count()],
                line,
                DT_NOPREFIX,
            );
            let top = (rect.top + rect.bottom - s(20)) / 2;
            let track = RECT {
                left: rect.right - s(16 + 40),
                top,
                right: rect.right - s(16),
                bottom: top + s(20),
            };
            let knob_left = if checked {
                track.right - s(16)
            } else {
                track.left + s(4)
            };
            let knob = RECT {
                left: knob_left,
                top: top + s(4),
                right: knob_left + s(12),
                bottom: top + s(16),
            };
            if checked {
                round(dc, &track, s(10), Some(color(c.accent)), c.accent);
                round(dc, &knob, s(6), Some(c.on_accent), c.on_accent);
            } else {
                round(dc, &track, s(10), None, color(c.muted));
                round(dc, &knob, s(6), Some(color(c.muted)), c.muted);
            }
            let state = RECT {
                left: track.left - s(40),
                right: track.left - s(10),
                ..track
            };
            let word: &[u16] = &[if checked { 0x5F00 } else { 0x5173 }];
            draw_text(dc, small, c.muted, word, state, DT_RIGHT | DT_NOPREFIX);
            if focused {
                let around = RECT {
                    left: track.left - s(3),
                    top: track.top - s(3),
                    right: track.right + s(3),
                    bottom: track.bottom + s(3),
                };
                ring(dc, &around, s(13), c.text);
            }
        }
        BS_AUTORADIOBUTTON => {
            fill(dc, &rect, c.window);
            let (back, fore) = if checked {
                (Some(color(c.accent)), c.on_accent)
            } else {
                (None, color(c.text))
            };
            if let Some(back) = back {
                round(dc, &rect, s(4), Some(back), back);
            }
            draw_text(dc, font, fore, text, rect, DT_CENTER | prefix);
            if focused {
                ring(dc, &rect, s(4), if checked { c.on_accent } else { c.text });
            }
        }
        _ => {
            let flat = matches!(id, PREVIOUS | NEXT);
            fill(dc, &rect, if flat { c.surface } else { c.window });
            // 对话框管理器会随焦点改写 BS_DEFPUSHBUTTON 样式，主按钮按编号认定。
            let primary = matches!(id, SEARCH | SAVE);
            let fore = match (primary, enabled) {
                (true, true) => {
                    round(dc, &rect, s(6), Some(c.accent), c.accent);
                    c.on_accent
                }
                (true, false) => {
                    round(dc, &rect, s(6), Some(c.line), c.line);
                    c.faint
                }
                (false, _) => {
                    if pushed {
                        round(dc, &rect, s(6), Some(c.highlight), c.highlight);
                    } else if !flat {
                        round(dc, &rect, s(6), Some(c.surface), c.surface);
                    }
                    if !flat {
                        round(dc, &rect, s(6), None, c.field);
                    }
                    color(c.text)
                }
            };
            draw_text(dc, font, fore, text, rect, DT_CENTER | prefix);
            if focused {
                ring(dc, &rect, s(6), if primary { c.on_accent } else { c.text });
            }
        }
    }
}

/// 自绘列表项与字形格：选中项为主题高亮底与左侧指示条，字形格带田字格虚线。
fn draw_item(item: &DRAWITEMSTRUCT) {
    let c = colors();
    let s = |value| scale(item.hwndItem, value);
    let rect = item.rcItem;
    let dc = item.hDC;
    if item.CtlType == ODT_STATIC {
        fill(dc, &rect, c.surface);
        round(dc, &rect, s(8), Some(c.window), c.window);
        round(dc, &rect, s(8), None, c.line);
        // SAFETY: 新建虚线画笔用后即删；直线端点为本地值。
        unsafe {
            let pen = CreatePen(PS_DOT, 1, c.line);
            let old = SelectObject(dc, pen);
            let (x, y) = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
            for (from, to) in [
                (
                    POINT {
                        x,
                        y: rect.top + s(8),
                    },
                    POINT {
                        x,
                        y: rect.bottom - s(8),
                    },
                ),
                (
                    POINT {
                        x: rect.left + s(8),
                        y,
                    },
                    POINT {
                        x: rect.right - s(8),
                        y,
                    },
                ),
            ] {
                MoveToEx(dc, from.x, from.y, null_mut());
                LineTo(dc, to.x, to.y);
            }
            SelectObject(dc, old);
            DeleteObject(pen);
        }
        let mut buffer = [0u16; 64];
        let length = window_text(item.hwndItem, &mut buffer);
        draw_text(
            dc,
            font_of(item.hwndItem),
            c.text,
            &buffer[..length],
            rect,
            DT_CENTER | DT_NOPREFIX,
        );
        return;
    }
    let chosen = item.itemState & ODS_SELECTED != 0;
    fill(dc, &rect, c.surface);
    if item.itemID == u32::MAX {
        return;
    }
    if chosen {
        round(dc, &rect, s(4), Some(c.highlight), c.highlight);
        let top = (rect.top + rect.bottom - s(18)) / 2;
        let bar = RECT {
            left: rect.left,
            top,
            right: rect.left + s(3),
            bottom: top + s(18),
        };
        fill(dc, &bar, c.accent);
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
    // 项目文字为“字 编码”：字用列表字体，编码用资料值的等宽字体。
    let text = &buffer[..length];
    let split = text
        .iter()
        .position(|&unit| unit == u16::from(b' '))
        .unwrap_or(length);
    let disabled = item.itemState & ODS_DISABLED != 0;
    let glyph = RECT {
        left: rect.left + s(14),
        right: rect.left + s(52),
        ..rect
    };
    draw_text(
        dc,
        font_of(item.hwndItem),
        if disabled { c.faint } else { c.text },
        &text[..split],
        glyph,
        DT_NOPREFIX,
    );
    let code = RECT {
        left: glyph.right,
        right: rect.right - s(8),
        ..rect
    };
    let hint = if chosen { c.highlight_text } else { c.muted };
    draw_text(
        dc,
        sibling_font(item.hwndItem, CODES),
        hint,
        &text[split.min(length)..],
        code,
        DT_NOPREFIX | DT_END_ELLIPSIS,
    );
    if item.itemState & ODS_FOCUS != 0 && item.itemState & ODS_NOFOCUSRECT == 0 {
        ring(dc, &rect, s(4), c.text);
    }
}

type Box4 = [i32; 4];
/// 查字窗布局（96 DPI 逻辑坐标，按客户区尺寸现算）：控件位置与需由父窗口画的卡片/分隔线。
struct MainSpec {
    controls: Vec<(usize, Box4)>,
    field: Box4,
    cards: [Box4; 2],
    lines: [Box4; 2],
}
fn main_spec(width: i32, height: i32) -> MainSpec {
    const M: i32 = 20;
    let card_top = 92;
    let card_bottom = (height - 68).max(card_top + 160);
    let card_height = card_bottom - card_top;
    let field = [M, 36, (width - 2 * M - 98).max(120), 40];
    let detail = [
        M + 232,
        card_top,
        (width - 2 * M - 232).max(240),
        card_height,
    ];
    let x0 = detail[0] + 28;
    let y0 = card_top + 28;
    let x1 = x0 + 128 + 32;
    let value_width = (detail[0] + detail[2] - 28 - (x1 + 96)).max(60);
    let mut controls = vec![
        (QUERY_LABEL, [M, 12, width - 2 * M, 20]),
        (QUERY, [field[0] + 12, 44, field[2] - 24, 24]),
        (SEARCH, [width - M - 88, 36, 88, 40]),
        (LIST, [M + 4, card_top + 4, 208, card_height - 52]),
        (PREVIOUS, [M + 6, card_bottom - 40, 64, 32]),
        (PAGE, [M + 72, card_bottom - 40, 72, 32]),
        (NEXT, [M + 146, card_bottom - 40, 64, 32]),
        (GLYPH, [x0, y0, 128, 128]),
        (CODEPOINT, [x0, y0 + 136, 128, 20]),
        (
            STATUS,
            [M, height - 52 + 14, (width - 2 * M - 240).max(80), 24],
        ),
        (COPY, [width - M - 88 - 8 - 132, height - 52 + 9, 132, 34]),
        (SETTINGS, [width - M - 88, height - 52 + 9, 88, 34]),
        (CLIP, [0, 0, 0, 0]),
    ];
    for (index, value) in [CODES, FULL, READING, PARTS].into_iter().enumerate() {
        let top = y0 + 4 + index as i32 * 40;
        controls.push((FIELD_LABEL + index, [x1, top, 84, 26]));
        controls.push((value, [x1 + 96, top, value_width, 26]));
    }
    MainSpec {
        controls,
        field,
        cards: [[M, card_top, 216, card_height], detail],
        lines: [
            [M + 4, card_bottom - 44, 208, 1],
            [0, height - 52, width, 1],
        ],
    }
}
/// 设置窗为固定尺寸对话框：八行卡片，前三行为开关，随后四行为分段单选，末行为自定义四基色。
const SETTINGS_SIZE: (i32, i32) = (560, 648);
const ROW_TOP: i32 = 80;
const ROW: i32 = 60;
const ROWS: i32 = 8;
const FOOTER_TOP: i32 = ROW_TOP + ROWS * ROW + 16;
/// 自定义四基色输入框外框（96 DPI）：第 k 个框。
fn custom_frame(k: i32) -> Box4 {
    let (width, _) = SETTINGS_SIZE;
    [
        width - 40 - (4 - k) * 84 + 6,
        ROW_TOP + 7 * ROW + 14,
        78,
        32,
    ]
}
fn settings_spec() -> Vec<(usize, Box4)> {
    let (width, _) = SETTINGS_SIZE;
    let mut controls = vec![
        (HEADING, [24, 14, width - 48, 30]),
        (SUB, [24, 48, width - 48, 20]),
        (NOTICE, [160, FOOTER_TOP + 10, width - 48 - 336, 44]),
        (OPEN_CONFIG, [24, FOOTER_TOP + 15, 128, 34]),
        (CANCEL, [width - 24 - 88 - 8 - 88, FOOTER_TOP + 15, 88, 34]),
        (SAVE, [width - 24 - 88, FOOTER_TOP + 15, 88, 34]),
        (ROW_TITLE + 4, [41, ROW_TOP + 7 * ROW + 9, 140, 22]),
        (ROW_DESC + 4, [41, ROW_TOP + 7 * ROW + 31, 140, 20]),
    ];
    for k in 0..4 {
        let [x, y, w, h] = custom_frame(k);
        controls.push((CUSTOM + k as usize, [x + 8, y + 5, w - 16, h - 10]));
    }
    for (index, (id, ..)) in TOGGLES.iter().enumerate() {
        controls.push((
            *id,
            [25, ROW_TOP + index as i32 * ROW + 1, width - 50, ROW - 2],
        ));
    }
    for (index, (_, _, first, segment)) in CHOICES.iter().enumerate() {
        let top = ROW_TOP + (3 + index as i32) * ROW;
        controls.push((ROW_TITLE + index, [41, top + 9, 180, 22]));
        controls.push((ROW_DESC + index, [41, top + 31, 180, 20]));
        let count = choice_count(*first) as i32;
        let left = width - 40 - count * segment - 4;
        for k in 0..count {
            controls.push((
                first + k as usize,
                [left + 2 + k * segment, top + 17, *segment, 26],
            ));
        }
    }
    controls
}
fn choice_count(first: usize) -> usize {
    match first {
        FORM => FORMS.len(),
        FONT => CandidateFont::ALL.len(),
        THEME => Theme::ALL.len(),
        MODE => TypingMode::ALL.len(),
        _ => Theme::ALL.len(),
    }
}

/// 父窗口自绘底色、卡片、输入框外框与分隔线；全部由客户区尺寸与控件状态现算。
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
        let r = |[x, y, w, h]: Box4| RECT {
            left: x * dpi / 96,
            top: y * dpi / 96,
            right: (x + w) * dpi / 96,
            bottom: (y + h) * dpi / 96,
        };
        let s = |value: i32| value * dpi / 96;
        let query = GetDlgItem(hwnd, QUERY as i32);
        if !query.is_null() {
            let spec = main_spec(client.right * 96 / dpi, client.bottom * 96 / dpi);
            for card in spec.cards {
                round(dc, &r(card), s(8), Some(c.surface), c.surface);
                round(dc, &r(card), s(8), None, c.line);
            }
            for line in spec.lines {
                let mut line = r(line);
                line.bottom = line.top + s(1).max(1);
                fill(dc, &line, c.line);
            }
            let field = r(spec.field);
            round(dc, &field, s(6), Some(c.surface), c.surface);
            round(dc, &field, s(6), None, c.field);
            if GetFocus() == query {
                let bottom = RECT {
                    left: field.left + s(1),
                    top: field.bottom - s(2),
                    right: field.right - s(1),
                    bottom: field.bottom,
                };
                fill(dc, &bottom, c.accent);
            }
        } else {
            let (width, _) = SETTINGS_SIZE;
            let card = r([24, ROW_TOP, width - 48, ROWS * ROW]);
            round(dc, &card, s(8), Some(c.surface), c.surface);
            round(dc, &card, s(8), None, c.line);
            for k in 0..4 {
                round(dc, &r(custom_frame(k)), s(6), None, c.field);
            }
            for index in 1..ROWS {
                let line = r([25, ROW_TOP + index * ROW, width - 50, 1]);
                fill(
                    dc,
                    &RECT {
                        bottom: line.top + s(1).max(1),
                        ..line
                    },
                    c.line,
                );
            }
            for (index, (_, _, first, segment)) in CHOICES.iter().enumerate() {
                let top = ROW_TOP + (3 + index as i32) * ROW;
                let count = choice_count(*first) as i32;
                let group = r([
                    width - 40 - count * segment - 4,
                    top + 15,
                    count * segment + 4,
                    30,
                ]);
                round(dc, &group, s(6), Some(c.window), c.window);
                round(dc, &group, s(6), None, c.field);
            }
            let footer = r([0, FOOTER_TOP, width, 1]);
            fill(
                dc,
                &RECT {
                    bottom: footer.top + s(1).max(1),
                    ..footer
                },
                c.line,
            );
        }
        EndPaint(hwnd, &paint);
    }
}

/// 卡片内的控件用卡片底色，说明类文字用次要色。
fn on_surface(id: usize) -> bool {
    matches!(id, QUERY | LIST | PAGE | GLYPH | CODEPOINT | CODES..=PARTS)
        || (FIELD_LABEL..FIELD_LABEL + 4).contains(&id)
        || (ROW_TITLE..ROW_TITLE + 5).contains(&id)
        || (ROW_DESC..ROW_DESC + 5).contains(&id)
        || (CUSTOM..CUSTOM + 4).contains(&id)
}
fn muted(id: usize) -> bool {
    matches!(id, QUERY_LABEL | STATUS | PAGE | CODEPOINT | SUB | NOTICE)
        || (FIELD_LABEL..FIELD_LABEL + 4).contains(&id)
        || (ROW_DESC..ROW_DESC + 5).contains(&id)
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
                let back = if on_surface(id) { c.surface } else { c.window };
                let dc = w as HDC;
                SetTextColor(dc, if muted(id) { c.muted } else { c.text });
                SetBkColor(dc, back);
                SetDCBrushColor(dc, back);
                GetStockObject(DC_BRUSH) as LRESULT
            }
            WM_MEASUREITEM if l != 0 => {
                let item = &mut *(l as *mut MEASUREITEMSTRUCT);
                if item.CtlType != ODT_LISTBOX {
                    return DefWindowProcW(hwnd, message, w, l);
                }
                item.itemHeight = ITEM_HEIGHT * GetDpiForWindow(hwnd).max(96) / 96;
                1
            }
            WM_DRAWITEM if l != 0 => {
                let item = &*(l as *const DRAWITEMSTRUCT);
                if item.CtlType != ODT_LISTBOX && item.CtlType != ODT_STATIC {
                    return DefWindowProcW(hwnd, message, w, l);
                }
                draw_item(item);
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
                    info.ptMinTrackSize.x = 720 * dpi / 96;
                    info.ptMinTrackSize.y = 520 * dpi / 96;
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
}
fn role(id: usize) -> Role {
    match id {
        HEADING => Role::Heading,
        QUERY | CODES | FULL | READING | CUSTOM..=253 => Role::Mono,
        LIST | PARTS => Role::List,
        GLYPH => Role::Glyph,
        _ if muted(id) => Role::Small,
        _ => Role::Body,
    }
}

struct Window {
    handle: HWND,
    controls: Vec<(usize, HWND)>,
    fonts: [HFONT; 6],
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
                940,
                700,
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
            fonts: [null_mut(); 6],
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
        // SAFETY: 控件归本视图持有，文本在同步调用期间存活。
        unsafe {
            SetWindowTextW(self.control(id), wide(text).as_ptr());
        }
    }
    fn enable(&self, id: usize, enabled: bool) {
        // SAFETY: 控件只在创建和销毁之间由所属 UI 线程访问。
        unsafe {
            EnableWindow(self.control(id), enabled.into());
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
        let mut fonts = [null_mut(); 6];
        let specs: [(&[&str], i32, i32); 6] = [
            (&[UI_FACE], 15, 400),
            (&[UI_FACE], 13, 400),
            (&[UI_FACE], 20, 600),
            (&["Cascadia Mono", "Consolas"], 17, 400),
            (&[face.face(), UI_FACE], 22, 400),
            (&[face.face(), UI_FACE], 88, 400),
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
        // EDIT 换字体会按字号重设左右边距，大字号的“拆分”会比其他资料值右移；统一清零。
        for (id, control) in &self.controls {
            if (CODES..=PARTS).contains(id) {
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
        let spec = if self.controls.iter().any(|(id, _)| *id == QUERY) {
            let dpi = self.dpi as i32;
            main_spec(rect.right * 96 / dpi, rect.bottom * 96 / dpi).controls
        } else {
            settings_spec()
        };
        for (id, place) in spec {
            self.place(id, place);
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
    /// Win11 标题栏与边框改用主题色，不随系统强调色；Win10 不支持时保持系统默认。
    fn decorate(&self, palette: Palette) {
        if high_contrast() {
            return;
        }
        for (attribute, value) in [
            (DWMWA_CAPTION_COLOR, rgb(palette.window)),
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
}
fn release(fonts: [HFONT; 6]) {
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
        // SAFETY: 子控件先随窗口销毁，之后释放字体；均在创建它们的 UI 线程执行。
        unsafe {
            DestroyWindow(self.handle);
        }
        release(self.fonts);
    }
}

struct Class(Vec<u16>);
impl Class {
    fn register() -> Result<Self, String> {
        let name = wide("TigerType.Lookup.Native.v2");
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

fn lookup_window(class: &Class, options: &Options) -> Result<Window, String> {
    let mut window = Window::create(&class.0, "TigerType 查字", null_mut(), WS_OVERLAPPEDWINDOW)?;
    window.add(QUERY_LABEL, "STATIC", "拼音或汉字（最多 64 字）(&Q)", 0)?;
    window.add(QUERY, "EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL as u32)?;
    // 粘贴上限保留足够余量；超过业务上限明确报错，不把截断文本当成有效查询。
    window.send(QUERY, EM_SETLIMITTEXT, 4096, 0);
    window.add(
        SEARCH,
        "BUTTON",
        "查询(&F)",
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
    )?;
    window.add(
        LIST,
        "LISTBOX",
        "",
        WS_TABSTOP
            | WS_VSCROLL
            | (LBS_OWNERDRAWFIXED | LBS_HASSTRINGS | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
    )?;
    window.add(GLYPH, "STATIC", "", SS_OWNERDRAW)?;
    window.add(CODEPOINT, "STATIC", "", SS_CENTER)?;
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
        window.add(
            value,
            "EDIT",
            "",
            WS_TABSTOP | ES_AUTOHSCROLL as u32 | ES_READONLY as u32,
        )?;
    }
    for (id, title) in [(PREVIOUS, "‹ 上页(&P)"), (NEXT, "下页(&N) ›")] {
        window.add(id, "BUTTON", title, WS_TABSTOP)?;
    }
    window.add(PAGE, "STATIC", "", SS_CENTER | SS_CENTERIMAGE)?;
    window.add(STATUS, "STATIC", "", SS_CENTERIMAGE | SS_NOPREFIX)?;
    for (id, title) in [(COPY, "复制本页资料(&C)"), (SETTINGS, "设置(&S)")] {
        window.add(id, "BUTTON", title, WS_TABSTOP)?;
    }
    // 隐藏控件不显示、不可聚焦，只承载整页资料供标准复制。
    window.add(CLIP, "EDIT", "", ES_MULTILINE as u32 | ES_READONLY as u32)?;
    // SAFETY: 隐藏刚创建的控件。
    unsafe {
        ShowWindow(window.control(CLIP), SW_HIDE);
    }
    window.decorate(options.palette());
    window.layout(options.candidate_font)?;
    // 初始尺寸按窗口 DPI 换算，高缩放下不被夹到最小尺寸。
    let dpi = window.dpi as i32;
    // SAFETY: 只调整本窗口尺寸，不移动、不激活。
    unsafe {
        SetWindowPos(
            window.handle,
            null_mut(),
            0,
            0,
            940 * dpi / 96,
            700 * dpi / 96,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
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
fn settings_window(class: &Class, owner: HWND, options: &Options) -> Result<Window, String> {
    let mut window = Window::create(&class.0, "TigerType 设置", owner, WS_CAPTION | WS_SYSMENU)?;
    window.add(HEADING, "STATIC", "设置", SS_NOPREFIX)?;
    window.add(
        SUB,
        "STATIC",
        "输入选项只影响普通输入；保存后即时生效。快捷键：单击 Shift 中英，Ctrl+. 中英文标点，Ctrl+L 学习信息，↑/↓选词。",
        SS_NOPREFIX,
    )?;
    for (index, (id, title, _)) in TOGGLES.iter().enumerate() {
        let group = if index == 0 { WS_GROUP } else { 0 };
        window.add(
            *id,
            "BUTTON",
            title,
            WS_TABSTOP | group | BS_AUTOCHECKBOX as u32,
        )?;
    }
    for (index, (title, description, first, _)) in CHOICES.iter().enumerate() {
        window.add(ROW_TITLE + index, "STATIC", title, SS_CENTERIMAGE)?;
        window.add(ROW_DESC + index, "STATIC", description, SS_NOPREFIX)?;
        let names: Vec<&str> = match *first {
            FORM => FORMS.iter().map(|(_, name)| *name).collect(),
            FONT => CandidateFont::ALL.iter().map(|font| font.name()).collect(),
            MODE => TypingMode::ALL.iter().map(|mode| mode.name()).collect(),
            _ => Theme::ALL.iter().map(|theme| theme.name()).collect(),
        };
        for (k, name) in names.into_iter().enumerate() {
            // 每组首项开新组并作 Tab 停靠；组内方向键切换，选中项随之勾选。
            let style = if k == 0 { WS_GROUP | WS_TABSTOP } else { 0 };
            window.add(first + k, "BUTTON", name, style | BS_AUTORADIOBUTTON as u32)?;
        }
    }
    window.add(ROW_TITLE + 4, "STATIC", "自定义配色(&U)", SS_CENTERIMAGE)?;
    window.add(
        ROW_DESC + 4,
        "STATIC",
        "代码模板：背景·文字·高亮·强调",
        SS_NOPREFIX,
    )?;
    for k in 0..4 {
        let group = if k == 0 { WS_GROUP } else { 0 };
        window.add(
            CUSTOM + k,
            "EDIT",
            "",
            group | WS_TABSTOP | ES_UPPERCASE as u32,
        )?;
        window.send(CUSTOM + k, EM_SETLIMITTEXT, 7, 0);
    }
    window.add(
        OPEN_CONFIG,
        "BUTTON",
        "打开配置文件(&O)",
        WS_GROUP | WS_TABSTOP,
    )?;
    window.add(CANCEL, "BUTTON", "取消", WS_TABSTOP)?;
    window.add(
        SAVE,
        "BUTTON",
        "保存(&S)",
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
    )?;
    window.add(
        NOTICE,
        "STATIC",
        "关闭本窗放弃未保存修改；查字不受输入设置影响。",
        SS_NOPREFIX,
    )?;
    window.decorate(options.palette());
    window.layout(options.candidate_font)?;
    // 固定客户区尺寸：按当前 DPI 换算外框。
    let dpi = window.dpi;
    let (width, height) = SETTINGS_SIZE;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: width * dpi as i32 / 96,
        bottom: height * dpi as i32 / 96,
    };
    // SAFETY: 输出 RECT 为本地值；只调整本窗口尺寸，不激活。
    unsafe {
        AdjustWindowRectExForDpi(
            &mut rect,
            WS_CAPTION | WS_SYSMENU,
            0,
            WS_EX_CONTROLPARENT,
            dpi,
        );
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
        Err("无法打开配置文件，请先保存设置".into())
    } else {
        Ok(())
    }
}
fn checked(window: &Window, id: usize) -> bool {
    window.send(id, BM_GETCHECK, 0, 0) == BST_CHECKED as isize
}
fn check(window: &Window, id: usize, on: bool) {
    window.send(
        id,
        BM_SETCHECK,
        if on { BST_CHECKED } else { BST_UNCHECKED } as usize,
        0,
    );
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
    let custom = options.custom;
    for (k, color) in [custom.window, custom.text, custom.highlight, custom.accent]
        .into_iter()
        .enumerate()
    {
        window.text(CUSTOM + k, &format!("{color:06X}"));
    }
}
/// 读出设置窗草稿并在本地先校验，偏蓝或对比度不足的自定义配色不发往后台。
fn get_options(window: &Window) -> Result<Options, String> {
    let pick = |first: usize, count: usize| (0..count).find(|k| checked(window, first + k));
    let color = |k: usize| {
        let mut buffer = [0u16; 64];
        let length = window_text(window.control(CUSTOM + k), &mut buffer);
        let text = String::from_utf16_lossy(&buffer[..length]);
        let hex = text.trim().trim_start_matches('#');
        u32::from_str_radix(hex, 16)
            .ok()
            .filter(|_| hex.len() == 6)
            .ok_or_else(|| format!("自定义颜色“{text}”须为 6 位十六进制，如 F9F8F5"))
    };
    let options = Options {
        auto_commit_four: checked(window, AUTO),
        common_only: checked(window, COMMON),
        show_learning: checked(window, LEARNING),
        output_form: FORMS[pick(FORM, FORMS.len()).ok_or("必须选择输出字形")?].0,
        candidate_font: CandidateFont::ALL
            [pick(FONT, CandidateFont::ALL.len()).ok_or("必须选择候选字体")?],
        theme: Theme::ALL[pick(THEME, Theme::ALL.len()).ok_or("必须选择配色主题")?],
        typing_mode: TypingMode::ALL
            [pick(MODE, TypingMode::ALL.len()).ok_or("必须选择输入模式")?],
        custom: CustomColors {
            window: color(0)?,
            text: color(1)?,
            highlight: color(2)?,
            accent: color(3)?,
        },
    };
    options.validate()?;
    Ok(options)
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
/// 换上新查询页：列表逐字列出“字 首个编码”，整页资料交给隐藏复制框，默认选中首字。
fn show_page(main: &Window, model: &Model) {
    main.send(LIST, LB_RESETCONTENT, 0, 0);
    let items = model
        .result
        .as_ref()
        .map_or(&[][..], |page| &page.items[..]);
    for item in items {
        let code = item.learning.codes.first().map_or("", String::as_str);
        let text = wide(&format!("{} {code}", item.text));
        main.send(LIST, LB_ADDSTRING, 0, text.as_ptr() as LPARAM);
    }
    main.send(
        LIST,
        LB_SETCURSEL,
        if items.is_empty() { usize::MAX } else { 0 },
        0,
    );
    main.text(
        CLIP,
        &model.result.as_ref().map(details).unwrap_or_default(),
    );
    show_detail(main, model);
}
fn show_detail(main: &Window, model: &Model) {
    let index = main.send(LIST, LB_GETCURSEL, 0, 0);
    let item = model
        .result
        .as_ref()
        .and_then(|page| page.items.get(usize::try_from(index).ok()?));
    let list = |items: &[String]| {
        if items.is_empty() {
            "缺资料".to_owned()
        } else {
            items.join("　")
        }
    };
    let (glyph, point, codes, full, reading, parts) = match item {
        Some(item) => (
            item.text.clone(),
            item.text
                .chars()
                .next()
                .map_or_else(String::new, |c| format!("U+{:04X}", c as u32)),
            list(&item.learning.codes),
            list(&item.learning.full_codes),
            list(&item.learning.readings),
            item.learning
                .decomposition
                .clone()
                .unwrap_or_else(|| "缺资料".into()),
        ),
        None => Default::default(),
    };
    for (id, text) in [
        (GLYPH, glyph),
        (CODEPOINT, point),
        (CODES, codes),
        (FULL, full),
        (READING, reading),
        (PARTS, parts),
    ] {
        main.text(id, &text);
    }
    // SAFETY: 自绘字形格改字后重画。
    unsafe {
        InvalidateRect(main.control(GLYPH), null(), 0);
    }
}
fn refresh(main: &Window, settings: &Window, model: &Model) {
    for id in [QUERY, SEARCH, SETTINGS] {
        main.enable(id, !model.busy);
    }
    main.enable(
        PREVIOUS,
        !model.busy && model.page > 0 && model.result.is_some(),
    );
    main.enable(
        NEXT,
        !model.busy && model.result.as_ref().is_some_and(|p| p.has_more),
    );
    main.enable(
        COPY,
        !model.busy && model.result.as_ref().is_some_and(|p| !p.items.is_empty()),
    );
    for id in [AUTO, COMMON, LEARNING, SAVE, CANCEL, OPEN_CONFIG]
        .into_iter()
        .chain(FORM..FORM + FORMS.len())
        .chain(FONT..FONT + CandidateFont::ALL.len())
        .chain(THEME..THEME + Theme::ALL.len())
        .chain(MODE..MODE + TypingMode::ALL.len())
        .chain(CUSTOM..CUSTOM + 4)
    {
        settings.enable(id, !model.busy);
    }
    main.text(STATUS, &model.notice);
    let page = model
        .result
        .as_ref()
        .map(|_| format!("第 {} 页", model.page + 1));
    main.text(PAGE, &page.unwrap_or_default());
    settings.text(NOTICE, &model.settings_notice);
}
/// 确认的配色/字体用于两个窗口：换绘制配色、标题栏与字体后整体重画。
fn apply_look(main: &mut Window, settings: &mut Window, options: &Options) -> Result<(), String> {
    PALETTE.with(|palette| palette.set(options.palette()));
    for window in [main, settings] {
        window.decorate(options.palette());
        window.layout(options.candidate_font)?;
    }
    Ok(())
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
    // IME 先处理确认/取消；只读资料与列表上 Enter 不意外发起查询或写设置。
    if ime {
        return None;
    }
    if settings {
        if key == VK_ESCAPE as usize || (key == VK_RETURN as usize && id == CANCEL) {
            return Some((CLOSE, 0));
        }
        if key == VK_RETURN as usize {
            return Some((ACTION, if id == OPEN_CONFIG { OPEN_CONFIG } else { SAVE }));
        }
    } else if key == VK_RETURN as usize {
        return match id {
            QUERY => Some((ACTION, SEARCH)),
            SEARCH | PREVIOUS | NEXT | COPY | SETTINGS => Some((ACTION, id)),
            _ => None,
        };
    }
    None
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
    let mut settings = settings_window(&class, main.handle, &options)?;
    let mut model = Model::new(options);
    set_options(&settings, options);
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
/// 设置页可单独打开（输入法图标或通知区图标），不连带显示查字窗；已打开的设置页保留草稿。
fn present(main: &Window, settings: &Window, model: &mut Model, panel: Panel) {
    let (window, focus) = match panel {
        Panel::Lookup => (main, main.control(QUERY)),
        Panel::Settings => {
            if model.open_settings() {
                set_options(settings, model.options);
            }
            (settings, settings.control(AUTO))
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
    let mut settings_focus = None;
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
            // 后台停止：放弃未保存的草稿与窗口，线程退出由调用方 join。
            Err(TryRecvError::Disconnected) => return Ok(()),
        }
        loop {
            match inbox.receiver.try_recv() {
                Ok(reply) => {
                    let before = model.options;
                    if model.receive(reply)? {
                        show_page(main, model);
                    }
                    if model.options != before {
                        // 保存期间关闭重开会把控件恢复成旧值（控件在途时禁用，没有用户草稿可丢），
                        // 成功后同步，否则再保存会回滚刚确认的设置。
                        set_options(settings, model.options);
                        apply_look(main, settings, &model.options)?;
                    }
                    refresh(main, settings, model);
                    if let Some(id) = main_focus.take() {
                        main.restore_focus(id);
                    }
                    if let Some(id) = settings_focus.take() {
                        settings.restore_focus(id);
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
                // 后台常驻：关闭主窗只隐藏（设置窗一并收起并丢弃草稿），保留查询结果，等下次唤起。
                CLOSE if message.hwnd == main.handle => {
                    model.settings_open = false;
                    settings_focus = None;
                    main_focus = None;
                    set_options(settings, model.options);
                    // SAFETY: 两个窗口仅隐藏，所有权继续由本函数持有。
                    unsafe {
                        ShowWindow(settings.handle, SW_HIDE);
                        ShowWindow(main.handle, SW_HIDE);
                    }
                }
                CLOSE if message.hwnd == settings.handle => {
                    model.settings_open = false;
                    settings_focus = None;
                    set_options(settings, model.options);
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
                ACTION => {
                    let id = message.wParam & 0xffff;
                    let notification = (message.wParam >> 16) as u32;
                    if id == QUERY && notification == EN_CHANGE && !model.busy {
                        model.result = None;
                        model.page = 0;
                        show_page(main, model);
                        model.notice = "文本已修改，请点击查询。".into();
                        refresh(main, settings, model);
                        continue;
                    }
                    if id == LIST && notification == LBN_SELCHANGE {
                        show_detail(main, model);
                        continue;
                    }
                    if notification != BN_CLICKED || model.busy {
                        continue;
                    }
                    let request = match id {
                        SEARCH => Some(
                            query(main).and_then(|q| model.lookup(q, 0).map_err(str::to_owned)),
                        ),
                        PREVIOUS | NEXT => {
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
                        SAVE => Some(
                            get_options(settings)
                                .and_then(|o| model.save(o).map_err(str::to_owned)),
                        ),
                        CANCEL => {
                            post(settings.handle, CLOSE, 0);
                            None
                        }
                        OPEN_CONFIG => {
                            model.settings_notice = match open_config(settings_path) {
                                Ok(()) => {
                                    "已打开配置文件。手工修改不会自动载入，下次保存会检查冲突。"
                                        .into()
                                }
                                Err(error) => error,
                            };
                            None
                        }
                        SETTINGS => {
                            present(main, settings, model, Panel::Settings);
                            None
                        }
                        COPY if model.result.as_ref().is_some_and(|p| !p.items.is_empty()) => {
                            // 只在用户点击时委托标准 EDIT 复制整页资料，不读取或监视剪贴板。
                            main.send(CLIP, EM_SETSEL, 0, -1);
                            main.send(CLIP, WM_COPY, 0, 0);
                            None
                        }
                        _ => None,
                    };
                    if let Some(request) = request {
                        match request {
                            Ok(request) => match requests.try_send(request) {
                                Ok(()) => {
                                    main_focus = main.focused_control();
                                    settings_focus = settings.focused_control();
                                }
                                Err(error) => model.fail(format!("操作未送达：{error}")),
                            },
                            Err(error) => {
                                if id == SAVE {
                                    model.settings_notice = error;
                                } else {
                                    model.notice = error;
                                }
                            }
                        }
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

    /// 由隐藏窗口测试调用（窗口类同名，不能与之并行注册）：选区、选中段与开关按主题绘制而非系统蓝。
    fn assert_themed_drawing(main: &Window, settings: &Window) {
        // SAFETY: 只读本测试窗口的窗口过程。
        let current = unsafe { GetWindowLongPtrW(main.control(CODES), GWLP_WNDPROC) };
        assert_eq!(current as usize, edit_procedure as *const () as usize);
        // SAFETY: 同上。
        let current = unsafe { GetWindowLongPtrW(settings.control(SAVE), GWLP_WNDPROC) };
        assert_eq!(current as usize, button_procedure as *const () as usize);
        // 用户开启高对比度时沿用系统色，不核对主题像素。
        if high_contrast() {
            return;
        }
        let c = themed(PALETTE.with(Cell::get));
        assert_eq!(rgb(0xB45309), 0x0009_53B4);
        // 选区“乙丙”为主题选区色，未选中的“甲”“丁”保持原样。
        main.text(CODES, "甲乙丙丁");
        main.send(CODES, EM_SETSEL, 1, 3);
        let canvas = Canvas::new(800, 100);
        paint_selection(main.control(CODES), canvas.0);
        let at = |index: usize| {
            let position = main.send(CODES, EM_POSFROMCHAR, index, 0);
            (
                (position & 0xffff) as i16 as i32,
                ((position >> 16) & 0xffff) as i16 as i32,
            )
        };
        for index in [1, 2] {
            let (x, y) = at(index);
            assert_eq!(canvas.pixel(x + 1, y), c.selection, "第 {index} 字");
        }
        for index in [0, 3] {
            let (x, y) = at(index);
            assert_eq!(canvas.pixel(x + 1, y), 0x00FF_FFFF, "第 {index} 字");
        }
        // 选中的分段单选为强调色；关闭的开关行为卡片底色。
        check(settings, FORM + 1, true);
        for (id, x, expected) in [(FORM + 1, 6, c.accent), (AUTO, 4, c.surface)] {
            let canvas = Canvas::new(600, 80);
            draw_button(settings.control(id), canvas.0);
            assert_eq!(canvas.pixel(x, 12), expected, "控件 {id}");
        }
    }

    #[test]
    fn keyboard_commands_preserve_ime_handling_and_cancel_closes_settings() {
        assert_eq!(
            key_command(false, QUERY, VK_RETURN as usize, false),
            Some((ACTION, SEARCH))
        );
        for id in [SEARCH, PREVIOUS, NEXT, COPY, SETTINGS] {
            assert_eq!(
                key_command(false, id, VK_RETURN as usize, false),
                Some((ACTION, id))
            );
        }
        for id in [LIST, CODES] {
            assert_eq!(key_command(false, id, VK_RETURN as usize, false), None);
        }
        assert_eq!(key_command(false, QUERY, VK_ESCAPE as usize, false), None);
        for id in [AUTO, COMMON, LEARNING, FORM, FONT + 1, THEME + 3, SAVE] {
            assert_eq!(
                key_command(true, id, VK_RETURN as usize, false),
                Some((ACTION, SAVE))
            );
            assert_eq!(
                key_command(true, id, VK_ESCAPE as usize, false),
                Some((CLOSE, 0))
            );
        }
        assert_eq!(
            key_command(true, OPEN_CONFIG, VK_RETURN as usize, false),
            Some((ACTION, OPEN_CONFIG))
        );
        assert_eq!(
            key_command(true, OPEN_CONFIG, VK_ESCAPE as usize, false),
            Some((CLOSE, 0))
        );
        assert_eq!(
            key_command(true, CANCEL, VK_RETURN as usize, false),
            Some((CLOSE, 0))
        );
        for key in [VK_RETURN, VK_ESCAPE] {
            assert_eq!(key_command(false, QUERY, key as usize, true), None);
        }
    }
    #[test]
    fn layouts_keep_controls_inside_the_window_at_minimum_size() {
        let spec = main_spec(720 - 16, 520 - 39);
        for (id, [x, y, width, height]) in &spec.controls {
            if *id != CLIP {
                assert!(*x >= 0 && *y >= 0 && width > &0 && height > &0, "控件 {id}");
                assert!(x + width <= 720 - 16 && y + height <= 520 - 39, "控件 {id}");
            }
        }
        let (width, height) = SETTINGS_SIZE;
        for (id, [x, y, w, h]) in settings_spec() {
            assert!(
                x >= 0 && y >= 0 && x + w <= width && y + h <= height,
                "控件 {id}"
            );
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
    /// 查字/设置窗的菜单模态展开期间后台停止：窗口菜单（系统菜单与弹出菜单同经 WM_ENTERMENULOOP）
    /// 与文本框自带右键菜单（不发该通知）都须立即收起，停止信号留给事件循环，UI 线程随即退出。
    /// 菜单在屏幕上一闪而过，不移动鼠标。
    fn window_menu_closes_when_the_backend_stops(
        main: &mut Window,
        settings: &mut Window,
        model: &mut Model,
    ) {
        for edit in [false, true] {
            menu_closes_when_the_backend_stops(main, settings, model, edit);
        }
    }
    fn menu_closes_when_the_backend_stops(
        main: &mut Window,
        settings: &mut Window,
        model: &mut Model,
        edit: bool,
    ) {
        let (sender, show) = crate::mailbox::<Panel>().unwrap();
        watch(Some(&show));
        let query = main.control(QUERY);
        // SAFETY: 菜单在本函数内创建并销毁，所有者为本测试线程的查字窗；文本框右键菜单按键盘方式（lParam -1）在插入符处弹出。
        bounded(sender, || unsafe {
            if edit {
                SendMessageW(query, WM_CONTEXTMENU, query as WPARAM, -1);
                return;
            }
            let menu = CreatePopupMenu();
            AppendMenuW(menu, MF_STRING, 1, wide("测试").as_ptr());
            TrackPopupMenuEx(menu, TPM_RETURNCMD, 0, 0, main.handle, null());
            DestroyMenu(menu);
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
                (Path::new("settings.json"), None),
            )
            .is_ok()
        );
        watch(None);
    }
    #[test]
    fn hidden_native_controls_roundtrip_unicode_settings_and_cleanup() {
        // 合成控件测试不显示窗口、不碰剪贴板、不加载个人资源。
        let _dpi = DpiContext::enter().unwrap();
        for _ in 0..2 {
            let class = Class::register().unwrap();
            let defaults = Options::default();
            let mut main = lookup_window(&class, &defaults).unwrap();
            let mut settings = settings_window(&class, main.handle, &defaults).unwrap();
            // 控件编号重复会让布局、助记键与字体角色落到别的控件上。
            for window in [&main, &settings] {
                let mut ids: Vec<_> = window.controls.iter().map(|(id, _)| *id).collect();
                ids.sort_unstable();
                let count = ids.len();
                ids.dedup();
                assert_eq!(ids.len(), count, "控件编号重复");
            }
            assert_themed_drawing(&main, &settings);
            // 隐藏窗收到迟到回复也不能从用户桌面抢走焦点。
            // SAFETY: 仅观察本测试线程焦点，测试不激活或显示窗口。
            let original_focus = unsafe { GetFocus() };
            main.restore_focus(QUERY);
            settings.restore_focus(SAVE);
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
                ..defaults
            };
            set_options(&settings, options);
            assert_eq!(get_options(&settings).unwrap(), options);
            // 偏蓝的自定义基色在本地即被拒绝，不发往后台。
            settings.text(CUSTOM + 3, "0078D4");
            assert!(get_options(&settings).unwrap_err().contains("偏蓝"));
            settings.text(CUSTOM + 3, "12345");
            assert!(get_options(&settings).is_err());
            set_options(&settings, options);
            // 重复唤起只激活窗口，不重新加载确认值；取消后重开才放弃草稿。
            let mut draft_model = Model::new(Options::default());
            assert!(draft_model.open_settings());
            if draft_model.open_settings() {
                set_options(&settings, draft_model.options);
            }
            assert_eq!(get_options(&settings).unwrap(), options);
            draft_model.settings_open = false;
            if draft_model.open_settings() {
                set_options(&settings, draft_model.options);
            }
            assert_eq!(get_options(&settings).unwrap(), Options::default());
            // 隐藏窗口仍走真实消息泵和事件等待，证明查询回复与工作线程退出都能被观察到。
            main.text(QUERY, "𠀀");
            let mut model = Model::new(Options::default());
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
                    .send(tigertype_contracts::ui::Reply::Lookup(Ok(
                        tigertype_contracts::LookupPage {
                            items: vec![tigertype_contracts::LookupItem {
                                text: "𠀀".into(),
                                learning: Default::default(),
                            }],
                            has_more: false,
                        },
                    )))
                    .unwrap();
            });
            post(main.handle, ACTION, SEARCH);
            assert!(
                event_loop(
                    &mut main,
                    &mut settings,
                    &mut model,
                    &requests,
                    &inbox,
                    &show,
                    (Path::new("settings.json"), None)
                )
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
            assert_eq!(String::from_utf16_lossy(&buffer[..length]), "缺资料");
            // 保存回复走真实事件循环并换上新主题，但不重写资料与列表选择。
            let (requests, _receiver) = std::sync::mpsc::sync_channel(1);
            let (replies, inbox) = crate::mailbox().unwrap();
            model.save(options).unwrap();
            // 回复前关闭再重开设置页：控件恢复为旧确认值，成功回复须把它们同步为新值。
            model.settings_open = false;
            set_options(&settings, model.options);
            assert!(model.open_settings());
            set_options(&settings, model.options);
            assert_eq!(get_options(&settings).unwrap(), Options::default());
            replies
                .send(tigertype_contracts::ui::Reply::Saved(Ok(options)))
                .unwrap();
            drop(replies);
            assert!(
                event_loop(
                    &mut main,
                    &mut settings,
                    &mut model,
                    &requests,
                    &inbox,
                    &show,
                    (Path::new("settings.json"), None)
                )
                .is_err()
            );
            assert_eq!(main.send(LIST, LB_GETCURSEL, 0, 0), 0);
            assert_eq!(model.options, options);
            assert_eq!(get_options(&settings).unwrap(), options);
            assert_eq!(PALETTE.with(Cell::get), options.palette());
            assert_eq!(main.face, CandidateFont::KaiTi);
            let (replies, inbox) = crate::mailbox().unwrap();
            replies
                .send(tigertype_contracts::ui::Reply::Saved(Ok(options)))
                .unwrap();
            drop(replies);
            assert!(
                event_loop(
                    &mut main,
                    &mut settings,
                    &mut model,
                    &requests,
                    &inbox,
                    &show,
                    (Path::new("settings.json"), None)
                )
                .unwrap_err()
                .contains("回复与在途操作不符")
            );
            // 后台常驻：关闭主窗只收起（连同设置窗与草稿），循环继续；本例由随后的 WM_QUIT 结束。
            let (_replies, inbox) = crate::mailbox().unwrap();
            model.settings_open = true;
            post(main.handle, CLOSE, 0);
            // SAFETY: 只向本测试线程的消息队列投递退出消息，排在 CLOSE 之后取出。
            unsafe { PostQuitMessage(0) };
            assert!(
                event_loop(
                    &mut main,
                    &mut settings,
                    &mut model,
                    &requests,
                    &inbox,
                    &show,
                    (Path::new("settings.json"), None)
                )
                .is_ok()
            );
            assert!(!model.settings_open);
            let mut message = MSG::default();
            // 确认退出消息已被循环取走，而不是 CLOSE 提前返回后残留在队列里。
            // SAFETY: MSG 栈对象完整有效，只查看本线程队列。
            let left =
                unsafe { PeekMessageW(&mut message, null_mut(), WM_QUIT, WM_QUIT, PM_REMOVE) };
            assert_eq!(left, 0);
            // 唤起邮箱断开即后台停止，UI 线程正常退出。
            drop(show_sender);
            assert!(
                event_loop(
                    &mut main,
                    &mut settings,
                    &mut model,
                    &requests,
                    &inbox,
                    &show,
                    (Path::new("settings.json"), None)
                )
                .is_ok()
            );
            window_menu_closes_when_the_backend_stops(&mut main, &mut settings, &mut model);
            PALETTE.with(|palette| palette.set(Options::default().palette()));
            // 反复创建可发现遗漏的窗口类注销，避免把线程退出误当资源已释放。
            drop(settings);
            drop(main);
            drop(class);
        }
    }
}
