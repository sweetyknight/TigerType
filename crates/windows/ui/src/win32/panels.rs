//! 查字/设置窗口的构建及 Model 到控件的映射；只返回语义请求，由事件循环发送。
use super::controls::*;
use super::drawing::*;
use super::window::*;
use super::*;

pub(super) fn lookup_window(class: &Class, options: &Options) -> Result<Window, String> {
    let style = WS_OVERLAPPEDWINDOW & !WS_MAXIMIZEBOX;
    let mut window = Window::create(&class.0, LOOKUP_TITLE, null_mut(), style)?;
    // 零尺寸标签只为读屏名称与 Alt+Q；查询框里另画占位提示。
    window.add(QUERY_LABEL, "STATIC", "汉字或拼音(&Q)", 0)?;
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
    window.add(PREVIOUS, "BUTTON", "上一页(&P)", WS_TABSTOP)?;
    window.add(PAGE, "STATIC", "", SS_CENTER | SS_CENTERIMAGE)?;
    window.add(NEXT, "BUTTON", "下一页(&N)", WS_TABSTOP)?;
    window.add(GLYPH, "STATIC", "", SS_OWNERDRAW)?;
    window.add(CODEPOINT, "STATIC", "", SS_CENTER)?;
    window.add(COPY, "BUTTON", "复制(&C)", WS_TABSTOP)?;
    for (index, (value, label)) in [
        (CODES, "可用编码"),
        (FULL, "完整编码"),
        (READING, "读音"),
        (PARTS, "拆分"),
    ]
    .into_iter()
    .enumerate()
    {
        window.add(
            FIELD_LABEL + index,
            "STATIC",
            label,
            SS_CENTERIMAGE | SS_NOPREFIX,
        )?;
        let style = if value == READING {
            SS_CENTERIMAGE | SS_NOPREFIX
        } else {
            SS_OWNERDRAW
        };
        window.add(value, "STATIC", "", style)?;
    }
    window.add(STATUS, "STATIC", "", SS_CENTER | SS_NOPREFIX)?;
    window.add(SETTINGS, "BUTTON", "设置(&S)", WS_TABSTOP)?;
    // 隐藏控件不显示、不可聚焦，只承载当前字的资料供标准复制。
    window.add(CLIP, "EDIT", "", ES_MULTILINE as u32 | ES_READONLY as u32)?;
    window.show(CLIP, false);
    window.decorate(options.palette(), options.palette().window);
    window.layout(options.candidate_font)?;
    window.set_icons();
    size_client(&window, LOOKUP_SIZE, style);
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
    for (k, (name, _)) in PAGES.iter().enumerate() {
        let group = if k == 0 { WS_GROUP | WS_TABSTOP } else { 0 };
        window.add(NAV + k, "BUTTON", name, group | BS_AUTORADIOBUTTON as u32)?;
    }
    // 侧栏底部只在出错时显示原因，平时留空。
    window.add(NOTICE, "STATIC", "", SS_NOPREFIX)?;
    window.add(HEADING, "STATIC", PAGES[0].0, SS_NOPREFIX)?;
    for (k, name) in GROUPS.iter().enumerate() {
        window.add(GROUP + k, "STATIC", name, SS_NOPREFIX)?;
    }
    let choice = |window: &mut Window, row: usize| -> Result<(), String> {
        let (title, description, first, count, _) = CHOICES[row];
        window.add(ROW_TITLE + row, "STATIC", title, SS_CENTERIMAGE)?;
        window.add(ROW_DESC + row, "STATIC", description, SS_NOPREFIX)?;
        let names: Vec<String> = match first {
            MODE => TypingMode::ALL.iter().map(|m| m.name().into()).collect(),
            FORM => FORMS.iter().map(|(_, name)| (*name).into()).collect(),
            FONT => CandidateFont::ALL.iter().map(|f| f.name().into()).collect(),
            _ => Options::PAGE_SIZES.map(|n| n.to_string()).collect(),
        };
        debug_assert_eq!(names.len(), count);
        for (k, name) in names.iter().enumerate() {
            // 每组首项开新组并作 Tab 停靠；组内方向键切换，选中项随之勾选。
            let style = if k == 0 { WS_GROUP | WS_TABSTOP } else { 0 };
            window.add(first + k, "BUTTON", name, style | BS_AUTORADIOBUTTON as u32)?;
        }
        Ok(())
    };
    let toggle = |window: &mut Window, id: usize| -> Result<(), String> {
        let title = TOGGLES
            .iter()
            .find(|(key, ..)| *key == id)
            .map_or("", |(_, t, _)| *t);
        window.add(
            id,
            "BUTTON",
            title,
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
    for (k, theme) in Theme::ALL.iter().enumerate() {
        let style = if k == 0 { WS_GROUP | WS_TABSTOP } else { 0 };
        window.add(
            THEME + k,
            "BUTTON",
            theme.name(),
            style | BS_AUTORADIOBUTTON as u32,
        )?;
    }
    for (k, (name, _)) in COLORS.iter().enumerate() {
        window.add(
            SWATCH + k,
            "BUTTON",
            &format!("{name}色"),
            WS_GROUP | WS_TABSTOP,
        )?;
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
        let spoken = format!("{label}：{}", spec.replace(['[', ']'], ""));
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
pub(super) fn set_custom(window: &Window, custom: CustomColors) {
    for (k, color) in [custom.window, custom.text, custom.highlight, custom.accent]
        .into_iter()
        .enumerate()
    {
        window.text(CUSTOM + k, &format!("#{color:06X}"));
    }
}
pub(super) fn set_options(window: &Window, options: Options) {
    check(window, AUTO, options.auto_commit_four);
    check(window, COMMON, options.common_only);
    check(window, LEARNING, options.show_learning);
    for (k, (form, _)) in FORMS.iter().enumerate() {
        check(window, FORM + k, *form == options.output_form);
    }
    for (k, font) in CandidateFont::ALL.iter().enumerate() {
        check(window, FONT + k, *font == options.candidate_font);
    }
    for (k, theme) in Theme::ALL.iter().enumerate() {
        check(window, THEME + k, *theme == options.theme);
    }
    for (k, mode) in TypingMode::ALL.iter().enumerate() {
        check(window, MODE + k, *mode == options.typing_mode);
    }
    for (k, size) in Options::PAGE_SIZES.enumerate() {
        check(window, SIZE_CHOICE + k, size == options.page_size);
    }
    set_custom(window, options.custom);
}
pub(super) fn query(window: &Window) -> Result<String, String> {
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
pub(super) fn selected<'a>(
    main: &Window,
    model: &'a Model,
) -> Option<&'a tigertype_contracts::LookupItem> {
    let index = main.send(LIST, LB_GETCURSEL, 0, 0);
    model
        .result
        .as_ref()
        .and_then(|page| page.items.get(usize::try_from(index).ok()?))
}
/// 换上新查询页：字块逐个列出，列宽容得下最长的词，默认选中首项。
pub(super) fn show_page(main: &Window, model: &Model) {
    main.send(LIST, LB_RESETCONTENT, 0, 0);
    let items = model
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
        if items.is_empty() { usize::MAX } else { 0 },
        0,
    );
    show_detail(main, model);
}
/// 详情卡：有选中字时显示资料，否则显示提示（空态、无结果或出错）。
pub(super) fn show_detail(main: &Window, model: &Model) {
    let item = selected(main, model);
    if let Some(item) = item {
        let join = |items: &[String], separator: &str| {
            if items.is_empty() {
                "缺资料".to_owned()
            } else {
                items.join(separator)
            }
        };
        let mut chars = item.text.chars();
        let point = match (chars.next(), chars.next()) {
            (Some(c), None) => format!("U+{:04X}", c as u32),
            _ => String::new(),
        };
        main.text(GLYPH, &item.text);
        main.text(CODEPOINT, &point);
        main.text(CODES, &join(&item.learning.codes, " "));
        main.text(FULL, &join(&item.learning.full_codes, " "));
        main.text(READING, &join(&item.learning.readings, "、"));
        main.text(
            PARTS,
            item.learning.decomposition.as_deref().unwrap_or("缺资料"),
        );
        main.text(CLIP, &details(item));
    }
    for id in [GLYPH, CODEPOINT, COPY, CODES, FULL, READING, PARTS]
        .into_iter()
        .chain(FIELD_LABEL..FIELD_LABEL + 4)
    {
        main.show(id, item.is_some());
    }
    main.show(STATUS, item.is_none());
}
pub(super) fn refresh(main: &Window, settings: &Window, model: &Model) {
    main.enable(QUERY, !model.busy);
    main.enable(
        PREVIOUS,
        !model.busy && model.page > 0 && model.result.is_some(),
    );
    main.enable(
        NEXT,
        !model.busy && model.result.as_ref().is_some_and(|p| p.has_more),
    );
    main.enable(COPY, selected(main, model).is_some());
    main.text(STATUS, &model.notice);
    let page = model.result.as_ref().map(|_| (model.page + 1).to_string());
    main.text(PAGE, &page.unwrap_or_default());
    settings.text(NOTICE, &model.settings_notice);
    settings.text(CUSTOM_STATUS, &custom_status(settings.handle));
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
    settings.text(HEADING, PAGES[page].0);
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
/// 即时生效：草稿合法且与已确认设置不同才保存；在途时不排队，回复后由事件循环再比一次。
/// 草稿不合法（色值未填完、偏蓝或不可读）时不保存，侧栏写明原因，在其他分区改设置时也看得到；
/// 草稿回到已确认设置后清掉这条提示。
pub(super) fn commit(settings: &Window, model: &mut Model) -> Option<Request> {
    match read_options(settings.handle) {
        Ok(options) if options == model.options => {
            if model.settings_notice.starts_with('✗') {
                model.settings_notice.clear();
            }
            None
        }
        Ok(options) if !model.busy => model.save(options).ok(),
        Ok(_) => None,
        Err(error) => {
            model.settings_notice = format!("✗ 未保存：{error}");
            None
        }
    }
}
/// 草稿里会影响设置的控件：点击后即保存。
fn is_option(id: usize) -> bool {
    matches!(id, AUTO | COMMON | LEARNING)
        || (MODE..MODE + TypingMode::ALL.len()).contains(&id)
        || (FORM..FORM + FORMS.len()).contains(&id)
        || (FONT..FONT + CandidateFont::ALL.len()).contains(&id)
        || (SIZE_CHOICE..SIZE_CHOICE + Options::PAGE_SIZES.count()).contains(&id)
}
/// 设置窗的命令：切换分区、即时保存、色板、打开与重新载入配置文件。
pub(super) fn settings_action(
    settings: &mut Window,
    model: &mut Model,
    id: usize,
    notification: u32,
    settings_path: &Path,
) -> Result<Option<Request>, String> {
    let face = model.options.candidate_font;
    Ok(match notification {
        EN_CHANGE if (CUSTOM..CUSTOM + COLORS.len()).contains(&id) => {
            repaint(settings);
            commit(settings, model)
        }
        BN_CLICKED if (NAV..NAV + PAGES.len()).contains(&id) => {
            select_page(settings, id - NAV, face)?;
            None
        }
        BN_CLICKED if (SWATCH..SWATCH + COLORS.len()).contains(&id) => {
            // 改写色值框会触发 EN_CHANGE，随后照常校验并保存。
            if let Some(color) = palette_menu(settings, id - SWATCH) {
                settings.text(CUSTOM + id - SWATCH, &format!("#{color:06X}"));
            }
            None
        }
        BN_CLICKED if (THEME..THEME + Theme::ALL.len()).contains(&id) => {
            // 离开自定义时，未填完或不合格的四色退回已保存的值，免得挡住换预设主题的保存。
            if id != THEME + Theme::ALL.len() - 1 && read_options(settings.handle).is_err() {
                set_custom(settings, model.options.custom);
            }
            // 选不选自定义决定是否露出四色编辑行。
            settings.layout(face)?;
            commit(settings, model)
        }
        BN_CLICKED if is_option(id) => {
            repaint(settings);
            commit(settings, model)
        }
        BN_CLICKED => match id {
            RESET => {
                set_custom(settings, CustomColors::default());
                None
            }
            OPEN_CONFIG => {
                model.settings_notice = match open_config(settings_path) {
                    Ok(()) => "已用默认编辑器打开 settings.toml".into(),
                    Err(error) => error,
                };
                None
            }
            RELOAD => match model.reload() {
                Ok(request) => Some(request),
                Err(error) => {
                    model.settings_notice = error.into();
                    None
                }
            },
            _ => None,
        },
        _ => None,
    })
}
