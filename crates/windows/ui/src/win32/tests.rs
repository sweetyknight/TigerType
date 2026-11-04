//! Win32 视图回归：合成模型、私有桌面与有界子进程，不安装或切换输入法。
use super::controls::*;
use super::drawing::*;
use super::events::*;
use super::modal::*;
use super::panels::*;
use super::test_support::bounded;
use super::window::*;
use super::*;

#[test]
fn explicit_present_overrides_hidden_process_startup() {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, CreateDesktopW, HDESK};
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CreateProcessW, GetExitCodeProcess, PROCESS_INFORMATION,
        STARTF_USESHOWWINDOW, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
    };
    // 启动显示标志只作用于进程首次显示；必须在独立限时子进程中测，不能被其他窗口测试提前消耗。
    let image = std::env::current_exe().unwrap();
    let mut command = wide(&format!(
        "\"{}\" --ignored --exact win32::tests::hidden_startup_present_worker --nocapture",
        image.display()
    ));
    struct Desktop(HDESK);
    impl Drop for Desktop {
        fn drop(&mut self) {
            // SAFETY: 只关闭本测试创建的桌面；子进程已回收，未向用户切换此桌面。
            unsafe { CloseDesktop(self.0) };
        }
    }
    let mut desktop_name = wide(&format!("TigerType-StartupTest-{}", std::process::id()));
    // SAFETY: 名称包含测试进程号；GENERIC_ALL 仅申请新桌面的访问权，不切换或修改用户桌面。
    let desktop = Desktop(unsafe {
        CreateDesktopW(
            desktop_name.as_ptr(),
            null(),
            null(),
            0,
            0x1000_0000,
            null(),
        )
    });
    assert!(!desktop.0.is_null(), "私有测试桌面创建失败：{}", error());
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        dwFlags: STARTF_USESHOWWINDOW,
        wShowWindow: SW_HIDE as u16,
        lpDesktop: desktop_name.as_mut_ptr(),
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    // SAFETY: 命令行可写且以 NUL 结尾；只启动自身测试映像，句柄在下面配对关闭。
    let created = unsafe {
        CreateProcessW(
            null(),
            command.as_mut_ptr(),
            null(),
            null(),
            0,
            CREATE_NO_WINDOW,
            null(),
            null(),
            &startup,
            &mut process,
        )
    };
    assert_ne!(created, 0, "隐藏启动子进程创建失败：{}", error());
    // SAFETY: 只等待和回收本测试创建的子进程；超时或等待失败不能遗留后台测试程序。
    let (wait, code) = unsafe {
        let wait = WaitForSingleObject(process.hProcess, 15_000);
        if wait != WAIT_OBJECT_0 {
            TerminateProcess(process.hProcess, 1);
            WaitForSingleObject(process.hProcess, 5_000);
        }
        let mut code = 1;
        GetExitCodeProcess(process.hProcess, &mut code);
        windows_sys::Win32::Foundation::CloseHandle(process.hThread);
        windows_sys::Win32::Foundation::CloseHandle(process.hProcess);
        (wait, code)
    };
    assert_eq!(wait, WAIT_OBJECT_0, "隐藏启动子进程超时或等待失败");
    assert_eq!(code, 0, "隐藏启动后的首次显式唤起失败");
}

#[test]
#[ignore = "由 explicit_present_overrides_hidden_process_startup 带隐藏启动标志执行"]
fn hidden_startup_present_worker() {
    use windows_sys::Win32::System::Threading::{
        GetStartupInfoW, STARTF_USESHOWWINDOW, STARTUPINFOW,
    };
    let mut startup = STARTUPINFOW::default();
    // SAFETY: 只读本子进程启动信息，防止不带隐藏标志的运行被误当作验证通过。
    unsafe { GetStartupInfoW(&mut startup) };
    assert_ne!(startup.dwFlags & STARTF_USESHOWWINDOW, 0);
    assert_eq!(startup.wShowWindow, SW_HIDE as u16);
    let class = Class::register().unwrap();
    let options = Options::default();
    let main = lookup_window(&class, &options).unwrap();
    let settings = settings_window(
        &class,
        main.handle,
        &options,
        Path::new("synthetic-settings.toml"),
    )
    .unwrap();
    let editor = Editor::new(options);
    // 私有桌面内仍移到屏幕外；不覆盖用户桌面、不加载配置或激活输入法。
    for window in [&main, &settings] {
        // SAFETY: 两个窗口由本线程持有，移动时不激活且不改变尺寸或 z 序。
        unsafe {
            SetWindowPos(
                window.handle,
                null_mut(),
                -32_000,
                -32_000,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
    }
    present(&main, &settings, &editor, Panel::Settings);
    assert!(visible(&settings), "隐藏启动时首次设置唤起必须可见");
    assert!(!visible(&main), "单独开设置不连带显示查字窗");
    present(&main, &settings, &editor, Panel::Lookup);
    assert!(visible(&main));
    // SAFETY: 仅最小化本测试窗口，接着经正式唤起路径还原。
    unsafe {
        ShowWindow(main.handle, SW_MINIMIZE);
    }
    present(&main, &settings, &editor, Panel::Lookup);
    assert!(visible(&main));
    // SAFETY: 只查询本测试窗口是否已还原。
    assert_eq!(unsafe { IsIconic(main.handle) }, 0);
}

/// 在内存位图上作画并读像素，不显示窗口、不碰桌面。
struct Canvas(HDC, HBITMAP, HGDIOBJ);
impl Canvas {
    fn new(width: i32, height: i32) -> Self {
        // SAFETY: 创建与屏幕兼容的内存 DC 和位图，Drop 时配对释放；先铺白底。
        unsafe {
            let screen = GetDC(null_mut());
            let dc = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            ReleaseDC(null_mut(), screen);
            let old = SelectObject(dc, bitmap);
            fill(
                dc,
                &RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                },
                0x00FF_FFFF,
            );
            Self(dc, bitmap, old)
        }
    }
    fn pixel(&self, x: i32, y: i32) -> COLORREF {
        // SAFETY: 读本内存 DC 的像素。
        unsafe { GetPixel(self.0, x, y) }
    }
}
impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: 还原原位图后删除自建位图与 DC。
        unsafe {
            SelectObject(self.0, self.2);
            DeleteObject(self.1);
            DeleteDC(self.0);
        }
    }
}
fn client(window: &Window, id: usize) -> RECT {
    let mut rect = RECT::default();
    // SAFETY: 只读本测试窗口控件的客户区。
    unsafe { GetClientRect(window.control(id), &mut rect) };
    rect
}

/// 由隐藏窗口测试调用（窗口类同名，不能与之并行注册）：开关、分段与侧栏按主题绘制而非系统蓝。
fn assert_themed_drawing(main: &Window, settings: &Window) {
    // SAFETY: 只读本测试窗口的窗口过程。
    let current = unsafe { GetWindowLongPtrW(main.control(QUERY), GWLP_WNDPROC) };
    assert_eq!(current as usize, edit_procedure as *const () as usize);
    // SAFETY: 同上。
    let current = unsafe { GetWindowLongPtrW(settings.control(RELOAD), GWLP_WNDPROC) };
    assert_eq!(current as usize, button_procedure as *const () as usize);
    // 用户开启高对比度时沿用系统色，不核对主题像素。
    if high_contrast() {
        return;
    }
    let c = themed(PALETTE.with(Cell::get));
    assert_eq!(rgb(0xB45309), 0x0009_53B4);
    // 打开的开关轨道为强调色，关着的开关行为卡片底色；选中的分段为卡片色小块，未选中为轨道色。
    check(settings, AUTO, true);
    check(settings, COMMON, false);
    check(settings, FORM + 1, true);
    check(settings, FORM, false);
    for (id, on) in [(AUTO, true), (COMMON, false)] {
        let rect = client(settings, id);
        let canvas = Canvas::new(rect.right, rect.bottom);
        draw_button(settings.control(id), canvas.0);
        let track = canvas.pixel(
            rect.right - scale(settings.handle, 15 + 30),
            rect.bottom / 2,
        );
        if on {
            assert_eq!(track, c.accent, "控件 {id}");
        } else {
            assert_eq!(canvas.pixel(4, 4), c.surface, "控件 {id}");
        }
    }
    for (id, expected) in [(FORM + 1, c.surface), (FORM, c.track)] {
        let rect = client(settings, id);
        let canvas = Canvas::new(rect.right, rect.bottom);
        draw_button(settings.control(id), canvas.0);
        assert_eq!(canvas.pixel(rect.right / 2, 3), expected, "控件 {id}");
    }
    // 侧栏选中项为主题高亮底（琥珀为浅杏色），不是系统蓝。
    let rect = client(settings, NAV);
    let canvas = Canvas::new(rect.right, rect.bottom);
    draw_button(settings.control(NAV), canvas.0);
    assert_eq!(canvas.pixel(rect.right - 6, rect.bottom / 2), c.nav);
    // 关闭〔拆分 · 编码〕时预览的提示列为空串：曾把空 Vec 的悬空指针交给 DrawTextW，打开外观区即令后台崩溃。
    for learning in [false, true] {
        check(settings, LEARNING, learning);
        let rect = client(settings, PREVIEW);
        let canvas = Canvas::new(rect.right.max(1), rect.bottom.max(1));
        let item = DRAWITEMSTRUCT {
            hwndItem: settings.control(PREVIEW),
            hDC: canvas.0,
            rcItem: rect,
            ..Default::default()
        };
        draw_preview(&item, &c);
    }
    check(settings, LEARNING, false);
    check(settings, AUTO, false);
    check(settings, COMMON, true);
    check(settings, FORM + 1, false);
    check(settings, FORM, true);
}

#[test]
fn keyboard_commands_preserve_ime_handling_and_escape_closes_settings() {
    assert_eq!(
        key_command(false, QUERY, VK_RETURN as usize, false),
        Some((ACTION, SEARCH))
    );
    for id in [PREVIOUS, NEXT, COPY, SETTINGS] {
        assert_eq!(
            key_command(false, id, VK_RETURN as usize, false),
            Some((ACTION, id))
        );
    }
    for id in [LIST, CODES] {
        assert_eq!(key_command(false, id, VK_RETURN as usize, false), None);
    }
    assert_eq!(key_command(false, QUERY, VK_ESCAPE as usize, false), None);
    for id in [
        AUTO,
        COMMON,
        LEARNING,
        FORM,
        FONT + 1,
        THEME + 3,
        CUSTOM,
        NAV + 2,
    ] {
        // 即时生效：开关与单选上 Enter 不再触发保存。
        assert_eq!(key_command(true, id, VK_RETURN as usize, false), None);
        assert_eq!(
            key_command(true, id, VK_ESCAPE as usize, false),
            Some((CLOSE, 0))
        );
    }
    for id in [OPEN_CONFIG, RELOAD, RESET, SWATCH + 1] {
        assert_eq!(
            key_command(true, id, VK_RETURN as usize, false),
            Some((ACTION, id))
        );
    }
    for key in [VK_RETURN, VK_ESCAPE] {
        assert_eq!(key_command(false, QUERY, key as usize, true), None);
    }
}
#[test]
fn layouts_keep_controls_inside_their_areas() {
    let (min_width, min_height) = LOOKUP_MIN;
    for (width, height) in [LOOKUP_MIN, LOOKUP_SIZE] {
        let spec = lookup_spec(width, height, [""; 4], |_| 0);
        for (id, [x, y, w, h]) in &spec.controls {
            if !matches!(*id, CLIP | QUERY_LABEL) {
                assert!(*x >= 0 && *y >= 0 && *w > 0 && *h > 0, "控件 {id}");
                assert!(
                    x + w <= width && y + h <= height,
                    "控件 {id} {width}×{height}"
                );
            }
        }
        assert!(spec.card[1] + spec.card[3] <= height.max(min_height));
    }
    assert!(lookup_spec(min_width, min_height, [""; 4], |_| 0).card[2] > 0);
    let (width, height) = SETTINGS_SIZE;
    for page in 0..PAGES.len() {
        for custom in [false, true] {
            let spec = settings_spec(page, custom);
            let mut ids: Vec<_> = spec
                .fixed
                .iter()
                .chain(&spec.controls)
                .map(|(id, _)| *id)
                .collect();
            let count = ids.len();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), count, "分区 {page} 控件重复");
            for (id, [x, y, w, h]) in &spec.fixed {
                assert!(
                    *x >= 0 && x + w <= SIDEBAR && *y >= 0 && y + h <= height,
                    "控件 {id}"
                );
            }
            for (id, [x, y, w, h]) in &spec.controls {
                assert!(
                    *x >= CX && x + w <= CX + CW && *y >= 0,
                    "分区 {page} 控件 {id}"
                );
                assert!(y + h <= spec.height, "分区 {page} 控件 {id}");
            }
            // 滚动条出现时客户区变窄约 17px，内容仍须放得下。
            assert!(CX + CW <= width - 20);
            // 只有外观区会超出窗口高度并滚动。
            assert_eq!(spec.height > height, page == 1, "分区 {page}");
            let shows_editor = spec.controls.iter().any(|(id, _)| *id == CUSTOM);
            assert_eq!(shows_editor, page == 1 && custom);
        }
    }
}
/// 查字/设置窗的菜单模态展开期间后台停止：窗口菜单、设置窗色板（同经 WM_ENTERMENULOOP）
/// 与文本框自带右键菜单（不发该通知）都须立即收起，停止信号留给事件循环，UI 线程随即退出。
/// 菜单在屏幕上一闪而过，不移动鼠标。
fn window_menu_closes_when_the_backend_stops(
    main: &mut Window,
    settings: &mut Window,
    editor: &mut Editor,
) {
    for kind in 0..3 {
        menu_closes_when_the_backend_stops(main, settings, editor, kind);
    }
}
fn menu_closes_when_the_backend_stops(
    main: &mut Window,
    settings: &mut Window,
    editor: &mut Editor,
    kind: usize,
) {
    let (sender, show) = crate::mailbox::<Panel>().unwrap();
    watch(Some(&show));
    let query = main.control(QUERY);
    // SAFETY: 菜单在本函数内创建并销毁，所有者为本测试线程的窗口；文本框右键菜单按键盘方式（lParam -1）在插入符处弹出。
    bounded(sender, || unsafe {
        match kind {
            0 => {
                let menu = CreatePopupMenu();
                AppendMenuW(menu, MF_STRING, 1, wide("测试").as_ptr());
                TrackPopupMenuEx(menu, TPM_RETURNCMD, 0, 0, main.handle, null());
                DestroyMenu(menu);
            }
            1 => {
                SendMessageW(query, WM_CONTEXTMENU, query as WPARAM, -1);
            }
            _ => {
                assert_eq!(palette_menu(settings, 3), None);
            }
        }
    });
    let (requests, _receiver) = std::sync::mpsc::sync_channel(1);
    let (_replies, inbox) = crate::mailbox().unwrap();
    assert!(
        event_loop(
            main,
            settings,
            editor,
            &requests,
            &inbox,
            &show,
            (Path::new("settings.toml"), None),
        )
        .is_ok()
    );
    watch(None);
}
/// 让设置窗控件显示给定设置（测试绘制用）；正式路径由共用状态机同步。
fn show_options(settings: &Window, options: Options) {
    sync_settings(settings, &Editor::new(options), None);
}
/// 保存回复补处理空查询或超长查询时，不会再收到查字回复，消息泵必须直接撤下旧结果。
fn saved_reply_clears_lookup_controls_when_catch_up_clears_the_model() {
    for query in [String::new(), "你".repeat(65)] {
        let class = Class::register().unwrap();
        let defaults = Options::default();
        let mut main = lookup_window(&class, &defaults).unwrap();
        let mut settings =
            settings_window(&class, main.handle, &defaults, Path::new("synthetic.toml")).unwrap();
        let mut editor = Editor::new(defaults);
        editor.act(Action::Query("你".into()));
        editor.reply(Ok(Reply::Lookup(Ok(tigertype_contracts::LookupPage {
            items: vec![tigertype_contracts::LookupItem {
                text: "你".into(),
                learning: Default::default(),
                roots: vec![],
            }],
            has_more: false,
        }))));
        show_page(&main, &editor);
        let Some(Task::Send(Request::SaveSettings(saved))) = editor.act(Action::Set(Setting::Auto))
        else {
            panic!("缺少保存请求");
        };
        editor.act(Action::Query(query));
        let (_show_sender, show) = crate::mailbox::<Panel>().unwrap();
        let (requests, _receiver) = std::sync::mpsc::sync_channel(1);
        let (replies, inbox) = crate::mailbox().unwrap();
        replies.send(Reply::Saved(Ok(saved))).unwrap();
        drop(replies);
        assert!(
            event_loop(
                &mut main,
                &mut settings,
                &mut editor,
                &requests,
                &inbox,
                &show,
                (Path::new("synthetic.toml"), None),
            )
            .unwrap_err()
            .contains("工作线程已退出")
        );
        assert!(editor.model.result.is_none());
        assert_eq!(main.send(LIST, LB_GETCOUNT, 0, 0), 0);
        assert!(!shown(main.control(GLYPH)) && shown(main.control(STATUS)));
    }
}
/// 两框的 WM_COMMAND 已转为排队 ACTION：处理第一框时不能覆盖第二框尚未入草稿的文字。
fn color_notifications_preserve_other_pending_fields_and_coalesce_saves() {
    let class = Class::register().unwrap();
    let options = Options {
        theme: Theme::Custom,
        ..Default::default()
    };
    let main = lookup_window(&class, &options).unwrap();
    let mut settings =
        settings_window(&class, main.handle, &options, Path::new("synthetic.toml")).unwrap();
    let mut editor = Editor::new(options);
    sync_settings(&settings, &editor, None);
    settings.text(CUSTOM, "#FAFAFA");
    settings.text(CUSTOM + 1, "#222222");
    let Some(Request::SaveSettings(first)) = settings_action(
        &mut settings,
        &mut editor,
        CUSTOM,
        EN_CHANGE,
        Path::new("synthetic.toml"),
    )
    .unwrap() else {
        panic!("缺少首个色值的保存");
    };
    assert_eq!(
        String::from_utf16_lossy(&control_text(settings.control(CUSTOM + 1))),
        "#222222"
    );
    assert!(
        settings_action(
            &mut settings,
            &mut editor,
            CUSTOM + 1,
            EN_CHANGE,
            Path::new("synthetic.toml"),
        )
        .unwrap()
        .is_none()
    );
    let Some(Request::SaveSettings(last)) = editor.reply(Ok(Reply::Saved(Ok(first)))) else {
        panic!("缺少合并后的补存");
    };
    assert_eq!((last.custom.window, last.custom.text), (0xFAFAFA, 0x222222));
    editor.reply(Ok(Reply::Saved(Ok(last))));
    editor.act(Action::Reload);
    // 重新载入期间控件仍保留焦点，但拒收的文字必须复原，不能与只读草稿不一致。
    settings.text(CUSTOM, "#12");
    assert!(
        settings_action(
            &mut settings,
            &mut editor,
            CUSTOM,
            EN_CHANGE,
            Path::new("synthetic.toml"),
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(
        String::from_utf16_lossy(&control_text(settings.control(CUSTOM))),
        "#FAFAFA"
    );
}
/// 投递一次控件命令（与系统 WM_COMMAND 经窗口过程转成的 ACTION 相同）。
fn command(window: &Window, id: usize, notification: u32) {
    post(window.handle, ACTION, (notification as usize) << 16 | id);
}
/// 同一隐藏窗口内复核查字拆分与预览；不另注册窗口类，以免并行测试冲突。
fn assert_learning_fonts(main: &mut Window) {
    let canvas = Canvas::new(600, 100);
    let probe = units("亻\u{e4e4}小文辶日\u{e46a}页缶\u{e432}人〔 · jxyx / 123〕");
    let preview = TempFont::new(canvas.0, &LEARNING_FACES, 12, 400);
    for face in CandidateFont::ALL {
        main.ensure_fonts(face).unwrap();
        for font in [font_of(main.control(PARTS)), preview.0] {
            // SAFETY: 字体与内存 DC 存活；选中对象在查询后还原。
            unsafe {
                let old = SelectObject(canvas.0, font);
                let mut glyphs = vec![0; probe.len()];
                assert_ne!(
                    GetGlyphIndicesW(
                        canvas.0,
                        probe.as_ptr(),
                        probe.len() as i32,
                        glyphs.as_mut_ptr(),
                        GGI_MARK_NONEXISTING_GLYPHS
                    ),
                    u32::MAX
                );
                assert!(
                    glyphs.iter().all(|g| *g != 0 && *g != 0xffff),
                    "{face:?} 拆分缺字"
                );
                assert!(face_is(canvas.0, font, LEARNING_FACES[0]));
                let mut rect = RECT::default();
                DrawTextW(
                    canvas.0,
                    probe.as_ptr(),
                    probe.len() as i32,
                    &mut rect,
                    DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
                );
                assert_eq!(extent(canvas.0, font, &probe), rect.right);
                SelectObject(canvas.0, old);
            }
        }
    }
    assert_eq!(extent(canvas.0, preview.0, &[]), 0);
    main.ensure_fonts(CandidateFont::default()).unwrap();
}
#[test]
fn hidden_native_controls_roundtrip_unicode_settings_and_cleanup() {
    // 合成控件测试不显示窗口、不碰剪贴板、不加载个人资源。
    let _dpi = DpiContext::enter().unwrap();
    // 窗口类为进程级资源，各隐藏窗口场景顺序执行，避免测试线程并行重复注册。
    saved_reply_clears_lookup_controls_when_catch_up_clears_the_model();
    color_notifications_preserve_other_pending_fields_and_coalesce_saves();
    // 只在测试里私有注册仓库原件，不依赖机器预装；随测试进程结束释放。
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../resources/fonts/LXGWWenKaiGBScreen.ttf");
    let path = wide(&path.to_string_lossy());
    // SAFETY: 路径 NUL 结尾；注册只在本进程可见。
    let added = unsafe {
        windows_sys::Win32::Graphics::Gdi::AddFontResourceExW(
            path.as_ptr(),
            windows_sys::Win32::Graphics::Gdi::FR_PRIVATE,
            std::ptr::null(),
        )
    };
    assert_ne!(added, 0);
    for _ in 0..2 {
        let class = Class::register().unwrap();
        let defaults = Options::default();
        let mut main = lookup_window(&class, &defaults).unwrap();
        let mut settings =
            settings_window(&class, main.handle, &defaults, Path::new("settings.toml")).unwrap();
        // 控件编号重复会让布局、助记键与字体角色落到别的控件上。
        for window in [&main, &settings] {
            let mut ids: Vec<_> = window.controls.iter().map(|(id, _)| *id).collect();
            ids.sort_unstable();
            let count = ids.len();
            ids.dedup();
            assert_eq!(ids.len(), count, "控件编号重复");
        }
        show_options(&settings, defaults);
        assert_learning_fonts(&mut main);
        assert_themed_drawing(&main, &settings);
        // 隐藏窗收到迟到回复也不能从用户桌面抢走焦点。
        // SAFETY: 仅观察本测试线程焦点，测试不激活或显示窗口。
        let original_focus = unsafe { GetFocus() };
        main.restore_focus(QUERY);
        // SAFETY: GetFocus 无输出指针，前后观察同一测试线程。
        assert_eq!(unsafe { GetFocus() }, original_focus);
        main.text(QUERY, "𠀀ü漢");
        assert_eq!(query(&main), "𠀀ü漢");
        // 读出全文交给共用状态机判定上限，不把截断文本当成有效查询。
        main.text(QUERY, &"𠀀".repeat(65));
        let mut probe = Editor::new(defaults);
        assert!(probe.act(Action::Query(query(&main))).is_none());
        assert!(probe.model.notice.contains("64"));
        let options = Options {
            auto_commit_four: true,
            common_only: false,
            show_learning: true,
            output_form: OutputForm::Traditional,
            theme: Theme::Ink,
            candidate_font: CandidateFont::KaiTi,
            page_size: 7,
            ..defaults
        };
        show_options(&settings, options);
        assert_eq!(read_options(settings.handle).unwrap(), options);
        // 预览按控件显示的草稿取色：偏蓝或格式不对时读不出，退回已确认配色。
        settings.text(CUSTOM + 3, "#0078D4");
        assert!(read_options(settings.handle).unwrap_err().contains("偏蓝"));
        settings.text(CUSTOM + 3, "12345");
        assert!(read_options(settings.handle).is_err());
        show_options(&settings, options);
        // 分区切换只显示该区控件；选了自定义才露出四色编辑行。
        select_page(&mut settings, 1, defaults.candidate_font).unwrap();
        let on = |window: &Window, id| shown(window.control(id));
        assert!(on(&settings, PREVIEW) && !on(&settings, AUTO) && !on(&settings, CUSTOM));
        check(&settings, THEME + 1, false);
        check(&settings, THEME + 4, true);
        settings.layout(defaults.candidate_font).unwrap();
        assert!(on(&settings, CUSTOM) && on(&settings, SWATCH + 3));
        select_page(&mut settings, 0, defaults.candidate_font).unwrap();
        assert!(on(&settings, AUTO) && !on(&settings, CUSTOM) && !on(&settings, PREVIEW));
        show_options(&settings, defaults);
        settings.layout(defaults.candidate_font).unwrap();

        // 隐藏窗口仍走真实消息泵和事件等待，证明查询回复与工作线程退出都能被观察到。
        // 前面两次改查询框、这一次与随后的 Enter 都已排在消息队列：首个改动即发查询，
        // 其余在途期间只记下，回复后按当前文本补查恰好一次。
        main.text(QUERY, "𠀀");
        // 设置窗色值框的改写通知也在队列里，按共用状态机逐个校验，最终回到合法默认值，不产生保存。
        let mut editor = Editor::new(defaults);
        // 唤起邮箱保持连接，代表后台仍在运行。
        let (show_sender, show) = crate::mailbox::<Panel>().unwrap();
        let (requests, receiver) = std::sync::mpsc::sync_channel(1);
        let (replies, inbox) = crate::mailbox().unwrap();
        let worker = std::thread::spawn(move || {
            for _ in 0..2 {
                let request = receiver
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
                assert!(matches!(request, Request::Lookup { query, page: 0 } if query == "𠀀"));
                replies
                    .send(Reply::Lookup(Ok(tigertype_contracts::LookupPage {
                        items: vec![tigertype_contracts::LookupItem {
                            text: "𠀀".into(),
                            learning: tigertype_contracts::LearningInfo {
                                codes: vec!["ab".into(), "abc".into()],
                                ..Default::default()
                            },
                            roots: vec![vec![('亻', "jr".into()), ('\u{e4e4}', "x".into())]],
                        }],
                        has_more: false,
                    })))
                    .unwrap();
            }
            // 没有第三次查询：连续改动不会排队。
            assert!(
                receiver
                    .recv_timeout(std::time::Duration::from_millis(200))
                    .is_err()
            );
        });
        post(main.handle, ACTION, SEARCH);
        let run = |main: &mut Window,
                   settings: &mut Window,
                   editor: &mut Editor,
                   requests: &SyncSender<Request>,
                   inbox: &Inbox<Reply>| {
            event_loop(
                main,
                settings,
                editor,
                requests,
                inbox,
                &show,
                (Path::new("settings.toml"), None),
            )
        };
        assert!(
            run(&mut main, &mut settings, &mut editor, &requests, &inbox)
                .unwrap_err()
                .contains("工作线程已退出")
        );
        worker.join().unwrap();
        assert_eq!(editor.model.result.as_ref().unwrap().items[0].text, "𠀀");
        assert!(!editor.model.busy);
        assert_eq!(main.send(LIST, LB_GETCOUNT, 0, 0), 1);
        let mut buffer = [0u16; 64];
        let length = window_text(main.control(GLYPH), &mut buffer);
        assert_eq!(String::from_utf16_lossy(&buffer[..length]), "𠀀");
        let length = window_text(main.control(CODES), &mut buffer);
        assert_eq!(String::from_utf16_lossy(&buffer[..length]), "ab abc");
        let length = window_text(main.control(PARTS), &mut buffer);
        assert_eq!(
            String::from_utf16_lossy(&buffer[..length]),
            "亻jr \u{e4e4}x"
        );
        assert!(!shown(main.control(STATUS)));

        // 长词：完整编码与拆分按值区宽折行、行高随之增加；内容高于窗口时出现纵向滚动，而不是截掉。
        let word = "中华人民共和国".repeat(4);
        let current = editor.model.result.take();
        editor.model.result = Some(tigertype_contracts::LookupPage {
            items: vec![tigertype_contracts::LookupItem {
                text: word.clone(),
                learning: tigertype_contracts::LearningInfo {
                    codes: vec!["djjr".into()],
                    full_codes: word.chars().map(|_| "jvns".into()).collect(),
                    readings: vec!["zhōng huá rén mín gòng hé guó ".repeat(4).trim().into()],
                    ..Default::default()
                },
                roots: word
                    .chars()
                    .map(|_| {
                        vec![
                            ('亻', "jr".into()),
                            ('匕', "vb".into()),
                            ('十', "ns".into()),
                        ]
                    })
                    .collect(),
            }],
            has_more: false,
        });
        show_page(&main, &editor);
        let parts_text = control_text(main.control(PARTS));
        assert!(parts_text.len() > 256, "拆分串不能被定长缓冲截断");
        let line = |rows: i32| scale(main.handle, rows);
        assert!(client(&main, FULL).bottom > line(28));
        assert!(client(&main, READING).bottom > line(28));
        assert!(client(&main, PARTS).bottom > line(48));
        let mut info = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL,
            ..Default::default()
        };
        // SAFETY: 只读本测试窗口滚动条。
        unsafe { GetScrollInfo(main.handle, SB_VERT, &mut info) };
        assert!(info.nMax + 1 > info.nPage as i32, "长词内容应可滚动");
        let mut full = RECT::default();
        // SAFETY: 只读本测试窗口客户区。
        unsafe { GetClientRect(main.handle, &mut full) };
        let spec = lookup_layout(
            main.handle,
            full.right * 96 / scale(main.handle, 96),
            10_000,
        );
        let parts_box = spec.controls.iter().find(|(id, _)| *id == PARTS).unwrap().1;
        let parts_value = String::from_utf16_lossy(&parts_text);
        let tiles = tile_boxes(&parts_value, parts_box[2]);
        assert_eq!(tiles.len(), 28 * 3);
        assert!(tiles.iter().all(|(b, ..)| b[1] + 48 <= parts_box[3]));
        editor.model.result = current;
        show_page(&main, &editor);
        // SAFETY: 同上。
        unsafe { GetScrollInfo(main.handle, SB_VERT, &mut info) };
        assert!(info.nMax < info.nPage as i32, "单字不出现滚动");

        // 即时生效：点一下开关就发出保存，回复后换上新设置，并不改动查字结果与选择。
        let (requests, receiver) = std::sync::mpsc::sync_channel(1);
        let (replies, inbox) = crate::mailbox().unwrap();
        check(&settings, AUTO, true);
        command(&settings, AUTO, BN_CLICKED);
        let worker = std::thread::spawn(move || {
            let request = receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            let Request::SaveSettings(saved) = request else {
                panic!("开关须立即保存");
            };
            assert!(saved.auto_commit_four);
            replies.send(Reply::Saved(Ok(saved))).unwrap();
        });
        assert!(run(&mut main, &mut settings, &mut editor, &requests, &inbox).is_err());
        worker.join().unwrap();
        assert!(editor.model.options.auto_commit_four);
        assert_eq!(main.send(LIST, LB_GETCURSEL, 0, 0), 0);

        // 换主题与字体：前一次保存在途时的改动合并成一次补存，回复后整套界面换上新配色与字体。
        let (requests, receiver) = std::sync::mpsc::sync_channel(1);
        let (replies, inbox) = crate::mailbox().unwrap();
        command(&settings, THEME + 1, BN_CLICKED);
        command(&settings, FONT + 1, BN_CLICKED);
        let worker = std::thread::spawn(move || {
            for expected in [CandidateFont::WenKai, CandidateFont::KaiTi] {
                let Request::SaveSettings(saved) = receiver
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap()
                else {
                    panic!("换主题须立即保存");
                };
                assert_eq!((saved.theme, saved.candidate_font), (Theme::Ink, expected));
                replies.send(Reply::Saved(Ok(saved))).unwrap();
            }
        });
        assert!(run(&mut main, &mut settings, &mut editor, &requests, &inbox).is_err());
        worker.join().unwrap();
        assert_eq!(editor.model.options.theme, Theme::Ink);
        assert_eq!(PALETTE.with(Cell::get), editor.model.options.palette());
        assert_eq!(main.face, CandidateFont::KaiTi);

        // 保存失败：控件撤回到已确认设置，提示写明原因。
        let (requests, receiver) = std::sync::mpsc::sync_channel(1);
        let (replies, inbox) = crate::mailbox().unwrap();
        check(&settings, COMMON, false);
        command(&settings, COMMON, BN_CLICKED);
        let worker = std::thread::spawn(move || {
            receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            replies
                .send(Reply::Saved(Err("settings.toml 已在外部修改".into())))
                .unwrap();
        });
        assert!(run(&mut main, &mut settings, &mut editor, &requests, &inbox).is_err());
        worker.join().unwrap();
        assert!(checked_in(settings.handle, COMMON));
        assert_eq!(read_options(settings.handle).unwrap(), editor.model.options);
        assert!(editor.model.settings_notice.contains("外部修改"));

        // 重新载入：控件换成文件里的设置。
        let (requests, receiver) = std::sync::mpsc::sync_channel(1);
        let (replies, inbox) = crate::mailbox().unwrap();
        let edited = Options {
            page_size: 9,
            ..editor.model.options
        };
        command(&settings, RELOAD, BN_CLICKED);
        let worker = std::thread::spawn(move || {
            assert!(matches!(
                receiver
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap(),
                Request::ReloadSettings
            ));
            replies.send(Reply::Saved(Ok(edited))).unwrap();
        });
        assert!(run(&mut main, &mut settings, &mut editor, &requests, &inbox).is_err());
        worker.join().unwrap();
        assert_eq!(editor.model.options, edited);
        assert_eq!(read_options(settings.handle).unwrap(), edited);

        // 自定义色值不合格时侧栏写明未保存，回复后也不补存。
        let (requests, receiver) = std::sync::mpsc::sync_channel(1);
        let (replies, inbox) = crate::mailbox().unwrap();
        let kept = editor.model.options.custom;
        command(&settings, THEME + 4, BN_CLICKED);
        // 改写色值框产生的编辑通知与用户输入走同一路径。
        settings.text(CUSTOM + 3, "#0078D4");
        let worker = std::thread::spawn(move || {
            let Request::SaveSettings(saved) = receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
            else {
                panic!("换自定义主题须保存");
            };
            assert_eq!((saved.theme, saved.custom), (Theme::Custom, kept));
            replies.send(Reply::Saved(Ok(saved))).unwrap();
            assert!(
                receiver
                    .recv_timeout(std::time::Duration::from_millis(200))
                    .is_err(),
                "偏蓝色值不能补存"
            );
        });
        assert!(run(&mut main, &mut settings, &mut editor, &requests, &inbox).is_err());
        worker.join().unwrap();
        assert!(String::from_utf16_lossy(&control_text(settings.control(NOTICE))).starts_with('✗'));
        assert!(
            String::from_utf16_lossy(&control_text(settings.control(CUSTOM_STATUS)))
                .starts_with('✗')
        );
        // 换回预设主题时四色退回已保存的值，换主题照常保存，提示撤下。
        let (requests, receiver) = std::sync::mpsc::sync_channel(1);
        let (replies, inbox) = crate::mailbox().unwrap();
        command(&settings, THEME, BN_CLICKED);
        let worker = std::thread::spawn(move || {
            let Request::SaveSettings(saved) = receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
            else {
                panic!("换预设主题须保存");
            };
            assert_eq!((saved.theme, saved.custom), (Theme::Amber, kept));
            replies.send(Reply::Saved(Ok(saved))).unwrap();
        });
        assert!(run(&mut main, &mut settings, &mut editor, &requests, &inbox).is_err());
        worker.join().unwrap();
        assert_eq!(editor.model.options.theme, Theme::Amber);
        assert_eq!(read_options(settings.handle).unwrap(), editor.model.options);
        assert!(control_text(settings.control(NOTICE)).is_empty());

        // 后台常驻：关闭主窗只收起（连同设置窗），循环继续；本例由随后的 WM_QUIT 结束。
        let (requests, _receiver) = std::sync::mpsc::sync_channel(1);
        let (_replies, inbox) = crate::mailbox().unwrap();
        post(main.handle, CLOSE, 0);
        // SAFETY: 只向本测试线程的消息队列投递退出消息，排在 CLOSE 之后取出。
        unsafe { PostQuitMessage(0) };
        assert!(run(&mut main, &mut settings, &mut editor, &requests, &inbox).is_ok());
        let mut message = MSG::default();
        // 确认退出消息已被循环取走，而不是 CLOSE 提前返回后残留在队列里。
        // SAFETY: MSG 栈对象完整有效，只查看本线程队列。
        let left = unsafe { PeekMessageW(&mut message, null_mut(), WM_QUIT, WM_QUIT, PM_REMOVE) };
        assert_eq!(left, 0);
        // 唤起邮箱断开即后台停止，UI 线程正常退出。
        drop(show_sender);
        assert!(run(&mut main, &mut settings, &mut editor, &requests, &inbox).is_ok());
        window_menu_closes_when_the_backend_stops(&mut main, &mut settings, &mut editor);
        PALETTE.with(|palette| palette.set(Options::default().palette()));
        // 反复创建可发现遗漏的窗口类注销，避免把线程退出误当资源已释放。
        drop(settings);
        drop(main);
        drop(class);
    }
}
