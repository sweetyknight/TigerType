//! 查字/设置窗口的构建及共用状态机到控件的映射；只返回语义请求，由事件循环发送。
use super::controls::*;
use super::drawing::*;
use super::window::*;
use super::*;

pub(super) fn lookup_window(class: &Class, options: &Options) -> Result<Window, String> {
    // 长词资料超出窗口时整窗纵向滚动；滚动条只在需要时出现。
    let style = (WS_OVERLAPPEDWINDOW & !WS_MAXIMIZEBOX) | WS_VSCROLL;
    let mut window = Window::create(&class.0, LOOKUP_TITLE, null_mut(), style)?;
    // 零尺寸标签只为读屏名称与 Alt+Q；查询框里另画占位提示。
    window.add(QUERY_LABEL, "STATIC", &view::text(QUERY_LABEL, 0), 0)?;
    window.add(QUERY, "EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL as u32)?;
    // 粘贴上限保留足够余量；超过业务上限明确报错，不把截断文本当成有效查询。
    window.send(QUERY, EM_SETLIMITTEXT, 4096, 0);
    window.add(
        LIST,
        "LISTBOX",
        "",
        WS_TABSTOP
            | (LBS_OWNERDRAWFIXED
                | LBS_HASSTRINGS
                | LBS_NOTIFY
                | LBS_NOINTEGRALHEIGHT
                | LBS_MULTICOLUMN) as u32,
    )?;
    window.add(PREVIOUS, "BUTTON", &view::text(PREVIOUS, 0), WS_TABSTOP)?;
    window.add(PAGE, "STATIC", "", SS_CENTER | SS_CENTERIMAGE)?;
    window.add(NEXT, "BUTTON", &view::text(NEXT, 0), WS_TABSTOP)?;
    window.add(GLYPH, "STATIC", "", SS_OWNERDRAW)?;
    window.add(CODEPOINT, "STATIC", "", SS_CENTER)?;
    window.add(COPY, "BUTTON", &view::text(COPY, 0), WS_TABSTOP)?;
    for (index, value) in [CODES, FULL, READING, PARTS].into_iter().enumerate() {
        window.add(
            FIELD_LABEL + index,
            "STATIC",
            &view::text(FIELD_LABEL + index, 0),
            SS_CENTERIMAGE | SS_NOPREFIX,
        )?;
        // 四项都自绘：读音也要按共用布局折行，行高才与布局一致。
        window.add(value, "STATIC", "", SS_OWNERDRAW)?;
    }
    window.add(STATUS, "STATIC", "", SS_CENTER | SS_NOPREFIX)?;
    window.add(SETTINGS, "BUTTON", &view::text(SETTINGS, 0), WS_TABSTOP)?;
    // 隐藏控件不显示、不可聚焦，只承载当前字的资料供标准复制。
    window.add(CLIP, "EDIT", "", ES_MULTILINE as u32 | ES_READONLY as u32)?;
    window.show(CLIP, false);
    window.decorate(options.palette(), options.palette().window);
    window.layout(options.candidate_font)?;
    window.set_icons();
    size_client(&window, LOOKUP_SIZE, style & !WS_VSCROLL);
    initialize_cues(window.handle);
    Ok(window)
}
pub(super) fn settings_window(
    class: &Class,
    owner: HWND,
    options: &Options,
    settings_path: &Path,
) -> Result<Window, String> {
    let style = WS_CAPTION | WS_SYSMENU | WS_VSCROLL;
    let mut window = Window::create(&class.0, SETTINGS_TITLE, owner, style)?;
    for k in 0..PAGES.len() {
        let group = if k == 0 { WS_GROUP | WS_TABSTOP } else { 0 };
        let name = view::text(NAV + k, 0);
        window.add(NAV + k, "BUTTON", &name, group | BS_AUTORADIOBUTTON as u32)?;
    }
    // 侧栏底部只在出错时显示原因，平时留空。
    window.add(NOTICE, "STATIC", "", SS_NOPREFIX)?;
    window.add(HEADING, "STATIC", &view::text(HEADING, 0), SS_NOPREFIX)?;
    for k in 0..GROUPS.len() {
        window.add(GROUP + k, "STATIC", &view::text(GROUP + k, 0), SS_NOPREFIX)?;
    }
    let choice = |window: &mut Window, row: usize| -> Result<(), String> {
        let (_, _, first, count, _) = CHOICES[row];
        let title = view::text(ROW_TITLE + row, 0);
        window.add(ROW_TITLE + row, "STATIC", &title, SS_CENTERIMAGE)?;
        let description = view::text(ROW_DESC + row, 0);
        window.add(ROW_DESC + row, "STATIC", &description, SS_NOPREFIX)?;
        for k in 0..count {
            // 每组首项开新组并作 Tab 停靠；组内方向键切换，选中项随之勾选。
            let style = if k == 0 { WS_GROUP | WS_TABSTOP } else { 0 };
            let name = view::text(first + k, 0);
            window.add(
                first + k,
                "BUTTON",
                &name,
                style | BS_AUTORADIOBUTTON as u32,
            )?;
        }
        Ok(())
    };
    let toggle = |window: &mut Window, id: usize| -> Result<(), String> {
        let title = view::text(id, 0);
        window.add(
            id,
            "BUTTON",
            &title,
            WS_TABSTOP | WS_GROUP | BS_AUTOCHECKBOX as u32,
        )
    };
    // 输入
    choice(&mut window, 0)?;
    toggle(&mut window, COMMON)?;
    toggle(&mut window, AUTO)?;
    choice(&mut window, 1)?;
    // 外观
    window.add(PREVIEW, "STATIC", "候选窗预览", SS_OWNERDRAW)?;
    choice(&mut window, 2)?;
    choice(&mut window, 3)?;
    toggle(&mut window, LEARNING)?;
    for k in 0..Theme::ALL.len() {
        let style = if k == 0 { WS_GROUP | WS_TABSTOP } else { 0 };
        let name = view::text(THEME + k, 0);
        window.add(
            THEME + k,
            "BUTTON",
            &name,
            style | BS_AUTORADIOBUTTON as u32,
        )?;
    }
    for k in 0..COLORS.len() {
        let name = view::text(SWATCH + k, 0);
        window.add(SWATCH + k, "BUTTON", &name, WS_GROUP | WS_TABSTOP)?;
        window.add(
            CUSTOM + k,
            "EDIT",
            "",
            WS_GROUP | WS_TABSTOP | ES_UPPERCASE as u32,
        )?;
        window.send(CUSTOM + k, EM_SETLIMITTEXT, 7, 0);
    }
    window.add(CUSTOM_STATUS, "STATIC", "", SS_CENTERIMAGE | SS_NOPREFIX)?;
    window.add(RESET, "BUTTON", RESET_TEXT, WS_GROUP | WS_TABSTOP)?;
    // 快捷键
    window.add(KEYS_NOTE, "STATIC", KEYS_NOTE_TEXT, SS_NOPREFIX)?;
    for (k, (label, spec)) in KEYMAP.iter().enumerate() {
        // 读屏朗读去掉键帽括号的文字，画面由自绘给出。
        let spoken = view::key_speech(label, spec);
        window.add(KEYS + k, "STATIC", &spoken, SS_OWNERDRAW)?;
    }
    // 高级
    window.add(FILE_NAME, "STATIC", "settings.toml", SS_NOPREFIX)?;
    window.add(
        FILE_PATH,
        "STATIC",
        &settings_path.display().to_string(),
        SS_NOPREFIX | SS_PATHELLIPSIS,
    )?;
    window.add(OPEN_CONFIG, "BUTTON", OPEN_TEXT, WS_GROUP | WS_TABSTOP)?;
    window.add(ROW_TITLE + 4, "STATIC", RELOAD_TITLE, SS_NOPREFIX)?;
    window.add(ROW_DESC + 4, "STATIC", RELOAD_DESC, SS_NOPREFIX)?;
    window.add(RELOAD, "BUTTON", RELOAD_TEXT, WS_GROUP | WS_TABSTOP)?;
    window.add(ABOUT_TITLE, "STATIC", ABOUT_NAME, SS_NOPREFIX)?;
    window.add(ABOUT_DESC, "STATIC", &about_description(), SS_NOPREFIX)?;
    check(&window, NAV, true);
    window.decorate(options.palette(), shades(options.palette()).sidebar);
    window.layout(options.candidate_font)?;
    window.set_icons();
    size_client(&window, SETTINGS_SIZE, style & !WS_VSCROLL);
    initialize_cues(window.handle);
    Ok(window)
}

/// 用系统默认编辑器打开受管配置文件；文件不存在时返回可读错误，不创建或覆盖配置。
pub(super) fn open_config(path: &Path) -> Result<(), String> {
    let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    let target: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: 两个 UTF-16 缓冲区在调用期间有效且以 NUL 结尾；ShellExecuteW 不转移其所有权。
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    } as usize;
    if result <= 32 {
        Err("无法打开 settings.toml，请确认系统有可用的文本编辑器".into())
    } else {
        Ok(())
    }
}
pub(super) fn check(window: &Window, id: usize, on: bool) {
    window.send(
        id,
        BM_SETCHECK,
        if on { BST_CHECKED } else { BST_UNCHECKED } as usize,
        0,
    );
}
/// 设置窗会改动草稿的控件：开关、单选、主题与侧栏分区。
fn draft_controls() -> impl Iterator<Item = usize> {
    [AUTO, COMMON, LEARNING]
        .into_iter()
        .chain(MODE..MODE + TypingMode::ALL.len())
        .chain(FORM..FORM + FORMS.len())
        .chain(FONT..FONT + CandidateFont::ALL.len())
        .chain(SIZE_CHOICE..SIZE_CHOICE + Options::PAGE_SIZES.count())
        .chain(THEME..THEME + Theme::ALL.len())
        .chain(NAV..NAV + PAGES.len())
}
/// 控件显示共用草稿：开关与单选按草稿勾选。控件不再是草稿本身，只是它的画面；自绘仍按控件状态取色。
/// 色值逐框比较动作前的草稿，只回写发生变化的那一框，避免吞掉其他框尚在排队的编辑；
/// 初次显示和明确丢弃草稿时传 None；重新载入期间拒收编辑，也须恢复全部只读草稿。
pub(super) fn sync_settings(window: &Window, editor: &Editor, before: Option<&[String; 4]>) {
    for id in draft_controls() {
        check(window, id, view::selected(editor, id));
    }
    for (k, value) in editor.colors.iter().enumerate().filter(|(k, value)| {
        editor.model.reloading() || before.is_none_or(|before| before[*k] != **value)
    }) {
        let current = String::from_utf16_lossy(&control_text(window.control(CUSTOM + k)));
        if current != *value {
            window.text(CUSTOM + k, value);
        }
    }
}
/// 查询框全文；业务上限与空查询由共用状态机判定，不在此截断，避免把截断文本当成有效查询。
pub(super) fn query(window: &Window) -> String {
    String::from_utf16_lossy(&control_text(window.control(QUERY)))
}
/// 换上新查询页：字块逐个列出，列宽容得下最长的词，默认选中首项。
pub(super) fn show_page(main: &Window, editor: &Editor) {
    main.send(LIST, LB_RESETCONTENT, 0, 0);
    let items = editor
        .model
        .result
        .as_ref()
        .map_or(&[][..], |page| &page.items[..]);
    let list = main.control(LIST);
    let mut widest = 0;
    // SAFETY: 取列表 DC 只用于测量，结束时归还。
    let dc = unsafe { GetDC(list) };
    for item in items {
        let text = wide(&item.text);
        main.send(LIST, LB_ADDSTRING, 0, text.as_ptr() as LPARAM);
        widest = widest.max(extent(dc, font_of(list), &units(&item.text)));
    }
    // SAFETY: 归还上面取得的 DC。
    unsafe { ReleaseDC(list, dc) };
    let dpi = main.dpi.max(96) as i32;
    let column = (CHIP * dpi / 96).max(widest + 18 * dpi / 96);
    main.send(LIST, LB_SETCOLUMNWIDTH, column as usize, 0);
    main.send(
        LIST,
        LB_SETCURSEL,
        if items.is_empty() {
            usize::MAX
        } else {
            editor.selected
        },
        0,
    );
    show_detail(main, editor);
}
/// 详情卡：有选中字时显示资料，否则显示提示（空态、无结果或出错）。
pub(super) fn show_detail(main: &Window, editor: &Editor) {
    if let Some(item) = editor.item() {
        let [codes, full, reading, parts] = view::detail_texts(item);
        main.text(GLYPH, &item.text);
        main.text(CODEPOINT, &view::codepoint(item));
        main.text(CODES, &codes);
        main.text(FULL, &full);
        main.text(READING, &reading);
        main.text(PARTS, &parts);
    }
    for id in [GLYPH, CODEPOINT, COPY, CODES, FULL, READING, PARTS, STATUS]
        .into_iter()
        .chain(FIELD_LABEL..FIELD_LABEL + 4)
    {
        main.show(id, view::lookup_visible(editor, id));
    }
    // 行高随资料折行而变，换字后立即重排。
    main.layout_lookup();
}
/// 查询框始终可编辑：边输边查时在途请求不能禁用它，否则正在打字的焦点会丢。
/// 设置控件在重新载入期间也不停用（停用会夺走键盘焦点）；其间的编辑由共用状态机拒收，控件随后按草稿复原。
pub(super) fn refresh(main: &Window, settings: &Window, editor: &Editor) {
    for id in [PREVIOUS, NEXT, COPY] {
        main.enable(id, view::enabled(editor, id));
    }
    main.text(STATUS, &editor.model.notice);
    let page = view::lookup_value(editor, PAGE, &Default::default());
    main.text(PAGE, &page.unwrap_or_default());
    settings.text(NOTICE, &editor.model.settings_notice);
    settings.text(CUSTOM_STATUS, &view::custom_status(editor));
}
/// 确认的配色/字体用于两个窗口：换绘制配色、标题栏与字体后整体重画。
pub(super) fn apply_look(
    main: &mut Window,
    settings: &mut Window,
    options: &Options,
) -> Result<(), String> {
    let palette = options.palette();
    PALETTE.with(|current| current.set(palette));
    main.decorate(palette, palette.window);
    settings.decorate(palette, shades(palette).sidebar);
    main.layout(options.candidate_font)?;
    settings.layout(options.candidate_font)
}
/// 切换设置分区：回到顶部重新布局；焦点若落在已隐藏的控件上，移到侧栏当前项。
pub(super) fn select_page(
    settings: &mut Window,
    page: usize,
    face: CandidateFont,
) -> Result<(), String> {
    for k in 0..PAGES.len() {
        check(settings, NAV + k, k == page);
    }
    settings.text(HEADING, &view::text(HEADING, page));
    set_scroll_offset(settings.handle, 0);
    settings.layout(face)?;
    let nav = settings.control(NAV + page);
    // SAFETY: 只查询并改变本线程窗口焦点。
    unsafe {
        let focus = GetFocus();
        if !focus.is_null() && GetParent(focus) == settings.handle && IsWindowVisible(focus) == 0 {
            SetFocus(nav);
        }
    }
    Ok(())
}
/// 色块按钮下方弹出无蓝色板（8 列色相 × 4 档明度的自绘菜单），返回所选颜色；取消为 None。
pub(super) fn palette_menu(settings: &Window, k: usize) -> Option<u32> {
    let current = hex_in(settings.handle, k).ok();
    // SAFETY: 菜单与画刷在本函数内创建并释放；文字缓冲在插入调用期间有效（菜单复制文字）。
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return None;
        }
        let mut position = 0;
        for column in 0..8 {
            for row in 0..4 {
                let index = row * 8 + column;
                let color = SWATCHES[index];
                let mut text = wide(&format!("#{color:06X}"));
                let item = MENUITEMINFOW {
                    cbSize: size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_ID | MIIM_STRING | MIIM_FTYPE | MIIM_DATA | MIIM_STATE,
                    fType: MFT_OWNERDRAW
                        | if row == 0 && column > 0 {
                            MFT_MENUBREAK
                        } else {
                            0
                        },
                    fState: if current == Some(color) {
                        MFS_CHECKED
                    } else {
                        0
                    },
                    wID: index as u32 + 1,
                    dwItemData: color as usize,
                    dwTypeData: text.as_mut_ptr(),
                    ..Default::default()
                };
                InsertMenuItemW(menu, position, 1, &item);
                position += 1;
            }
        }
        let brush = CreateSolidBrush(colors().surface);
        SetMenuInfo(
            menu,
            &MENUINFO {
                cbSize: size_of::<MENUINFO>() as u32,
                fMask: MIM_BACKGROUND | MIM_STYLE,
                dwStyle: MNS_NOCHECK,
                hbrBack: brush,
                ..Default::default()
            },
        );
        let mut rect = RECT::default();
        GetWindowRect(settings.control(SWATCH + k), &mut rect);
        let chosen = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
            rect.left,
            rect.bottom,
            settings.handle,
            null(),
        ) as usize;
        DestroyMenu(menu);
        DeleteObject(brush);
        chosen
            .checked_sub(1)
            .and_then(|index| SWATCHES.get(index).copied())
    }
}
/// 设置窗的命令：切换分区、草稿编辑、色板、打开与重新载入配置文件。保存规则全在共用状态机。
pub(super) fn settings_action(
    settings: &mut Window,
    editor: &mut Editor,
    id: usize,
    notification: u32,
    settings_path: &Path,
) -> Result<Option<Request>, String> {
    let (face, colors) = (editor.model.options.candidate_font, editor.colors.clone());
    let action = match notification {
        EN_CHANGE if (CUSTOM..CUSTOM + COLORS.len()).contains(&id) => {
            let value = String::from_utf16_lossy(&control_text(settings.control(id)));
            Action::Color(id - CUSTOM, value)
        }
        BN_CLICKED if (NAV..NAV + PAGES.len()).contains(&id) => {
            editor.act(Action::Page(id - NAV));
            select_page(settings, editor.page, face)?;
            return Ok(None);
        }
        BN_CLICKED if (SWATCH..SWATCH + COLORS.len()).contains(&id) => {
            match palette_menu(settings, id - SWATCH) {
                Some(color) => Action::Color(id - SWATCH, format!("#{color:06X}")),
                None => return Ok(None),
            }
        }
        BN_CLICKED => match id {
            RESET => Action::Reset,
            OPEN_CONFIG => Action::Open,
            RELOAD => Action::Reload,
            _ => match Setting::of(id) {
                Some(setting) => Action::Set(setting),
                None => return Ok(None),
            },
        },
        _ => return Ok(None),
    };
    let request = match editor.act(action) {
        Some(Task::Send(request)) => Some(request),
        Some(Task::OpenConfig) => {
            editor.model.settings_notice = match open_config(settings_path) {
                Ok(()) => "已用默认编辑器打开 settings.toml".into(),
                Err(error) => error,
            };
            None
        }
        _ => None,
    };
    sync_settings(settings, editor, Some(&colors));
    if (THEME..THEME + Theme::ALL.len()).contains(&id) {
        // 选不选自定义决定是否露出四色编辑行。
        settings.layout(face)?;
    } else {
        repaint(settings);
    }
    Ok(request)
}
