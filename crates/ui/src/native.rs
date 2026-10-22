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
use tigerrime_contracts::{
    Options, OutputForm,
    ui::{Reply, Request},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WAIT_FAILED, WPARAM},
    Graphics::Gdi::{
        CLEARTYPE_QUALITY, COLOR_WINDOW, CreateFontW, DEFAULT_CHARSET, DeleteObject,
        GetSysColorBrush, HFONT,
    },
    System::{LibraryLoader::GetModuleHandleW, Threading::INFINITE},
    UI::{
        Controls::{BST_CHECKED, BST_UNCHECKED, EM_SETLIMITTEXT, EM_SETSEL},
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

// SAFETY: Windows 在所属 UI 线程调用；不解引用业务指针、不使用全局状态，不执行可能 panic 的 Rust 操作。
unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: 只转发值类型通知；WM_DPICHANGED 的 RECT 仅在本次系统回调内读取，绝不排队裸指针。
    unsafe {
        match message {
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
        let name = wide("TigerRime.Lookup.Native.v1");
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
    let mut window = Window::create(&class.0, "TigerRime 独立查字", null_mut())?;
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
    let mut window = Window::create(&class.0, "TigerRime 设置", owner)?;
    for (id, title) in [
        (AUTO, "四码唯一时自动上屏(&A)"),
        (COMMON, "普通输入仅常用字(&C)"),
        (LEARNING, "普通输入显示学习提示(&L)"),
    ] {
        window.add(id, "BUTTON", title, WS_TABSTOP | BS_AUTOCHECKBOX as u32)?;
    }
    window.add(210, "STATIC", "输出字形(&O)", 0)?;
    window.add(FORM, "COMBOBOX", "", WS_TABSTOP | CBS_DROPDOWNLIST as u32)?;
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
                    .send(tigerrime_contracts::ui::Reply::Lookup(Ok(
                        tigerrime_contracts::LookupPage {
                            items: vec![tigerrime_contracts::LookupItem {
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
                .send(tigerrime_contracts::ui::Reply::Saved(Ok(options)))
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
                .send(tigerrime_contracts::ui::Reply::Saved(Ok(options)))
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
