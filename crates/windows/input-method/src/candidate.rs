//! 宿主进程内候选窗：TSF 线程惰性创建、停用时销毁；以上下文视图窗口为 owner，与宿主处于同一 z-order 层级。
//! 自绘与命中测试，不建标准控件，避免同线程控件取得焦点而终止组合；窗口创建时临时启用逐显示器 DPI，避免系统位图缩放文字。
//! 窗口过程只经本线程登记表以弱引用找回候选窗，窗口数据里不存业务指针。
//! 配色与字体随快照中的选项切换；Win11 由 DWM 画圆角与边框，Win10 退回直角自绘外框。
use crate::uia;
use std::{
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    rc::{Rc, Weak},
};
use tigertype_contracts::{CandidateFont, InputState, MAX_CANDIDATES, Options, Palette, mix};
use tigertype_input_client::metrics::*;
use tigertype_input_client::{Rect, bounded, footer, position, rows};
use windows::{
    Win32::{
        Foundation::{
            COLORREF, E_FAIL, E_INVALIDARG, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT,
            POINT, RECT, SIZE, WPARAM,
        },
        Graphics::{
            Dwm::{
                DWM_WINDOW_CORNER_PREFERENCE, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE,
                DWMWCP_ROUND, DwmSetWindowAttribute,
            },
            Gdi::*,
        },
        System::Threading::GetCurrentThreadId,
        UI::{
            Accessibility::{
                HCF_HIGHCONTRASTON, HIGHCONTRASTW, IRawElementProviderSimple, NotifyWinEvent,
                UIA_EVENT_ID, UIA_MenuClosedEventId, UIA_MenuOpenedEventId,
                UIA_SelectionItem_ElementSelectedEventId, UiaClientsAreListening,
                UiaRaiseAutomationEvent, UiaReturnRawElementProvider, UiaRootObjectId,
            },
            HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
                SetThreadDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::{GetCapture, GetFocus, ReleaseCapture, SetCapture},
            WindowsAndMessaging::*,
        },
    },
    core::{Error, PCWSTR, Result, w},
};

/// 窗口过程投递给自己的点击通知：WPARAM 为展示编号，LPARAM 为当前页内序号。
const CLICK: u32 = WM_APP + 0x54;
const CLASS: PCWSTR = w!("TigerType.Candidate");
pub(crate) const UI_FACE: &str = "Microsoft YaHei UI";
/// 候选字体的 Windows 字体名；未安装时由 create_font 回退到界面字体。
pub(crate) const fn font_face(font: CandidateFont) -> &'static str {
    match font {
        CandidateFont::WenKai => "LXGW WenKai GB Screen",
        CandidateFont::KaiTi => "KaiTi",
    }
}
/// 拆分只用安装器全机注册的锁定文楷：字根是私用区字形，任何替代字体都会显示成方块。
const LEARNING_FACES: [&str; 1] = [font_face(CandidateFont::WenKai)];

thread_local! {
    // 窗口过程按句柄找回本线程的候选窗；弱引用随候选窗释放自动失效，不经窗口数据传指针。
    static POPUPS: RefCell<Vec<(usize, Weak<Popup>)>> = const { RefCell::new(Vec::new()) };
}
fn find(hwnd: HWND) -> Option<Rc<Popup>> {
    POPUPS.with(|popups| {
        let popups = popups.try_borrow().ok()?;
        let (_, popup) = popups.iter().find(|(key, _)| *key == hwnd.0 as usize)?;
        popup.upgrade()
    })
}
fn forget(hwnd: HWND) {
    POPUPS.with(|popups| {
        if let Ok(mut popups) = popups.try_borrow_mut() {
            popups.retain(|(key, popup)| *key != hwnd.0 as usize && popup.strong_count() > 0);
        }
    });
}

/// 系统屏幕矩形与 input-client 可移植矩形同为左上原点，逐字段互换；摆放与退路规则在 input-client。
pub(crate) const fn portable(rect: RECT) -> Rect {
    Rect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}
const fn native(rect: Rect) -> RECT {
    RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}
/// 组合所在显示器及其工作区。
fn work_area(caret: Rect) -> Result<(HMONITOR, Rect)> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: 输出结构大小已填写，矩形为本地值。
    unsafe {
        let monitor = MonitorFromRect(&native(caret), MONITOR_DEFAULTTONEAREST);
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return Err(E_FAIL.into());
        }
        Ok((monitor, portable(info.rcWork)))
    }
}
/// 视图区域也取不到时的输入区域：视图窗口（为空则焦点窗口）的屏幕矩形。
pub(crate) fn window_area(owner: HWND) -> Option<Rect> {
    let hwnd = if owner.is_invalid() {
        // SAFETY: 只读本线程焦点窗口。
        unsafe { GetFocus() }
    } else {
        owner
    };
    if hwnd.is_invalid() {
        return None;
    }
    let mut rect = RECT::default();
    // SAFETY: 只读该窗口的屏幕矩形，输出为本地值。
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some(portable(rect)).filter(bounded)
}
/// 组合布局不可用时退回本线程系统光标。
pub(crate) fn fallback_caret() -> Option<Rect> {
    let mut info = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: 输出结构大小已填写，只查询本线程的光标信息。
    unsafe { GetGUIThreadInfo(GetCurrentThreadId(), &mut info) }.ok()?;
    if info.hwndCaret.is_invalid() {
        return None;
    }
    let caret = info.rcCaret;
    let mut corners = [
        POINT {
            x: caret.left,
            y: caret.top,
        },
        POINT {
            x: caret.right,
            y: caret.bottom,
        },
    ];
    for corner in &mut corners {
        // SAFETY: 光标窗口来自系统查询，点为本地值。
        if !unsafe { ClientToScreen(info.hwndCaret, corner) }.as_bool() {
            return None;
        }
    }
    let rect = Rect {
        left: corners[0].x,
        top: corners[0].y,
        right: corners[1].x,
        bottom: corners[1].y,
    };
    Some(rect).filter(bounded)
}
/// 绘制用颜色。产品要求界面不得出现蓝色，所以不用系统强调色/选中色，取主题配色；
/// 仅在用户自己开启高对比度时改用其系统颜色，保证可读性。
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Colors {
    pub(crate) window: COLORREF,
    border: COLORREF,
    pub(crate) line: COLORREF,
    field: COLORREF,
    pub(crate) text: COLORREF,
    label: COLORREF,
    pub(crate) muted: COLORREF,
    faint: COLORREF,
    pub(crate) highlight: COLORREF,
    pub(crate) chosen: COLORREF,
    chosen_hint: COLORREF,
    /// 菜单悬停底：有高亮底用高亮底，玫红这类无高亮底的主题用选区色。
    pub(crate) hover: COLORREF,
    /// 菜单状态徽标底：窗口色向文字色靠 6%。
    pub(crate) track: COLORREF,
}
/// 主题颜色为 0xRRGGBB，GDI 的 COLORREF 为 0x00BBGGRR。
pub(crate) const fn rgb(value: u32) -> COLORREF {
    COLORREF((value & 0xFF) << 16 | (value & 0xFF00) | (value >> 16 & 0xFF))
}
const fn themed(p: Palette) -> Colors {
    Colors {
        window: rgb(p.window),
        border: rgb(p.border),
        line: rgb(p.line),
        field: rgb(p.field),
        text: rgb(p.text),
        label: rgb(p.label),
        muted: rgb(p.muted),
        faint: rgb(p.faint),
        highlight: rgb(p.highlight),
        chosen: rgb(p.chosen),
        chosen_hint: rgb(p.highlight_text),
        hover: rgb(if p.highlight != p.window {
            p.highlight
        } else {
            p.selection
        }),
        track: rgb(mix(p.window, p.text, 6)),
    }
}
pub(crate) fn colors(palette: Palette) -> Colors {
    let mut contrast = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    // SAFETY: 输出结构大小已填写，只读取系统设置。
    let high = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            Some((&mut contrast as *mut HIGHCONTRASTW).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok()
        && contrast.dwFlags.contains(HCF_HIGHCONTRASTON);
    if !high {
        return themed(palette);
    }
    // SAFETY: 只读系统颜色。
    let system = |index| COLORREF(unsafe { GetSysColor(index) });
    let (window, text, gray) = (
        system(COLOR_WINDOW),
        system(COLOR_WINDOWTEXT),
        system(COLOR_GRAYTEXT),
    );
    Colors {
        window,
        border: text,
        line: text,
        field: text,
        text,
        label: gray,
        muted: gray,
        faint: gray,
        highlight: system(COLOR_HIGHLIGHT),
        chosen: system(COLOR_HIGHLIGHTTEXT),
        chosen_hint: system(COLOR_HIGHLIGHTTEXT),
        hover: system(COLOR_HIGHLIGHT),
        track: window,
    }
}
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}
/// 按优先顺序建字体：GDI 找不到字体会静默替换，所以核对实际字体名，不符换下一个。
pub(crate) fn create_font(
    dc: HDC,
    faces: &[&str],
    point: i32,
    dpi: u32,
    weight: i32,
) -> Result<HFONT> {
    let mut last = HFONT::default();
    for (index, face) in faces.iter().enumerate() {
        let name: Vec<u16> = face.encode_utf16().chain(Some(0)).collect();
        // SAFETY: 名称 NUL 结尾；新建字体由调用方持有并负责释放。
        let font = unsafe {
            CreateFontW(
                -(point * dpi as i32 / 72),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                PCWSTR(name.as_ptr()),
            )
        };
        if font.is_invalid() {
            continue;
        }
        if (index + 1 == faces.len() && faces != LEARNING_FACES) || face_is(dc, font, face) {
            last = font;
            break;
        }
        // SAFETY: 字体只在 face_is 内临时选入并已还原。
        unsafe {
            let _ = DeleteObject(font.into());
        }
    }
    if last.is_invalid() {
        return Err(E_FAIL.into());
    }
    Ok(last)
}
fn face_is(dc: HDC, font: HFONT, face: &str) -> bool {
    let mut buffer = [0u16; 64];
    // SAFETY: 临时选入字体读取实际字体名后还原；缓冲区为本地值。
    let length = unsafe {
        let old = SelectObject(dc, font.into());
        let length = GetTextFaceW(dc, Some(&mut buffer));
        SelectObject(dc, old);
        length
    };
    let actual = String::from_utf16_lossy(&buffer[..(length.max(1) as usize - 1).min(63)]);
    actual.eq_ignore_ascii_case(face)
}
/// 用系统 DC 画刷/画笔改色绘制，逐键重绘不新建 GDI 对象。
pub(crate) fn fill(dc: HDC, rect: &RECT, color: COLORREF) {
    // SAFETY: DC 画刷为库存对象，无需释放。
    unsafe {
        SetDCBrushColor(dc, color);
        FillRect(dc, rect, HBRUSH(GetStockObject(DC_BRUSH).0));
    }
}
/// 圆角矩形：fill 为 Some 时实心填充，否则只描边。
pub(crate) fn round(dc: HDC, rect: &RECT, radius: i32, fill: Option<COLORREF>, line: COLORREF) {
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
        let _ = RoundRect(
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
pub(crate) fn draw_text(
    dc: HDC,
    font: HFONT,
    color: COLORREF,
    text: &str,
    rect: RECT,
    format: DRAW_TEXT_FORMAT,
) {
    // 长度为 0 时 DrawTextW 仍会读首字符，空 Vec 的悬空指针会使宿主程序崩溃。
    if text.is_empty() {
        return;
    }
    let mut rect = rect;
    // SAFETY: 字符串借用在同步调用内有效；字体由调用方在绘制结束前还原。
    unsafe {
        SelectObject(dc, font.into());
        SetTextColor(dc, color);
        DrawTextW(
            dc,
            &mut wide(text),
            &mut rect,
            DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | format,
        );
    }
}
fn point(lparam: LPARAM) -> POINT {
    POINT {
        x: (lparam.0 & 0xffff) as i16 as i32,
        y: ((lparam.0 >> 16) & 0xffff) as i16 as i32,
    }
}
fn inside(rect: &RECT, at: POINT) -> bool {
    at.x >= rect.left && at.x < rect.right && at.y >= rect.top && at.y < rect.bottom
}
#[cfg(target_pointer_width = "64")]
fn set_owner(hwnd: HWND, owner: HWND) {
    // SAFETY: 只修改本候选窗的 owner；宿主窗口句柄来自本线程上下文视图或焦点。
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, owner.0 as isize);
    }
}
#[cfg(target_pointer_width = "32")]
fn set_owner(hwnd: HWND, owner: HWND) {
    // SAFETY: x86 没有 Ptr 版本，句柄宽度与 LONG 相同；只修改本候选窗的 owner。
    unsafe {
        SetWindowLongW(hwnd, GWLP_HWNDPARENT, owner.0 as i32);
    }
}

/// 测量结果：候选行、页脚的客户区矩形，以及候选字与注释列的起点。
#[derive(Default, Clone, Debug, PartialEq)]
pub(crate) struct Layout {
    pub rows: Vec<RECT>,
    pub footer: Option<RECT>,
    pub text_x: i32,
    pub comment_x: i32,
    pub size: SIZE,
}
#[derive(Default)]
pub(crate) struct View {
    /// 正在显示的快照；隐藏时为 None。
    pub state: Option<InputState>,
    /// 本地展示编号：每次显示/隐藏递增，旧鼠标消息与旧点击不能重解释到新内容。
    /// 32 位回绕后才可能重复，届时后台快照编号仍会拒绝旧选择。
    pub serial: u32,
    pub layout: Layout,
    /// 当前摆放所依据的组合矩形；宿主暂时给不出位置时原位刷新。
    caret: Option<Rect>,
    pressed: Option<u8>,
    /// 已排队一个点击会话；展示变化或会话结束即解除。
    pending: bool,
}

/// 按 DPI 与候选字体缓存：候选字、序号（等宽）、界面文字及独立的拆分注释字体。
#[derive(Clone, Copy, Default)]
struct Fonts {
    text: HFONT,
    small: HFONT,
    ui: HFONT,
    comment: HFONT,
    dpi: u32,
    face: CandidateFont,
}
impl Fonts {
    fn release(self) {
        for font in [self.text, self.small, self.ui, self.comment] {
            if !font.is_invalid() {
                // SAFETY: 字体只在绘制/测量期间临时选入并已还原，不再被任何 DC 使用。
                unsafe {
                    let _ = DeleteObject(font.into());
                }
            }
        }
    }
}

#[cfg(test)]
type BeforeNotify = Box<dyn Fn(u32)>;
/// 测试记录窗口实际经过的每个位置；屏幕上能看到的是这些中间位置，不只是最终结果。
#[cfg(test)]
type Moves = RefCell<Vec<(i32, i32)>>;

pub struct Popup {
    pub(crate) hwnd: HWND,
    pub(crate) view: RefCell<View>,
    fonts: Cell<Fonts>,
    /// DWM 接受圆角（Win11）时由系统画边框，否则自绘直角外框。
    rounded: bool,
    /// 已交给 DWM 的边框颜色，变化时才重设。
    border: Cell<Option<COLORREF>>,
    click: Box<dyn Fn(u32, u8)>,
    root: RefCell<Option<IRawElementProviderSimple>>,
    /// 测试保持隐藏，不在桌面显示窗口；其余行为不变。
    quiet: bool,
    /// 测试在外部通知发出前检查首帧已画完，不把通知时间误当成绘制时间。
    #[cfg(test)]
    before_notify: RefCell<Option<BeforeNotify>>,
    #[cfg(test)]
    moves: Moves,
}

// SAFETY: 回调截住 panic；只按句柄在本线程登记表中查找候选窗，其余消息交系统默认处理。
unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match message {
        // 点击不激活候选窗，宿主保持焦点与组合。
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_NCDESTROY => forget(hwnd),
        _ => {}
    }
    let handled = catch_unwind(AssertUnwindSafe(|| find(hwnd)?.handle(message, w, l)))
        .ok()
        .flatten();
    // SAFETY: 未处理的消息按系统流程处理。
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, message, w, l) })
}

impl Popup {
    /// 系统可随 owner 销毁 HWND；用登记身份而非 IsWindow 判断，避免旧句柄复用后误操作别人的窗口。
    pub(crate) fn alive(&self) -> bool {
        POPUPS.with(|popups| {
            popups.try_borrow().is_ok_and(|popups| {
                popups.iter().any(|(hwnd, popup)| {
                    *hwnd == self.hwnd.0 as usize && std::ptr::eq(popup.as_ptr(), self)
                })
            })
        })
    }
    /// 在当前 TSF 线程创建隐藏候选窗；click 收到（展示编号，页内序号）。
    pub fn new(click: impl Fn(u32, u8) + 'static, quiet: bool) -> Result<Rc<Self>> {
        let instance = crate::module()?;
        let class = WNDCLASSW {
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(procedure),
            hInstance: instance.into(),
            // SAFETY: 系统共享光标，无需释放。
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
            lpszClassName: CLASS,
            ..Default::default()
        };
        // SAFETY: 类以 TIP 模块注册、回调无状态；模块驻留至进程退出，不注销，重复注册视为已存在。
        if unsafe { RegisterClassW(&class) } == 0 {
            let error = Error::from_thread();
            if error.code() != ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
                return Err(error);
            }
        }
        // SAFETY: 只改变当前 TSF 线程的 DPI 上下文；窗口创建完成后立即恢复宿主设置，避免影响宿主控件。
        let previous_dpi =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        // SAFETY: NOACTIVATE/TOOLWINDOW 不抢宿主焦点、不进任务栏；未传入创建参数指针。
        let hwnd = unsafe {
            let result = CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                CLASS,
                w!("TigerType 候选"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            );
            // SAFETY: 恢复进入前的线程 DPI 上下文；失败时返回值仍为可判定的空句柄。
            if !previous_dpi.0.is_null() {
                SetThreadDpiAwarenessContext(previous_dpi);
            }
            result?
        };
        let preference = DWMWCP_ROUND;
        // SAFETY: 只设置本窗口属性，参数为本地值；Win10 不支持圆角时返回失败。
        let rounded = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&preference as *const DWM_WINDOW_CORNER_PREFERENCE).cast(),
                size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            )
        }
        .is_ok();
        let popup = Rc::new(Self {
            hwnd,
            view: RefCell::default(),
            fonts: Cell::new(Fonts::default()),
            rounded,
            border: Cell::new(None),
            click: Box::new(click),
            root: RefCell::new(None),
            quiet,
            #[cfg(test)]
            before_notify: RefCell::new(None),
            #[cfg(test)]
            moves: RefCell::default(),
        });
        POPUPS.with(|popups| {
            popups
                .borrow_mut()
                .push((hwnd.0 as usize, Rc::downgrade(&popup)))
        });
        Ok(popup)
    }
    fn handle(self: &Rc<Self>, message: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
        #[cfg(test)]
        if message == WM_WINDOWPOSCHANGED {
            // SAFETY: 系统保证本消息的 LPARAM 指向有效 WINDOWPOS。
            let moved = unsafe { &*(l.0 as *const WINDOWPOS) };
            if !moved.flags.contains(SWP_NOMOVE) {
                self.moves.borrow_mut().push((moved.x, moved.y));
            }
        }
        match message {
            WM_PAINT => self.paint(),
            WM_LBUTTONDOWN => {
                let row = self.hit(point(l));
                self.view.try_borrow_mut().ok()?.pressed = row;
                if row.is_some() {
                    // SAFETY: 捕获本窗口鼠标，抬起或展示更新时释放。
                    unsafe { SetCapture(self.hwnd) };
                }
            }
            WM_LBUTTONUP => {
                let pressed = self.view.try_borrow_mut().ok()?.pressed.take();
                self.release();
                if let Some(row) = pressed
                    && self.hit(point(l)) == Some(row)
                {
                    let serial = self.view.try_borrow().ok()?.serial;
                    // 离开鼠标消息处理后再请求编辑会话；投递失败只是丢掉这次点击。
                    // SAFETY: 投递给本窗口，参数只含整数。
                    let _ = unsafe {
                        PostMessageW(
                            Some(self.hwnd),
                            CLICK,
                            WPARAM(serial as usize),
                            LPARAM(row as isize),
                        )
                    };
                }
            }
            WM_CAPTURECHANGED => {
                if let Ok(mut view) = self.view.try_borrow_mut() {
                    view.pressed = None;
                }
            }
            CLICK => {
                if let Ok(index) = u8::try_from(l.0) {
                    (self.click)(w.0 as u32, index);
                }
            }
            WM_GETOBJECT if l.0 as i32 == UiaRootObjectId => {
                // SAFETY: 按 UIA 约定在 WM_GETOBJECT 中返回本窗口根提供者。
                return Some(unsafe { UiaReturnRawElementProvider(self.hwnd, w, l, &self.root()) });
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }
    fn scale(&self, value: i32) -> i32 {
        value * self.fonts.get().dpi.max(96) as i32 / 96
    }
    fn hit(&self, at: POINT) -> Option<u8> {
        let view = self.view.try_borrow().ok()?;
        let index = view.layout.rows.iter().position(|rect| inside(rect, at))?;
        u8::try_from(index).ok()
    }
    fn release(&self) {
        if !self.alive() {
            return;
        }
        // SAFETY: 只释放本窗口持有的鼠标捕获；会同步收到 WM_CAPTURECHANGED。
        unsafe {
            if GetCapture() == self.hwnd {
                let _ = ReleaseCapture();
            }
        }
    }
    fn paint(&self) {
        let mut paint = PAINTSTRUCT::default();
        // SAFETY: WM_PAINT 内配对 BeginPaint/EndPaint；内存 DC 与位图在本次绘制结束前还原并删除。
        unsafe {
            let dc = BeginPaint(self.hwnd, &mut paint);
            let mut client = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut client);
            // 先画到内存位图再一次拷贝，逐键刷新不闪烁；建不出缓冲时直接画。
            let memory = CreateCompatibleDC(Some(dc));
            let bitmap = CreateCompatibleBitmap(dc, client.right, client.bottom);
            if memory.is_invalid() || bitmap.is_invalid() {
                self.draw(dc, &client);
            } else {
                let old = SelectObject(memory, bitmap.into());
                self.draw(memory, &client);
                let _ = BitBlt(
                    dc,
                    0,
                    0,
                    client.right,
                    client.bottom,
                    Some(memory),
                    0,
                    0,
                    SRCCOPY,
                );
                SelectObject(memory, old);
            }
            if !bitmap.is_invalid() {
                let _ = DeleteObject(bitmap.into());
            }
            if !memory.is_invalid() {
                let _ = DeleteDC(memory);
            }
            let _ = EndPaint(self.hwnd, &paint);
        }
    }
    /// 按设计稿绘制：高亮行为圆角底色加左侧指示条（无底色主题只换字色），注释列对齐，页脚以分隔线隔开。
    pub(crate) fn draw(&self, dc: HDC, client: &RECT) {
        let Ok(view) = self.view.try_borrow() else {
            return;
        };
        let palette = view
            .state
            .as_ref()
            .map_or(Options::default(), |s| s.options)
            .palette();
        let c = colors(palette);
        fill(dc, client, c.window);
        let Some(state) = &view.state else {
            return;
        };
        if !self.rounded {
            round(dc, client, 0, None, c.border);
        }
        let (fonts, s, layout) = (self.fonts.get(), |v| self.scale(v), &view.layout);
        // SAFETY: 记下原字体，绘制结束前还原。
        let old = unsafe {
            SetBkMode(dc, TRANSPARENT);
            SelectObject(dc, fonts.text.into())
        };
        for (row, rect) in rows(state).zip(&layout.rows) {
            let chosen = row.chosen;
            // 高亮只画圆角底色；底与窗口同色的主题（玫红）只靠字色区分。
            if chosen && c.highlight != c.window {
                round(dc, rect, s(4), Some(c.highlight), c.highlight);
            }
            let (text, hint) = if chosen {
                (c.chosen, c.chosen_hint)
            } else {
                (c.text, c.muted)
            };
            let right = rect.right - s(ROW_RIGHT);
            let span = |left| RECT {
                left,
                right,
                ..*rect
            };
            let label = if chosen { c.chosen_hint } else { c.label };
            draw_text(
                dc,
                fonts.small,
                label,
                row.label,
                span(rect.left + s(ROW_LEFT)),
                DRAW_TEXT_FORMAT(0),
            );
            draw_text(
                dc,
                fonts.text,
                text,
                row.text,
                span(layout.text_x),
                DRAW_TEXT_FORMAT(0),
            );
            if !row.comment.is_empty() {
                draw_text(
                    dc,
                    fonts.comment,
                    hint,
                    row.comment,
                    span(layout.comment_x),
                    DT_END_ELLIPSIS,
                );
            }
        }
        if let (Some(rect), Some(view)) = (&layout.footer, footer(state)) {
            let line = RECT {
                left: rect.left + s(6),
                top: rect.top,
                right: rect.right - s(6),
                bottom: rect.top + s(1).max(1),
            };
            fill(dc, &line, c.line);
            let body = RECT {
                top: rect.top + s(1),
                ..*rect
            };
            if view.pinyin {
                let width = self.extent(dc, fonts.ui, "拼音") + s(12);
                let top = (body.top + body.bottom - s(18)) / 2;
                let tag = RECT {
                    left: body.left + s(8),
                    top,
                    right: body.left + s(8) + width,
                    bottom: top + s(18),
                };
                round(dc, &tag, s(4), None, c.field);
                draw_text(dc, fonts.ui, c.muted, "拼音", tag, DT_CENTER);
            }
            if let Some(pager) = view.pager {
                let mut right = body.right - s(6);
                let page = pager.page.to_string();
                for (text, enabled) in [
                    ("›", pager.next),
                    (page.as_str(), true),
                    ("‹", pager.previous),
                ] {
                    let width = self.extent(dc, fonts.small, text) + s(4);
                    let cell = RECT {
                        left: right - width,
                        right,
                        ..body
                    };
                    let color = if enabled { c.label } else { c.faint };
                    draw_text(dc, fonts.small, color, text, cell, DT_CENTER);
                    right -= width;
                }
            }
        }
        // SAFETY: 还原原字体。
        unsafe {
            SelectObject(dc, old);
        }
    }
    fn extent(&self, dc: HDC, font: HFONT, text: &str) -> i32 {
        if text.is_empty() {
            return 0;
        }
        let mut rect = RECT::default();
        // SAFETY: 与绘制同用 DrawTextW，系统缺字回退后的宽度也一致；调用方统一还原字体。
        unsafe {
            SelectObject(dc, font.into());
            DrawTextW(
                dc,
                &mut wide(text),
                &mut rect,
                DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
        rect.right - rect.left
    }
    /// 字号随窗口 DPI、候选字体随选项；全部新字体建好后才释放旧字体。
    fn ensure_fonts(&self, dpi: u32, face: CandidateFont) -> Result<()> {
        let old = self.fonts.get();
        if old.dpi == dpi && old.face == face && !old.text.is_invalid() {
            return Ok(());
        }
        // SAFETY: 取本窗口 DC 只用于核对字体名，结束前归还。
        let dc = unsafe { GetDC(Some(self.hwnd)) };
        if dc.is_invalid() {
            return Err(E_FAIL.into());
        }
        let created = (|| {
            let mut fonts = Fonts {
                dpi,
                face,
                ..Default::default()
            };
            fonts.text = create_font(
                dc,
                &[font_face(face), UI_FACE],
                TEXT_POINT,
                dpi,
                FW_NORMAL.0 as i32,
            )?;
            fonts.small = create_font(
                dc,
                &["Cascadia Mono", "Consolas"],
                SMALL_POINT,
                dpi,
                FW_NORMAL.0 as i32,
            )
            .inspect_err(|_| fonts.release())?;
            fonts.ui = create_font(dc, &[UI_FACE], SMALL_POINT, dpi, FW_NORMAL.0 as i32)
                .inspect_err(|_| fonts.release())?;
            fonts.comment = create_font(dc, &LEARNING_FACES, SMALL_POINT, dpi, FW_NORMAL.0 as i32)
                .inspect_err(|_| fonts.release())?;
            Ok::<_, Error>(fonts)
        })();
        // SAFETY: 归还上面取得的 DC。
        unsafe {
            ReleaseDC(Some(self.hwnd), dc);
        }
        self.fonts.set(created?);
        old.release();
        Ok(())
    }
    /// 量出各行矩形、列起点与窗口尺寸；长注释由绘制时省略号截断。
    fn measure(&self, state: &InputState) -> Result<Layout> {
        // SAFETY: 取本窗口 DC 测量文本，选入的字体在 ReleaseDC 前还原。
        let dc = unsafe { GetDC(Some(self.hwnd)) };
        if dc.is_invalid() {
            return Err(E_FAIL.into());
        }
        let fonts = self.fonts.get();
        // SAFETY: 同上，DC 有效。
        let old = unsafe { SelectObject(dc, fonts.text.into()) };
        let mut metrics = TEXTMETRICW::default();
        // SAFETY: 输出结构为本地值。
        let _ = unsafe { GetTextMetricsW(dc, &mut metrics) };
        let widest = |font, texts: &mut dyn Iterator<Item = &str>| {
            texts
                .map(|text| self.extent(dc, font, text))
                .max()
                .unwrap_or(0)
        };
        let s = |v| self.scale(v);
        let candidates = &state.candidates;
        let label = widest(
            fonts.small,
            &mut candidates.iter().map(|c| c.label.as_str()),
        );
        let text = widest(fonts.text, &mut candidates.iter().map(|c| c.text.as_str()));
        let comment = widest(
            fonts.comment,
            &mut candidates.iter().map(|c| c.comment.as_str()),
        );
        let text_x = s(PAD + ROW_LEFT) + label + s(GAP);
        let comment_x = text_x + text + s(GAP);
        let content = if comment > 0 {
            comment_x + comment
        } else {
            text_x + text
        };
        let mut width = s(MIN_WIDTH).max(content + s(ROW_RIGHT + PAD));
        let footer = footer(state).is_some();
        if footer {
            let tag = self.extent(dc, fonts.ui, "拼音") + s(12);
            width =
                width.max(s(PAD + 8 + 16 + 6 + PAD) + tag + self.extent(dc, fonts.small, "‹ 99 ›"));
        }
        // SAFETY: 还原原字体并归还 DC。
        unsafe {
            SelectObject(dc, old);
            ReleaseDC(Some(self.hwnd), dc);
        }
        let width = width.min(s(MAX_WIDTH));
        let line = s(ROW).max(metrics.tmHeight + s(4));
        let band = |top, height| RECT {
            left: s(PAD),
            top,
            right: width - s(PAD),
            bottom: top + height,
        };
        let rows: Vec<_> = (0..candidates.len())
            .map(|index| band(s(PAD) + index as i32 * line, line))
            .collect();
        let mut bottom = s(PAD) + rows.len() as i32 * line;
        // 页脚和上方候选之间留 4px，再画分隔线。
        let footer = footer.then(|| {
            let top = if bottom > s(PAD) {
                bottom + s(4)
            } else {
                bottom
            };
            bottom = top + s(FOOTER);
            band(top, s(FOOTER))
        });
        Ok(Layout {
            rows,
            footer,
            text_x,
            comment_x,
            size: SIZE {
                cx: width,
                cy: bottom + s(PAD),
            },
        })
    }
    /// 窗口已在该组合矩形应有的位置：组合变长只改矩形右缘，不必重摆重画。
    fn placed(&self, caret: Rect, work: Rect) -> bool {
        let Ok(view) = self.view.try_borrow() else {
            return false;
        };
        let size = view.layout.size;
        let mut current = RECT::default();
        // SAFETY: 只读本窗口矩形，输出为本地值。
        unsafe { GetWindowRect(self.hwnd, &mut current) }.is_ok()
            && portable(current) == position(caret, work, size.cx, size.cy)
    }
    /// 显示或刷新：owner 取视图窗口（空则取焦点窗口），按组合位置放在工作区内，不激活。
    pub fn show(self: &Rc<Self>, state: &InputState, caret: Rect, owner: HWND) -> Result<()> {
        if !self.alive() {
            return Err(E_FAIL.into());
        }
        if !bounded(&caret) || state.candidates.len() > MAX_CANDIDATES {
            return Err(E_INVALIDARG.into());
        }
        // 同一快照保留展示编号与在途点击；几何不变也须检查 owner、系统显隐与遮挡层级。
        let (same, moved) = self.view.try_borrow().map_or((false, true), |view| {
            (
                view.state.as_ref() == Some(state),
                view.caret != Some(caret),
            )
        });
        let owner = if owner.is_invalid() {
            // SAFETY: 只读本线程焦点窗口。
            unsafe { GetFocus() }
        } else {
            owner
        };
        // SAFETY: 只读本窗口当前 owner。
        let owner_changed = unsafe { GetWindow(self.hwnd, GW_OWNER) }.unwrap_or_default() != owner;
        // 系统可能在最小化或关闭 owned window 时自行隐藏候选；缓存有快照不代表窗口可见。
        // SAFETY: 登记已核对为本对象的存活窗口，仅查询可见性。
        let visible = self.quiet || unsafe { IsWindowVisible(self.hwnd) }.as_bool();
        let (monitor, work) = work_area(caret)?;
        // 可见且不跨显示器时 DPI 不变：直接移到算好的位置，用户看不到未夹取的中间位置。
        // SAFETY: 只查询本窗口所在显示器。
        let settled =
            visible && unsafe { MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST) } == monitor;
        if same && !owner_changed && settled && self.placed(caret, work) {
            if moved && let Ok(mut view) = self.view.try_borrow_mut() {
                view.caret = Some(caret);
            }
            // SAFETY: 只在原 z-order 层内提到前面，不变为全局 topmost、不激活；不重新布局或重画。
            return unsafe {
                SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOP),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
            };
        }
        if !same {
            self.release();
        }
        if owner_changed {
            set_owner(self.hwnd, owner);
        }
        if !settled {
            // SAFETY: 先移到组合所在显示器再读 DPI，使逐显示器感知的宿主得到正确字号；不激活、不改 z 序。
            unsafe {
                SetWindowPos(
                    self.hwnd,
                    None,
                    caret.left,
                    caret.bottom,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                )?;
            }
        }
        // SAFETY: 读取本窗口所在上下文的 DPI；未声明感知的宿主恒为 96。
        let dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(96);
        // 布局通知只移动同快照且 DPI 未变时复用测量结果，不重新量每行文本。
        let remeasure = !same || self.fonts.get().dpi != dpi;
        self.ensure_fonts(dpi, state.options.candidate_font)?;
        let layout = if remeasure {
            self.measure(state)?
        } else {
            self.view
                .try_borrow()
                .map_err(|_| Error::from(E_FAIL))?
                .layout
                .clone()
        };
        let size = layout.size;
        let border = colors(state.options.palette()).border;
        if self.rounded && self.border.get() != Some(border) {
            // SAFETY: 只设置本窗口的 DWM 边框颜色，避免系统按强调色描边；失败时保留系统默认。
            let set = unsafe {
                DwmSetWindowAttribute(
                    self.hwnd,
                    DWMWA_BORDER_COLOR,
                    (&border as *const COLORREF).cast(),
                    size_of::<COLORREF>() as u32,
                )
            };
            self.border.set(set.is_ok().then_some(border));
        }
        let place = position(caret, work, size.cx, size.cy);
        // SAFETY: 工作区夹取后的位置与尺寸；不激活、不改 z 序。
        unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                place.left,
                place.top,
                place.right - place.left,
                place.bottom - place.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )?;
        }
        let shown = {
            let mut view = self
                .view
                .try_borrow_mut()
                .map_err(|_| Error::from(E_FAIL))?;
            let shown = view.state.is_some();
            view.state = Some(state.clone());
            view.layout = layout;
            view.caret = Some(caret);
            if !same {
                view.serial = view.serial.wrapping_add(1);
                view.pressed = None;
                view.pending = false;
            }
            shown
        };
        // SAFETY: 先显示再立即处理 WM_PAINT；否则 DWM 会先呈现空白边框，宿主回调结束后才补画文字。
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
        if !self.quiet {
            // SAFETY: 每次显式显示并提到原层前面，不激活、不全局置顶；不用 ShowWindow，避免启动标志覆盖。
            unsafe {
                SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOP),
                    0,
                    0,
                    0,
                    0,
                    SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )?;
            }
        }
        // 先画首帧再通知外部监听者；读屏器或宿主 hook 变慢时不能把空白边框留在屏幕上。
        // SAFETY: 句柄属于本线程候选窗，UpdateWindow 只同步处理该窗口的无状态 WM_PAINT。
        unsafe {
            let _ = UpdateWindow(self.hwnd);
        }
        if shown && visible {
            self.notify(EVENT_OBJECT_IME_CHANGE);
        } else {
            self.notify(EVENT_OBJECT_IME_SHOW);
            self.raise(|| Some(self.root()), UIA_MenuOpenedEventId);
        }
        if let Some(index) = state.highlighted.filter(|_| !same) {
            self.raise(
                || uia::item(self, index),
                UIA_SelectionItem_ElementSelectedEventId,
            );
        }
        Ok(())
    }
    /// 正在显示时的组合矩形。
    pub fn anchor(&self) -> Option<Rect> {
        if !self.alive() {
            return None;
        }
        let view = self.view.try_borrow().ok()?;
        view.state.as_ref().and(view.caret)
    }
    pub fn hide(self: &Rc<Self>) {
        self.release();
        let shown = match self.view.try_borrow_mut() {
            Ok(mut view) => {
                view.serial = view.serial.wrapping_add(1);
                view.pressed = None;
                view.pending = false;
                view.layout = Layout::default();
                view.caret = None;
                view.state.take().is_some()
            }
            Err(_) => false,
        };
        if shown && self.alive() {
            // SAFETY: 原子隐藏并收为零面积，避免 DWM 留下旧圆角边框；不激活、不改位置或 z 序。
            // 窗口与字体缓存保留，下一次 show 会按新快照重新布局；隐藏测试也核对真实窗口尺寸。
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_HIDEWINDOW | SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            self.notify(EVENT_OBJECT_IME_HIDE);
            self.raise(|| Some(self.root()), UIA_MenuClosedEventId);
        }
    }
    /// 点击可受理时取当前快照编号并标记待执行；同一展示只受理一个点击。
    pub fn selectable(&self, serial: u32, index: u8) -> Option<u64> {
        if !self.alive() {
            return None;
        }
        let mut view = self.view.try_borrow_mut().ok()?;
        let state = view.state.as_ref()?;
        if view.serial != serial || view.pending || index as usize >= state.candidates.len() {
            return None;
        }
        let revision = state.revision;
        view.pending = true;
        Some(revision)
    }
    /// 该展示仍在屏幕上且未被更新。
    pub fn showing(&self, serial: u32) -> bool {
        self.alive()
            && self
                .view
                .try_borrow()
                .is_ok_and(|view| view.serial == serial && view.state.is_some())
    }
    /// 点击会话未受理或已结束而展示未变：允许下一次点击。
    pub fn unblock(&self, serial: u32) {
        if let Ok(mut view) = self.view.try_borrow_mut()
            && view.serial == serial
        {
            view.pending = false;
        }
    }
    pub(crate) fn root(self: &Rc<Self>) -> IRawElementProviderSimple {
        self.root
            .borrow_mut()
            .get_or_insert_with(|| uia::root(self))
            .clone()
    }
    /// 客户区矩形转屏幕坐标，供 UIA 报告候选项位置。
    pub(crate) fn screen(&self, rect: RECT) -> RECT {
        let mut corners = [
            POINT {
                x: rect.left,
                y: rect.top,
            },
            POINT {
                x: rect.right,
                y: rect.bottom,
            },
        ];
        for corner in &mut corners {
            // SAFETY: 本窗口句柄有效，点为本地值。
            unsafe {
                let _ = ClientToScreen(self.hwnd, corner);
            }
        }
        RECT {
            left: corners[0].x,
            top: corners[0].y,
            right: corners[1].x,
            bottom: corners[1].y,
        }
    }
    /// 屏幕坐标命中的候选行。
    pub(crate) fn hit_screen(&self, x: i32, y: i32) -> Option<usize> {
        if !self.alive() {
            return None;
        }
        let mut at = POINT { x, y };
        // SAFETY: 本窗口句柄有效，点为本地值。
        if !unsafe { ScreenToClient(self.hwnd, &mut at) }.as_bool() {
            return None;
        }
        self.hit(at).map(usize::from)
    }
    fn notify(&self, event: u32) {
        #[cfg(test)]
        if let Some(notify) = self.before_notify.borrow().as_ref() {
            notify(event);
        }
        // SAFETY: 为本窗口客户区发出 IME 显示/隐藏/变化事件。
        unsafe { NotifyWinEvent(event, self.hwnd, OBJID_CLIENT.0, CHILDID_SELF as i32) };
    }
    /// 只有 UIA 客户端监听时才构造元素并发事件，平时不增加按键路径开销。
    fn raise(
        &self,
        provider: impl FnOnce() -> Option<IRawElementProviderSimple>,
        event: UIA_EVENT_ID,
    ) {
        // SAFETY: 只查询是否有监听者。
        if !unsafe { UiaClientsAreListening() }.as_bool() {
            return;
        }
        if let Some(provider) = provider() {
            // SAFETY: 提供者属于本线程窗口；事件失败不影响输入。
            let _ = unsafe { UiaRaiseAutomationEvent(&provider, event) };
        }
    }
}
impl Drop for Popup {
    fn drop(&mut self) {
        // WM_NCDESTROY 已注销的句柄可能被宿主复用，不能再发 UIA 消息或 DestroyWindow。
        if self.alive() {
            forget(self.hwnd);
            // SAFETY: 先让 UIA 释放本窗口的提供者引用，再在所属线程销毁窗口并回收字体。
            unsafe {
                UiaReturnRawElementProvider(
                    self.hwnd,
                    WPARAM(0),
                    LPARAM(0),
                    None::<&IRawElementProviderSimple>,
                );
                let _ = DestroyWindow(self.hwnd);
            }
        }
        self.fonts.get().release();
    }
}

#[cfg(test)]
#[path = "candidate_tests.rs"]
pub(crate) mod tests;
