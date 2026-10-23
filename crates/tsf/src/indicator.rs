//! 任务栏输入法图标（输入模式语言栏按钮）：单击打开设置页，右键弹出“设置/查字”菜单。
//! 按钮只声明普通按钮样式：系统右键时只调用 OnClick(右键)、从不调用 InitMenu，
//! 所以菜单由本输入法在 OnClick 内自行弹出（小狼毫、Mozc 同样做法）。
//! 只在 TIP 线程使用；动作经激活状态给的回调转成对后台的唤起请求，按钮本身不持有连接、不读盘。
use crate::{
    candidate::{UI_FACE, colors, create_font, draw_text, fill, rgb, round},
    service::{CLSID_TEXT_SERVICE, guarded},
};
use std::{
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr, slice,
};
use tigertype_contracts::{Options, Palette, Theme, protocol::Panel};
use windows::{
    Win32::{
        Foundation::{
            E_FAIL, E_INVALIDARG, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT,
            SIZE, WPARAM,
        },
        Graphics::Gdi::*,
        System::Ole::{CONNECT_E_ADVISELIMIT, CONNECT_E_CANNOTCONNECT, CONNECT_E_NOCONNECTION},
        UI::{
            Controls::{
                DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_DISABLED, ODS_GRAYED, ODS_SELECTED, ODT_MENU,
            },
            HiDpi::GetDpiForWindow,
            TextServices::*,
            WindowsAndMessaging::*,
        },
    },
    core::{
        BOOL, BSTR, Error, GUID, IUnknown, Interface, PCWSTR, PWSTR, Ref, Result, implement, w,
    },
};

pub(crate) const SETTINGS: u32 = 1;
pub(crate) const LOOKUP: u32 = 2;
/// 菜单项：编号、文字、目标窗口。自绘项不处理助记键，用方向键与 Enter 选择。
const ITEMS: [(u32, &str, Panel); 2] = [
    (SETTINGS, "设置", Panel::Settings),
    (LOOKUP, "查字", Panel::Lookup),
];
/// 后台不可用时附在末尾的原因说明；编号 0 与“取消”同值，且置灰不可选。
const OFFLINE: &str = "后台未运行：从开始菜单打开 TigerType 查字与设置";
const MENU_CLASS: PCWSTR = w!("TigerType.Menu");
/// 菜单文字字号（磅）与行高（96 DPI 像素）。
const MENU_POINT: i32 = 11;
const ITEM_HEIGHT: i32 = 32;
/// 只接受一个语言栏 sink，cookie 固定。
const COOKIE: u32 = 1;

thread_local! {
    // 菜单弹出期间的绘制配色：只在弹出前写入，自绘回调只读，不是业务状态。
    static PALETTE: Cell<Palette> = Cell::new(Options::default().palette());
}

#[implement(ITfLangBarItemButton, ITfSource)]
pub(crate) struct Indicator {
    /// 弹出菜单前确认后台可连接（受单次回调预算约束）并给出当前配色；连不上时菜单项置灰并说明原因。
    probe: Box<dyn Fn() -> (bool, Palette)>,
    show: Box<dyn Fn(Panel)>,
    sink: RefCell<Option<ITfLangBarItemSink>>,
}
impl Indicator {
    pub(crate) fn new(
        probe: impl Fn() -> (bool, Palette) + 'static,
        show: impl Fn(Panel) + 'static,
    ) -> Self {
        Self {
            probe: Box::new(probe),
            show: Box::new(show),
            sink: RefCell::new(None),
        }
    }
}

impl ITfLangBarItem_Impl for Indicator_Impl {
    fn GetInfo(&self, info: *mut TF_LANGBARITEMINFO) -> Result<()> {
        if info.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // 末位保持 0 作为结尾。
        let mut description = [0u16; 32];
        for (slot, unit) in description[..31]
            .iter_mut()
            .zip("TigerType 虎码".encode_utf16())
        {
            *slot = unit;
        }
        // SAFETY: 输出指针已判空，由调用方提供完整结构。
        unsafe {
            info.write(TF_LANGBARITEMINFO {
                clsidService: CLSID_TEXT_SERVICE,
                // Win8 起任务栏只显示此 GUID 的按钮作为输入法图标。
                guidItem: GUID_LBI_INPUTMODE,
                dwStyle: TF_LBI_STYLE_BTN_BUTTON | TF_LBI_STYLE_SHOWNINTRAY,
                ulSort: 0,
                szDescription: description,
            });
        }
        Ok(())
    }
    fn GetStatus(&self) -> Result<u32> {
        Ok(0)
    }
    fn Show(&self, _: BOOL) -> Result<()> {
        // 显隐由系统按当前输入法决定，按钮不自行隐藏。
        Ok(())
    }
    fn GetTooltipString(&self) -> Result<BSTR> {
        Ok(BSTR::from("TigerType 虎码：单击打开设置，右键更多"))
    }
}

impl ITfLangBarItemButton_Impl for Indicator_Impl {
    fn OnClick(&self, click: TfLBIClick, point: &POINT, _: *const RECT) -> Result<()> {
        guarded(|| {
            let panel = match click {
                TF_LBI_CLK_LEFT => Some(Panel::Settings),
                TF_LBI_CLK_RIGHT => {
                    let (available, palette) = (self.probe)();
                    menu(*point, available, palette)?
                }
                _ => None,
            };
            if let Some(panel) = panel {
                (self.show)(panel);
            }
            Ok(())
        })
    }
    // 未声明 TF_LBI_STYLE_BTN_MENU，系统不调用这两项；菜单在 OnClick(右键) 内弹出。
    fn InitMenu(&self, _: Ref<ITfMenu>) -> Result<()> {
        Ok(())
    }
    fn OnMenuSelect(&self, _: u32) -> Result<()> {
        Ok(())
    }
    fn GetIcon(&self) -> Result<HICON> {
        guarded(icon)
    }
    fn GetText(&self) -> Result<BSTR> {
        Ok(BSTR::from("TigerType"))
    }
}

impl ITfSource_Impl for Indicator_Impl {
    fn AdviseSink(&self, iid: *const GUID, sink: Ref<IUnknown>) -> Result<u32> {
        // SAFETY: 调用方传入的 IID 指针已判空，只比较值。
        if iid.is_null() || unsafe { *iid } != ITfLangBarItemSink::IID {
            return Err(CONNECT_E_CANNOTCONNECT.into());
        }
        let mut slot = self.sink.try_borrow_mut().map_err(|_| E_FAIL)?;
        if slot.is_some() {
            return Err(CONNECT_E_ADVISELIMIT.into());
        }
        *slot = Some(sink.ok()?.cast()?);
        Ok(COOKIE)
    }
    fn UnadviseSink(&self, cookie: u32) -> Result<()> {
        let mut slot = self.sink.try_borrow_mut().map_err(|_| E_FAIL)?;
        if cookie != COOKIE || slot.take().is_none() {
            return Err(CONNECT_E_NOCONNECTION.into());
        }
        Ok(())
    }
}

/// 建菜单：后台不可用时两项置灰，并在末尾附原因说明（同样置灰）。项为自绘，文字仍随项保存供读屏器朗读。
/// 菜单归调用方 DestroyMenu。
pub(crate) fn build(available: bool) -> Result<HMENU> {
    // SAFETY: 新建空菜单，失败路径在返回前销毁。
    let menu = unsafe { CreatePopupMenu()? };
    let offline = (!available).then_some((0, OFFLINE));
    let items = ITEMS
        .iter()
        .map(|(id, text, _)| (*id, *text))
        .chain(offline);
    for (position, (id, text)) in items.enumerate() {
        let mut text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let item = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_ID | MIIM_STRING | MIIM_FTYPE | MIIM_STATE,
            fType: MFT_OWNERDRAW,
            fState: if available { MFS_ENABLED } else { MFS_DISABLED },
            wID: id,
            dwTypeData: PWSTR(text.as_mut_ptr()),
            ..Default::default()
        };
        // SAFETY: 文字缓冲在调用期间有效，菜单自行复制文字。
        if let Err(error) = unsafe { InsertMenuItemW(menu, position as u32, true, &item) } {
            // SAFETY: 销毁本函数刚建的菜单。
            let _ = unsafe { DestroyMenu(menu) };
            return Err(error);
        }
    }
    Ok(menu)
}

/// 在右键位置弹出菜单并等待选择，返回目标窗口；取消（或后台不可用时）为 None。
/// 所有者是本线程临时建的隐藏窗口：自绘消息须交给本模块，而不是宿主窗口；不激活、不改宿主焦点，用完即毁。
fn menu(at: POINT, available: bool, palette: Palette) -> Result<Option<Panel>> {
    let instance = crate::module()?;
    let class = WNDCLASSW {
        lpfnWndProc: Some(procedure),
        hInstance: instance.into(),
        lpszClassName: MENU_CLASS,
        ..Default::default()
    };
    // SAFETY: 类以 TIP 模块注册、回调无状态；模块驻留至进程退出，不注销，重复注册视为已存在。
    if unsafe { RegisterClassW(&class) } == 0 {
        let error = Error::from_thread();
        if error.code() != ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
            return Err(error);
        }
    }
    // 建在右键位置，逐显示器感知的宿主即按该显示器 DPI 量取菜单项。
    // SAFETY: 零尺寸、从不显示、不激活；未传入创建参数指针，本函数返回前销毁。
    let owner = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            MENU_CLASS,
            w!("TigerType 菜单"),
            WS_POPUP,
            at.x,
            at.y,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )?
    };
    let chosen = build(available).map(|menu| {
        PALETTE.set(palette);
        // SAFETY: 系统度量查询无副作用。
        let align = if unsafe { GetSystemMetrics(SM_MENUDROPALIGNMENT) } != 0 {
            TPM_RIGHTALIGN
        } else {
            TPM_LEFTALIGN
        };
        let flags = TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | align;
        // SAFETY: 菜单与所有者都属本线程且在模态循环期间有效；TPM_RETURNCMD 直接返回所选编号，不发 WM_COMMAND。
        let chosen = unsafe { TrackPopupMenuEx(menu, flags.0, at.x, at.y, owner, None) }.0 as u32;
        // SAFETY: 销毁本函数建的菜单。
        let _ = unsafe { DestroyMenu(menu) };
        chosen
    });
    // SAFETY: 销毁本函数建的所有者窗口。
    let _ = unsafe { DestroyWindow(owner) };
    let chosen = chosen?;
    Ok(ITEMS
        .iter()
        .find(|(id, ..)| *id == chosen)
        .map(|(.., panel)| *panel))
}

fn label(id: u32) -> &'static str {
    ITEMS
        .iter()
        .find(|(key, ..)| *key == id)
        .map_or(OFFLINE, |(_, text, _)| *text)
}

// SAFETY: 回调截住 panic；只处理本菜单的自绘消息，结构指针由系统给出且已判空，只在本次回调内使用；
// 其余消息交系统默认处理。
unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let handled = catch_unwind(AssertUnwindSafe(|| match message {
        WM_MEASUREITEM if l.0 != 0 => {
            // SAFETY: 同上。
            let item = unsafe { &mut *(l.0 as *mut MEASUREITEMSTRUCT) };
            (item.CtlType == ODT_MENU).then(|| measure(hwnd, item))
        }
        WM_DRAWITEM if l.0 != 0 => {
            // SAFETY: 同上。
            let item = unsafe { &*(l.0 as *const DRAWITEMSTRUCT) };
            (item.CtlType == ODT_MENU).then(|| draw(hwnd, item))
        }
        _ => None,
    }))
    .ok()
    .flatten();
    match handled {
        Some(()) => LRESULT(1),
        // SAFETY: 未处理的消息按系统流程处理。
        None => unsafe { DefWindowProcW(hwnd, message, w, l) },
    }
}

fn dpi(hwnd: HWND) -> i32 {
    // SAFETY: 只读窗口 DPI；空句柄返回 0，按 96 处理。
    unsafe { GetDpiForWindow(hwnd) }.max(96) as i32
}

/// 行高按设计稿，宽度取文字实宽加左右留白。
fn measure(hwnd: HWND, item: &mut MEASUREITEMSTRUCT) {
    let dpi = dpi(hwnd);
    let s = |value: i32| value * dpi / 96;
    let mut size = SIZE::default();
    let text: Vec<u16> = label(item.itemID).encode_utf16().collect();
    // SAFETY: 取本窗口 DC 测量文字，选入的字体在归还 DC 前还原并删除。
    unsafe {
        let dc = GetDC(Some(hwnd));
        if let Ok(font) = create_font(dc, &[UI_FACE], MENU_POINT, dpi as u32, FW_NORMAL.0 as i32) {
            let old = SelectObject(dc, font.into());
            let _ = GetTextExtentPoint32W(dc, &text, &mut size);
            SelectObject(dc, old);
            let _ = DeleteObject(font.into());
        }
        ReleaseDC(Some(hwnd), dc);
    }
    item.itemWidth = s(128).max(size.cx + s(40)) as u32;
    item.itemHeight = s(ITEM_HEIGHT) as u32;
}

/// 选中项为主题高亮底加左侧强调条（与候选窗、通知区菜单一致）；置灰项用次要文字色、不高亮；
/// 高对比度时沿用系统颜色。绘制后把 DC 还原为原状态。
pub(crate) fn draw(hwnd: HWND, item: &DRAWITEMSTRUCT) {
    let c = colors(PALETTE.get());
    let (dc, rect, dpi) = (item.hDC, item.rcItem, dpi(hwnd));
    let s = |value: i32| value * dpi / 96;
    let grayed = (item.itemState.0 & (ODS_GRAYED.0 | ODS_DISABLED.0)) != 0;
    let selected = (item.itemState.0 & ODS_SELECTED.0) != 0 && !grayed;
    fill(dc, &rect, c.window);
    if selected {
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
        fill(dc, &bar, c.bar);
    }
    let color = match (grayed, selected) {
        (true, _) => c.muted,
        (_, true) => c.chosen,
        _ => c.text,
    };
    let Ok(font) = create_font(dc, &[UI_FACE], MENU_POINT, dpi as u32, FW_NORMAL.0 as i32) else {
        return;
    };
    let area = RECT {
        left: rect.left + s(20),
        ..rect
    };
    // SAFETY: 记下原字体与背景模式，绘制后还原；新建字体随即删除。
    unsafe {
        let old = SelectObject(dc, font.into());
        let mode = SetBkMode(dc, TRANSPARENT);
        draw_text(
            dc,
            font,
            color,
            label(item.itemID),
            area,
            DRAW_TEXT_FORMAT(0),
        );
        SetBkMode(dc, BACKGROUND_MODE(mode as u32));
        SelectObject(dc, old);
        let _ = DeleteObject(font.into());
    }
}

/// 现画小图标：琥珀主题强调色圆角底上的白色“虎”，深浅任务栏都看得清且无蓝色。
/// 每次新建，HICON 归调用方并由其 DestroyIcon。
fn icon() -> Result<HICON> {
    // SAFETY: 系统度量查询无副作用。
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let accent = Theme::Amber.preset().ok_or(E_FAIL)?.accent;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            // 负高度为自上而下的行序。
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // 单色掩码全零：透明度完全由彩色位图的 alpha 决定。行按 16 位对齐。
    let mask_bits = vec![0u8; (size as usize).div_ceil(16) * 2 * size as usize];
    let face: Vec<u16> = "Microsoft YaHei UI".encode_utf16().chain(Some(0)).collect();
    // SAFETY: 内存 DC、位图与字体都在本函数内创建，选入的对象在删除前还原；
    // 像素缓冲属于 DIB，只在 DIB 删除前读写；CreateIconIndirect 复制位图，之后即可释放。
    unsafe {
        let dc = CreateCompatibleDC(None);
        let mut bits = ptr::null_mut();
        let color = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .unwrap_or_default();
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr().cast()));
        let font = CreateFontW(
            -(size * 7 / 8),
            0,
            0,
            0,
            FW_BOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            0,
            PCWSTR(face.as_ptr()),
        );
        let mut result = Err(E_FAIL.into());
        if !dc.is_invalid()
            && !color.is_invalid()
            && !mask.is_invalid()
            && !font.is_invalid()
            && !bits.is_null()
        {
            let old_bitmap = SelectObject(dc, color.into());
            let old_font = SelectObject(dc, font.into());
            let old_brush = SelectObject(dc, GetStockObject(DC_BRUSH));
            let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
            SetDCBrushColor(dc, rgb(accent));
            // 无画笔时填充少画右/下一像素，补齐。
            let radius = size / 4;
            let _ = RoundRect(dc, 0, 0, size + 1, size + 1, radius * 2, radius * 2);
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, rgb(0xFFFFFF));
            let mut area = RECT {
                left: 0,
                top: 0,
                right: size,
                bottom: size,
            };
            let mut text: Vec<u16> = "虎".encode_utf16().collect();
            DrawTextW(
                dc,
                &mut text,
                &mut area,
                DT_SINGLELINE | DT_CENTER | DT_VCENTER | DT_NOPREFIX,
            );
            let _ = GdiFlush();
            SelectObject(dc, old_pen);
            SelectObject(dc, old_brush);
            SelectObject(dc, old_font);
            SelectObject(dc, old_bitmap);
            // GDI 不写 alpha：底色与字都非零，圆角外仍为 0，据此补出不透明区域。
            for pixel in slice::from_raw_parts_mut(bits.cast::<u32>(), (size * size) as usize) {
                if *pixel != 0 {
                    *pixel |= 0xFF00_0000;
                }
            }
            result = CreateIconIndirect(&ICONINFO {
                fIcon: true.into(),
                xHotspot: 0,
                yHotspot: 0,
                hbmMask: mask,
                hbmColor: color,
            });
        }
        for object in [
            HGDIOBJ::from(color),
            HGDIOBJ::from(mask),
            HGDIOBJ::from(font),
        ] {
            if !object.is_invalid() {
                let _ = DeleteObject(object);
            }
        }
        if !dc.is_invalid() {
            let _ = DeleteDC(dc);
        }
        result
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{rc::Rc, sync::Mutex};
    use windows::Win32::{
        System::Threading::GetCurrentThreadId,
        UI::{
            Controls::{ODA_DRAWENTIRE, ODS_FLAGS},
            Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_DOWN, VK_ESCAPE, VK_RETURN},
        },
    };

    /// 同一时刻只让一个测试弹真实菜单。
    static MENU: Mutex<()> = Mutex::new(());

    // SAFETY: 系统在本线程菜单模态循环内调用；只收起本线程菜单。
    unsafe extern "system" fn give_up(_: HWND, _: u32, _: usize, _: u32) {
        // 预置按键没被菜单吃掉时收起菜单，让测试失败而不是挂住。
        // SAFETY: 同上。
        let _ = unsafe { EndMenu() };
    }

    /// 走系统右键的真实入口：先向本线程投递按键，再调用 OnClick(右键)；系统菜单模态循环真实运行，
    /// 像用户按方向键/Enter/Esc 一样吃掉这些按键。返回后核对按键已全部被菜单取走，证明菜单确实弹出过。
    /// 菜单会在屏幕左上角一闪而过，不移动鼠标。
    pub(crate) fn right_click(button: &ITfLangBarItemButton, keys: &[VIRTUAL_KEY]) {
        let _only = MENU.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // SAFETY: 只向本测试线程投递按键与定时器；按钮在本线程同步回调，定时器用后注销。
        unsafe {
            let thread = GetCurrentThreadId();
            for key in keys {
                PostThreadMessageW(thread, WM_KEYDOWN, WPARAM(key.0 as usize), LPARAM(0)).unwrap();
            }
            let timer = SetTimer(None, 0, 3000, Some(give_up));
            let clicked = button.OnClick(TF_LBI_CLK_RIGHT, POINT::default(), &RECT::default());
            let _ = KillTimer(None, timer);
            clicked.unwrap();
            let mut left = MSG::default();
            assert!(
                !PeekMessageW(&mut left, None, WM_KEYDOWN, WM_KEYDOWN, PM_REMOVE).as_bool(),
                "菜单没有取走预置按键：{keys:?}"
            );
        }
    }

    #[test]
    fn right_click_pops_a_real_menu_whose_choice_opens_that_panel_and_offline_items_cannot_be_chosen()
     {
        let shown = Rc::new(RefCell::new(Vec::new()));
        let online = Rc::new(Cell::new(true));
        let (log, probe) = (shown.clone(), online.clone());
        let button: ITfLangBarItemButton = Indicator::new(
            move || (probe.get(), Options::default().palette()),
            move |panel| log.borrow_mut().push(panel),
        )
        .into();
        right_click(&button, &[VK_DOWN, VK_RETURN]);
        right_click(&button, &[VK_DOWN, VK_DOWN, VK_RETURN]);
        right_click(&button, &[VK_ESCAPE]);
        assert_eq!(*shown.borrow(), [Panel::Settings, Panel::Lookup]);
        // 后台不可用：项全部置灰，Enter 收起菜单但不选中任何项，什么都不唤起。
        online.set(false);
        right_click(&button, &[VK_DOWN, VK_RETURN]);
        assert_eq!(shown.borrow().len(), 2);
        // 左键不弹菜单，直接打开设置页。
        // SAFETY: 本线程同步回调本按钮。
        unsafe { button.OnClick(TF_LBI_CLK_LEFT, POINT::default(), &RECT::default()) }.unwrap();
        assert_eq!(
            *shown.borrow(),
            [Panel::Settings, Panel::Lookup, Panel::Settings]
        );
        // 系统不对普通按钮调用菜单接口；即使被调用也不唤起任何窗口。
        // SAFETY: 同上。
        unsafe { button.OnMenuSelect(SETTINGS) }.unwrap();
        assert_eq!(shown.borrow().len(), 3);
    }

    #[test]
    fn menu_lists_settings_and_lookup_as_owner_drawn_items_and_explains_when_offline() {
        for available in [true, false] {
            let menu = build(available).unwrap();
            let mut items = Vec::new();
            // SAFETY: 读回本测试刚建的菜单项，读完销毁菜单。
            unsafe {
                for position in 0..GetMenuItemCount(Some(menu)) as u32 {
                    let mut text = [0u16; 64];
                    let mut item = MENUITEMINFOW {
                        cbSize: size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_ID | MIIM_STRING | MIIM_FTYPE | MIIM_STATE,
                        dwTypeData: PWSTR(text.as_mut_ptr()),
                        cch: text.len() as u32,
                        ..Default::default()
                    };
                    GetMenuItemInfoW(menu, position, true, &mut item).unwrap();
                    assert!(item.fType.contains(MFT_OWNERDRAW));
                    let text = String::from_utf16_lossy(&text[..item.cch as usize]);
                    items.push((item.wID, item.fState.contains(MFS_DISABLED), text));
                }
                DestroyMenu(menu).unwrap();
            }
            let mut expected = vec![
                (SETTINGS, !available, "设置".to_string()),
                (LOOKUP, !available, "查字".to_string()),
            ];
            if !available {
                expected.push((0, true, OFFLINE.to_string()));
            }
            assert_eq!(items, expected);
        }
    }

    #[test]
    fn menu_items_draw_the_theme_highlight_and_accent_bar_and_grayed_items_never_highlight() {
        let (width, height) = (128, 32);
        for theme in Theme::ALL {
            let palette = Options {
                theme,
                ..Default::default()
            }
            .palette();
            PALETTE.set(palette);
            let c = colors(palette);
            for (state, fill, bar) in [
                (ODS_SELECTED, c.highlight, c.bar),
                (
                    ODS_FLAGS(ODS_SELECTED.0 | ODS_GRAYED.0 | ODS_DISABLED.0),
                    c.window,
                    c.window,
                ),
                (ODS_FLAGS(0), c.window, c.window),
            ] {
                // SAFETY: 内存 DC 与位图只在本次循环内使用，读完像素即还原并删除。
                let (at_fill, at_bar) = unsafe {
                    let screen = GetDC(None);
                    let dc = CreateCompatibleDC(Some(screen));
                    let bitmap = CreateCompatibleBitmap(screen, width, height);
                    ReleaseDC(None, screen);
                    let old = SelectObject(dc, bitmap.into());
                    draw(
                        HWND::default(),
                        &DRAWITEMSTRUCT {
                            CtlType: ODT_MENU,
                            itemID: SETTINGS,
                            itemAction: ODA_DRAWENTIRE,
                            itemState: state,
                            hDC: dc,
                            rcItem: RECT {
                                right: width,
                                bottom: height,
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    );
                    // 右侧远离文字处为底色，左侧 5px 处为强调条。
                    let pixels = (GetPixel(dc, 110, 16), GetPixel(dc, 5, 16));
                    SelectObject(dc, old);
                    let _ = DeleteObject(bitmap.into());
                    let _ = DeleteDC(dc);
                    pixels
                };
                assert_eq!((at_fill, at_bar), (fill, bar), "{theme:?} {state:?}");
                for color in [at_fill, at_bar] {
                    let [red, green, blue, _] = color.0.to_le_bytes();
                    assert!(blue <= red && blue <= green, "{theme:?} {color:?}");
                }
            }
        }
    }

    #[test]
    fn icon_is_an_opaque_badge_with_transparent_corners_and_no_blue() {
        let icon = icon().unwrap();
        let mut info = ICONINFO::default();
        // SAFETY: 读回本测试刚建的图标位图，读完删除位图与图标。
        unsafe {
            GetIconInfo(icon, &mut info).unwrap();
            let mut bitmap = BITMAP::default();
            GetObjectW(
                info.hbmColor.into(),
                size_of::<BITMAP>() as i32,
                Some((&mut bitmap as *mut BITMAP).cast()),
            );
            let size = bitmap.bmWidth;
            let mut header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size,
                    biHeight: -size,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u32; (size * size) as usize];
            let dc = CreateCompatibleDC(None);
            let rows = GetDIBits(
                dc,
                info.hbmColor,
                0,
                size as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut header,
                DIB_RGB_COLORS,
            );
            let _ = DeleteDC(dc);
            let _ = DeleteObject(info.hbmColor.into());
            let _ = DeleteObject(info.hbmMask.into());
            DestroyIcon(icon).unwrap();
            assert_eq!(rows, size);
            // 角上透明，中心不透明；所有不透明像素的蓝分量都不高于红、绿（无蓝）。
            assert_eq!(pixels[0] >> 24, 0);
            let center = pixels[(size / 2 * size + size / 2) as usize];
            assert_eq!(center >> 24, 0xFF);
            for pixel in pixels.iter().filter(|p| *p >> 24 != 0) {
                let (r, g, b) = (pixel >> 16 & 0xFF, pixel >> 8 & 0xFF, pixel & 0xFF);
                assert!(b <= r && b <= g, "{pixel:08X}");
            }
        }
    }
}
