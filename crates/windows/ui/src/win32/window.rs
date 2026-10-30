//! 窗口、类、字体和 DPI 的生命周期与控件布局；不发查字或保存请求。
use super::controls::*;
use super::drawing::*;
use super::*;

/// 控件自身是否带可见样式（不看父窗口）。
pub(super) fn shown(control: HWND) -> bool {
    // SAFETY: 只读本线程控件的样式位。
    unsafe { GetWindowLongW(control, GWL_STYLE) as u32 & WS_VISIBLE != 0 }
}
pub(super) fn visible(window: &Window) -> bool {
    // SAFETY: 只查询本线程窗口显隐。
    unsafe { IsWindowVisible(window.handle) != 0 }
}

/// 控件字体角色；字号为 96 DPI 像素，与设计稿一致。
#[derive(Clone, Copy)]
enum Role {
    Body,
    Small,
    Heading,
    Mono,
    List,
    Glyph,
    Title,
}
const ROLES: usize = 7;
fn role(id: usize) -> Role {
    match id {
        HEADING => Role::Heading,
        CODES | FULL | PAGE | CODEPOINT | FILE_PATH | CUSTOM..=253 => Role::Mono,
        LIST | PARTS => Role::List,
        GLYPH => Role::Glyph,
        FILE_NAME | ABOUT_TITLE => Role::Title,
        _ if (GROUP..GROUP + GROUPS.len()).contains(&id) => Role::Title,
        _ if muted(id) => Role::Small,
        _ => Role::Body,
    }
}

pub(super) struct Window {
    pub(super) handle: HWND,
    pub(super) controls: Vec<(usize, HWND)>,
    pub(super) fonts: [HFONT; ROLES],
    pub(super) icons: [HICON; 2],
    pub(super) dpi: u32,
    pub(super) face: CandidateFont,
}
impl Window {
    pub(super) fn create(
        class: &[u16],
        title: &str,
        owner: HWND,
        style: u32,
    ) -> Result<Self, String> {
        // 设置窗可在查字窗隐藏时单独打开，被拥有窗口默认无任务栏按钮，须显式要一个，免得被挡住后找不回。
        let extra = if owner.is_null() { 0 } else { WS_EX_APPWINDOW };
        // SAFETY: 字符串 NUL 终止；窗口归当前线程所有，没有传入业务指针。
        let handle = unsafe {
            CreateWindowExW(
                WS_EX_CONTROLPARENT | extra,
                class.as_ptr(),
                wide(title).as_ptr(),
                style | WS_CLIPCHILDREN,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                LOOKUP_SIZE.0,
                LOOKUP_SIZE.1,
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
            fonts: [null_mut(); ROLES],
            icons: [null_mut(); 2],
            dpi: 0,
            face: CandidateFont::default(),
        })
    }
    pub(super) fn add(
        &mut self,
        id: usize,
        class: &str,
        title: &str,
        style: u32,
    ) -> Result<(), String> {
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
        let procedure: usize = match class {
            "EDIT" => edit_procedure as *const () as usize,
            "BUTTON" => button_procedure as *const () as usize,
            _ => 0,
        };
        if procedure != 0 {
            // SAFETY: 本线程拥有的控件，新过程以类登记的原过程处理其余消息；控件随父窗口销毁，无需还原。
            unsafe {
                SetWindowLongPtrW(handle, GWLP_WNDPROC, procedure as _);
            }
        }
        self.controls.push((id, handle));
        Ok(())
    }
    pub(super) fn control(&self, id: usize) -> HWND {
        self.controls
            .iter()
            .find(|(key, _)| *key == id)
            .expect("已创建的控件")
            .1
    }
    pub(super) fn text(&self, id: usize, text: &str) {
        // SAFETY: 控件归本视图持有，文本在同步调用期间存活；自绘静态控件改字后重画。
        unsafe {
            SetWindowTextW(self.control(id), wide(text).as_ptr());
            InvalidateRect(self.control(id), null(), 0);
        }
    }
    pub(super) fn enable(&self, id: usize, enabled: bool) {
        // SAFETY: 控件只在创建和销毁之间由所属 UI 线程访问。
        unsafe {
            EnableWindow(self.control(id), enabled.into());
        }
    }
    pub(super) fn show(&self, id: usize, visible: bool) {
        let control = self.control(id);
        // 看控件自身的可见样式：父窗口隐藏时 IsWindowVisible 恒为假，不能据此判断。
        // SAFETY: 本线程拥有的子控件；只在显隐确实变化时调用，避免闪烁。
        unsafe {
            if shown(control) != visible {
                ShowWindow(control, if visible { SW_SHOWNA } else { SW_HIDE });
            }
        }
    }
    pub(super) fn focused_control(&self) -> Option<usize> {
        // SAFETY: 只查询当前 UI 线程焦点，返回控件编号而非跨线程裸句柄。
        let focus = unsafe { GetFocus() };
        self.controls
            .iter()
            .find(|(_, hwnd)| *hwnd == focus)
            .map(|(id, _)| *id)
    }
    pub(super) fn restore_focus(&self, id: usize) {
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
    pub(super) fn send(&self, id: usize, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        // SAFETY: 本模块只发送已匹配控件类型的同步消息；字符串参数在调用方保持存活。
        unsafe { SendMessageW(self.control(id), message, w, l) }
    }
    pub(super) fn place(&self, id: usize, [x, y, width, height]: Box4) {
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
    /// 字体随 DPI 与候选字体变化重建，控件全部换上新字体后才释放旧字体。
    pub(super) fn ensure_fonts(&mut self, face: CandidateFont) -> Result<(), String> {
        // SAFETY: 查询本线程拥有窗口的当前 DPI。
        let dpi = unsafe { GetDpiForWindow(self.handle) }.max(96);
        if self.dpi == dpi && self.face == face {
            return Ok(());
        }
        // SAFETY: 取本窗口 DC 只用于核对字体名，结束前归还。
        let dc = unsafe { GetDC(self.handle) };
        let mut fonts = [null_mut(); ROLES];
        let specs: [(&[&str], i32, i32); ROLES] = [
            (&[UI_FACE], 14, 400),
            (&[UI_FACE], 12, 400),
            (&[UI_FACE], 20, 600),
            (&MONO_FACES, 13, 400),
            (&[font_face(face), UI_FACE], 20, 400),
            (&[font_face(face), UI_FACE], 76, 400),
            (&[UI_FACE], 13, 600),
        ];
        let mut result = Ok(());
        for (slot, (faces, pixels, weight)) in fonts.iter_mut().zip(specs) {
            match create_font(dc, faces, pixels, dpi, weight) {
                Ok(font) => *slot = font,
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        // SAFETY: 归还上面取得的 DC。
        unsafe {
            ReleaseDC(self.handle, dc);
        }
        if let Err(error) = result {
            release(fonts);
            return Err(error);
        }
        for (id, control) in &self.controls {
            // SAFETY: 新字体在整个控件使用期保持存活，消息同步处理。
            unsafe {
                SendMessageW(*control, WM_SETFONT, fonts[role(*id) as usize] as usize, 1);
            }
        }
        release(std::mem::replace(&mut self.fonts, fonts));
        self.dpi = dpi;
        self.face = face;
        if let Some((_, list)) = self.controls.iter().find(|(id, _)| *id == LIST) {
            // SAFETY: 本线程拥有的列表框，值类型参数。
            unsafe {
                SendMessageW(
                    *list,
                    LB_SETITEMHEIGHT,
                    0,
                    (ITEM_HEIGHT * dpi / 96) as LPARAM,
                );
            }
        }
        Ok(())
    }
    pub(super) fn layout(&mut self, face: CandidateFont) -> Result<(), String> {
        self.ensure_fonts(face)?;
        // EDIT 换字体会按字号重设左右边距；色值框与查询框统一清零，位置由外框决定。
        for (id, control) in &self.controls {
            if *id == QUERY || (CUSTOM..CUSTOM + 4).contains(id) {
                // SAFETY: 本线程拥有的 EDIT，值类型参数。
                unsafe {
                    SendMessageW(
                        *control,
                        EM_SETMARGINS,
                        (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
                        0,
                    );
                }
            }
        }
        let mut rect = RECT::default();
        // SAFETY: 输出 RECT 由栈分配且窗口有效。
        if unsafe { GetClientRect(self.handle, &mut rect) } == 0 {
            return Err(error());
        }
        let dpi = self.dpi as i32;
        if self.controls.iter().any(|(id, _)| *id == QUERY) {
            for (id, place) in lookup_spec(rect.right * 96 / dpi, rect.bottom * 96 / dpi).controls {
                self.place(id, place);
            }
        } else {
            self.layout_settings(rect.bottom * 96 / dpi);
        }
        // SAFETY: 重画本窗口及其子控件，使卡片与新位置一致。
        unsafe {
            RedrawWindow(
                self.handle,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_ALLCHILDREN,
            );
        }
        Ok(())
    }
    /// 设置窗：按当前分区与是否自定义配色排布，隐藏其余控件；内容高于窗口时更新滚动条并按滚动位置平移。
    pub(super) fn layout_settings(&self, view: i32) {
        let spec = settings_spec(
            current_page(self.handle),
            checked_in(self.handle, THEME + 4),
        );
        let offset = scroll_offset(self.handle).clamp(0, (spec.height - view).max(0));
        set_scroll_offset(self.handle, offset);
        let info = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: spec.height - 1,
            nPage: view.max(0) as u32,
            nPos: offset,
            nTrackPos: 0,
        };
        // SAFETY: 结构大小已填，只设置本窗口滚动条；页长不小于范围时系统自动隐藏滚动条。
        unsafe { SetScrollInfo(self.handle, SB_VERT, &info, 1) };
        for (id, place) in &spec.fixed {
            self.place(*id, *place);
        }
        for (id, [x, y, width, height]) in &spec.controls {
            self.place(*id, [*x, y - offset, *width, *height]);
        }
        for (id, _) in &self.controls {
            let visible = spec
                .fixed
                .iter()
                .chain(&spec.controls)
                .any(|(key, _)| key == id);
            self.show(*id, visible);
        }
    }
    /// Win11 标题栏与边框改用主题色，不随系统强调色；Win10 不支持时保持系统默认。
    pub(super) fn decorate(&self, palette: Palette, caption: u32) {
        if high_contrast() {
            return;
        }
        for (attribute, value) in [
            (DWMWA_CAPTION_COLOR, rgb(caption)),
            (DWMWA_BORDER_COLOR, rgb(palette.border)),
            (DWMWA_TEXT_COLOR, rgb(palette.text)),
            (DWMWA_USE_IMMERSIVE_DARK_MODE, palette.dark() as u32),
        ] {
            // SAFETY: 只设置本窗口属性，参数为本地 4 字节值；失败不影响使用。
            unsafe {
                DwmSetWindowAttribute(
                    self.handle,
                    attribute as _,
                    (&value as *const u32).cast(),
                    size_of::<u32>() as u32,
                );
            }
        }
    }
    /// 标题栏与任务栏按钮用虎码印章；图标随窗口保留，Drop 时释放。
    pub(super) fn set_icons(&mut self) {
        for (slot, (kind, metric)) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)]
            .into_iter()
            .enumerate()
        {
            // SAFETY: 系统度量查询无副作用；图标在窗口存活期间有效，换下的旧图标随即释放。
            unsafe {
                let side = GetSystemMetricsForDpi(metric, GetDpiForWindow(self.handle).max(96));
                if let Some(icon) = seal::icon(side) {
                    SendMessageW(self.handle, WM_SETICON, kind as usize, icon as LPARAM);
                    let old = std::mem::replace(&mut self.icons[slot], icon);
                    if !old.is_null() {
                        DestroyIcon(old);
                    }
                }
            }
        }
    }
}
pub(super) fn release(fonts: [HFONT; ROLES]) {
    for font in fonts {
        if !font.is_null() {
            // SAFETY: 控件已改用其他字体或已销毁，旧 GDI 对象不再被引用。
            unsafe {
                DeleteObject(font);
            }
        }
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: 子控件先随窗口销毁，之后释放字体与图标；均在创建它们的 UI 线程执行。
        unsafe {
            DestroyWindow(self.handle);
            for icon in self.icons {
                if !icon.is_null() {
                    DestroyIcon(icon);
                }
            }
        }
        release(self.fonts);
    }
}

pub(super) struct Class(pub(super) Vec<u16>);
impl Class {
    pub(super) fn register() -> Result<Self, String> {
        let name = wide("TigerType.Lookup.Native.v3");
        // SAFETY: 仅注册无状态系统回调；不设背景画刷，底色由 WM_PAINT 按主题绘制。
        let registered = unsafe {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: GetModuleHandleW(null()),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
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

/// 按客户区尺寸设定窗口外框（按窗口 DPI 换算），不移动、不激活。
pub(super) fn size_client(window: &Window, (width, height): (i32, i32), style: u32) {
    let dpi = window.dpi;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: width * dpi as i32 / 96,
        bottom: height * dpi as i32 / 96,
    };
    // SAFETY: 输出 RECT 为本地值；只调整本窗口尺寸，不激活。
    unsafe {
        AdjustWindowRectExForDpi(&mut rect, style, 0, WS_EX_CONTROLPARENT, dpi);
        SetWindowPos(
            window.handle,
            null_mut(),
            0,
            0,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// 按系统约定初始化焦点环与助记下划线：用户未用键盘时先隐藏，按 Alt/Tab 后再显示。
pub(super) fn initialize_cues(hwnd: HWND) {
    // SAFETY: 发给本线程拥有的顶层窗口，值类型参数。
    unsafe {
        // 高字指定受影响的提示；为 0 时什么都不改。
        let flags = (UISF_HIDEACCEL | UISF_HIDEFOCUS) << 16;
        SendMessageW(hwnd, WM_CHANGEUISTATE, (UIS_INITIALIZE | flags) as usize, 0);
    }
}
/// 设置窗标题栏与侧栏同色。
pub(super) fn repaint(window: &Window) {
    // SAFETY: 只使本窗口及子控件失效，绘制在各自 WM_PAINT 中进行。
    unsafe {
        RedrawWindow(
            window.handle,
            null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ALLCHILDREN,
        );
    }
}
