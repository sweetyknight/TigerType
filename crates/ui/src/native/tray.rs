//! 后台常驻的通知区图标：不依赖前台程序与当前输入法，任何时候单击都打开设置页，右键菜单打开设置页或查字窗。
//! 只在后台固定 UI 线程；回调不存业务状态、不分配，选择经线程消息交给事件循环。
//! 登录早期资源管理器尚未就绪或其后重启时，都靠 TaskbarCreated 广播重新挂上。
use super::*;
use windows_sys::Win32::UI::{
    Controls::ODT_MENU,
    Shell::{
        NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION,
        NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
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
            Ok(Self { hwnd, class })
        }
    }
    /// 建窗口并挂图标；资源管理器尚未就绪时登记失败，等 TaskbarCreated 再挂，不算故障。
    pub(super) fn new() -> Result<Self, String> {
        let tray = Self::window()?;
        add(tray.hwnd);
        Ok(tray)
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
        // SAFETY: 删除本窗口登记的图标（未登记时系统只返回失败），再释放图标、销毁窗口并注销类，均在创建线程。
        unsafe {
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
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: NOTIFY,
        hIcon: icon,
        szTip: utf16("TigerType 虎码：单击打开设置，右键更多"),
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

/// 现画小图标，与 TSF 任务栏输入法图标同样式：琥珀主题强调色圆角底上的白色“虎”，圆角外透明，无蓝色。
fn icon() -> Option<HICON> {
    const MAX: i32 = 256;
    let accent = rgb(Theme::Amber.preset()?.accent);
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
        // 灰度抗锯齿：ClearType 的彩色边缘可能偏蓝。
        let font = CreateFontW(
            -(size * 7 / 8),
            0,
            0,
            0,
            FW_BOLD as i32,
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
            let old_bitmap = SelectObject(dc, color);
            let area = RECT {
                left: 0,
                top: 0,
                right: size,
                bottom: size,
            };
            round(dc, &area, size / 4, Some(accent), accent);
            draw_text(
                dc,
                font,
                rgb(0xFFFFFF),
                &glyph[..1],
                area,
                DT_CENTER | DT_NOPREFIX,
            );
            GdiFlush();
            SelectObject(dc, old_bitmap);
            // GDI 不写 alpha：底色与字都非零，圆角外仍为 0，据此补出不透明区域。
            for pixel in std::slice::from_raw_parts_mut(bits.cast::<u32>(), (size * size) as usize)
            {
                if *pixel != 0 {
                    *pixel |= 0xFF00_0000;
                }
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

/// 首次唤起前只有通知区图标：等唤起信号或图标操作，期间照常派发图标消息；不建查字/设置窗。
/// 返回 None 表示后台停止（唤起邮箱断开或收到正常退出）。
pub(super) fn first(show: &Inbox<Panel>) -> Result<Option<Panel>, String> {
    let event = show.event.as_raw_handle();
    loop {
        // SAFETY: Inbox 持有事件直到函数返回；消息与唤起共用一次阻塞等待，空闲不耗 CPU。
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(1, &event, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        if wait == WAIT_FAILED {
            return Err(error());
        }
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
        // 单击与键盘确认都打开设置页；其他鼠标事件不打开任何窗口。
        for event in [WM_MOUSEMOVE, NIN_SELECT, NIN_KEYSELECT] {
            // SAFETY: 本线程同步调用隐藏窗口的过程，参数为值类型。
            unsafe { procedure(tray.hwnd, NOTIFY, 0, event as LPARAM) };
        }
        assert_eq!(first(&show).unwrap(), Some(Panel::Settings));
        assert_eq!(first(&show).unwrap(), Some(Panel::Settings));
        post(tray.hwnd, OPEN, encode(Panel::Lookup));
        assert_eq!(first(&show).unwrap(), Some(Panel::Lookup));
        sender.send(Panel::Lookup).unwrap();
        assert_eq!(first(&show).unwrap(), Some(Panel::Lookup));
        drop(sender);
        assert_eq!(first(&show).unwrap(), None);
        for panel in [Panel::Settings, Panel::Lookup] {
            assert_eq!(decode(encode(panel)), panel);
        }
    }

    #[test]
    fn icon_is_an_opaque_badge_with_transparent_corners_and_no_blue() {
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
            assert_eq!(pixels[0] >> 24, 0);
            assert_eq!(pixels[(size / 2 * size + size / 2) as usize] >> 24, 0xFF);
            for pixel in pixels.iter().filter(|p| *p >> 24 != 0) {
                let (r, g, b) = (pixel >> 16 & 0xFF, pixel >> 8 & 0xFF, pixel & 0xFF);
                assert!(b <= r && b <= g, "{pixel:08X}");
            }
        }
    }
}
