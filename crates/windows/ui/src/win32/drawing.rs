//! GDI 绘制基础、颜色与字体；只查询本线程控件，不持有业务模型或窗口生命周期。
use super::*;

thread_local! {
    // 仅本 UI 线程的绘制配色，设置确认后更新；不是跨线程业务状态。
    pub(super) static PALETTE: Cell<Palette> = Cell::new(Options::default().palette());
}

pub(super) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
/// 不带结尾 NUL 的 UTF-16，供 DrawTextW 与测量使用。
pub(super) fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}
pub(super) fn error() -> String {
    io::Error::last_os_error().to_string()
}

pub(super) fn post(hwnd: HWND, message: u32, value: WPARAM) {
    // SAFETY: 只排队值类型通知；队列满时退出并报告故障，不能悄悄丢失用户操作。
    unsafe {
        if PostMessageW(hwnd, message, value, 0) == 0 {
            PostQuitMessage(1);
        }
    }
}

pub(super) fn high_contrast() -> bool {
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
/// 只在用户自己开启高对比度时沿用系统颜色保证可读。侧栏、分段轨道等派生色由主题色混出。
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Colors {
    pub(super) window: COLORREF,
    pub(super) surface: COLORREF,
    pub(super) line: COLORREF,
    pub(super) field: COLORREF,
    pub(super) text: COLORREF,
    pub(super) label: COLORREF,
    pub(super) muted: COLORREF,
    pub(super) faint: COLORREF,
    pub(super) highlight: COLORREF,
    pub(super) highlight_text: COLORREF,
    pub(super) accent: COLORREF,
    pub(super) on_accent: COLORREF,
    pub(super) selection: COLORREF,
    /// 侧栏与预览台面：窗口色向文字色靠 4%。
    pub(super) sidebar: COLORREF,
    /// 分段单选轨道、键帽底与字块选中外的浅底：靠 6%。
    pub(super) track: COLORREF,
    /// 侧栏选中项与字块选中底：有高亮底用高亮底，玫红这类无高亮底的主题用选区色。
    pub(super) nav: COLORREF,
}
/// 主题颜色为 0xRRGGBB，GDI 为 0x00BBGGRR。
pub(super) const fn rgb(value: u32) -> COLORREF {
    (value & 0xFF) << 16 | (value & 0xFF00) | (value >> 16 & 0xFF)
}
pub(super) const fn themed(p: Palette) -> Colors {
    let shade = shades(p);
    Colors {
        window: rgb(p.window),
        surface: rgb(p.surface),
        line: rgb(p.line),
        field: rgb(p.field),
        text: rgb(p.text),
        label: rgb(p.label),
        muted: rgb(p.muted),
        faint: rgb(p.faint),
        highlight: rgb(p.highlight),
        highlight_text: rgb(p.highlight_text),
        accent: rgb(p.accent),
        on_accent: rgb(p.on_accent),
        selection: rgb(p.selection),
        sidebar: rgb(shade.sidebar),
        track: rgb(shade.track),
        nav: rgb(shade.nav),
    }
}
pub(super) fn colors() -> Colors {
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
        label: text,
        muted: text,
        faint: system(COLOR_GRAYTEXT),
        highlight: system(COLOR_HIGHLIGHT),
        highlight_text: system(COLOR_HIGHLIGHTTEXT),
        accent: system(COLOR_HIGHLIGHT),
        on_accent: system(COLOR_HIGHLIGHTTEXT),
        selection: system(COLOR_HIGHLIGHT),
        sidebar: window,
        track: window,
        nav: system(COLOR_HIGHLIGHT),
    }
}

/// 用系统 DC 画刷/画笔改色绘制，重绘不新建 GDI 对象。
pub(super) fn fill(dc: HDC, rect: &RECT, color: COLORREF) {
    // SAFETY: DC 画刷为库存对象，无需释放。
    unsafe {
        SetDCBrushColor(dc, color);
        FillRect(dc, rect, GetStockObject(DC_BRUSH));
    }
}
/// 圆角矩形：fill 为 Some 时实心填充，否则只描 1px 边。
pub(super) fn round(dc: HDC, rect: &RECT, radius: i32, fill: Option<COLORREF>, line: COLORREF) {
    match fill {
        Some(color) => rounded(dc, rect, radius, color, None),
        None => rounded(dc, rect, radius, line, Some(1)),
    }
}
/// 2px 描边：键盘焦点环、选中卡片外框，替代系统虚线框。
pub(super) fn ring(dc: HDC, rect: &RECT, radius: i32, color: COLORREF) {
    // 右/下各收 1px，与原先两道 1px 同心描边覆盖的范围一致；一次画完，避免两道抗锯齿边相接处漏底色。
    let outer = RECT {
        right: rect.right - 1,
        bottom: rect.bottom - 1,
        ..*rect
    };
    rounded(dc, &outer, radius, color, Some(2));
}
/// 覆盖像素 [left, right)×[top, bottom) 的圆角矩形；width 为 None 时实心，否则沿内侧描 width 像素边。
/// GDI 的 RoundRect 不抗锯齿，开关、圆钮这类小圆角会成台阶：直边像素对齐，直接 FillRect；
/// 四角取一张 2r×2r 的抗锯齿圆（预乘 ARGB），分四象限混合到四角。
pub(super) fn rounded(dc: HDC, rect: &RECT, radius: i32, color: COLORREF, width: Option<i32>) {
    let RECT {
        left,
        top,
        right,
        bottom,
    } = *rect;
    if right <= left || bottom <= top {
        return;
    }
    let r = radius.clamp(0, (right - left).min(bottom - top) / 2);
    let band = |left, top, right, bottom| RECT {
        left,
        top,
        right,
        bottom,
    };
    let bands = match width {
        None => [
            band(left + r, top, right - r, bottom),
            band(left, top + r, left + r, bottom - r),
            band(right - r, top + r, right, bottom - r),
            RECT::default(),
        ],
        Some(w) => [
            band(left + r, top, right - r, top + w),
            band(left + r, bottom - w, right - r, bottom),
            band(left, top + r, left + w, bottom - r),
            band(right - w, top + r, right, bottom - r),
        ],
    };
    for part in &bands {
        // 空矩形（胶囊两端相接、默认占位）FillRect 不画。
        fill(dc, part, color);
    }
    if r > 0 {
        corners(dc, rect, r, color, width);
    }
}
/// 把 disc 画好的圆按象限贴到四角；GDI 对象建不出时跳过四角（同 TempFont 的约定）。
fn corners(dc: HDC, rect: &RECT, r: i32, color: COLORREF, width: Option<i32>) {
    let side = 2 * r;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: side,
            // 负高度为自上而下的行序。
            biHeight: -side,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    // SAFETY: 内存 DC 与 DIB 均在本函数内新建并配对释放；像素缓冲有 side² 个 32 位像素，
    // 归 DIB 所有，只在 DIB 存活期间经此切片写入。
    unsafe {
        let memory = CreateCompatibleDC(dc);
        if memory.is_null() {
            return;
        }
        let mut bits = null_mut();
        let bitmap = CreateDIBSection(memory, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if !bitmap.is_null() && !bits.is_null() {
            let pixels = std::slice::from_raw_parts_mut(bits.cast(), (side * side) as usize);
            disc(pixels, r, color, width);
            let old = SelectObject(memory, bitmap);
            for (x, y) in [(0, 0), (r, 0), (0, r), (r, r)] {
                let to_x = if x == 0 { rect.left } else { rect.right - r };
                let to_y = if y == 0 { rect.top } else { rect.bottom - r };
                GdiAlphaBlend(dc, to_x, to_y, r, r, memory, x, y, r, r, blend);
            }
            SelectObject(memory, old);
        }
        if !bitmap.is_null() {
            DeleteObject(bitmap);
        }
        DeleteDC(memory);
    }
}
/// 半径 r 的圆铺满 2r×2r 像素，写成预乘 BGRA（DIB 字节序）；覆盖率按像素中心到圆周的
/// 有符号距离在 ±0.5px 内线性过渡。描边为外圆减去同心内圆，与直边宽度一致、接缝连续。
pub(super) fn disc(pixels: &mut [u32], r: i32, color: COLORREF, width: Option<i32>) {
    let side = 2 * r;
    let cover = |distance: f32| (0.5 - distance).clamp(0.0, 1.0);
    for (index, pixel) in pixels.iter_mut().enumerate() {
        let x = (index as i32 % side) as f32 + 0.5 - r as f32;
        let y = (index as i32 / side) as f32 + 0.5 - r as f32;
        let distance = (x * x + y * y).sqrt() - r as f32;
        let inner = width.map_or(0.0, |w| cover(distance + w as f32));
        let alpha = ((cover(distance) - inner) * 255.0).round() as u32;
        // COLORREF 为 0x00BBGGRR，DIB 像素为 0xAARRGGBB。
        let channel = |shift: u32| ((color >> shift & 0xFF) * alpha + 127) / 255;
        *pixel = alpha << 24 | channel(0) << 16 | channel(8) << 8 | channel(16);
    }
}
pub(super) fn inset(rect: &RECT, by: i32) -> RECT {
    RECT {
        left: rect.left + by,
        top: rect.top + by,
        right: rect.right - by,
        bottom: rect.bottom - by,
    }
}
pub(super) fn draw_text(
    dc: HDC,
    font: HFONT,
    color: COLORREF,
    text: &[u16],
    rect: RECT,
    format: u32,
) {
    // 长度为 0 时 DrawTextW 仍会读首字符，空 Vec 的悬空指针（0x2）会使后台崩溃。
    if text.is_empty() {
        return;
    }
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
/// 与绘制同用 DrawTextW，避免字体回退时测量与显示宽度不一致。
pub(super) fn extent(dc: HDC, font: HFONT, text: &[u16]) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let mut rect = RECT::default();
    // SAFETY: 字体临时选入后还原；文字缓冲在调用期间有效。
    unsafe {
        let old = SelectObject(dc, font);
        DrawTextW(
            dc,
            text.as_ptr(),
            text.len() as i32,
            &mut rect,
            DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(dc, old);
    }
    rect.right - rect.left
}
/// 临时字体：按 96 DPI 像素与设备 DPI 新建，用完由 Drop 删除；建不出时为空，绘制跳过。
pub(super) struct TempFont(pub(super) HFONT);
impl TempFont {
    pub(super) fn new(dc: HDC, faces: &[&str], pixels: i32, weight: i32) -> Self {
        // SAFETY: 只读绘制 DC 的 DPI。
        let dpi = unsafe { GetDeviceCaps(dc, LOGPIXELSY as i32) }.max(96) as u32;
        Self(create_font(dc, faces, pixels, dpi, weight).unwrap_or(null_mut()))
    }
}
impl Drop for TempFont {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: 本对象新建的字体，绘制结束后已不再选在 DC 里。
            unsafe { DeleteObject(self.0) };
        }
    }
}
/// 画一个图标字体字形（居中）。
pub(super) fn draw_icon(dc: HDC, code: u16, pixels: i32, color: COLORREF, rect: RECT) {
    let font = TempFont::new(dc, &ICON_FACES, pixels, 400);
    if !font.0.is_null() {
        draw_text(dc, font.0, color, &[code], rect, DT_CENTER | DT_NOPREFIX);
    }
}
/// 按已有字体改字号（设备像素）新建临时字体，沿用字体名与粗细；建不出时为空。
pub(super) fn resized(font: HFONT, pixels: i32) -> TempFont {
    // SAFETY: LOGFONTW 为本地值，按结构大小读取；新建字体归 TempFont 所有并由其删除。
    unsafe {
        let mut logfont: LOGFONTW = std::mem::zeroed();
        if GetObjectW(
            font,
            size_of::<LOGFONTW>() as i32,
            (&raw mut logfont).cast(),
        ) == 0
        {
            return TempFont(null_mut());
        }
        logfont.lfHeight = -pixels;
        logfont.lfWidth = 0;
        TempFont(CreateFontIndirectW(&logfont))
    }
}
/// 控件全文按实际长度取：长词的拆分串可上千个 UTF-16 单元，定长缓冲会截掉后面的字。
pub(super) fn control_text(hwnd: HWND) -> Vec<u16> {
    // SAFETY: 缓冲区长度含结尾 NUL，与传入容量一致；只读本线程控件文字。
    unsafe {
        let mut buffer = vec![0u16; GetWindowTextLengthW(hwnd).max(0) as usize + 1];
        let length = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        buffer.truncate(length.max(0) as usize);
        buffer
    }
}
/// 文字宽换回 96 DPI 逻辑像素并向上取整；查字布局与绘制同用，读音折行处两边一致。
pub(super) fn logical_extent(dc: HDC, font: HFONT, dpi: i32, text: &str) -> i32 {
    (extent(dc, font, &units(text)) * 96 + dpi - 1) / dpi
}
pub(super) fn window_text<const N: usize>(hwnd: HWND, buffer: &mut [u16; N]) -> usize {
    // SAFETY: 缓冲区长度与传入容量相同。
    unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32).max(0) as usize }
}
pub(super) fn font_of(hwnd: HWND) -> HFONT {
    // SAFETY: 同步查询控件字体；字体由所属 Window 持有。
    unsafe { SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT }
}
pub(super) fn parent(hwnd: HWND) -> HWND {
    // SAFETY: 只查询本线程控件的父窗口。
    unsafe { GetParent(hwnd) }
}
fn sibling(hwnd: HWND, id: usize) -> HWND {
    // SAFETY: 只在同一父窗口内按编号找控件。
    unsafe { GetDlgItem(GetParent(hwnd), id as i32) }
}
pub(super) fn sibling_font(hwnd: HWND, id: usize) -> HFONT {
    font_of(sibling(hwnd, id))
}
pub(super) fn scale(hwnd: HWND, value: i32) -> i32 {
    // SAFETY: 只读窗口 DPI。
    value * unsafe { GetDpiForWindow(hwnd) }.max(96) as i32 / 96
}
/// 用户未用键盘时按系统约定隐藏焦点环与助记下划线。
pub(super) fn cues(hwnd: HWND) -> (bool, u32) {
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
pub(super) fn create_font(
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
        if (index + 1 == faces.len() && faces != LEARNING_FACES) || face_is(dc, font, face) {
            return Ok(font);
        }
        // SAFETY: 字体只在 face_is 内临时选入并已还原。
        unsafe {
            DeleteObject(font);
        }
    }
    Err(if faces == LEARNING_FACES {
        "无法选择系统已安装的虎码文楷".into()
    } else {
        error()
    })
}
pub(super) fn face_is(dc: HDC, font: HFONT, face: &str) -> bool {
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
