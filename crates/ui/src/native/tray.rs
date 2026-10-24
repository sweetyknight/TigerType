//! 后台常驻的通知区图标：不依赖前台程序与当前输入法，任何时候单击都打开设置页，右键菜单打开设置页或查字窗。
//! 前台程序正用 TigerType 时任务栏已有本输入法图标，此时隐藏通知区图标，同一时刻只显示一个。
//! 打开溢出区等操作把前台交给任务栏，宿主随之报告失去焦点，但任务栏仍显示先前程序的输入法图标，
//! 所以前台是任务栏或其弹出面板时保持隐藏，等前台换到其他窗口（进程外 WinEvent 回调，不注入）再定。
//! 只在后台固定 UI 线程；回调不存业务状态、不分配，选择经线程消息交给事件循环。
//! 登录早期资源管理器尚未就绪或其后重启时，都靠 TaskbarCreated 广播重新挂上（保持当前显隐）。
use super::*;
use crate::Latest;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
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
/// 菜单项：编号、文字、目标窗口。自绘菜单项不自动处理助记键，所以不写助记符，用方向键与 Enter 选择。
const ITEMS: [(u32, &str, Panel); 2] = [(1, "设置", Panel::Settings), (2, "查字", Panel::Lookup)];

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
        szTip: utf16("TigerType 虎码：单击打开设置，右键更多"),
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

/// 深浅色切换后重画图标并原位替换（不删再加，图标不挪位置）；成功后才释放旧图标。
fn recolor(hwnd: HWND) {
    let Some(icon) = icon() else {
        return;
    };
    let data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        uFlags: NIF_ICON,
        hIcon: icon,
        ..Default::default()
    };
    // SAFETY: 结构大小已填，图标在调用期间有效；未登记时系统只返回失败，新图标随即释放。
    unsafe {
        let old = if Shell_NotifyIconW(NIM_MODIFY, &data) != 0 {
            ICON.with(|slot| slot.replace(icon))
        } else {
            icon
        };
        if !old.is_null() {
            DestroyIcon(old);
        }
    }
}

/// 任务栏是否为浅色：系统“Windows 模式”设置，读不到按深色（Win10 默认）。
fn light_taskbar() -> bool {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    let key = utf16::<64>(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = utf16::<24>("SystemUsesLightTheme");
    // SAFETY: 名称 NUL 结尾，输出缓冲为本地 DWORD 且大小已填；只读当前用户的一个值。
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    read == 0 && value != 0
}

/// 现画小图标：与任务栏输入法图标同风格，只画一个“虎”字、底全透明，字色随任务栏深浅，无蓝色。
/// TSF 输入法图标显示中英状态，品牌“虎”只在此处，两处不重复。
fn icon() -> Option<HICON> {
    const MAX: i32 = 256;
    let ink = if light_taskbar() { 0x1A1A1A } else { 0xFFFFFF };
    // SAFETY: 系统度量查询无副作用。
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, MAX);
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            // 负高度为自上而下的行序。
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    // 单色掩码全零：透明度完全由彩色位图的 alpha 决定；行按 16 位对齐，按最大尺寸留栈缓冲。
    let mask_bits = [0u8; (MAX as usize / 16 * 2) * MAX as usize];
    let face = utf16::<32>(UI_FACE);
    let glyph = utf16::<2>("虎");
    // SAFETY: 内存 DC、位图与字体都在本函数内创建，选入的对象在删除前还原；
    // 像素缓冲属于 DIB，只在 DIB 删除前读写；CreateIconIndirect 复制位图，之后即可释放。
    unsafe {
        let dc = CreateCompatibleDC(null_mut());
        let mut bits = null_mut();
        let color = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let mask = CreateBitmap(size, size, 1, 1, mask_bits.as_ptr().cast());
        // 灰度抗锯齿：ClearType 的彩色边缘可能偏蓝。字身取图标的 3/4，与任务栏输入法图标一致。
        let font = CreateFontW(
            -(size * 3 / 4),
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            ANTIALIASED_QUALITY as u32,
            0,
            face.as_ptr(),
        );
        let mut result = None;
        if !dc.is_null()
            && !color.is_null()
            && !mask.is_null()
            && !font.is_null()
            && !bits.is_null()
        {
            // 新 DIB 全零（黑）；白字画上去后各通道即覆盖率。
            let old_bitmap = SelectObject(dc, color);
            let old_font = SelectObject(dc, font);
            SetBkMode(dc, TRANSPARENT as _);
            SetTextColor(dc, rgb(0xFFFFFF));
            let (x, y) = centered(dc, glyph[0], size);
            SetTextAlign(dc, TA_BASELINE | TA_LEFT);
            TextOutW(dc, x, y, glyph.as_ptr(), 1);
            GdiFlush();
            SelectObject(dc, old_font);
            SelectObject(dc, old_bitmap);
            // 覆盖率当 alpha、像素换成字色（非预乘 ARGB），覆盖率为 0 处全透明。
            for pixel in std::slice::from_raw_parts_mut(bits.cast::<u32>(), (size * size) as usize)
            {
                let alpha = (*pixel >> 16 & 0xFF)
                    .max(*pixel >> 8 & 0xFF)
                    .max(*pixel & 0xFF);
                *pixel = if alpha == 0 { 0 } else { alpha << 24 | ink };
            }
            let icon = CreateIconIndirect(&ICONINFO {
                fIcon: 1,
                xHotspot: 0,
                yHotspot: 0,
                hbmMask: mask,
                hbmColor: color,
            });
            result = (!icon.is_null()).then_some(icon);
        }
        for object in [color, mask, font] {
            if !object.is_null() {
                DeleteObject(object);
            }
        }
        if !dc.is_null() {
            DeleteDC(dc);
        }
        result
    }
}

/// 按字形墨迹框居中，返回基线起点：GDI 的垂直居中按字符单元算，雅黑的单元比汉字高，
/// 字会偏下甚至被裁掉底边。取不到字形度量时退回按字身估算。
/// SAFETY: 调用方保证 dc 有效且已选入目标字体。
unsafe fn centered(dc: HDC, glyph: u16, size: i32) -> (i32, i32) {
    let one = FIXED { fract: 0, value: 1 };
    let identity = MAT2 {
        eM11: one,
        eM12: FIXED { fract: 0, value: 0 },
        eM21: FIXED { fract: 0, value: 0 },
        eM22: one,
    };
    // SAFETY: GLYPHMETRICS 为纯数据，全零合法。
    let mut metrics: GLYPHMETRICS = unsafe { std::mem::zeroed() };
    // SAFETY: 只取度量（GGO_METRICS 不写缓冲），输出结构为本地值。
    let read = unsafe {
        GetGlyphOutlineW(
            dc,
            glyph.into(),
            GGO_METRICS,
            &mut metrics,
            0,
            null_mut(),
            &identity,
        )
    };
    if read == GDI_ERROR as u32 {
        return (size / 8, size * 7 / 8);
    }
    let (width, height) = (metrics.gmBlackBoxX as i32, metrics.gmBlackBoxY as i32);
    let origin = metrics.gmptGlyphOrigin;
    (
        (size - width) / 2 - origin.x,
        (size - height) / 2 + origin.y,
    )
}

/// 右键菜单：自绘菜单项（保留文字供读屏器朗读），底色与高亮按主题，不用系统蓝色高亮。
fn menu(hwnd: HWND, x: i32, y: i32) {
    // SAFETY: 菜单与画刷在本函数内创建并释放；文字缓冲在插入调用期间有效（菜单复制文字）。
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        for (id, text, _) in ITEMS {
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
        if let Some((_, _, panel)) = ITEMS.iter().find(|(id, ..)| *id == chosen) {
            post(hwnd, OPEN, encode(*panel));
        }
    }
}

/// 自绘菜单项：选中项为主题高亮底与左侧强调条，与查字列表一致；高对比度时沿用系统颜色。
fn draw_menu_item(item: &DRAWITEMSTRUCT) {
    let Some((_, text, _)) = ITEMS.iter().find(|(id, ..)| *id == item.itemID) else {
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
        round(dc, &inner, s(4), Some(c.highlight), c.highlight);
        let top = (rect.top + rect.bottom - s(16)) / 2;
        let bar = RECT {
            left: inner.left,
            top,
            right: inner.left + s(3),
            bottom: top + s(16),
        };
        fill(dc, &bar, c.accent);
    }
    let Ok(font) = create_font(dc, &[UI_FACE], 14, dpi as u32, 400) else {
        return;
    };
    let text = utf16::<8>(text);
    let length = text
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(text.len());
    let area = RECT {
        left: rect.left + s(20),
        ..rect
    };
    draw_text(dc, font, c.text, &text[..length], area, DT_NOPREFIX);
    // SAFETY: 字体已在 draw_text 内还原，此处释放本次新建的字体。
    unsafe {
        DeleteObject(font);
    }
}

/// 逐单元比对 NUL 结尾的宽字符串，不分配；遇到首个不同（含对方提前结束的 NUL）即停，不越过其结尾。
/// SAFETY: 调用方保证 text 指向 NUL 结尾的宽字符串。
unsafe fn named(text: *const u16, name: &str) -> bool {
    let mut index = 0;
    for unit in name.encode_utf16() {
        // SAFETY: 之前各单元都相等且非 NUL，故尚未越过结尾。
        if unsafe { *text.add(index) } != unit {
            return false;
        }
        index += 1;
    }
    // SAFETY: 同上。
    unsafe { *text.add(index) == 0 }
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
                item.itemWidth = 128 * dpi / 96;
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
            // 切换深浅色时系统广播设置名 ImmersiveColorSet，按新任务栏底色重画图标。
            WM_SETTINGCHANGE if l != 0 && named(l as *const u16, "ImmersiveColorSet") => {
                recolor(hwnd);
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
    fn icon_is_a_bare_glyph_in_the_taskbar_ink_on_a_transparent_background() {
        let icon = icon().unwrap();
        let mut info = ICONINFO::default();
        // SAFETY: 读回本测试刚建的图标位图，读完删除位图与图标。
        unsafe {
            assert_ne!(GetIconInfo(icon, &mut info), 0);
            let mut bitmap = BITMAP::default();
            GetObjectW(
                info.hbmColor,
                size_of::<BITMAP>() as i32,
                (&mut bitmap as *mut BITMAP).cast(),
            );
            let size = bitmap.bmWidth;
            let mut header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size,
                    biHeight: -size,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u32; (size * size) as usize];
            let dc = CreateCompatibleDC(null_mut());
            let rows = GetDIBits(
                dc,
                info.hbmColor,
                0,
                size as u32,
                pixels.as_mut_ptr().cast(),
                &mut header,
                DIB_RGB_COLORS,
            );
            DeleteDC(dc);
            DeleteObject(info.hbmColor);
            DeleteObject(info.hbmMask);
            DestroyIcon(icon);
            assert_eq!(rows, size);
            // 没有底板：角上透明、可见像素只占少数且有完全不透明的笔画，可见像素一律是任务栏字色（无蓝）。
            let ink = if light_taskbar() { 0x1A1A1A } else { 0xFFFFFF };
            assert_eq!(pixels[0] >> 24, 0);
            let visible: Vec<u32> = pixels.iter().copied().filter(|p| p >> 24 != 0).collect();
            assert!(
                visible.len() * 2 < pixels.len(),
                "{}/{}",
                visible.len(),
                pixels.len()
            );
            assert!(visible.iter().any(|p| p >> 24 == 0xFF));
            // 字形按墨迹居中：四边都不贴边，上下、左右留白相差不超过 1px。
            let inked = |at: &dyn Fn(i32) -> bool| (0..size).filter(|&k| at(k)).collect::<Vec<_>>();
            let rows = inked(&|y| (0..size).any(|x| pixels[(y * size + x) as usize] >> 24 != 0));
            let cols = inked(&|x| (0..size).any(|y| pixels[(y * size + x) as usize] >> 24 != 0));
            for span in [rows, cols] {
                let (first, last) = (span[0], span[span.len() - 1]);
                assert!(first >= 1 && last <= size - 2, "{first}..{last} / {size}");
                assert!(
                    (first - (size - 1 - last)).abs() <= 1,
                    "{first}..{last} / {size}"
                );
            }
            for pixel in visible {
                assert_eq!(pixel & 0xFF_FFFF, ink, "{pixel:08X}");
            }
        }
    }

    #[test]
    fn only_the_color_mode_setting_name_triggers_a_recolor() {
        let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        // SAFETY: 测试字符串均 NUL 结尾。
        let named = |text: &str| unsafe { named(wide(text).as_ptr(), "ImmersiveColorSet") };
        assert!(named("ImmersiveColorSet"));
        // 前缀、加长与其他设置名都不算。
        for other in ["Immersive", "ImmersiveColorSetX", "Policy", ""] {
            assert!(!named(other), "{other}");
        }
    }
}
