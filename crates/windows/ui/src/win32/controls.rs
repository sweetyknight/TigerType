//! 原生控件子类化、主题绘制与值类型窗口通知；只依赖绘制/模态基础，不读取业务 Model。
use super::drawing::*;
use super::modal::*;
use super::*;

#[cfg(target_pointer_width = "64")]
fn class_procedure(hwnd: HWND) -> WNDPROC {
    // SAFETY: 读取控件类登记的原窗口过程（不受本窗口子类化影响），与 WNDPROC 同宽。
    unsafe { std::mem::transmute::<usize, WNDPROC>(GetClassLongPtrW(hwnd, GCLP_WNDPROC)) }
}
#[cfg(target_pointer_width = "32")]
fn class_procedure(hwnd: HWND) -> WNDPROC {
    // SAFETY: x86 没有 Ptr 版本，类长整数与函数指针同宽。
    unsafe { std::mem::transmute::<usize, WNDPROC>(GetClassLongW(hwnd, GCLP_WNDPROC) as usize) }
}

// SAFETY: 系统在所属 UI 线程调用；先交 EDIT 原过程处理，再在本次回调内按主题重画可见选区与空查询框提示；
// 不保存指针、不分配，选区相关以外的消息原样返回（UNBLOCK 由本过程处理，不交 EDIT）。
pub(super) unsafe extern "system" fn edit_procedure(
    hwnd: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    if message == UNBLOCK {
        unblock(hwnd);
        return 0;
    }
    // EDIT 自带的右键菜单不发 WM_ENTERMENULOOP，在原过程弹菜单前后自行登记/注销模态等待。
    let menu = message == WM_CONTEXTMENU && modal_enter(hwnd);
    // SAFETY: 参数原样交给 EDIT 类的原窗口过程。
    let result = unsafe { CallWindowProcW(class_procedure(hwnd), hwnd, message, w, l) };
    if menu {
        modal_exit();
    }
    if matches!(message, WM_SETFOCUS | WM_KILLFOCUS) {
        // 输入框外框随焦点变色，由父窗口重画。
        // SAFETY: 只使父窗口失效，绘制在其 WM_PAINT 中进行。
        unsafe {
            InvalidateRect(GetParent(hwnd), null(), 0);
        }
    }
    if message == WM_PAINT {
        paint_cue(hwnd);
    }
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

/// 查询框为空时画占位提示（不启用通用控件 6 版，EM_SETCUEBANNER 不可用）。
fn paint_cue(hwnd: HWND) {
    // SAFETY: 只读本控件编号与文字长度；取本窗口 DC 并在结束时归还，插入符先隐藏避免残影。
    unsafe {
        if GetDlgCtrlID(hwnd) as usize != QUERY || GetWindowTextLengthW(hwnd) != 0 {
            return;
        }
        let dc = GetDC(hwnd);
        if dc.is_null() {
            return;
        }
        let mut format = RECT::default();
        SendMessageW(hwnd, EM_GETRECT, 0, &mut format as *mut RECT as LPARAM);
        HideCaret(hwnd);
        draw_text(
            dc,
            font_of(hwnd),
            colors().muted,
            &units(QUERY_HINT),
            format,
            DT_NOPREFIX,
        );
        ShowCaret(hwnd);
        ReleaseDC(hwnd, dc);
    }
}

/// EDIT 只能用系统选中色（蓝）画选区；原过程画完后，按字符位置用主题选区色重画仍可见的选区，
/// 选择与复制行为不变。选区只在拥有焦点时可见（未设 ES_NOHIDESEL）。
fn repaint_selection(hwnd: HWND) {
    // SAFETY: 只读本线程焦点。
    if unsafe { GetFocus() } != hwnd || high_contrast() {
        return;
    }
    // SAFETY: 取本窗口 DC 并在结束时归还；先隐藏插入符，避免异或插入符在选区色上留下残影。
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
    let c = colors();
    let mut format = RECT::default();
    send(EM_GETRECT, 0, &mut format as *mut RECT as LPARAM);
    let first = send(EM_LINEFROMCHAR, start as usize, 0).max(0) as usize;
    let last = send(EM_LINEFROMCHAR, end as usize, 0).max(0) as usize;
    // SAFETY: 选入本控件字体测量与绘制，结束前还原；文字缓冲区在调用期间存活。
    unsafe {
        let old = SelectObject(dc, send(WM_GETFONT, 0, 0) as HFONT);
        let mut metrics: TEXTMETRICW = std::mem::zeroed();
        GetTextMetricsW(dc, &mut metrics);
        SetTextColor(dc, c.text);
        SetBkColor(dc, c.selection);
        // 编辑框都是单行短文本；上限只防异常输入拖慢绘制。
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

// SAFETY: 系统在所属 UI 线程调用；WM_PAINT 完全自绘，其余消息先交 BUTTON 原过程，
// 原过程会在状态变化时直接画经典外观，随后立即按主题覆盖；不保存指针、不分配。
pub(super) unsafe extern "system" fn button_procedure(
    hwnd: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    match message {
        WM_ERASEBKGND => return 1,
        WM_PAINT => {
            // SAFETY: WM_PAINT 内配对 BeginPaint/EndPaint。
            unsafe {
                let mut paint: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(hwnd, &mut paint);
                draw_button(hwnd, dc);
                EndPaint(hwnd, &paint);
            }
            return 0;
        }
        _ => {}
    }
    // SAFETY: 参数原样交给 BUTTON 类的原窗口过程。
    let result = unsafe { CallWindowProcW(class_procedure(hwnd), hwnd, message, w, l) };
    if matches!(
        message,
        BM_SETCHECK
            | BM_SETSTATE
            | BM_CLICK
            | WM_SETFOCUS
            | WM_KILLFOCUS
            | WM_ENABLE
            | WM_SETTEXT
            | WM_UPDATEUISTATE
            | WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_LBUTTONDBLCLK
            | WM_KEYDOWN
            | WM_KEYUP
            | WM_CAPTURECHANGED
    ) {
        // SAFETY: 取本控件 DC 覆盖原过程直接画出的经典外观，结束时归还。
        unsafe {
            let dc = GetDC(hwnd);
            if !dc.is_null() {
                draw_button(hwnd, dc);
                ReleaseDC(hwnd, dc);
            }
        }
    }
    result
}

fn is_checked(control: HWND) -> bool {
    // SAFETY: 同步查询本线程按钮的勾选状态；空句柄返回 0。
    unsafe { SendMessageW(control, BM_GETCHECK, 0, 0) == BST_CHECKED as isize }
}
pub(super) fn checked_in(parent: HWND, id: usize) -> bool {
    // SAFETY: 只在本窗口内按编号找控件。
    is_checked(unsafe { GetDlgItem(parent, id as i32) })
}
/// 一组分段单选中被选中的序号。
fn pick(parent: HWND, first: usize, count: usize) -> Option<usize> {
    (0..count).find(|k| checked_in(parent, first + k))
}
fn page_size_in(parent: HWND) -> u8 {
    pick(parent, SIZE_CHOICE, 7).map_or(Options::default().page_size, |k| k as u8 + 3)
}
/// 读一个十六进制色值框：可带或不带“#”，须恰为 6 位。
pub(super) fn hex_in(parent: HWND, k: usize) -> Result<u32, String> {
    let mut buffer = [0u16; 64];
    // SAFETY: 只在本窗口内按编号找控件。
    let length = window_text(
        unsafe { GetDlgItem(parent, (CUSTOM + k) as i32) },
        &mut buffer,
    );
    let text = String::from_utf16_lossy(&buffer[..length]);
    let hex = text.trim().trim_start_matches('#');
    u32::from_str_radix(hex, 16)
        .ok()
        .filter(|_| hex.len() == 6)
        .ok_or_else(|| format!("{}色“{text}”须为 6 位十六进制，如 #F9F8F5", COLORS[k].0))
}
fn custom_in(parent: HWND) -> Result<CustomColors, String> {
    Ok(CustomColors {
        window: hex_in(parent, 0)?,
        text: hex_in(parent, 1)?,
        highlight: hex_in(parent, 2)?,
        accent: hex_in(parent, 3)?,
    })
}
/// 设置窗控件即草稿：读出并在本地校验，偏蓝、不可读或格式不对的不发往后台。
pub(super) fn read_options(parent: HWND) -> Result<Options, String> {
    let choice = |first: usize, count: usize, name: &str| {
        pick(parent, first, count).ok_or_else(|| format!("必须选择{name}"))
    };
    let options = Options {
        auto_commit_four: checked_in(parent, AUTO),
        common_only: checked_in(parent, COMMON),
        show_learning: checked_in(parent, LEARNING),
        output_form: FORMS[choice(FORM, FORMS.len(), "输出字形")?].0,
        candidate_font: CandidateFont::ALL[choice(FONT, CandidateFont::ALL.len(), "候选字体")?],
        theme: Theme::ALL[choice(THEME, Theme::ALL.len(), "配色主题")?],
        typing_mode: TypingMode::ALL[choice(MODE, TypingMode::ALL.len(), "输入模式")?],
        page_size: choice(SIZE_CHOICE, 7, "每页候选数")? as u8 + 3,
        custom: custom_in(parent)?,
    };
    options.validate()?;
    Ok(options)
}
/// 自定义配色行下方的提示：可读时报最低对比度，否则说明原因。
pub(super) fn custom_status(parent: HWND) -> String {
    let check = custom_in(parent).and_then(|custom| {
        let options = Options {
            theme: Theme::Custom,
            custom,
            ..Options::default()
        };
        options
            .validate()
            .map(|()| options.palette().min_contrast())
    });
    match check {
        Ok(contrast) => format!("✓ 全部文字可读，最低对比度 {contrast:.1} : 1"),
        Err(error) => format!("✗ {error}；修正后自动保存"),
    }
}
/// 主题卡片与预览所用配色：预设取固定值，自定义取输入框里的草稿，草稿无效时退回当前配色。
fn theme_palette(parent: HWND, k: usize) -> Palette {
    match Theme::ALL[k].preset() {
        Some(palette) => palette,
        None => custom_in(parent).map_or_else(|_| PALETTE.with(Cell::get), CustomColors::palette),
    }
}

/// 按钮外观：开关行（复选框）、侧栏分区、主题卡片、分段单选、色块行与普通按钮。
pub(super) fn draw_button(hwnd: HWND, dc: HDC) {
    let c = colors();
    let s = |value| scale(hwnd, value);
    let mut rect = RECT::default();
    let mut buffer = [0u16; 64];
    let length = window_text(hwnd, &mut buffer);
    let text = &buffer[..length];
    // SAFETY: 只读本控件的样式、状态与焦点。
    let (kind, state, id, enabled, focused) = unsafe {
        GetClientRect(hwnd, &mut rect);
        (
            GetWindowLongW(hwnd, GWL_STYLE) & BS_TYPEMASK,
            SendMessageW(hwnd, BM_GETSTATE, 0, 0) as u32,
            GetDlgCtrlID(hwnd) as usize,
            IsWindowEnabled(hwnd) != 0,
            GetFocus() == hwnd,
        )
    };
    let (show_focus, prefix) = cues(hwnd);
    let focused = focused && show_focus;
    let (checked, pushed) = (state & BST_CHECKED != 0, state & BST_PUSHED != 0);
    let font = font_of(hwnd);
    let color = |on: COLORREF| if enabled { on } else { c.faint };
    let small = sibling_font(hwnd, NOTICE);
    match kind {
        BS_AUTOCHECKBOX => {
            fill(dc, &rect, c.surface);
            let description = TOGGLES
                .iter()
                .find(|(key, ..)| *key == id)
                .map_or("", |(.., description)| *description);
            let title = RECT {
                left: rect.left + s(15),
                top: rect.top + s(9),
                right: rect.right - s(80),
                bottom: rect.top + s(31),
            };
            draw_text(dc, font, color(c.text), text, title, prefix);
            let line = RECT {
                top: rect.top + s(32),
                bottom: rect.top + s(50),
                ..title
            };
            draw_text(dc, small, c.muted, &units(description), line, DT_NOPREFIX);
            let top = (rect.top + rect.bottom - s(22)) / 2;
            let track = RECT {
                left: rect.right - s(15 + 40),
                top,
                right: rect.right - s(15),
                bottom: top + s(22),
            };
            if checked {
                round(dc, &track, s(11), Some(color(c.accent)), c.accent);
                let knob = RECT {
                    left: track.right - s(18),
                    top: top + s(4),
                    right: track.right - s(4),
                    bottom: top + s(18),
                };
                round(dc, &knob, s(7), Some(c.on_accent), c.on_accent);
            } else {
                round(dc, &track, s(11), None, color(c.field));
                round(dc, &inset(&track, 1), s(10), None, color(c.field));
                let knob = RECT {
                    left: track.left + s(5),
                    top: top + s(6),
                    right: track.left + s(15),
                    bottom: top + s(16),
                };
                round(dc, &knob, s(5), Some(color(c.muted)), c.muted);
            }
            if focused {
                ring(dc, &inset(&track, -s(3)), s(14), c.text);
            }
        }
        BS_AUTORADIOBUTTON if (NAV..NAV + PAGES.len()).contains(&id) => {
            fill(dc, &rect, c.sidebar);
            if checked {
                round(dc, &rect, s(6), Some(c.nav), c.nav);
            }
            let icon = RECT {
                left: rect.left + s(10),
                right: rect.left + s(30),
                ..rect
            };
            let tint = if checked { c.accent } else { c.muted };
            draw_icon(dc, PAGES[id - NAV].1, 16, tint, icon);
            let label = RECT {
                left: rect.left + s(38),
                ..rect
            };
            draw_text(dc, font, c.text, text, label, prefix);
            if focused {
                ring(dc, &rect, s(6), c.text);
            }
        }
        BS_AUTORADIOBUTTON if (THEME..THEME + Theme::ALL.len()).contains(&id) => {
            fill(dc, &rect, c.surface);
            let p = theme_palette(parent(hwnd), id - THEME);
            ring(dc, &rect, s(8), if checked { c.accent } else { c.line });
            let mini = RECT {
                left: rect.left + s(8),
                top: rect.top + s(8),
                right: rect.right - s(8),
                bottom: rect.top + s(52),
            };
            round(dc, &mini, s(5), Some(rgb(p.window)), rgb(p.window));
            round(dc, &mini, s(5), None, rgb(p.border));
            let glyphs = TempFont::new(dc, &[font_face(CandidateFont::WenKai), UI_FACE], 12, 400);
            for (row, (label, ink)) in [("1 拉", p.chosen), ("2 隹", p.text)]
                .into_iter()
                .enumerate()
            {
                let band = RECT {
                    left: mini.left + s(4),
                    top: mini.top + s(4) + row as i32 * s(19),
                    right: mini.right - s(4),
                    bottom: mini.top + s(21) + row as i32 * s(19),
                };
                if row == 0 && p.highlight != p.window {
                    round(dc, &band, s(3), Some(rgb(p.highlight)), rgb(p.highlight));
                }
                let words = RECT {
                    left: band.left + s(5),
                    ..band
                };
                draw_text(dc, glyphs.0, rgb(ink), &units(label), words, DT_NOPREFIX);
            }
            let name = RECT {
                top: mini.bottom + s(2),
                bottom: rect.bottom - s(3),
                ..rect
            };
            draw_text(dc, font, c.text, text, name, DT_CENTER | prefix);
            if focused {
                ring(dc, &inset(&rect, s(3)), s(6), c.text);
            }
        }
        BS_AUTORADIOBUTTON => {
            // 轨道由父窗口画；选中段为卡片色小块加细边，不用整块强调色。
            fill(dc, &rect, c.track);
            if checked {
                round(dc, &rect, s(5), Some(c.surface), c.surface);
                round(dc, &rect, s(5), None, c.line);
            }
            let ink = color(if checked { c.text } else { c.muted });
            let face = (FONT..FONT + CandidateFont::ALL.len())
                .contains(&id)
                .then(|| {
                    TempFont::new(
                        dc,
                        &[font_face(CandidateFont::ALL[id - FONT]), UI_FACE],
                        15,
                        400,
                    )
                });
            let font = face.as_ref().map_or(font, |face| face.0);
            draw_text(dc, font, ink, text, rect, DT_CENTER | prefix);
            if focused {
                ring(dc, &rect, s(5), c.text);
            }
        }
        _ if (SWATCH..SWATCH + COLORS.len()).contains(&id) => {
            let k = id - SWATCH;
            fill(dc, &rect, c.surface);
            let value = hex_in(parent(hwnd), k).unwrap_or(match k {
                0 => PALETTE.with(Cell::get).window,
                1 => PALETTE.with(Cell::get).text,
                2 => PALETTE.with(Cell::get).highlight,
                _ => PALETTE.with(Cell::get).accent,
            });
            let chip = RECT {
                left: rect.left,
                top: rect.top + s(5),
                right: rect.left + s(30),
                bottom: rect.top + s(35),
            };
            round(dc, &chip, s(7), Some(rgb(value)), rgb(value));
            round(dc, &chip, s(7), None, c.field);
            let title = RECT {
                left: chip.right + s(12),
                top: rect.top + s(2),
                right: rect.right,
                bottom: rect.top + s(21),
            };
            draw_text(dc, font, c.text, text, title, prefix);
            let line = RECT {
                top: rect.top + s(21),
                bottom: rect.top + s(38),
                ..title
            };
            draw_text(dc, small, c.muted, &units(COLORS[k].1), line, DT_NOPREFIX);
            if focused {
                ring(dc, &inset(&chip, -s(3)), s(9), c.text);
            }
        }
        _ => {
            let flat = matches!(id, PREVIOUS | NEXT);
            let back = if on_surface(id) { c.surface } else { c.window };
            fill(dc, &rect, back);
            if pushed {
                round(dc, &rect, s(6), Some(c.track), c.track);
            } else if !flat {
                round(dc, &rect, s(6), Some(c.surface), c.surface);
            }
            if !flat {
                round(dc, &rect, s(6), None, c.field);
            }
            let ink = color(c.text);
            let icon = match id {
                SETTINGS => Some(0xE713),
                PREVIOUS => Some(0xE76B),
                NEXT => Some(0xE76C),
                COPY => Some(0xE8C8),
                RELOAD => Some(0xE72C),
                _ => None,
            };
            match icon {
                // 只有图标的按钮：文字留作读屏名称与助记键，不画出来。
                Some(code) if matches!(id, SETTINGS | PREVIOUS | NEXT) => {
                    draw_icon(dc, code, 16, ink, rect);
                }
                Some(code) => {
                    let label = &text[..text
                        .iter()
                        .position(|&unit| unit == u16::from(b'('))
                        .unwrap_or(text.len())];
                    let width = s(14) + s(6) + extent(dc, font, label);
                    let left = (rect.left + rect.right - width) / 2;
                    let glyph = RECT {
                        left,
                        right: left + s(14),
                        ..rect
                    };
                    draw_icon(dc, code, 13, ink, glyph);
                    let words = RECT {
                        left: glyph.right + s(6),
                        ..rect
                    };
                    draw_text(dc, font, ink, label, words, DT_NOPREFIX);
                }
                None => draw_text(dc, font, ink, text, rect, DT_CENTER | prefix),
            }
            if focused {
                ring(dc, &rect, s(6), c.text);
            }
        }
    }
}

/// 自绘静态控件与字块列表：字形格、预览、快捷键格、编码键帽、拆分字块、查字字块。
fn draw_item(item: &DRAWITEMSTRUCT) {
    let c = colors();
    let s = |value| scale(item.hwndItem, value);
    let rect = item.rcItem;
    let dc = item.hDC;
    if item.CtlType == ODT_LISTBOX {
        draw_chip(item, &c);
        return;
    }
    let id = item.CtlID as usize;
    match id {
        PREVIEW => return draw_preview(item, &c),
        _ if (KEYS..KEYS + KEYMAP.len()).contains(&id) => return draw_keys(item, &c, id - KEYS),
        _ => {}
    }
    let mut buffer = [0u16; 64];
    let length = window_text(item.hwndItem, &mut buffer);
    let text = &buffer[..length];
    fill(dc, &rect, c.surface);
    if id == GLYPH {
        round(dc, &rect, s(8), None, c.line);
        // SAFETY: 新建虚线画笔用后即删；直线端点为本地值。
        unsafe {
            let pen = CreatePen(PS_DOT, 1, c.line);
            let old = SelectObject(dc, pen);
            let (x, y) = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
            MoveToEx(dc, x, rect.top + s(8), null_mut());
            LineTo(dc, x, rect.bottom - s(8));
            MoveToEx(dc, rect.left + s(8), y, null_mut());
            LineTo(dc, rect.right - s(8), y);
            SelectObject(dc, old);
            DeleteObject(pen);
        }
        // 词组放不进字形格，改用字块字号。
        let single = String::from_utf16_lossy(text).chars().count() <= 1;
        let font = if single {
            font_of(item.hwndItem)
        } else {
            sibling_font(item.hwndItem, LIST)
        };
        draw_text(dc, font, c.text, text, rect, DT_CENTER | DT_NOPREFIX);
        return;
    }
    let missing = String::from_utf16_lossy(text) == "缺资料";
    if missing || text.is_empty() {
        draw_text(
            dc,
            sibling_font(item.hwndItem, FIELD_LABEL),
            c.muted,
            text,
            rect,
            DT_NOPREFIX,
        );
        return;
    }
    let font = font_of(item.hwndItem);
    let mut left = rect.left;
    if id == PARTS {
        // 拆分字根逐个放进字块，字根可能是扩展区字符，按 Unicode 标量切分。
        for part in String::from_utf16_lossy(text).chars() {
            let tile = RECT {
                left,
                top: rect.top,
                right: left + s(30),
                bottom: rect.top + s(30),
            };
            if tile.right > rect.right {
                break;
            }
            round(dc, &tile, s(6), None, c.line);
            let mut code = [0u16; 2];
            draw_text(
                dc,
                font,
                c.text,
                part.encode_utf16(&mut code),
                tile,
                DT_CENTER | DT_NOPREFIX,
            );
            left = tile.right + s(6);
        }
        return;
    }
    // 编码以空格分隔，各成一枚键帽。
    for code in text
        .split(|&unit| unit == u16::from(b' '))
        .filter(|code| !code.is_empty())
    {
        let chip = RECT {
            left,
            top: rect.top + (rect.bottom - rect.top - s(26)) / 2,
            right: left + extent(dc, font, code) + s(18),
            bottom: rect.top + (rect.bottom - rect.top + s(26)) / 2,
        };
        if chip.right > rect.right {
            break;
        }
        round(dc, &chip, s(5), Some(c.track), c.track);
        draw_text(dc, font, c.text, code, chip, DT_CENTER | DT_NOPREFIX);
        left = chip.right + s(6);
    }
}

/// 查字字块：选中为浅底加强调色描边；键盘焦点另画正文色环。
fn draw_chip(item: &DRAWITEMSTRUCT, c: &Colors) {
    let s = |value| scale(item.hwndItem, value);
    let rect = item.rcItem;
    let dc = item.hDC;
    fill(dc, &rect, c.window);
    if item.itemID == u32::MAX {
        return;
    }
    let mut buffer = [0u16; 64];
    // SAFETY: 先核对长度不超过缓冲区，再取本控件自己的字符串。
    let length = unsafe {
        let length = SendMessageW(item.hwndItem, LB_GETTEXTLEN, item.itemID as usize, 0);
        if !(0..buffer.len() as isize).contains(&length)
            || SendMessageW(
                item.hwndItem,
                LB_GETTEXT,
                item.itemID as usize,
                buffer.as_mut_ptr() as LPARAM,
            ) < 0
        {
            return;
        }
        length as usize
    };
    let chip = RECT {
        left: rect.left + s(3),
        top: rect.top + s(2),
        right: rect.right - s(3),
        bottom: rect.bottom - s(2),
    };
    let chosen = item.itemState & ODS_SELECTED != 0;
    if chosen {
        round(dc, &chip, s(7), Some(c.nav), c.nav);
        ring(dc, &chip, s(7), c.accent);
    }
    let disabled = item.itemState & ODS_DISABLED != 0;
    draw_text(
        dc,
        font_of(item.hwndItem),
        if disabled { c.faint } else { c.text },
        &buffer[..length],
        chip,
        DT_CENTER | DT_NOPREFIX,
    );
    if item.itemState & ODS_FOCUS != 0 && item.itemState & ODS_NOFOCUSRECT == 0 && !chosen {
        ring(dc, &chip, s(7), c.text);
    }
}

/// 外观区预览：按设置窗草稿现画一段带组合的文字和候选窗；草稿无效时用当前配色。
pub(super) fn draw_preview(item: &DRAWITEMSTRUCT, c: &Colors) {
    let s = |value| scale(item.hwndItem, value);
    let (dc, rect) = (item.hDC, item.rcItem);
    let parent = parent(item.hwndItem);
    let draft = read_options(parent).ok();
    let palette = draft.map_or_else(|| PALETTE.with(Cell::get), |o| o.palette());
    let face = draft.map_or(CandidateFont::default(), |o| o.candidate_font);
    let learning = checked_in(parent, LEARNING);
    fill(dc, &rect, c.window);
    round(dc, &rect, s(8), Some(c.sidebar), c.sidebar);
    round(dc, &rect, s(8), None, c.line);
    let body = TempFont::new(dc, &[UI_FACE], 15, 400);
    let mono = TempFont::new(dc, &MONO_FACES, 12, 400);
    // 注释的拆分与内嵌编码同用文楷；序号和其他独立编码保留原等宽字体。
    let hint_font = TempFont::new(dc, &LEARNING_FACES, 12, 400);
    let glyphs = TempFont::new(dc, &[font_face(face), UI_FACE], 19, 400);
    let lead = units("明天我们一起去");
    let line = RECT {
        left: rect.left + s(20),
        top: rect.top + s(14),
        right: rect.right - s(20),
        bottom: rect.top + s(36),
    };
    draw_text(dc, body.0, c.text, &lead, line, DT_NOPREFIX);
    let start = line.left + extent(dc, body.0, &lead);
    let code = units("ui");
    let composition = RECT {
        left: start,
        ..line
    };
    draw_text(dc, body.0, c.text, &code, composition, DT_NOPREFIX);
    let end = start + extent(dc, body.0, &code);
    // 组合下的点状下划线与插入符。
    let mut x = start;
    while x < end {
        let dot = RECT {
            left: x,
            top: line.bottom - s(2),
            right: x + s(1).max(1),
            bottom: line.bottom - s(1),
        };
        fill(dc, &dot, c.text);
        x += s(3);
    }
    let caret = RECT {
        left: end + s(1),
        top: line.top + s(2),
        right: end + s(1) + 1,
        bottom: line.bottom - s(2),
    };
    fill(dc, &caret, c.text);
    let rows = [
        ("1", "拉", "〔扌立 · uil〕"),
        ("2", "隹", "〔隹 · ui / uii〕"),
    ];
    let label_w = extent(dc, mono.0, &units("1"));
    let text_w = extent(dc, glyphs.0, &units("隹"));
    let comment_w = if learning {
        rows.iter()
            .map(|(.., comment)| extent(dc, hint_font.0, &units(comment)))
            .max()
            .unwrap_or(0)
            + s(8)
    } else {
        0
    };
    let window = RECT {
        left: start,
        top: line.bottom + s(6),
        right: (start + s(4 + 8) + label_w + s(8) + text_w + comment_w + s(14))
            .max(start + s(132))
            .min(rect.right - s(12)),
        bottom: line.bottom + s(6 + 4 + 31 * 2 + 3),
    };
    round(
        dc,
        &window,
        s(8),
        Some(rgb(palette.window)),
        rgb(palette.window),
    );
    round(dc, &window, s(8), None, rgb(palette.border));
    for (index, (label, text, comment)) in rows.into_iter().enumerate() {
        let band = RECT {
            left: window.left + s(4),
            top: window.top + s(4) + index as i32 * s(31),
            right: window.right - s(4),
            bottom: window.top + s(34) + index as i32 * s(31),
        };
        let chosen = index == 0;
        if chosen && palette.highlight != palette.window {
            round(
                dc,
                &band,
                s(4),
                Some(rgb(palette.highlight)),
                rgb(palette.highlight),
            );
        }
        let (label_ink, text_ink, hint_ink) = if chosen {
            (
                palette.highlight_text,
                palette.chosen,
                palette.highlight_text,
            )
        } else {
            (palette.label, palette.text, palette.muted)
        };
        let mut left = band.left + s(8);
        for (font, ink, words) in [
            (mono.0, label_ink, label),
            (glyphs.0, text_ink, text),
            (hint_font.0, hint_ink, if learning { comment } else { "" }),
        ] {
            let words = units(words);
            let cell = RECT { left, ..band };
            draw_text(dc, font, rgb(ink), &words, cell, DT_NOPREFIX);
            left += extent(dc, font, &words) + s(8);
        }
    }
}

/// 快捷键格：上为说明，下为键帽；“N”换成当前每页候选数。
fn draw_keys(item: &DRAWITEMSTRUCT, c: &Colors, index: usize) {
    let s = |value| scale(item.hwndItem, value);
    let (dc, rect) = (item.hDC, item.rcItem);
    fill(dc, &rect, c.surface);
    let (label, spec) = KEYMAP[index];
    let title = RECT {
        left: rect.left + s(15),
        top: rect.top + s(8),
        right: rect.right - s(8),
        bottom: rect.top + s(28),
    };
    draw_text(
        dc,
        font_of(item.hwndItem),
        c.text,
        &units(label),
        title,
        DT_NOPREFIX,
    );
    let size = page_size_in(parent(item.hwndItem)).to_string();
    let spec = spec.replace("[N]", &format!("[{size}]"));
    let mono = sibling_font(item.hwndItem, CUSTOM);
    let small = sibling_font(item.hwndItem, NOTICE);
    let mut left = title.left;
    let (top, bottom) = (rect.top + s(32), rect.top + s(54));
    for part in spec
        .split_inclusive(']')
        .flat_map(|part| match part.find('[') {
            Some(at) => vec![&part[..at], &part[at..]],
            None => vec![part],
        })
    {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(key) = part.strip_prefix('[').and_then(|key| key.strip_suffix(']')) {
            let key = units(key);
            let cap = RECT {
                left,
                top,
                right: left + (extent(dc, mono, &key) + s(12)).max(s(22)),
                bottom,
            };
            round(dc, &cap, s(5), Some(c.window), c.window);
            round(dc, &cap, s(5), None, c.field);
            draw_text(dc, mono, c.text, &key, cap, DT_CENTER | DT_NOPREFIX);
            left = cap.right + s(5);
        } else {
            let words = units(part);
            let cell = RECT {
                left,
                top,
                right: rect.right,
                bottom,
            };
            draw_text(dc, small, c.muted, &words, cell, DT_NOPREFIX);
            left += extent(dc, small, &words) + s(5);
        }
    }
}

/// 色板菜单项：圆角色块，悬停时画正文色环；底色同卡片，不用系统蓝色高亮。
fn draw_swatch(item: &DRAWITEMSTRUCT) {
    let c = colors();
    let dc = item.hDC;
    // SAFETY: 只读绘制 DC 的 DPI。
    let dpi = unsafe { GetDeviceCaps(dc, LOGPIXELSY as i32) }.max(96);
    let s = |value: i32| value * dpi / 96;
    let rect = item.rcItem;
    fill(dc, &rect, c.surface);
    let chip = inset(&rect, s(4));
    let value = rgb(item.itemData as u32);
    round(dc, &chip, s(5), Some(value), value);
    round(dc, &chip, s(5), None, c.line);
    if item.itemState & ODS_SELECTED != 0 {
        ring(dc, &inset(&rect, s(1)), s(7), c.text);
    }
}

/// 当前分区：侧栏单选中被选中的一项。
pub(super) fn current_page(hwnd: HWND) -> usize {
    pick(hwnd, NAV, PAGES.len()).unwrap_or(0)
}
/// 设置窗滚动位置（96 DPI 逻辑像素），存于窗口数据：值类型，不是指针。
pub(super) fn scroll_offset(hwnd: HWND) -> i32 {
    // SAFETY: 只读本线程窗口的用户数据整数。
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as i32 }
}
pub(super) fn set_scroll_offset(hwnd: HWND, offset: i32) {
    // SAFETY: 只写本线程窗口的用户数据整数。
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, offset as _) };
}
/// 滚动到 position（逻辑像素）：按滚动条记录的范围夹紧，改了才重新布局。
pub(super) fn scroll_to(hwnd: HWND, position: i32) {
    let mut info = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_ALL,
        ..Default::default()
    };
    // SAFETY: 输出结构大小已填，只读本窗口滚动条。
    if unsafe { GetScrollInfo(hwnd, SB_VERT, &mut info) } == 0 {
        return;
    }
    let max = (info.nMax - info.nPage as i32 + 1).max(0);
    let position = position.clamp(0, max);
    if position != scroll_offset(hwnd) {
        set_scroll_offset(hwnd, position);
        post(hwnd, LAYOUT, 0);
    }
}

/// 卡片内的控件用卡片底色，说明类文字用次要色。
fn on_surface(id: usize) -> bool {
    matches!(
        id,
        QUERY | GLYPH | CODEPOINT | COPY | STATUS | CODES
            ..=PARTS
                | OPEN_CONFIG
                | RELOAD
                | RESET
                | CUSTOM_STATUS
                | FILE_NAME
                | FILE_PATH
                | ABOUT_TITLE
                | ABOUT_DESC
    ) || (FIELD_LABEL..FIELD_LABEL + 4).contains(&id)
        || (ROW_TITLE..ROW_TITLE + 5).contains(&id)
        || (ROW_DESC..ROW_DESC + 5).contains(&id)
        || (CUSTOM..CUSTOM + 4).contains(&id)
}
pub(super) fn muted(id: usize) -> bool {
    matches!(
        id,
        QUERY_LABEL
            | STATUS
            | PAGE
            | CODEPOINT
            | NOTICE
            | CUSTOM_STATUS
            | FILE_PATH
            | ABOUT_DESC
            | KEYS_NOTE
    ) || (FIELD_LABEL..FIELD_LABEL + 4).contains(&id)
        || (ROW_DESC..ROW_DESC + 5).contains(&id)
        || (GROUP..GROUP + GROUPS.len()).contains(&id)
}

/// 父窗口自绘底色、侧栏、卡片、轨道、输入框外框与分隔线；全部由客户区尺寸与控件状态现算。
fn paint_window(hwnd: HWND) {
    let c = colors();
    // SAFETY: WM_PAINT 内配对 BeginPaint/EndPaint；只查询本窗口子控件。
    unsafe {
        let mut paint: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(hwnd, &mut paint);
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client);
        fill(dc, &client, c.window);
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        let s = |value: i32| value * dpi / 96;
        let query = GetDlgItem(hwnd, QUERY as i32);
        if !query.is_null() {
            let r = |[x, y, w, h]: Box4| RECT {
                left: s(x),
                top: s(y),
                right: s(x + w),
                bottom: s(y + h),
            };
            let spec = lookup_spec(client.right * 96 / dpi, client.bottom * 96 / dpi);
            let card = r(spec.card);
            round(dc, &card, s(10), Some(c.surface), c.surface);
            round(dc, &card, s(10), None, c.line);
            let field = r(spec.field);
            round(dc, &field, s(8), Some(c.surface), c.surface);
            round(
                dc,
                &field,
                s(8),
                None,
                if GetFocus() == query {
                    c.accent
                } else {
                    c.field
                },
            );
            let icon = RECT {
                left: field.left + s(10),
                right: field.left + s(30),
                ..field
            };
            draw_icon(dc, 0xE721, 15, c.muted, icon);
            let mono = TempFont::new(dc, &MONO_FACES, 11, 400);
            let hint = units("Enter");
            let width = extent(dc, mono.0, &hint) + s(12);
            let cap = RECT {
                left: field.right - s(8) - width,
                top: field.top + s(9),
                right: field.right - s(8),
                bottom: field.bottom - s(9),
            };
            round(dc, &cap, s(4), Some(c.track), c.track);
            draw_text(dc, mono.0, c.muted, &hint, cap, DT_CENTER | DT_NOPREFIX);
        } else {
            let sidebar = RECT {
                right: s(SIDEBAR),
                ..client
            };
            fill(dc, &sidebar, c.sidebar);
            let offset = scroll_offset(hwnd);
            let r = |[x, y, w, h]: Box4| RECT {
                left: s(x),
                top: s(y - offset),
                right: s(x + w),
                bottom: s(y - offset + h),
            };
            let spec = settings_spec(current_page(hwnd), checked_in(hwnd, THEME + 4));
            for card in &spec.cards {
                round(dc, &r(*card), s(8), Some(c.surface), c.surface);
                round(dc, &r(*card), s(8), None, c.line);
            }
            for line in &spec.lines {
                let mut line = r(*line);
                line.right = line.right.max(line.left + 1);
                line.bottom = line.bottom.max(line.top + 1);
                fill(dc, &line, c.line);
            }
            for track in &spec.tracks {
                round(dc, &r(*track), s(7), Some(c.track), c.track);
            }
            let focus = GetFocus();
            for (k, frame) in spec.frames.iter().enumerate() {
                let edit = GetDlgItem(hwnd, (CUSTOM + k) as i32);
                round(dc, &r(*frame), s(6), Some(c.surface), c.surface);
                round(
                    dc,
                    &r(*frame),
                    s(6),
                    None,
                    if focus == edit { c.accent } else { c.field },
                );
            }
            if let Some(tile) = spec.tile {
                let tile = r(tile);
                round(dc, &tile, s(8), Some(c.track), c.track);
                draw_icon(dc, 0xE8A5, 18, c.accent, tile);
            }
            if let Some(place) = spec.seal {
                let place = r(place);
                let side = place.right - place.left;
                if let Some(icon) = seal::icon(side) {
                    DrawIconEx(
                        dc,
                        place.left,
                        place.top,
                        icon,
                        side,
                        side,
                        0,
                        null_mut(),
                        DI_NORMAL,
                    );
                    DestroyIcon(icon);
                }
            }
        }
        EndPaint(hwnd, &paint);
    }
}

// SAFETY: Windows 在所属 UI 线程调用；不解引用业务指针、不使用业务状态，不执行可能 panic 的 Rust 操作。
pub(super) unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    // SAFETY: 只转发值类型通知；WM_DPICHANGED 的 RECT、自绘结构仅在本次系统回调内读取，绝不排队裸指针。
    unsafe {
        match message {
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                paint_window(hwnd);
                0
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
                let c = colors();
                let id = GetDlgCtrlID(l as HWND) as usize;
                let back = if (NAV..NAV + PAGES.len()).contains(&id) || id == NOTICE {
                    c.sidebar
                } else if on_surface(id) {
                    c.surface
                } else {
                    c.window
                };
                let dc = w as HDC;
                let fore = if (GROUP..GROUP + GROUPS.len()).contains(&id) {
                    c.label
                } else if muted(id) {
                    c.muted
                } else {
                    c.text
                };
                SetTextColor(dc, fore);
                SetBkColor(dc, back);
                SetDCBrushColor(dc, back);
                GetStockObject(DC_BRUSH) as LRESULT
            }
            WM_MEASUREITEM if l != 0 => {
                let item = &mut *(l as *mut MEASUREITEMSTRUCT);
                let dpi = GetDpiForWindow(hwnd).max(96);
                match item.CtlType {
                    ODT_LISTBOX => item.itemHeight = ITEM_HEIGHT * dpi / 96,
                    ODT_MENU => {
                        item.itemWidth = 30 * dpi / 96;
                        item.itemHeight = 30 * dpi / 96;
                    }
                    _ => return DefWindowProcW(hwnd, message, w, l),
                }
                1
            }
            WM_DRAWITEM if l != 0 => {
                let item = &*(l as *const DRAWITEMSTRUCT);
                match item.CtlType {
                    ODT_LISTBOX | ODT_STATIC => draw_item(item),
                    ODT_MENU => draw_swatch(item),
                    _ => return DefWindowProcW(hwnd, message, w, l),
                }
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
            WM_ENTERMENULOOP | WM_ENTERSIZEMOVE => {
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
            // 设置窗内容超出时纵向滚动；滚动条与滚轮都只改滚动位置，再交给事件循环重新布局。
            WM_VSCROLL => {
                let mut info = SCROLLINFO {
                    cbSize: size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_ALL,
                    ..Default::default()
                };
                GetScrollInfo(hwnd, SB_VERT, &mut info);
                let position = match (w & 0xFFFF) as i32 {
                    SB_LINEUP => info.nPos - 40,
                    SB_LINEDOWN => info.nPos + 40,
                    SB_PAGEUP => info.nPos - info.nPage as i32,
                    SB_PAGEDOWN => info.nPos + info.nPage as i32,
                    SB_THUMBTRACK | SB_THUMBPOSITION => info.nTrackPos,
                    SB_TOP => 0,
                    SB_BOTTOM => info.nMax,
                    _ => info.nPos,
                };
                scroll_to(hwnd, position);
                0
            }
            WM_MOUSEWHEEL => {
                let delta = (w >> 16 & 0xFFFF) as u16 as i16 as i32;
                scroll_to(hwnd, scroll_offset(hwnd) - delta * 48 / 120);
                0
            }
            // 拖动缩放期间系统模态循环会吞掉投递的布局消息，结束时补发一次。
            WM_SIZE | WM_EXITSIZEMOVE => {
                if message == WM_EXITSIZEMOVE {
                    modal_exit();
                }
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
            // 只限制可缩放的查字窗；固定尺寸的设置窗不受此下限撑宽。
            WM_GETMINMAXINFO => {
                if l != 0 && GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_THICKFRAME != 0 {
                    let info = &mut *(l as *mut MINMAXINFO);
                    let dpi = GetDpiForWindow(hwnd) as i32;
                    let (width, height) = LOOKUP_MIN;
                    info.ptMinTrackSize.x = (width + 16) * dpi / 96;
                    info.ptMinTrackSize.y = (height + 39) * dpi / 96;
                }
                0
            }
            _ => DefWindowProcW(hwnd, message, w, l),
        }
    }
}
