//! 任务栏输入法图标（输入模式语言栏按钮）：显示中英状态“中/英”，单击切换中英（同小狼毫、微软拼音），
//! 右键弹出菜单：中文/英文、半角/全角、中文/英文标点三项显示当前状态，点击即切换；其下为查字与设置。
//! 后台未运行时菜单项置灰。品牌印章由输入法配置图标与通知区图标显示，本按钮只画“中/英”。
//! 按钮只声明普通按钮样式：系统右键时只调用 OnClick(右键)、从不调用 InitMenu，
//! 所以菜单由本输入法在 OnClick 内自行弹出（小狼毫、Mozc 同样做法）。
//! 只在 TIP 线程使用；动作经激活状态给的回调转成对后台的切换/唤起请求，按钮本身不持有连接、不读盘。
use crate::{
    candidate::{UI_FACE, colors, create_font, draw_text, fill, rgb, round},
    service::{CLSID_TEXT_SERVICE, guarded},
};
use std::{
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr, slice,
};
use tigertype_contracts::{Key, Options, Palette, protocol::Panel};
use windows::{
    Win32::{
        Foundation::{
            E_FAIL, E_INVALIDARG, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT,
            SIZE, WPARAM,
        },
        Graphics::Gdi::*,
        System::{
            Ole::{CONNECT_E_ADVISELIMIT, CONNECT_E_CANNOTCONNECT, CONNECT_E_NOCONNECTION},
            Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
        },
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
pub(crate) const SHAPE: u32 = 3;
pub(crate) const MODE: u32 = 4;
pub(crate) const PUNCTUATION: u32 = 5;
/// 状态项与动作项之间的分隔线（自绘、置灰不可选）。
const SEPARATOR: u32 = 6;
/// 菜单项编号；自绘项不处理助记键，用方向键与 Enter 选择。
const ITEMS: [u32; 6] = [MODE, SHAPE, PUNCTUATION, SEPARATOR, LOOKUP, SETTINGS];
const MENU_CLASS: PCWSTR = w!("TigerType.Menu");
/// 菜单文字字号（磅）与行高（96 DPI 像素）。
const MENU_POINT: i32 = 11;
const ITEM_HEIGHT: i32 = 34;
const ICON_FACES: [&str; 2] = ["Segoe Fluent Icons", "Segoe MDL2 Assets"];
/// 只接受一个语言栏 sink，cookie 固定。
const COOKIE: u32 = 1;

/// 菜单里显示的会话状态，取自上次回复；新连接的会话为默认（中文、半角、中文标点）。
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    pub ascii: bool,
    pub punctuation: bool,
    pub full: bool,
}

thread_local! {
    // 菜单弹出期间的绘制配色与状态：只在弹出前写入，自绘回调只读，不是业务状态。
    static PALETTE: Cell<Palette> = Cell::new(Options::default().palette());
    static STATUS: Cell<Status> = Cell::new(Status::default());
}

#[implement(ITfLangBarItemButton, ITfSource)]
pub(crate) struct Indicator {
    /// 弹出菜单前确认后台可连接（受单次回调预算约束），并给出当前配色与状态；连不上时菜单项置灰。
    probe: Box<dyn Fn() -> (bool, Palette, Status)>,
    show: Box<dyn Fn(Panel)>,
    /// 请后台翻转开关：单击与菜单都可切中英（图标随回复经 [`Indicator::update`] 刷新），菜单另可切全角与标点。
    toggle: Box<dyn Fn(Key)>,
    sink: RefCell<Option<ITfLangBarItemSink>>,
    /// 上次回复的英文模式；新连接的会话默认中文。
    ascii: Cell<bool>,
    /// 建按钮时的任务栏深浅，决定图标字色。
    light: bool,
}
impl Indicator {
    pub(crate) fn new(
        probe: impl Fn() -> (bool, Palette, Status) + 'static,
        show: impl Fn(Panel) + 'static,
        toggle: impl Fn(Key) + 'static,
    ) -> Self {
        Self {
            probe: Box::new(probe),
            show: Box::new(show),
            toggle: Box::new(toggle),
            sink: RefCell::new(None),
            ascii: Cell::new(false),
            light: light_taskbar(),
        }
    }
    /// 按后台回复同步中英状态；变化时通知系统重取图标与提示。
    pub(crate) fn update(&self, ascii: bool) {
        if self.ascii.replace(ascii) == ascii {
            return;
        }
        // 先取出 sink 再通知：系统可能在通知内回调本按钮。
        let sink = self.sink.try_borrow().ok().and_then(|sink| sink.clone());
        if let Some(sink) = sink {
            // SAFETY: 本线程同步通知已登记的语言栏 sink；失败只是图标暂未刷新。
            let _ = unsafe { sink.OnUpdate(TF_LBI_ICON | TF_LBI_TOOLTIP) };
        }
    }
    fn mode(&self) -> &'static str {
        if self.ascii.get() { "英" } else { "中" }
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
        Ok(BSTR::from(
            format!(
                "TigerType 虎码（{}文）：单击切换中英，右键更多",
                self.mode()
            )
            .as_str(),
        ))
    }
}

impl ITfLangBarItemButton_Impl for Indicator_Impl {
    fn OnClick(&self, click: TfLBIClick, point: &POINT, _: *const RECT) -> Result<()> {
        guarded(|| {
            match click {
                TF_LBI_CLK_LEFT => (self.toggle)(Key::ToggleAscii),
                TF_LBI_CLK_RIGHT => {
                    let (available, palette, status) = (self.probe)();
                    match menu(*point, available, palette, status)? {
                        Some(SETTINGS) => (self.show)(Panel::Settings),
                        Some(LOOKUP) => (self.show)(Panel::Lookup),
                        Some(MODE) => (self.toggle)(Key::ToggleAscii),
                        Some(SHAPE) => (self.toggle)(Key::ToggleFullShape),
                        Some(PUNCTUATION) => (self.toggle)(Key::TogglePunctuation),
                        _ => {}
                    }
                }
                _ => {}
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
        guarded(|| icon(self.mode(), self.light))
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

/// 建菜单：三个状态项写当前状态（如“半角”，全角时写“全角”），分隔线下为查字、设置；
/// 后台不可用时各项置灰。项为自绘，文字仍随项保存供读屏器朗读。
/// 菜单归调用方 DestroyMenu。
pub(crate) fn build(available: bool, status: Status) -> Result<HMENU> {
    STATUS.set(status);
    // SAFETY: 新建空菜单，失败路径在返回前销毁。
    let menu = unsafe { CreatePopupMenu()? };
    for (position, id) in ITEMS.into_iter().enumerate() {
        let mut text: Vec<u16> = label(id).encode_utf16().chain(Some(0)).collect();
        let item = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_ID | MIIM_STRING | MIIM_FTYPE | MIIM_STATE,
            // 分隔线也自绘，但标成分隔项，方向键会跳过它。
            fType: if id == SEPARATOR {
                MFT_OWNERDRAW | MFT_SEPARATOR
            } else {
                MFT_OWNERDRAW
            },
            fState: if available && id != SEPARATOR {
                MFS_ENABLED
            } else {
                MFS_DISABLED
            },
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

/// 在右键位置弹出菜单并等待选择，返回所选编号；取消（或后台不可用时）为 None。
/// 所有者是本线程临时建的隐藏窗口：自绘消息须交给本模块，而不是宿主窗口；不激活、不改宿主焦点，用完即毁。
fn menu(at: POINT, available: bool, palette: Palette, status: Status) -> Result<Option<u32>> {
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
    let chosen = build(available, status).map(|menu| {
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
    Ok((ITEMS.contains(&chosen) && chosen != SEPARATOR).then_some(chosen))
}

/// 菜单项文字：状态项为当前状态，不写“切换到”，也不并列两种状态。
fn label(id: u32) -> &'static str {
    let status = STATUS.get();
    match id {
        SETTINGS => "设置",
        LOOKUP => "查字",
        MODE if status.ascii => "英文",
        MODE => "中文",
        SHAPE if status.full => "全角",
        SHAPE => "半角",
        PUNCTUATION if status.punctuation => "英文标点",
        PUNCTUATION => "中文标点",
        _ => "",
    }
}
/// 状态项右侧的快捷键提示。
fn shortcut(id: u32) -> &'static str {
    match id {
        MODE => "Shift",
        PUNCTUATION => "Ctrl+.",
        _ => "",
    }
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

/// 行高按设计稿；宽度留出徽标、文字与快捷键提示。分隔线只占一条细缝。
fn measure(hwnd: HWND, item: &mut MEASUREITEMSTRUCT) {
    let dpi = dpi(hwnd);
    let s = |value: i32| value * dpi / 96;
    if item.itemID == SEPARATOR {
        item.itemWidth = s(200) as u32;
        item.itemHeight = s(9) as u32;
        return;
    }
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
    item.itemWidth = s(200).max(size.cx + s(110)) as u32;
    item.itemHeight = s(ITEM_HEIGHT) as u32;
}

/// 悬停项为主题高亮圆角底（玫红用选区色），不用系统蓝色；置灰项用次要文字色、不高亮。
/// 状态项左侧画当前状态徽标（中/英、新月/满月、中英文标点），动作项画图标；高对比度时沿用系统颜色。
/// 绘制后把 DC 还原为原状态。
pub(crate) fn draw(hwnd: HWND, item: &DRAWITEMSTRUCT) {
    let c = colors(PALETTE.get());
    let (dc, rect, dpi) = (item.hDC, item.rcItem, dpi(hwnd));
    let s = |value: i32| value * dpi / 96;
    fill(dc, &rect, c.window);
    if item.itemID == SEPARATOR {
        let middle = (rect.top + rect.bottom) / 2;
        let line = RECT {
            left: rect.left + s(10),
            top: middle,
            right: rect.right - s(10),
            bottom: middle + s(1).max(1),
        };
        fill(dc, &line, c.line);
        return;
    }
    let grayed = (item.itemState.0 & (ODS_GRAYED.0 | ODS_DISABLED.0)) != 0;
    let selected = (item.itemState.0 & ODS_SELECTED.0) != 0 && !grayed;
    if selected {
        let inner = RECT {
            left: rect.left + s(4),
            right: rect.right - s(4),
            ..rect
        };
        round(dc, &inner, s(5), Some(c.hover), c.hover);
    }
    let ink = if grayed { c.muted } else { c.text };
    let status = STATUS.get();
    let badge = RECT {
        left: rect.left + s(12),
        top: (rect.top + rect.bottom - s(22)) / 2,
        right: rect.left + s(34),
        bottom: (rect.top + rect.bottom + s(22)) / 2,
    };
    let font = |faces: &[&str], point: i32, weight: i32| {
        create_font(dc, faces, point, dpi as u32, weight).ok()
    };
    // SAFETY: 记下原背景模式，绘制后还原；新建字体随即删除。
    unsafe {
        let mode = SetBkMode(dc, TRANSPARENT);
        match item.itemID {
            MODE | PUNCTUATION => {
                round(dc, &badge, s(5), Some(c.track), c.track);
                let (text, point) = match item.itemID {
                    MODE if status.ascii => ("英", 9),
                    MODE => ("中", 9),
                    _ if status.punctuation => (", .", 8),
                    _ => ("，。", 8),
                };
                if let Some(face) = font(&[UI_FACE], point, FW_SEMIBOLD.0 as i32) {
                    draw_text(dc, face, ink, text, badge, DT_CENTER);
                    let _ = DeleteObject(face.into());
                }
            }
            SHAPE => {
                round(dc, &badge, s(5), Some(c.track), c.track);
                // 满月为全角、新月为半角（同微软拼音），新月用两圆相减画出。
                let (x, y, r) = (
                    (badge.left + badge.right) / 2,
                    (badge.top + badge.bottom) / 2,
                    s(6),
                );
                let disc = |left: i32, top: i32, color| {
                    let shape = RECT {
                        left,
                        top,
                        right: left + 2 * r,
                        bottom: top + 2 * r,
                    };
                    round(dc, &shape, r, Some(color), color);
                };
                disc(x - r, y - r, ink);
                if !status.full {
                    disc(x - r + s(5), y - r - s(3), c.track);
                }
            }
            LOOKUP | SETTINGS => {
                let code = if item.itemID == LOOKUP {
                    "\u{E721}"
                } else {
                    "\u{E713}"
                };
                if let Some(face) = font(&ICON_FACES, 10, FW_NORMAL.0 as i32) {
                    draw_text(dc, face, ink, code, badge, DT_CENTER);
                    let _ = DeleteObject(face.into());
                }
            }
            _ => {}
        }
        if let Some(face) = font(&[UI_FACE], MENU_POINT, FW_NORMAL.0 as i32) {
            let indent = if item.itemID == 0 { s(12) } else { s(44) };
            let area = RECT {
                left: rect.left + indent,
                right: rect.right - s(12),
                ..rect
            };
            draw_text(dc, face, ink, label(item.itemID), area, DRAW_TEXT_FORMAT(0));
            let _ = DeleteObject(face.into());
        }
        let hint = shortcut(item.itemID);
        if !hint.is_empty()
            && let Some(face) = font(&["Cascadia Mono", "Consolas"], 9, FW_NORMAL.0 as i32)
        {
            let area = RECT {
                right: rect.right - s(12),
                ..rect
            };
            draw_text(dc, face, c.muted, hint, area, DT_RIGHT);
            let _ = DeleteObject(face.into());
        }
        SetBkMode(dc, BACKGROUND_MODE(mode as u32));
    }
}

/// 任务栏是否为浅色：系统“Windows 模式”设置，读不到按深色（Win10 默认）。
/// 只在激活建按钮时读一次：输入路径不读配置。
/// shortcut: 激活期间切换深浅色要等下次激活才换字色，需即时跟随时改由后台推送。
fn light_taskbar() -> bool {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: 输出缓冲为本地 DWORD，大小已填；只读当前用户的一个值。
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    read.is_ok() && value != 0
}

/// 现画小图标：像微软拼音、小狼毫那样只画一个“中”/“英”字，底全透明；
/// 字色随任务栏深浅（深色用白、浅色用近黑），无蓝色。每次新建，HICON 归调用方并由其 DestroyIcon。
fn icon(glyph: &str, light: bool) -> Result<HICON> {
    // SAFETY: 系统度量查询无副作用。
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let ink: u32 = if light { 0x1A1A1A } else { 0xFFFFFF };
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
        // 灰度抗锯齿：覆盖率直接当 alpha，ClearType 的彩色边缘可能偏蓝。
        // 字身取图标的 3/4，汉字墨迹约占 2/3，四周留白与微软拼音相近。
        let font = CreateFontW(
            -(size * 3 / 4),
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
            // 新 DIB 全零（黑）；白字画上去后各通道即覆盖率。
            let old_bitmap = SelectObject(dc, color.into());
            let old_font = SelectObject(dc, font.into());
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, rgb(0xFFFFFF));
            let text: Vec<u16> = glyph.encode_utf16().collect();
            let (x, y) = centered(dc, text[0], size);
            SetTextAlign(dc, TA_BASELINE | TA_LEFT);
            let _ = TextOutW(dc, x, y, &text);
            let _ = GdiFlush();
            SelectObject(dc, old_font);
            SelectObject(dc, old_bitmap);
            for pixel in slice::from_raw_parts_mut(bits.cast::<u32>(), (size * size) as usize) {
                *pixel = coverage(*pixel, ink);
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

/// 按字形墨迹框居中，返回基线起点：GDI 的垂直居中按字符单元算，雅黑的单元比汉字高，
/// 字会偏下甚至被裁掉底边。取不到字形度量时退回按字身估算。
/// SAFETY: 调用方保证 dc 有效且已选入目标字体。
unsafe fn centered(dc: HDC, glyph: u16, size: i32) -> (i32, i32) {
    let one = FIXED { fract: 0, value: 1 };
    let identity = MAT2 {
        eM11: one,
        eM22: one,
        ..Default::default()
    };
    let mut metrics = GLYPHMETRICS::default();
    // SAFETY: 只取度量（GGO_METRICS 不写缓冲），输出结构为本地值。
    let read = unsafe {
        GetGlyphOutlineW(
            dc,
            glyph.into(),
            GGO_METRICS,
            &mut metrics,
            0,
            None,
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

/// 白字黑底的像素转成“字色 + 覆盖率 alpha”（图标位图为非预乘 ARGB），覆盖率为 0 的像素全透明。
const fn coverage(pixel: u32, ink: u32) -> u32 {
    let (red, green, blue) = (pixel >> 16 & 0xFF, pixel >> 8 & 0xFF, pixel & 0xFF);
    let alpha = if red > green { red } else { green };
    let alpha = if blue > alpha { blue } else { alpha };
    if alpha == 0 { 0 } else { alpha << 24 | ink }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{rc::Rc, sync::Mutex};
    use tigertype_contracts::{Theme, without_blue};
    use windows::Win32::Foundation::COLORREF;
    use windows::Win32::{
        System::Threading::GetCurrentThreadId,
        UI::{
            Controls::{ODA_DRAWENTIRE, ODS_FLAGS},
            Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_DOWN, VK_ESCAPE, VK_RETURN},
        },
    };
    use windows::core::ComObject;

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
    fn left_click_toggles_and_the_right_click_menu_opens_panels_or_switches_full_shape() {
        let shown = Rc::new(RefCell::new(Vec::new()));
        let toggled = Rc::new(RefCell::new(Vec::new()));
        let online = Rc::new(Cell::new(true));
        let (log, probe, keys) = (shown.clone(), online.clone(), toggled.clone());
        let button: ITfLangBarItemButton = Indicator::new(
            move || (probe.get(), Options::default().palette(), Status::default()),
            move |panel| log.borrow_mut().push(panel),
            move |key| keys.borrow_mut().push(key),
        )
        .into();
        // 左键不弹菜单，只切换中英。
        // SAFETY: 本线程同步回调本按钮。
        unsafe { button.OnClick(TF_LBI_CLK_LEFT, POINT::default(), &RECT::default()) }.unwrap();
        assert_eq!(*toggled.borrow(), [Key::ToggleAscii]);
        assert!(shown.borrow().is_empty());
        // 右键弹出真实菜单：三个状态项点击即翻转，分隔线被方向键跳过，其后是查字与设置。
        for downs in 1..=5 {
            let mut keys = vec![VK_DOWN; downs];
            keys.push(VK_RETURN);
            right_click(&button, &keys);
        }
        right_click(&button, &[VK_ESCAPE]);
        assert_eq!(*shown.borrow(), [Panel::Lookup, Panel::Settings]);
        assert_eq!(
            *toggled.borrow(),
            [
                Key::ToggleAscii,
                Key::ToggleAscii,
                Key::ToggleFullShape,
                Key::TogglePunctuation
            ]
        );
        // 后台不可用：项全部置灰，Enter 收起菜单但不选中任何项，什么都不发。
        online.set(false);
        right_click(&button, &[VK_DOWN, VK_RETURN]);
        assert_eq!((shown.borrow().len(), toggled.borrow().len()), (2, 4));
        // 系统不对普通按钮调用菜单接口；即使被调用也不唤起任何窗口。
        // SAFETY: 同上。
        unsafe { button.OnMenuSelect(SETTINGS) }.unwrap();
        assert_eq!(shown.borrow().len(), 2);
    }

    #[test]
    fn menu_shows_the_current_states_then_lookup_and_settings_and_grays_out_when_offline() {
        let switched = Status {
            ascii: true,
            punctuation: true,
            full: true,
        };
        for (available, status) in [
            (true, Status::default()),
            (true, switched),
            (false, Status::default()),
        ] {
            let menu = build(available, status).unwrap();
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
                    assert_eq!(item.fType.contains(MFT_SEPARATOR), item.wID == SEPARATOR);
                    let text = String::from_utf16_lossy(&text[..item.cch as usize]);
                    items.push((item.wID, item.fState.contains(MFS_DISABLED), text));
                }
                DestroyMenu(menu).unwrap();
            }
            // 状态项只写当前状态，不并列两种，也不写“切换到”。
            let (mode, shape, punctuation) = if status.ascii {
                ("英文", "全角", "英文标点")
            } else {
                ("中文", "半角", "中文标点")
            };
            let expected = vec![
                (MODE, !available, mode.to_string()),
                (SHAPE, !available, shape.to_string()),
                (PUNCTUATION, !available, punctuation.to_string()),
                (SEPARATOR, true, String::new()),
                (LOOKUP, !available, "查字".to_string()),
                (SETTINGS, !available, "设置".to_string()),
            ];
            assert_eq!(items, expected);
        }
    }

    /// 在内存位图上画一个菜单项并读回指定像素。
    fn render(id: u32, state: ODS_FLAGS, at: &[(i32, i32)]) -> Vec<COLORREF> {
        let (width, height) = (200, 32);
        // SAFETY: 内存 DC 与位图只在本函数内使用，读完像素即还原并删除。
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            ReleaseDC(None, screen);
            let old = SelectObject(dc, bitmap.into());
            draw(
                HWND::default(),
                &DRAWITEMSTRUCT {
                    CtlType: ODT_MENU,
                    itemID: id,
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
            let pixels = at.iter().map(|&(x, y)| GetPixel(dc, x, y)).collect();
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            pixels
        }
    }

    #[test]
    fn menu_items_hover_in_theme_colors_and_the_shape_badge_shows_the_current_width() {
        for theme in Theme::ALL {
            let palette = Options {
                theme,
                ..Default::default()
            }
            .palette();
            PALETTE.set(palette);
            let c = colors(palette);
            // 悬停为主题高亮底（玫红为选区色），置灰与普通项不高亮；右侧远离文字处取色。
            for (state, expected) in [
                (ODS_SELECTED, c.hover),
                (
                    ODS_FLAGS(ODS_SELECTED.0 | ODS_GRAYED.0 | ODS_DISABLED.0),
                    c.window,
                ),
                (ODS_FLAGS(0), c.window),
            ] {
                let pixel = render(LOOKUP, state, &[(180, 16)])[0];
                assert_eq!(pixel, expected, "{theme:?} {state:?}");
                let [red, green, blue, _] = pixel.0.to_le_bytes();
                assert!(without_blue(u32::from_be_bytes([0, red, green, blue])));
            }
        }
        PALETTE.set(Options::default().palette());
        let c = colors(Options::default().palette());
        // 徽标圆心在 (23, 16)、半径 6：半角时右上被挖成新月露出徽标底，全角为满月。
        for (full, top_right) in [(false, c.track), (true, c.text)] {
            STATUS.set(Status {
                full,
                ..Status::default()
            });
            let pixels = render(SHAPE, ODS_FLAGS(0), &[(19, 18), (27, 13)]);
            assert_eq!(pixels, [c.text, top_right], "全角 {full}");
        }
        STATUS.set(Status::default());
    }

    #[implement(ITfLangBarItemSink)]
    struct Sink(Rc<RefCell<Vec<u32>>>);
    impl ITfLangBarItemSink_Impl for Sink_Impl {
        fn OnUpdate(&self, flags: u32) -> Result<()> {
            self.0.borrow_mut().push(flags);
            Ok(())
        }
    }

    #[test]
    fn mode_changes_redraw_the_icon_and_tooltip_only_when_the_mode_actually_flips() {
        let object = ComObject::new(Indicator::new(
            || (true, Options::default().palette(), Status::default()),
            |_| {},
            |_| {},
        ));
        let button: ITfLangBarItemButton = object.to_interface();
        let updates = Rc::new(RefCell::new(Vec::new()));
        let sink: ITfLangBarItemSink = Sink(updates.clone()).into();
        // SAFETY: 本线程同步调用本测试按钮。
        let tooltip = || unsafe { button.GetTooltipString() }.unwrap().to_string();
        // SAFETY: 登记本测试 sink，按钮在本线程同步回调。
        unsafe {
            button
                .cast::<ITfSource>()
                .unwrap()
                .AdviseSink(&ITfLangBarItemSink::IID, &sink)
        }
        .unwrap();
        assert!(tooltip().contains("中文"));
        // 状态未变不打扰系统；切到英文与切回中文各通知一次图标与提示。
        object.update(false);
        object.update(true);
        assert!(tooltip().contains("英文"));
        object.update(true);
        object.update(false);
        assert!(tooltip().contains("中文"));
        let flags = TF_LBI_ICON | TF_LBI_TOOLTIP;
        assert_eq!(*updates.borrow(), [flags, flags]);
    }

    #[test]
    fn icon_is_a_bare_glyph_in_the_taskbar_ink_on_a_transparent_background() {
        for (light, ink) in [(false, 0xFFFFFF), (true, 0x1A1A1A)] {
            for glyph in ["中", "英"] {
                bare(icon(glyph, light).unwrap(), ink);
            }
        }
    }

    fn bare(icon: HICON, ink: u32) {
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
            // 没有底板：角上透明、可见像素只占少数；笔画处完全不透明，可见像素一律是任务栏字色（无蓝）。
            assert_eq!(pixels[0] >> 24, 0);
            let visible: Vec<u32> = pixels.iter().copied().filter(|p| p >> 24 != 0).collect();
            assert!(
                visible.len() * 2 < pixels.len(),
                "{}/{}",
                visible.len(),
                pixels.len()
            );
            assert!(visible.iter().any(|p| p >> 24 == 0xFF));
            // 字形按墨迹居中：四边都不贴边，上下、左右留白相差不超过 1px（按字符单元居中会偏下被裁）。
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
}
