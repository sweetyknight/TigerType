//! 宿主进程内候选窗：TSF 线程惰性创建、停用时销毁；以上下文视图窗口为 owner，与宿主处于同一 z-order 层级。
//! 自绘与命中测试，不建标准控件，避免同线程控件取得焦点而终止组合；不声明 DPI 感知，跟随宿主进程。
//! 窗口过程只经本线程登记表以弱引用找回候选窗，窗口数据里不存业务指针。
//! 配色暂用系统颜色（兼顾高对比度），Win11Light 锁定外观与字体优先顺序另行验收。
use crate::uia;
use std::{
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    rc::{Rc, Weak},
};
use tigertype_contracts::{Candidate, InputState, MAX_CANDIDATES};
use windows::{
    Win32::{
        Foundation::{
            COLORREF, E_FAIL, E_INVALIDARG, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT,
            POINT, RECT, SIZE, WPARAM,
        },
        Graphics::Gdi::*,
        System::Threading::GetCurrentThreadId,
        UI::{
            Accessibility::{
                HCF_HIGHCONTRASTON, HIGHCONTRASTW, IRawElementProviderSimple, NotifyWinEvent,
                UIA_EVENT_ID, UIA_MenuClosedEventId, UIA_MenuOpenedEventId,
                UIA_SelectionItem_ElementSelectedEventId, UiaClientsAreListening,
                UiaRaiseAutomationEvent, UiaReturnRawElementProvider, UiaRootObjectId,
            },
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{GetCapture, GetFocus, ReleaseCapture, SetCapture},
            WindowsAndMessaging::*,
        },
    },
    core::{Error, PCWSTR, Result, w},
};

/// 窗口过程投递给自己的点击通知：WPARAM 为展示编号，LPARAM 为当前页内序号。
const CLICK: u32 = WM_APP + 0x54;
const CLASS: PCWSTR = w!("TigerType.Candidate");
// 96 DPI 下的布局常量，按窗口 DPI 缩放。
const POINT_SIZE: i32 = 15;
const PAD_X: i32 = 8;
const PAD_Y: i32 = 4;
const GAP: i32 = 12;
const MIN_WIDTH: i32 = 120;
const MAX_WIDTH: i32 = 480;

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

/// 屏幕矩形须有序且数值有界，防止尺寸运算溢出；保留多显示器负坐标。
pub(crate) fn bounded(rect: &RECT) -> bool {
    rect.left <= rect.right
        && rect.top <= rect.bottom
        && [rect.left, rect.top, rect.right, rect.bottom]
            .iter()
            .all(|v| (-1_000_000..=1_000_000).contains(v))
}
/// 在工作区内放置：优先放在组合下方，下方不足则翻到上方，左右夹取。
pub(crate) fn position(caret: RECT, work: RECT, width: i32, height: i32) -> RECT {
    let width = width.min(work.right - work.left).max(0);
    let height = height.min(work.bottom - work.top).max(0);
    let left = caret.left.clamp(work.left, work.right - width);
    let top = if caret.bottom <= work.bottom - height {
        caret.bottom
    } else {
        caret.top - height
    }
    .clamp(work.top, work.bottom - height);
    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}
/// 组合布局不可用时退回本线程系统光标；同样取不到则不显示候选窗，不猜位置。
pub(crate) fn fallback_caret() -> Option<RECT> {
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
    let rect = RECT {
        left: corners[0].x,
        top: corners[0].y,
        right: corners[1].x,
        bottom: corners[1].y,
    };
    bounded(&rect).then_some(rect)
}
/// 候选窗配色。产品要求界面不得出现蓝色，所以不用系统强调色/选中色，固定为中性灰；
/// 仅在用户自己开启高对比度时改用其系统颜色，保证可读性。
#[derive(Clone, Copy, PartialEq, Debug)]
struct Palette {
    back: COLORREF,
    border: COLORREF,
    text: COLORREF,
    hint: COLORREF,
    chosen_back: COLORREF,
    chosen_text: COLORREF,
    chosen_hint: COLORREF,
}
const fn gray(level: u8) -> COLORREF {
    COLORREF(level as u32 * 0x01_01_01)
}
const NEUTRAL: Palette = Palette {
    back: gray(0xF9),
    border: gray(0xD6),
    text: gray(0x1A),
    hint: gray(0x70),
    chosen_back: gray(0xDC),
    chosen_text: gray(0x1A),
    chosen_hint: gray(0x4A),
};
fn palette() -> Palette {
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
        return NEUTRAL;
    }
    // SAFETY: 只读系统颜色。
    let system = |index| COLORREF(unsafe { GetSysColor(index) });
    Palette {
        back: system(COLOR_WINDOW),
        border: system(COLOR_WINDOWTEXT),
        text: system(COLOR_WINDOWTEXT),
        hint: system(COLOR_GRAYTEXT),
        chosen_back: system(COLOR_HIGHLIGHT),
        chosen_text: system(COLOR_HIGHLIGHTTEXT),
        chosen_hint: system(COLOR_HIGHLIGHTTEXT),
    }
}
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}
fn head(candidate: &Candidate) -> String {
    format!("{} {}", candidate.label, candidate.text)
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

/// 每个候选行的客户区矩形与“序号+候选字”宽度。
type Rows = Vec<(RECT, i32)>;
#[derive(Default)]
pub(crate) struct View {
    /// 正在显示的快照；隐藏时为 None。
    pub state: Option<InputState>,
    /// 本地展示编号：每次显示/隐藏递增，旧鼠标消息与旧点击不能重解释到新内容。
    /// 32 位回绕后才可能重复，届时后台快照编号仍会拒绝旧选择。
    pub serial: u32,
    pub rows: Rows,
    pub notice: Option<RECT>,
    pressed: Option<u8>,
    /// 已排队一个点击会话；展示变化或会话结束即解除。
    pending: bool,
}

pub struct Popup {
    pub(crate) hwnd: HWND,
    pub(crate) view: RefCell<View>,
    font: Cell<(HFONT, u32)>,
    click: Box<dyn Fn(u32, u8)>,
    root: RefCell<Option<IRawElementProviderSimple>>,
    /// 测试保持隐藏，不在桌面显示窗口；其余行为不变。
    quiet: bool,
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
        // SAFETY: NOACTIVATE/TOOLWINDOW 不抢宿主焦点、不进任务栏；未传入创建参数指针。
        let hwnd = unsafe {
            CreateWindowExW(
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
            )?
        };
        let popup = Rc::new(Self {
            hwnd,
            view: RefCell::default(),
            font: Cell::new((HFONT::default(), 0)),
            click: Box::new(click),
            root: RefCell::new(None),
            quiet,
        });
        POPUPS.with(|popups| {
            popups
                .borrow_mut()
                .push((hwnd.0 as usize, Rc::downgrade(&popup)))
        });
        Ok(popup)
    }
    fn handle(self: &Rc<Self>, message: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
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
    fn dpi(&self) -> i32 {
        self.font.get().1.max(96) as i32
    }
    fn scale(&self, value: i32) -> i32 {
        value * self.dpi() / 96
    }
    fn hit(&self, at: POINT) -> Option<u8> {
        let view = self.view.try_borrow().ok()?;
        let index = view.rows.iter().position(|(rect, _)| inside(rect, at))?;
        u8::try_from(index).ok()
    }
    fn release(&self) {
        // SAFETY: 只释放本窗口持有的鼠标捕获；会同步收到 WM_CAPTURECHANGED。
        unsafe {
            if GetCapture() == self.hwnd {
                let _ = ReleaseCapture();
            }
        }
    }
    fn paint(&self) {
        let colors = palette();
        let mut paint = PAINTSTRUCT::default();
        // SAFETY: WM_PAINT 内配对 BeginPaint/EndPaint；本次新建的画刷在结束前删除，字体由本窗口持有并在结束前还原。
        unsafe {
            let dc = BeginPaint(self.hwnd, &mut paint);
            let back = CreateSolidBrush(colors.back);
            let border = CreateSolidBrush(colors.border);
            let chosen_back = CreateSolidBrush(colors.chosen_back);
            let mut client = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut client);
            FillRect(dc, &client, back);
            FrameRect(dc, &client, border);
            let old = SelectObject(dc, self.font.get().0.into());
            SetBkMode(dc, TRANSPARENT);
            let pad = self.scale(PAD_X);
            let gap = self.scale(GAP);
            if let Ok(view) = self.view.try_borrow()
                && let Some(state) = &view.state
            {
                for (index, (candidate, (rect, width))) in
                    state.candidates.iter().zip(&view.rows).enumerate()
                {
                    let chosen = state.highlighted == Some(index);
                    if chosen {
                        FillRect(dc, rect, chosen_back);
                    }
                    SetTextColor(
                        dc,
                        if chosen {
                            colors.chosen_text
                        } else {
                            colors.text
                        },
                    );
                    let mut text = RECT {
                        left: rect.left + pad,
                        right: rect.right - pad,
                        ..*rect
                    };
                    let format = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX;
                    DrawTextW(dc, &mut wide(&head(candidate)), &mut text, format);
                    if !candidate.comment.is_empty() {
                        SetTextColor(
                            dc,
                            if chosen {
                                colors.chosen_hint
                            } else {
                                colors.hint
                            },
                        );
                        text.left += width + gap;
                        DrawTextW(
                            dc,
                            &mut wide(&candidate.comment),
                            &mut text,
                            format | DT_END_ELLIPSIS,
                        );
                    }
                }
                if let Some(rect) = view.notice {
                    SetTextColor(dc, colors.hint);
                    let mut text = RECT {
                        left: rect.left + pad,
                        right: rect.right - pad,
                        ..rect
                    };
                    DrawTextW(
                        dc,
                        &mut wide(&state.notice),
                        &mut text,
                        DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
                    );
                }
            }
            SelectObject(dc, old);
            for brush in [back, border, chosen_back] {
                let _ = DeleteObject(brush.into());
            }
            let _ = EndPaint(self.hwnd, &paint);
        }
    }
    /// 字号随窗口 DPI；新字体建好后才释放旧字体。
    fn ensure_font(&self, dpi: u32) -> Result<()> {
        let (font, current) = self.font.get();
        if current == dpi && !font.is_invalid() {
            return Ok(());
        }
        // SAFETY: 新建 GDI 字体由本窗口持有，Drop 时释放。
        let created = unsafe {
            CreateFontW(
                -(POINT_SIZE * dpi as i32 / 72),
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Microsoft YaHei UI"),
            )
        };
        if created.is_invalid() {
            return Err(E_FAIL.into());
        }
        self.font.set((created, dpi));
        if !font.is_invalid() {
            // SAFETY: 旧字体只在绘制/测量期间临时选入，已还原，不再被任何 DC 使用。
            unsafe {
                let _ = DeleteObject(font.into());
            }
        }
        Ok(())
    }
    /// 量出每行与提示行的客户区矩形及窗口尺寸；长注释由绘制时省略号截断。
    fn measure(&self, state: &InputState) -> Result<(Rows, Option<RECT>, SIZE)> {
        // SAFETY: 取本窗口 DC 测量文本，选入的字体在 ReleaseDC 前还原。
        let dc = unsafe { GetDC(Some(self.hwnd)) };
        if dc.is_invalid() {
            return Err(E_FAIL.into());
        }
        // SAFETY: 同上，DC 有效。
        let old = unsafe { SelectObject(dc, self.font.get().0.into()) };
        let extent = |text: &str| {
            let mut size = SIZE::default();
            // SAFETY: 字符串借用在同步调用内有效。
            let _ = unsafe { GetTextExtentPoint32W(dc, &wide(text), &mut size) };
            size.cx
        };
        let mut metrics = TEXTMETRICW::default();
        // SAFETY: 输出结构为本地值。
        let _ = unsafe { GetTextMetricsW(dc, &mut metrics) };
        let pad = self.scale(PAD_X);
        let mut width = self.scale(MIN_WIDTH);
        let mut heads = Vec::with_capacity(state.candidates.len());
        for candidate in &state.candidates {
            let head = extent(&head(candidate));
            let comment = if candidate.comment.is_empty() {
                0
            } else {
                self.scale(GAP) + extent(&candidate.comment)
            };
            width = width.max(2 * pad + head + comment);
            heads.push(head);
        }
        if !state.notice.is_empty() {
            width = width.max(2 * pad + extent(&state.notice));
        }
        // SAFETY: 还原原字体并归还 DC。
        unsafe {
            SelectObject(dc, old);
            ReleaseDC(Some(self.hwnd), dc);
        }
        let width = width.min(self.scale(MAX_WIDTH)) + 2;
        let line = metrics.tmHeight.max(1) + 2 * self.scale(PAD_Y);
        let row = |index: usize| RECT {
            left: 1,
            top: 1 + index as i32 * line,
            right: width - 1,
            bottom: 1 + (index as i32 + 1) * line,
        };
        let rows: Vec<_> = heads
            .into_iter()
            .enumerate()
            .map(|(index, head)| (row(index), head))
            .collect();
        let notice = (!state.notice.is_empty()).then(|| row(rows.len()));
        let lines = rows.len() + notice.is_some() as usize;
        Ok((
            rows,
            notice,
            SIZE {
                cx: width,
                cy: lines as i32 * line + 2,
            },
        ))
    }
    /// 显示或刷新：owner 取视图窗口（空则取焦点窗口），按组合位置放在工作区内，不激活。
    pub fn show(self: &Rc<Self>, state: &InputState, caret: RECT, owner: HWND) -> Result<()> {
        if !bounded(&caret) || state.candidates.len() > MAX_CANDIDATES {
            return Err(E_INVALIDARG.into());
        }
        self.release();
        let owner = if owner.is_invalid() {
            // SAFETY: 只读本线程焦点窗口。
            unsafe { GetFocus() }
        } else {
            owner
        };
        // SAFETY: 只读本窗口当前 owner。
        if unsafe { GetWindow(self.hwnd, GW_OWNER) }.unwrap_or_default() != owner {
            set_owner(self.hwnd, owner);
        }
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
        // SAFETY: 读取本窗口所在上下文的 DPI；未声明感知的宿主恒为 96。
        self.ensure_font(unsafe { GetDpiForWindow(self.hwnd) }.max(96))?;
        let (rows, notice, size) = self.measure(state)?;
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: 输出结构大小已填写，矩形为本地值。
        if !unsafe {
            GetMonitorInfoW(
                MonitorFromRect(&caret, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
        }
        .as_bool()
        {
            return Err(E_FAIL.into());
        }
        let place = position(caret, monitor.rcWork, size.cx, size.cy);
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
            view.rows = rows;
            view.notice = notice;
            view.serial = view.serial.wrapping_add(1);
            view.pressed = None;
            view.pending = false;
            shown
        };
        // SAFETY: 只使本窗口客户区失效，绘制在随后的 WM_PAINT 中进行。
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, true);
        }
        if shown {
            self.notify(EVENT_OBJECT_IME_CHANGE);
        } else {
            if !self.quiet {
                // SAFETY: SW_SHOWNA 不改变宿主的激活与键盘焦点。
                unsafe {
                    let _ = ShowWindow(self.hwnd, SW_SHOWNA);
                }
            }
            self.notify(EVENT_OBJECT_IME_SHOW);
            self.raise(|| Some(self.root()), UIA_MenuOpenedEventId);
        }
        if let Some(index) = state.highlighted {
            self.raise(
                || uia::item(self, index),
                UIA_SelectionItem_ElementSelectedEventId,
            );
        }
        Ok(())
    }
    pub fn hide(self: &Rc<Self>) {
        self.release();
        let shown = match self.view.try_borrow_mut() {
            Ok(mut view) => {
                view.serial = view.serial.wrapping_add(1);
                view.pressed = None;
                view.pending = false;
                view.rows.clear();
                view.notice = None;
                view.state.take().is_some()
            }
            Err(_) => false,
        };
        if shown {
            if !self.quiet {
                // SAFETY: 只隐藏本候选窗，不影响宿主焦点。
                unsafe {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
            }
            self.notify(EVENT_OBJECT_IME_HIDE);
            self.raise(|| Some(self.root()), UIA_MenuClosedEventId);
        }
    }
    /// 点击可受理时取当前快照编号并标记待执行；同一展示只受理一个点击。
    pub fn selectable(&self, serial: u32, index: u8) -> Option<u64> {
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
        self.view
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
        let mut at = POINT { x, y };
        // SAFETY: 本窗口句柄有效，点为本地值。
        if !unsafe { ScreenToClient(self.hwnd, &mut at) }.as_bool() {
            return None;
        }
        self.hit(at).map(usize::from)
    }
    fn notify(&self, event: u32) {
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
            let (font, _) = self.font.get();
            if !font.is_invalid() {
                let _ = DeleteObject(font.into());
            }
        }
    }
}

#[cfg(test)]
#[path = "candidate_tests.rs"]
mod tests;
