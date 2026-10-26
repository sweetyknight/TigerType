//! 后台常驻的通知区图标：不依赖前台程序与当前输入法，任何时候单击都打开设置页，右键菜单打开设置页或查字窗。
//! 前台程序正用 TigerType 时任务栏已有本输入法图标，此时隐藏通知区图标，同一时刻只显示一个。
//! 打开溢出区等操作把前台交给任务栏，宿主随之报告失去焦点，但任务栏仍显示先前程序的输入法图标，
//! 所以前台是任务栏或其弹出面板时保持隐藏，等前台换到其他窗口（进程外 WinEvent 回调，不注入）再定。
//! 只在后台固定 UI 线程；回调不存业务状态、不分配，选择经线程消息交给事件循环。
//! 登录早期资源管理器尚未就绪或其后重启时，都靠 TaskbarCreated 广播重新挂上（保持当前显隐）。
use super::*;
use crate::Latest;
use windows_sys::Win32::UI::{
    Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
    Controls::ODT_MENU,
    Shell::{
        NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_STATE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
        NIM_SETVERSION, NIN_SELECT, NIS_HIDDEN, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
        Shell_NotifyIconW,
    },
};

/// 图标回调消息。
const NOTIFY: u32 = WM_APP + 4;
/// 打开窗口的线程消息，wParam 由 [`encode`] 给出目标窗口。
pub(super) const OPEN: u32 = WM_APP + 5;
/// 版本 4 下键盘确认（Enter）的事件，windows-sys 未导出。
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const ICON_ID: u32 = 1;
/// 菜单项：编号、文字、目标窗口、图标字体码位。自绘菜单项不自动处理助记键，所以不写助记符，用方向键与 Enter 选择。
const ITEMS: [(u32, &str, Panel, u16); 2] = [
    (1, "查字", Panel::Lookup, 0xE721),
    (2, "设置", Panel::Settings, 0xE713),
];

thread_local! {
    // 当前登记给通知区的图标；系统未承诺复制图标，登记期间保持有效，换新或删除后才释放。
    static ICON: Cell<HICON> = const { Cell::new(null_mut()) };
    // 已应用的显隐；资源管理器重启后重新登记时沿用。
    static HIDDEN: Cell<bool> = const { Cell::new(false) };
    // 后台最近给出的：是否有宿主持有任务栏输入法图标。
    static HELD: Cell<bool> = const { Cell::new(false) };
    // 本线程的图标窗口：前台切换回调不带上下文，借此找到要改的图标。
    static WINDOW: Cell<HWND> = const { Cell::new(null_mut()) };
}

/// 任务栏及其弹出面板的窗口类：它们取得前台时任务栏仍显示先前程序的输入法图标。
const SHELL: [&str; 5] = [
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "XamlExplorerHostIslandWindow",
];
fn shell(class: &[u16]) -> bool {
    SHELL
        .iter()
        .any(|name| name.encode_utf16().eq(class.iter().copied()))
}
/// 有宿主持有时隐藏；无人持有时，前台是任务栏面板则维持现状，否则显示。
fn decide(held: bool, hidden: bool, shell: bool) -> bool {
    held || (hidden && shell)
}
/// 按当前持有与前台重新决定显隐，变化时才改登记的图标。不分配。
fn refresh(hwnd: HWND) {
    let mut class = [0u16; 64];
    // SAFETY: 只读前台窗口类名到栈缓冲；无前台窗口时得 0 个字符。
    let length = unsafe { GetClassNameW(GetForegroundWindow(), class.as_mut_ptr(), 64) };
    let foreground = &class[..length.max(0) as usize];
    let hidden = decide(HELD.get(), HIDDEN.get(), shell(foreground));
    if hidden == HIDDEN.replace(hidden) {
        return;
    }
    let data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        uFlags: NIF_STATE,
        dwState: if hidden { NIS_HIDDEN } else { 0 },
        dwStateMask: NIS_HIDDEN,
        ..Default::default()
    };
    // SAFETY: 结构大小已填，只改本窗口登记图标的隐藏位；未登记时系统只返回失败。
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
}
// SAFETY: 系统在本 UI 线程取消息时调用（进程外钩子）；只读线程局部状态并改本线程的图标，不分配。
unsafe extern "system" fn foreground(
    _: HWINEVENTHOOK,
    _: u32,
    _: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    let hwnd = WINDOW.get();
    if !hwnd.is_null() {
        refresh(hwnd);
    }
}

pub(super) fn encode(panel: Panel) -> WPARAM {
    match panel {
        Panel::Settings => 0,
        Panel::Lookup => 1,
    }
}
pub(super) fn decode(value: WPARAM) -> Panel {
    if value == 1 {
        Panel::Lookup
    } else {
        Panel::Settings
    }
}

/// 不分配的 NUL 结尾 UTF-16：回调内只用栈缓冲，超长截断。
fn utf16<const N: usize>(text: &str) -> [u16; N] {
    let mut buffer = [0u16; N];
    for (slot, unit) in buffer[..N - 1].iter_mut().zip(text.encode_utf16()) {
        *slot = unit;
    }
    buffer
}

pub(super) struct Tray {
    hwnd: HWND,
    class: [u16; 32],
    // 前台切换钩子；装不上时只是打开溢出区后图标可能多显示一会儿。
    hook: HWINEVENTHOOK,
}
impl Tray {
    /// 隐藏的顶层窗口接收图标回调与 TaskbarCreated 广播（仅消息窗口收不到广播），从不显示。
    pub(super) fn window() -> Result<Self, String> {
        let class = utf16::<32>("TigerType.Tray.v1");
        let title = utf16::<16>("TigerType");
        // SAFETY: 注册无状态回调并建窗口；类名在本对象存活期间有效，Drop 时配对销毁与注销。
        unsafe {
            if RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: GetModuleHandleW(null()),
                lpszClassName: class.as_ptr(),
                ..Default::default()
            }) == 0
            {
                return Err(error());
            }
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            if hwnd.is_null() {
                let failure = error();
                UnregisterClassW(class.as_ptr(), GetModuleHandleW(null()));
                return Err(failure);
            }
            WINDOW.set(hwnd);
            // 进程外钩子：回调在本线程取消息时执行，不向其他进程注入；Drop 时配对注销。
            let hook = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                null_mut(),
                Some(foreground),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            Ok(Self { hwnd, class, hook })
        }
    }
    /// 建窗口并挂图标；资源管理器尚未就绪时登记失败，等 TaskbarCreated 再挂，不算故障。
    pub(super) fn new() -> Result<Self, String> {
        let tray = Self::window()?;
        add(tray.hwnd);
        Ok(tray)
    }
    /// 取走后台给出的最新持有状态并重新决定显隐；图标尚未登记时只记下，登记时生效。
    pub(super) fn sync(&self, taskbar: &Latest<bool>) -> Result<(), String> {
        if let Some(held) = taskbar.take()? {
            HELD.set(held);
            refresh(self.hwnd);
        }
        Ok(())
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
        let data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: ICON_ID,
            ..Default::default()
        };
        // SAFETY: 注销钩子、删除本窗口登记的图标（未登记时系统只返回失败），再释放图标、销毁窗口并注销类，均在创建线程。
        unsafe {
            if !self.hook.is_null() {
                UnhookWinEvent(self.hook);
            }
            WINDOW.set(null_mut());
            Shell_NotifyIconW(NIM_DELETE, &data);
            let icon = ICON.with(|icon| icon.replace(null_mut()));
            if !icon.is_null() {
                DestroyIcon(icon);
            }
            DestroyWindow(self.hwnd);
            UnregisterClassW(self.class.as_ptr(), GetModuleHandleW(null()));
        }
    }
}

/// 登记图标并切到版本 4 回调（键盘选择与 WM_CONTEXTMENU 也会上报，读屏用户可用）。
fn add(hwnd: HWND) -> bool {
    let Some(icon) = icon() else {
        return false;
    };
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP | NIF_STATE,
        uCallbackMessage: NOTIFY,
        hIcon: icon,
        szTip: utf16("虎码：单击打开设置，右键更多"),
        dwState: if HIDDEN.get() { NIS_HIDDEN } else { 0 },
        dwStateMask: NIS_HIDDEN,
        ..Default::default()
    };
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    // SAFETY: 结构大小已填，窗口与图标在调用期间有效；成功后换下的旧图标才释放。
    unsafe {
        // 资源管理器重启后旧登记已失效，先删一次再加，避免同编号重复登记失败。
        Shell_NotifyIconW(NIM_DELETE, &data);
        let added =
            Shell_NotifyIconW(NIM_ADD, &data) != 0 && Shell_NotifyIconW(NIM_SETVERSION, &data) != 0;
        let old = if added {
            ICON.with(|slot| slot.replace(icon))
        } else {
            icon
        };
        if !old.is_null() {
            DestroyIcon(old);
        }
        added
    }
}

/// 现画小图标：虎码印章（琥珀底、米白“虎”字），与输入法配置图标一致；不随任务栏深浅变色，也无蓝色。
fn icon() -> Option<HICON> {
    // SAFETY: 系统度量查询无副作用。
    let side = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 256);
    super::seal::icon(side)
}

/// 右键菜单：自绘菜单项（保留文字供读屏器朗读），底色与高亮按主题，不用系统蓝色高亮。
fn menu(hwnd: HWND, x: i32, y: i32) {
    // SAFETY: 菜单与画刷在本函数内创建并释放；文字缓冲在插入调用期间有效（菜单复制文字）。
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        for (id, text, ..) in ITEMS {
            let mut text = utf16::<8>(text);
            let item = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_ID | MIIM_STRING | MIIM_FTYPE,
                fType: MFT_OWNERDRAW,
                wID: id,
                dwTypeData: text.as_mut_ptr(),
                ..Default::default()
            };
            InsertMenuItemW(menu, id, 0, &item);
        }
        let brush = CreateSolidBrush(colors().surface);
        SetMenuInfo(
            menu,
            &MENUINFO {
                cbSize: size_of::<MENUINFO>() as u32,
                fMask: MIM_BACKGROUND,
                hbrBack: brush,
                ..Default::default()
            },
        );
        // 菜单所有者须在前台，否则点别处不会收起；用户操作通知区图标时本进程获准切前台。
        SetForegroundWindow(hwnd);
        let align = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 {
            TPM_RIGHTALIGN
        } else {
            TPM_LEFTALIGN
        };
        let chosen = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | align,
            x,
            y,
            hwnd,
            null(),
        ) as u32;
        // 让菜单的模态循环在下次点击时正常结束（系统菜单在通知区的已知要求）。
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        DeleteObject(brush);
        if let Some((_, _, panel, _)) = ITEMS.iter().find(|(id, ..)| *id == chosen) {
            post(hwnd, OPEN, encode(*panel));
        }
    }
}

/// 自绘菜单项：图标加文字，悬停项为主题高亮圆角底，不用系统蓝色高亮；高对比度时沿用系统颜色。
fn draw_menu_item(item: &DRAWITEMSTRUCT) {
    let Some((_, text, _, code)) = ITEMS.iter().find(|(id, ..)| *id == item.itemID) else {
        return;
    };
    let c = colors();
    let dc = item.hDC;
    // SAFETY: 只读绘制 DC 的 DPI。
    let dpi = unsafe { GetDeviceCaps(dc, LOGPIXELSY as i32) }.max(96);
    let s = |value: i32| value * dpi / 96;
    let rect = item.rcItem;
    fill(dc, &rect, c.surface);
    if item.itemState & ODS_SELECTED != 0 {
        let inner = RECT {
            left: rect.left + s(4),
            right: rect.right - s(4),
            ..rect
        };
        round(dc, &inner, s(5), Some(c.nav), c.nav);
    }
    let icon = RECT {
        left: rect.left + s(14),
        right: rect.left + s(34),
        ..rect
    };
    draw_icon(dc, *code, 14, c.text, icon);
    let font = TempFont::new(dc, &[UI_FACE], 14, 400);
    let area = RECT {
        left: rect.left + s(42),
        ..rect
    };
    draw_text(dc, font.0, c.text, &units(text), area, DT_NOPREFIX);
}

// SAFETY: 系统在后台 UI 线程调用；不解引用业务指针、不保存状态、不分配；自绘结构只在本次回调内读取。
unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: 自绘结构指针由系统给出且已判空，只在本次回调内使用。
    unsafe {
        match message {
            NOTIFY => {
                match (l & 0xFFFF) as u32 {
                    NIN_SELECT | NIN_KEYSELECT => post(hwnd, OPEN, encode(Panel::Settings)),
                    // 版本 4 下 wParam 是弹出位置（屏幕坐标），键盘打开菜单时同样给出。
                    WM_CONTEXTMENU => menu(
                        hwnd,
                        (w & 0xFFFF) as u16 as i16 as i32,
                        (w >> 16 & 0xFFFF) as u16 as i16 as i32,
                    ),
                    _ => {}
                }
                0
            }
            WM_MEASUREITEM if l != 0 => {
                let item = &mut *(l as *mut MEASUREITEMSTRUCT);
                if item.CtlType != ODT_MENU {
                    return DefWindowProcW(hwnd, message, w, l);
                }
                let dpi = GetDpiForWindow(hwnd).max(96);
                item.itemWidth = 140 * dpi / 96;
                item.itemHeight = 32 * dpi / 96;
                1
            }
            WM_DRAWITEM if l != 0 => {
                let item = &*(l as *const DRAWITEMSTRUCT);
                if item.CtlType != ODT_MENU {
                    return DefWindowProcW(hwnd, message, w, l);
                }
                draw_menu_item(item);
                1
            }
            // 菜单模态期间由线程池代等唤起邮箱，后台停止或唤起时收起菜单（见 super::watch）。
            WM_ENTERMENULOOP => {
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
            // 登记消息编号从 0xC000 起，只在这一段比对，避免每条消息都查询。
            _ if message >= 0xC000
                && message == RegisterWindowMessageW(utf16::<16>("TaskbarCreated").as_ptr()) =>
            {
                add(hwnd);
                0
            }
            _ => DefWindowProcW(hwnd, message, w, l),
        }
    }
}

/// 首次唤起前只有通知区图标：等唤起信号或图标操作，期间照常派发图标消息、同步图标显隐；不建查字/设置窗。
/// 返回 None 表示后台停止（唤起邮箱断开或收到正常退出）。
pub(super) fn first(
    show: &Inbox<Panel>,
    tray: &Tray,
    taskbar: &Latest<bool>,
) -> Result<Option<Panel>, String> {
    let events = [show.event.as_raw_handle(), taskbar.event().as_raw_handle()];
    loop {
        // SAFETY: Inbox 与交接槽持有事件直到函数返回；消息、唤起与显隐共用一次阻塞等待，空闲不耗 CPU。
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(
                2,
                events.as_ptr(),
                INFINITE,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if wait == WAIT_FAILED {
            return Err(error());
        }
        tray.sync(taskbar)?;
        match show.receiver.try_recv() {
            Ok(panel) => return Ok(Some(panel)),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => return Ok(None),
        }
        let mut message = MSG::default();
        // SAFETY: MSG 栈对象完整有效，消息仅从当前线程提取；未接管的消息原样派发给窗口过程。
        while unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } != 0 {
            match message.message {
                WM_QUIT if message.wParam == 0 => return Ok(None),
                WM_QUIT => return Err("窗口消息投递失败".into()),
                OPEN => return Ok(Some(decode(message.wParam))),
                // SAFETY: 同上。
                _ => unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::tests::bounded;

    /// 只建隐藏窗口、不登记通知区图标，验证图标事件与唤起信号都能把首次唤起交给事件循环。
    #[test]
    fn icon_events_and_show_signals_open_the_right_panel_without_touching_the_tray() {
        let tray = Tray::window().unwrap();
        let (sender, show) = crate::mailbox::<Panel>().unwrap();
        let taskbar = Latest::new().unwrap();
        let first = |show| first(show, &tray, &taskbar);
        // 单击与键盘确认都打开设置页；其他鼠标事件不打开任何窗口。
        for event in [WM_MOUSEMOVE, NIN_SELECT, NIN_KEYSELECT] {
            // SAFETY: 本线程同步调用隐藏窗口的过程，参数为值类型。
            unsafe { procedure(tray.hwnd, NOTIFY, 0, event as LPARAM) };
        }
        assert_eq!(first(&show).unwrap(), Some(Panel::Settings));
        assert_eq!(first(&show).unwrap(), Some(Panel::Settings));
        // 等待首次唤起期间也取走后台给出的持有状态（本测试未登记图标，只改线程状态）；持有即隐藏。
        taskbar.set(true).unwrap();
        post(tray.hwnd, OPEN, encode(Panel::Lookup));
        assert_eq!(first(&show).unwrap(), Some(Panel::Lookup));
        assert!(HELD.get() && HIDDEN.get());
        taskbar.set(false).unwrap();
        sender.send(Panel::Lookup).unwrap();
        assert_eq!(first(&show).unwrap(), Some(Panel::Lookup));
        assert!(!HELD.get());
        drop(sender);
        assert_eq!(first(&show).unwrap(), None);
        for panel in [Panel::Settings, Panel::Lookup] {
            assert_eq!(decode(encode(panel)), panel);
        }
        an_open_tray_menu_closes_when_the_backend_stops(&tray);
    }

    /// 真实右键菜单（屏幕上一闪而过，不移动鼠标）展开期间后台停止：菜单须立即收起，
    /// 且停止信号留给事件循环，UI 线程得以退出而不是一直等用户操作。窗口类同名，由图标事件测试串行调用。
    fn an_open_tray_menu_closes_when_the_backend_stops(tray: &Tray) {
        let (sender, show) = crate::mailbox::<Panel>().unwrap();
        watch(Some(&show));
        bounded(sender, || menu(tray.hwnd, 0, 0));
        // 停止信号未被菜单吞掉：事件循环随即发现邮箱断开并退出。
        let taskbar = Latest::new().unwrap();
        assert_eq!(first(&show, tray, &taskbar).unwrap(), None);
        // 邮箱随本函数释放，不留悬空事件句柄。
        watch(None);
    }

    #[test]
    fn taskbar_flyouts_keep_the_icon_hidden_until_another_window_takes_the_foreground() {
        let wide = |text: &str| text.encode_utf16().collect::<Vec<_>>();
        assert!(shell(&wide("TopLevelWindowForOverflowXamlIsland")));
        assert!(shell(&wide("Shell_TrayWnd")));
        // 只认完整类名：前缀、普通程序窗口与桌面都不算任务栏。
        for class in ["Shell_Tray", "Notepad", "Progman", ""] {
            assert!(!shell(&wide(class)), "{class}");
        }
        // 有宿主持有即隐藏；失去时前台在任务栏面板则维持隐藏，换到其他窗口才显示；
        // 原本显示时任务栏面板取得前台不会让它隐藏。
        assert!(decide(true, false, false));
        assert!(decide(false, true, true));
        assert!(!decide(false, true, false));
        assert!(!decide(false, false, true));
    }

    #[test]
    fn icon_is_the_seal_at_the_small_icon_size() {
        let icon = icon().unwrap();
        // SAFETY: ICONINFO 为输出参数；取出的位图用后释放，图标随即销毁。
        unsafe {
            let mut info: ICONINFO = std::mem::zeroed();
            assert!(GetIconInfo(icon, &mut info) != 0);
            let mut bitmap: BITMAP = std::mem::zeroed();
            GetObjectW(
                info.hbmColor,
                size_of::<BITMAP>() as i32,
                (&mut bitmap as *mut BITMAP).cast(),
            );
            assert_eq!(bitmap.bmWidth, GetSystemMetrics(SM_CXSMICON).clamp(16, 256));
            DeleteObject(info.hbmColor);
            DeleteObject(info.hbmMask);
            DestroyIcon(icon);
        }
    }
}
