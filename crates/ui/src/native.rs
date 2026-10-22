//! Win32 标准控件提供键盘导航和系统可访问性；回调无业务状态、无分配、无可展开代码。
use crate::{
    Inbox,
    model::{Model, details},
};
use std::{
    io,
    os::windows::io::AsRawHandle,
    ptr::{null, null_mut},
    sync::mpsc::{SyncSender, TryRecvError},
};
use tigertype_contracts::{
    Options, OutputForm,
    ui::{Reply, Request},
};
use windows_sys::Win32::{
    Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, SIZE, WAIT_FAILED, WPARAM},
    Graphics::Gdi::{
        CLEARTYPE_QUALITY, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW,
        COLOR_WINDOWTEXT, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DT_NOPREFIX,
        DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawFocusRect, DrawTextW, ETO_CLIPPED, ETO_OPAQUE,
        ExtTextOutW, FillRect, GetDC, GetSysColor, GetSysColorBrush, GetTextExtentPoint32W,
        GetTextMetricsW, HDC, HFONT, ReleaseDC, SelectObject, SetBkColor, SetBkMode, SetTextColor,
        TEXTMETRICW, TRANSPARENT,
    },
    System::{LibraryLoader::GetModuleHandleW, Threading::INFINITE},
    UI::{
        Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
        Controls::{
            BST_CHECKED, BST_UNCHECKED, DRAWITEMSTRUCT, EM_GETLINE, EM_GETRECT, EM_GETSEL,
            EM_LINEFROMCHAR, EM_LINEINDEX, EM_LINELENGTH, EM_POSFROMCHAR, EM_SETLIMITTEXT,
            EM_SETSEL, MEASUREITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED, ODT_COMBOBOX,
        },
        HiDpi::*,
        Input::{
            Ime::{GCS_COMPSTR, ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext},
            KeyboardAndMouse::{
                EnableWindow, GetFocus, GetKeyState, IsWindowEnabled, SetFocus, VK_CONTROL,
                VK_ESCAPE, VK_MENU, VK_RETURN,
            },
        },
        WindowsAndMessaging::*,
    },
};

const ACTION: u32 = WM_APP + 1;
const CLOSE: u32 = WM_APP + 2;
const LAYOUT: u32 = WM_APP + 3;
const QUERY: usize = 101;
const SEARCH: usize = 102;
const OUTPUT: usize = 103;
const PREVIOUS: usize = 104;
const NEXT: usize = 105;
const COPY: usize = 106;
const SETTINGS: usize = 107;
const STATUS: usize = 108;
const AUTO: usize = 201;
const COMMON: usize = 202;
const LEARNING: usize = 203;
const FORM: usize = 204;
const SAVE: usize = 205;

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

/// 选区与下拉选中项的中性灰：产品要求界面不得出现蓝色（与候选窗配色一致），
/// 系统选中色多为蓝色，所以不用；只在用户自己开启高对比度时沿用系统颜色保证可读。
const SELECTED_BACK: COLORREF = 0x00DC_DCDC;
const SELECTED_TEXT: COLORREF = 0x001A_1A1A;
/// 下拉框每项高度（96 DPI），容纳 15pt 字体。
const ITEM_HEIGHT: u32 = 30;

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

#[cfg(target_pointer_width = "64")]
fn class_procedure(hwnd: HWND) -> WNDPROC {
    // SAFETY: 读取 EDIT 类登记的原窗口过程（不受本窗口子类化影响），与 WNDPROC 同宽。
    unsafe { std::mem::transmute::<usize, WNDPROC>(GetClassLongPtrW(hwnd, GCLP_WNDPROC)) }
}
#[cfg(target_pointer_width = "32")]
fn class_procedure(hwnd: HWND) -> WNDPROC {
    // SAFETY: x86 没有 Ptr 版本，类长整数与函数指针同宽。
    unsafe { std::mem::transmute::<usize, WNDPROC>(GetClassLongW(hwnd, GCLP_WNDPROC) as usize) }
}

// SAFETY: 系统在所属 UI 线程调用；先交 EDIT 原过程处理，再在本次回调内用灰色重画可见选区；
// 不保存指针、不分配，选区相关以外的消息原样返回。
unsafe extern "system" fn edit_procedure(
    hwnd: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    // SAFETY: 参数原样交给 EDIT 类的原窗口过程。
    let result = unsafe { CallWindowProcW(class_procedure(hwnd), hwnd, message, w, l) };
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

/// EDIT 只能用系统选中色（蓝）画选区；原过程画完后，按字符位置用中性灰重画仍可见的选区，
/// 选择与复制行为不变。选区只在拥有焦点时可见（未设 ES_NOHIDESEL）。
fn repaint_selection(hwnd: HWND) {
    // SAFETY: 只读本线程焦点。
    if unsafe { GetFocus() } != hwnd || high_contrast() {
        return;
    }
    // SAFETY: 取本窗口 DC 并在结束时归还；先隐藏插入符，避免异或插入符在灰色上留下残影。
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
    let mut format = RECT::default();
    send(EM_GETRECT, 0, &mut format as *mut RECT as LPARAM);
    let first = send(EM_LINEFROMCHAR, start as usize, 0).max(0) as usize;
    let last = send(EM_LINEFROMCHAR, end as usize, 0).max(0) as usize;
    // SAFETY: 选入本控件字体测量与绘制，结束前还原；文字缓冲区在调用期间存活。
    unsafe {
        let old = SelectObject(dc, send(WM_GETFONT, 0, 0) as HFONT);
        let mut metrics: TEXTMETRICW = std::mem::zeroed();
        GetTextMetricsW(dc, &mut metrics);
        SetTextColor(dc, SELECTED_TEXT);
        SetBkColor(dc, SELECTED_BACK);
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

/// 自绘下拉框各项：选中项用中性灰而非系统选中色；文字取自控件自身字符串，无分配。
fn draw_item(item: &DRAWITEMSTRUCT) {
    let chosen = item.itemState & ODS_SELECTED != 0;
    // SAFETY: 只读系统颜色。
    let system = |index| unsafe { GetSysColor(index) };
    let (back, mut text) = match (chosen, high_contrast()) {
        (true, false) => (SELECTED_BACK, SELECTED_TEXT),
        (true, true) => (system(COLOR_HIGHLIGHT), system(COLOR_HIGHLIGHTTEXT)),
        (false, _) => (system(COLOR_WINDOW), system(COLOR_WINDOWTEXT)),
    };
    if item.itemState & ODS_DISABLED != 0 {
        text = system(COLOR_GRAYTEXT);
    }
    // SAFETY: 在系统提供的 DC 与项目矩形内绘制；新建画刷用后即删，控件句柄来自本次通知。
    unsafe {
        let brush = CreateSolidBrush(back);
        FillRect(item.hDC, &item.rcItem, brush);
        DeleteObject(brush);
        if item.itemID != u32::MAX {
            let mut buffer = [0u16; 64];
            let length = SendMessageW(item.hwndItem, CB_GETLBTEXTLEN, item.itemID as usize, 0);
            if (0..buffer.len() as isize).contains(&length)
                && SendMessageW(
                    item.hwndItem,
                    CB_GETLBTEXT,
                    item.itemID as usize,
                    buffer.as_mut_ptr() as LPARAM,
                ) >= 0
            {
                let mut rect = RECT {
                    left: item.rcItem.left + (item.rcItem.bottom - item.rcItem.top) / 4,
                    ..item.rcItem
                };
                SetBkMode(item.hDC, TRANSPARENT as _);
                SetTextColor(item.hDC, text);
                DrawTextW(
                    item.hDC,
                    buffer.as_ptr(),
                    length as i32,
                    &mut rect,
                    DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                );
            }
        }
        if item.itemState & ODS_FOCUS != 0 {
            DrawFocusRect(item.hDC, &item.rcItem);
        }
    }
}

// SAFETY: Windows 在所属 UI 线程调用；不解引用业务指针、不使用全局状态，不执行可能 panic 的 Rust 操作。
unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: 只转发值类型通知；WM_DPICHANGED 的 RECT、自绘结构仅在本次系统回调内读取，绝不排队裸指针。
    unsafe {
        match message {
            WM_MEASUREITEM if l != 0 => {
                let item = &mut *(l as *mut MEASUREITEMSTRUCT);
                if item.CtlType != ODT_COMBOBOX {
                    return DefWindowProcW(hwnd, message, w, l);
                }
                item.itemHeight = ITEM_HEIGHT * GetDpiForWindow(hwnd).max(96) / 96;
                1
            }
            WM_DRAWITEM if l != 0 => {
                let item = &*(l as *const DRAWITEMSTRUCT);
                if item.CtlType != ODT_COMBOBOX {
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
            WM_SIZE => {
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
            WM_GETMINMAXINFO => {
                if l != 0 {
                    let info = &mut *(l as *mut MINMAXINFO);
                    let dpi = GetDpiForWindow(hwnd) as i32;
                    info.ptMinTrackSize.x = 560 * dpi / 96;
                    info.ptMinTrackSize.y = 400 * dpi / 96;
                }
                0
            }
            _ => DefWindowProcW(hwnd, message, w, l),
        }
    }
}

struct Window {
    handle: HWND,
    controls: Vec<(usize, HWND)>,
    font: HFONT,
    dpi: u32,
}
impl Window {
    fn create(class: &[u16], title: &str, owner: HWND) -> Result<Self, String> {
        // SAFETY: 字符串 NUL 终止；窗口归当前线程所有，没有传入业务指针。
        let handle = unsafe {
            CreateWindowExW(
                WS_EX_CONTROLPARENT,
                class.as_ptr(),
                wide(title).as_ptr(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                800,
                740,
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
            font: null_mut(),
            dpi: 0,
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
    /// 文本框改用会以灰色重画选区的过程；控件随父窗口销毁，无需还原。
    fn gray_selection(&self, id: usize) {
        // SAFETY: 本线程拥有的 EDIT 控件，新过程以类登记的原过程处理一切消息。
        unsafe {
            SetWindowLongPtrW(
                self.control(id),
                GWLP_WNDPROC,
                edit_procedure as *const () as usize as _,
            );
        }
    }
    fn send(&self, id: usize, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        // SAFETY: 本模块只发送已匹配控件类型的同步消息；字符串参数在调用方保持存活。
        unsafe { SendMessageW(self.control(id), message, w, l) }
    }
    fn place(&self, id: usize, x: i32, y: i32, width: i32, height: i32) {
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
    fn layout(&mut self, settings: bool) -> Result<(), String> {
        // SAFETY: 查询本线程拥有窗口的当前 DPI 和客户区。
        let dpi = unsafe { GetDpiForWindow(self.handle) }.max(96);
        if self.dpi != dpi {
            // SAFETY: 创建 15pt 的系统中文 UI 字体；控件替换完成后才释放旧字体。
            let font = unsafe {
                CreateFontW(
                    -(15 * dpi as i32 / 72),
                    0,
                    0,
                    0,
                    400,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET as u32,
                    0,
                    0,
                    CLEARTYPE_QUALITY as u32,
                    0,
                    wide("Microsoft YaHei UI").as_ptr(),
                )
            };
            if font.is_null() {
                return Err(error());
            }
            for (_, control) in &self.controls {
                // SAFETY: 新字体在整个控件使用期保持存活，消息同步处理。
                unsafe {
                    SendMessageW(*control, WM_SETFONT, font as usize, 1);
                }
            }
            if !self.font.is_null() {
                // SAFETY: 所有控件已改用新字体，旧 GDI 对象不再被引用。
                unsafe {
                    DeleteObject(self.font);
                }
            }
            self.font = font;
            self.dpi = dpi;
            // 自绘下拉框的选中框与列表项高度随 DPI 变化；-1 指选中框本身。
            if let Some((_, combo)) = self.controls.iter().find(|(id, _)| *id == FORM) {
                for index in [usize::MAX, 0] {
                    // SAFETY: 本线程拥有的下拉框，值类型参数。
                    unsafe {
                        SendMessageW(
                            *combo,
                            CB_SETITEMHEIGHT,
                            index,
                            (ITEM_HEIGHT * dpi / 96) as LPARAM,
                        );
                    }
                }
            }
        }
        if settings {
            self.place(AUTO, 20, 20, 490, 36);
            self.place(COMMON, 20, 66, 490, 36);
            self.place(LEARNING, 20, 112, 490, 36);
            self.place(210, 20, 164, 170, 32);
            self.place(FORM, 190, 160, 280, 150);
            self.place(SAVE, 20, 220, 180, 42);
            self.place(211, 20, 280, 490, 60);
        } else {
            let mut rect = RECT::default();
            // SAFETY: 输出 RECT 由栈分配且窗口有效。
            if unsafe { GetClientRect(self.handle, &mut rect) } == 0 {
                return Err(error());
            }
            let width = rect.right * 96 / dpi as i32;
            let height = rect.bottom * 96 / dpi as i32;
            self.place(110, 16, 12, width - 32, 28);
            self.place(QUERY, 16, 46, width - 146, 36);
            self.place(SEARCH, width - 120, 44, 104, 40);
            self.place(111, 16, 92, width - 32, 28);
            self.place(OUTPUT, 16, 124, width - 32, (height - 244).max(60));
            self.place(PREVIOUS, 16, height - 108, 100, 38);
            self.place(NEXT, 124, height - 108, 100, 38);
            self.place(COPY, 236, height - 108, 176, 38);
            self.place(SETTINGS, width - 120, height - 108, 104, 38);
            self.place(STATUS, 16, height - 58, width - 32, 52);
        }
        Ok(())
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: 子控件先随窗口销毁，之后释放字体；均在创建它们的 UI 线程执行。
        unsafe {
            DestroyWindow(self.handle);
            if !self.font.is_null() {
                DeleteObject(self.font);
            }
        }
    }
}

struct Class(Vec<u16>);
impl Class {
    fn register() -> Result<Self, String> {
        let name = wide("TigerType.Lookup.Native.v1");
        // SAFETY: 仅注册无状态系统回调；系统画刷/光标为共享对象，不由本程序释放。
        let registered = unsafe {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: GetModuleHandleW(null()),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hbrBackground: GetSysColorBrush(COLOR_WINDOW),
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

fn lookup_window(class: &Class) -> Result<Window, String> {
    let mut window = Window::create(&class.0, "TigerType 独立查字", null_mut())?;
    window.add(
        110,
        "STATIC",
        "拼音或汉字（最多 64 字，粘贴后点击查询）(&Q)",
        0,
    )?;
    window.add(
        QUERY,
        "EDIT",
        "",
        WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL as u32,
    )?;
    // 粘贴上限保留足够余量；超过业务上限明确报错，不把截断文本当成有效查询。
    window.send(QUERY, EM_SETLIMITTEXT, 4096, 0);
    window.gray_selection(QUERY);
    window.add(
        SEARCH,
        "BUTTON",
        "查询(&F)",
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
    )?;
    window.add(111, "STATIC", "本页学习资料（可选中文字复制）(&R)", 0)?;
    window.add(
        OUTPUT,
        "EDIT",
        "",
        WS_BORDER
            | WS_TABSTOP
            | WS_VSCROLL
            | ES_MULTILINE as u32
            | ES_AUTOVSCROLL as u32
            | ES_READONLY as u32,
    )?;
    window.gray_selection(OUTPUT);
    for (id, title) in [
        (PREVIOUS, "上页(&P)"),
        (NEXT, "下页(&N)"),
        (COPY, "复制本页资料(&C)"),
        (SETTINGS, "设置(&S)"),
    ] {
        window.add(id, "BUTTON", title, WS_TABSTOP)?;
    }
    window.add(STATUS, "STATIC", "", 0)?;
    window.layout(false)?;
    Ok(window)
}
fn settings_window(class: &Class, owner: HWND) -> Result<Window, String> {
    let mut window = Window::create(&class.0, "TigerType 设置", owner)?;
    for (id, title) in [
        (AUTO, "四码唯一时自动上屏(&A)"),
        (COMMON, "普通输入仅常用字(&C)"),
        (LEARNING, "普通输入显示学习提示(&L)"),
    ] {
        window.add(id, "BUTTON", title, WS_TABSTOP | BS_AUTOCHECKBOX as u32)?;
    }
    window.add(210, "STATIC", "输出字形(&O)", 0)?;
    // 自绘选中项以避开系统蓝色；保留字符串供键盘导航与读屏器使用。
    window.add(
        FORM,
        "COMBOBOX",
        "",
        WS_TABSTOP | (CBS_DROPDOWNLIST | CBS_OWNERDRAWFIXED | CBS_HASSTRINGS) as u32,
    )?;
    for text in ["原字", "简体", "繁体"] {
        if window.send(FORM, CB_ADDSTRING, 0, wide(text).as_ptr() as isize) < 0 {
            return Err("无法创建字形选项".into());
        }
    }
    window.add(
        SAVE,
        "BUTTON",
        "保存设置(&S)",
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
    )?;
    window.add(
        211,
        "STATIC",
        "关闭本窗放弃未保存修改；查字不受输入设置影响。",
        0,
    )?;
    window.layout(true)?;
    Ok(window)
}
fn set_options(window: &Window, options: Options) {
    for (id, checked) in [
        (AUTO, options.auto_commit_four),
        (COMMON, options.common_only),
        (LEARNING, options.show_learning),
    ] {
        window.send(
            id,
            BM_SETCHECK,
            if checked { BST_CHECKED } else { BST_UNCHECKED } as usize,
            0,
        );
    }
    window.send(
        FORM,
        CB_SETCURSEL,
        match options.output_form {
            OutputForm::Original => 0,
            OutputForm::Simplified => 1,
            OutputForm::Traditional => 2,
        },
        0,
    );
}
fn get_options(window: &Window) -> Result<Options, String> {
    Ok(Options {
        auto_commit_four: window.send(AUTO, BM_GETCHECK, 0, 0) == BST_CHECKED as isize,
        common_only: window.send(COMMON, BM_GETCHECK, 0, 0) == BST_CHECKED as isize,
        show_learning: window.send(LEARNING, BM_GETCHECK, 0, 0) == BST_CHECKED as isize,
        output_form: match window.send(FORM, CB_GETCURSEL, 0, 0) {
            0 => OutputForm::Original,
            1 => OutputForm::Simplified,
            2 => OutputForm::Traditional,
            _ => return Err("必须选择输出字形".into()),
        },
    })
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
    for id in [AUTO, COMMON, LEARNING, FORM, SAVE] {
        settings.enable(id, !model.busy);
    }
    main.text(STATUS, &model.notice);
    settings.text(211, &model.settings_notice);
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

fn key_command(
    settings: bool,
    id: usize,
    key: usize,
    dropdown: bool,
    ime: bool,
) -> Option<(u32, usize)> {
    // 下拉框和 IME 先处理确认/取消；只读资料框 Enter 不意外发起查询或写设置。
    if dropdown || ime {
        return None;
    }
    if settings {
        if key == VK_ESCAPE as usize {
            return Some((CLOSE, 0));
        }
        if key == VK_RETURN as usize {
            return Some((ACTION, SAVE));
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
            let dropdown = is_settings && settings.send(FORM, CB_GETDROPPEDSTATE, 0, 0) != 0;
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
            if let Some((event, action)) =
                key_command(is_settings, id, message.wParam, dropdown, ime)
            {
                post(root, event, action);
                return;
            }
        }
        // SAFETY: 使用系统导航处理 Tab/助记键，消息及窗口都在同一线程存活。
        if unsafe { IsDialogMessageW(dialog.handle, message) } != 0 {
            return;
        }
    }
    // SAFETY: 未接管的输入原样交回原生控件，包括 IME 和组合框弹出窗。
    unsafe {
        TranslateMessage(message);
        DispatchMessageW(message);
    }
}

/// 当前线程专用于原生 UI；事件唤醒回复、系统消息驱动操作，不使用定时轮询。
pub fn run(
    requests: SyncSender<Request>,
    inbox: Inbox<Reply>,
    options: Options,
) -> Result<(), String> {
    let _dpi = DpiContext::enter()?;
    let class = Class::register()?;
    let mut main = lookup_window(&class)?;
    let mut settings = settings_window(&class, main.handle)?;
    let mut model = Model::new(options);
    set_options(&settings, options);
    refresh(&main, &settings, &model);
    // SAFETY: 窗口和输入控件均创建成功，只显示独立工具，不注册或切换输入法。
    unsafe {
        ShowWindow(main.handle, SW_SHOW);
        SetFocus(main.control(QUERY));
    }
    event_loop(&mut main, &mut settings, &mut model, &requests, &inbox)
}

fn event_loop(
    main: &mut Window,
    settings: &mut Window,
    model: &mut Model,
    requests: &SyncSender<Request>,
    inbox: &Inbox<Reply>,
) -> Result<(), String> {
    let event = inbox.event.as_raw_handle();
    let mut main_focus = None;
    let mut settings_focus = None;
    loop {
        // SAFETY: Inbox 持有事件直到循环结束；消息和回复共享一次阻塞等待，空闲不耗 CPU。
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(1, &event, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        if wait == WAIT_FAILED {
            return Err(error());
        }
        loop {
            match inbox.receiver.try_recv() {
                Ok(reply) => {
                    if model.receive(reply)? {
                        main.text(
                            OUTPUT,
                            &model.result.as_ref().map(details).unwrap_or_default(),
                        );
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
                CLOSE if message.hwnd == main.handle => return Ok(()),
                CLOSE if message.hwnd == settings.handle => {
                    model.settings_open = false;
                    settings_focus = None;
                    set_options(settings, model.options);
                    // SAFETY: 设置窗仅隐藏，所有权继续由本函数持有。
                    unsafe {
                        ShowWindow(settings.handle, SW_HIDE);
                        SetFocus(main.control(QUERY));
                    }
                }
                LAYOUT => {
                    if message.hwnd == main.handle {
                        main.layout(false)?;
                    } else if message.hwnd == settings.handle {
                        settings.layout(true)?;
                    }
                }
                ACTION => {
                    let id = message.wParam & 0xffff;
                    let notification = (message.wParam >> 16) as u32;
                    if id == QUERY && notification == EN_CHANGE && !model.busy {
                        model.result = None;
                        model.page = 0;
                        main.text(OUTPUT, "");
                        model.notice = "文本已修改，请点击查询。".into();
                        refresh(main, settings, model);
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
                        SETTINGS => {
                            if model.open_settings() {
                                set_options(settings, model.options);
                            }
                            // SAFETY: 显示本程序拥有的设置窗，并把焦点放到第一个标准复选框。
                            unsafe {
                                ShowWindow(settings.handle, SW_SHOW);
                                SetForegroundWindow(settings.handle);
                                SetFocus(settings.control(AUTO));
                            }
                            None
                        }
                        COPY if model.result.as_ref().is_some_and(|p| !p.items.is_empty()) => {
                            // 只在用户点击时委托标准 EDIT 复制，不读取或监视剪贴板。
                            main.send(OUTPUT, EM_SETSEL, 0, -1);
                            main.send(OUTPUT, WM_COPY, 0, 0);
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
                _ => keyboard(&message, main, settings),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::{
        Graphics::Gdi::{CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, GetPixel},
        UI::Controls::ODA_DRAWENTIRE,
    };

    // 产品要求界面不得有蓝色：选区配色必须是 R=G=B 的中性灰，且文字比背景深。
    const _: () = {
        let [r, g, b, _] = SELECTED_BACK.to_le_bytes();
        let [tr, tg, tb, _] = SELECTED_TEXT.to_le_bytes();
        assert!(r == g && g == b && tr == tg && tg == tb && tr < r);
    };

    /// 在内存位图上作画并读像素，不显示窗口、不碰桌面。
    struct Canvas(HDC, *mut core::ffi::c_void, *mut core::ffi::c_void);
    impl Canvas {
        fn new(width: i32, height: i32) -> Self {
            // SAFETY: 创建与屏幕兼容的内存 DC 和位图，Drop 时配对释放；先铺白底。
            unsafe {
                let screen = GetDC(null_mut());
                let dc = CreateCompatibleDC(screen);
                let bitmap = CreateCompatibleBitmap(screen, width, height);
                ReleaseDC(null_mut(), screen);
                let old = SelectObject(dc, bitmap);
                let white = CreateSolidBrush(0x00FF_FFFF);
                FillRect(
                    dc,
                    &RECT {
                        left: 0,
                        top: 0,
                        right: width,
                        bottom: height,
                    },
                    white,
                );
                DeleteObject(white);
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

    /// 由隐藏窗口测试调用（窗口类同名，不能与之并行注册）：选区与下拉选中项为中性灰而非系统蓝。
    fn assert_neutral_highlights(main: &Window, settings: &Window) {
        for id in [QUERY, OUTPUT] {
            // SAFETY: 只读本测试窗口的窗口过程。
            let current = unsafe { GetWindowLongPtrW(main.control(id), GWLP_WNDPROC) };
            assert_eq!(current as usize, edit_procedure as *const () as usize);
        }
        let contrast = high_contrast();
        // SAFETY: 只读系统颜色。
        let system = |index| unsafe { GetSysColor(index) };

        // 跨行选区“乙丙 / 丁戊”：选中处为灰，未选中的“甲”与“己”保持原样。
        main.text(OUTPUT, "甲乙丙\r\n丁戊己");
        main.send(OUTPUT, EM_SETSEL, 1, 7);
        let canvas = Canvas::new(800, 400);
        paint_selection(main.control(OUTPUT), canvas.0);
        let at = |index: usize| {
            let position = main.send(OUTPUT, EM_POSFROMCHAR, index, 0);
            (
                (position & 0xffff) as i16 as i32,
                ((position >> 16) & 0xffff) as i16 as i32,
            )
        };
        for index in [1, 2, 5, 6] {
            let (x, y) = at(index);
            assert_eq!(canvas.pixel(x + 1, y), SELECTED_BACK, "第 {index} 字");
        }
        for index in [0, 7] {
            let (x, y) = at(index);
            assert_eq!(canvas.pixel(x + 1, y), 0x00FF_FFFF, "第 {index} 字");
        }

        // 下拉框选中项为灰，未选中项为窗口底色；高对比度时沿用系统颜色。
        for (state, expected) in [
            (
                ODS_SELECTED,
                if contrast {
                    system(COLOR_HIGHLIGHT)
                } else {
                    SELECTED_BACK
                },
            ),
            (0, system(COLOR_WINDOW)),
        ] {
            let canvas = Canvas::new(200, 30);
            draw_item(&DRAWITEMSTRUCT {
                CtlType: ODT_COMBOBOX,
                CtlID: FORM as u32,
                itemID: 1,
                itemAction: ODA_DRAWENTIRE,
                itemState: state,
                hwndItem: settings.control(FORM),
                hDC: canvas.0,
                rcItem: RECT {
                    left: 0,
                    top: 0,
                    right: 200,
                    bottom: 30,
                },
                itemData: 0,
            });
            assert_eq!(canvas.pixel(1, 1), expected);
        }
    }

    #[test]
    fn keyboard_commands_preserve_native_dropdown_and_ime_handling() {
        assert_eq!(
            key_command(false, QUERY, VK_RETURN as usize, false, false),
            Some((ACTION, SEARCH))
        );
        for id in [SEARCH, PREVIOUS, NEXT, COPY, SETTINGS] {
            assert_eq!(
                key_command(false, id, VK_RETURN as usize, false, false),
                Some((ACTION, id))
            );
        }
        assert_eq!(
            key_command(false, OUTPUT, VK_RETURN as usize, false, false),
            None
        );
        assert_eq!(
            key_command(false, QUERY, VK_ESCAPE as usize, false, false),
            None
        );
        for id in [AUTO, COMMON, LEARNING, FORM, SAVE] {
            assert_eq!(
                key_command(true, id, VK_RETURN as usize, false, false),
                Some((ACTION, SAVE))
            );
            assert_eq!(
                key_command(true, id, VK_ESCAPE as usize, false, false),
                Some((CLOSE, 0))
            );
        }
        for key in [VK_RETURN, VK_ESCAPE] {
            assert_eq!(key_command(true, FORM, key as usize, true, false), None);
            assert_eq!(key_command(false, QUERY, key as usize, false, true), None);
        }
    }
    #[test]
    fn hidden_native_controls_roundtrip_unicode_settings_and_cleanup() {
        // 合成控件测试不显示窗口、不碰剪贴板、不加载个人资源。
        let _dpi = DpiContext::enter().unwrap();
        for _ in 0..2 {
            let class = Class::register().unwrap();
            let mut main = lookup_window(&class).unwrap();
            let mut settings = settings_window(&class, main.handle).unwrap();
            assert_neutral_highlights(&main, &settings);
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
            };
            set_options(&settings, options);
            assert_eq!(get_options(&settings).unwrap(), options);
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
            let mut model = Model::new(options);
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
                                character: '𠀀',
                                learning: Default::default(),
                            }],
                            has_more: false,
                        },
                    )))
                    .unwrap();
            });
            post(main.handle, ACTION, SEARCH);
            assert!(
                event_loop(&mut main, &mut settings, &mut model, &requests, &inbox)
                    .unwrap_err()
                    .contains("工作线程已退出")
            );
            worker.join().unwrap();
            assert_eq!(model.result.as_ref().unwrap().items[0].character, '𠀀');
            assert!(!model.busy);
            // 保存回复走真实事件循环，但不重写资料框，保留用户选中的 UTF-16 范围。
            main.send(OUTPUT, EM_SETSEL, 1, 3);
            let (requests, _receiver) = std::sync::mpsc::sync_channel(1);
            let (replies, inbox) = crate::mailbox().unwrap();
            model.save(options).unwrap();
            replies
                .send(tigertype_contracts::ui::Reply::Saved(Ok(options)))
                .unwrap();
            drop(replies);
            assert!(event_loop(&mut main, &mut settings, &mut model, &requests, &inbox).is_err());
            assert_eq!(
                main.send(OUTPUT, windows_sys::Win32::UI::Controls::EM_GETSEL, 0, 0),
                1 | (3 << 16)
            );
            assert_eq!(model.options, options);
            let (replies, inbox) = crate::mailbox().unwrap();
            replies
                .send(tigertype_contracts::ui::Reply::Saved(Ok(options)))
                .unwrap();
            drop(replies);
            assert!(
                event_loop(&mut main, &mut settings, &mut model, &requests, &inbox)
                    .unwrap_err()
                    .contains("回复与在途操作不符")
            );
            // 反复创建可发现遗漏的窗口类注销，避免把线程退出误当资源已释放。
            drop(settings);
            drop(main);
            drop(class);
        }
    }
}
