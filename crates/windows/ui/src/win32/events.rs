//! 固定 UI 线程的消息/回复循环与键盘路由；组装窗口和托盘，空闲只等待事件。
use super::*;
use super::{controls::*, drawing::*, modal::*, panels::*, window::*};

fn composing(hwnd: HWND) -> bool {
    // SAFETY: 仅借用当前编辑控件的输入上下文，立即配对归还；不切换或配置用户输入法。
    unsafe {
        let context = ImmGetContext(hwnd);
        if context.is_null() {
            return false;
        }
        let length = ImmGetCompositionStringW(context, GCS_COMPSTR, null_mut(), 0);
        ImmReleaseContext(hwnd, context);
        length > 0
    }
}

pub(super) fn key_command(
    settings: bool,
    id: usize,
    key: usize,
    ime: bool,
) -> Option<(u32, usize)> {
    // IME 先处理确认/取消；开关、单选与色值框上 Enter 不触发别的动作。
    if ime {
        return None;
    }
    if settings {
        if key == VK_ESCAPE as usize {
            return Some((CLOSE, 0));
        }
        if key == VK_RETURN as usize
            && (matches!(id, OPEN_CONFIG | RELOAD | RESET)
                || (SWATCH..SWATCH + COLORS.len()).contains(&id))
        {
            return Some((ACTION, id));
        }
    } else if key == VK_RETURN as usize {
        return match id {
            QUERY => Some((ACTION, SEARCH)),
            PREVIOUS | NEXT | COPY | SETTINGS => Some((ACTION, id)),
            _ => None,
        };
    }
    None
}

/// Tab 移到滚出视野的控件时把它滚回来。
fn ensure_visible(settings: &Window) {
    let Some(id) = settings.focused_control() else {
        return;
    };
    if (NAV..NAV + PAGES.len()).contains(&id) {
        return;
    }
    let mut rect = RECT::default();
    let mut client = RECT::default();
    // SAFETY: 只读本线程窗口位置并换算到设置窗客户区坐标；输出为本地值。
    unsafe {
        GetWindowRect(settings.control(id), &mut rect);
        MapWindowPoints(
            null_mut(),
            settings.handle,
            (&mut rect as *mut RECT).cast(),
            2,
        );
        GetClientRect(settings.handle, &mut client);
    }
    let dpi = settings.dpi.max(96) as i32;
    let (top, bottom, view) = (
        rect.top * 96 / dpi,
        rect.bottom * 96 / dpi,
        client.bottom * 96 / dpi,
    );
    let offset = scroll_offset(settings.handle);
    if top < 12 {
        scroll_to(settings.handle, offset + top - 24);
    } else if bottom > view - 12 {
        scroll_to(settings.handle, offset + bottom - view + 24);
    }
}

fn keyboard(message: &MSG, main: &Window, settings: &Window) {
    // SAFETY: 消息源自本线程队列，仅本程序的顶层窗使用对话框导航。
    let root = unsafe { GetAncestor(message.hwnd, GA_ROOT) };
    let dialog = if root == settings.handle {
        Some(settings)
    } else if root == main.handle {
        Some(main)
    } else {
        None
    };
    if let Some(dialog) = dialog {
        // SAFETY: 查询当前线程按键状态，不注入全局键盘事件。
        let modified =
            unsafe { GetKeyState(VK_CONTROL as i32) < 0 || GetKeyState(VK_MENU as i32) < 0 };
        if message.message == WM_KEYDOWN && !modified {
            // SAFETY: hwnd 来自系统消息；无效/非控件句柄返回值不会匹配已知动作。
            let id = unsafe { GetDlgCtrlID(message.hwnd) } as usize;
            let is_settings = root == settings.handle;
            let ime = message.hwnd == main.control(QUERY)
                && matches!(message.wParam, key if key == VK_RETURN as usize || key == VK_ESCAPE as usize)
                && composing(message.hwnd);
            if ime {
                // SAFETY: IME 确认/取消必须直接交编辑控件，不能再被对话框默认按钮截获。
                unsafe {
                    TranslateMessage(message);
                    DispatchMessageW(message);
                }
                return;
            }
            if let Some((event, action)) = key_command(is_settings, id, message.wParam, ime) {
                post(root, event, action);
                return;
            }
        }
        // SAFETY: 使用系统导航处理 Tab/方向键/助记键，消息及窗口都在同一线程存活。
        if unsafe { IsDialogMessageW(dialog.handle, message) } != 0 {
            if root == settings.handle {
                ensure_visible(settings);
            }
            return;
        }
    }
    // SAFETY: 未接管的输入原样交回原生控件，包括 IME。
    unsafe {
        TranslateMessage(message);
        DispatchMessageW(message);
    }
}

/// 当前线程专用于原生 UI；事件唤醒回复、系统消息驱动操作，不使用定时轮询。
/// 后台进程内的固定 UI 线程：启动即挂通知区图标，查字/设置窗等首次唤起（唤起信号或图标操作）才建；
/// 关闭主窗只隐藏，再次唤起即前置目标窗口；show 断开（后台停止）才退出并删除图标。
/// taskbar 为后台给出的通知区图标显隐（前台程序正用本输入法时隐藏），全程随到随用。
pub fn run(
    requests: SyncSender<Request>,
    inbox: Inbox<Reply>,
    show: Inbox<Panel>,
    taskbar: &Latest<bool>,
    settings_path: &Path,
    options: Options,
) -> Result<(), String> {
    let _dpi = DpiContext::enter()?;
    PALETTE.with(|palette| palette.set(options.palette()));
    watch(Some(&show));
    // 邮箱比所有窗口活得久；退出时先撤销登记，不留悬空事件句柄。
    let result = serve(requests, inbox, &show, taskbar, settings_path, options);
    watch(None);
    result
}
fn serve(
    requests: SyncSender<Request>,
    inbox: Inbox<Reply>,
    show: &Inbox<Panel>,
    taskbar: &Latest<bool>,
    settings_path: &Path,
    options: Options,
) -> Result<(), String> {
    let tray = tray::Tray::new()?;
    let Some(panel) = tray::first(show, &tray, taskbar)? else {
        return Ok(());
    };
    let class = Class::register()?;
    let mut main = lookup_window(&class, &options)?;
    let mut settings = settings_window(&class, main.handle, &options, settings_path)?;
    let mut editor = Editor::new(options);
    sync_settings(&settings, &editor, None);
    settings.layout(options.candidate_font)?;
    show_page(&main, &editor);
    refresh(&main, &settings, &editor);
    present(&main, &settings, &editor, panel);
    event_loop(
        &mut main,
        &mut settings,
        &mut editor,
        &requests,
        &inbox,
        show,
        (settings_path, Some((&tray, taskbar))),
    )
}

/// 显示并前置目标窗口，最小化时先还原。唤起方已放行前台切换；通知区图标被点击时本进程获准切前台。
/// 设置页可单独打开（输入法图标或通知区图标），不连带显示查字窗；关闭时已丢弃未完成的色值。
pub(super) fn present(main: &Window, settings: &Window, editor: &Editor, panel: Panel) {
    let (window, focus) = match panel {
        Panel::Lookup => (main, main.control(QUERY)),
        Panel::Settings => {
            if !visible(settings) {
                sync_settings(settings, editor, None);
                post(settings.handle, LAYOUT, 0);
            }
            (
                settings,
                settings.control(NAV + current_page(settings.handle)),
            )
        }
    };
    // SAFETY: 窗口与控件由调用方持有，只在本线程显示与激活，不注册或切换输入法。
    unsafe {
        let mode = if IsIconic(window.handle) != 0 {
            SW_RESTORE
        } else {
            SW_SHOW
        };
        ShowWindow(window.handle, mode);
        // 首次 ShowWindow 可能被启动时的 SW_HIDE 覆盖；用户显式唤起不应继承后台的隐藏启动策略。
        if IsWindowVisible(window.handle) == 0 {
            SetWindowPos(
                window.handle,
                null_mut(),
                0,
                0,
                0,
                0,
                SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        SetForegroundWindow(window.handle);
        SetFocus(focus);
    }
}

pub(super) fn event_loop(
    main: &mut Window,
    settings: &mut Window,
    editor: &mut Editor,
    requests: &SyncSender<Request>,
    inbox: &Inbox<Reply>,
    show: &Inbox<Panel>,
    // 设置文件路径与通知区图标；测试不登记图标时为 None。
    context: (&Path, Option<(&tray::Tray, &Latest<bool>)>),
) -> Result<(), String> {
    let (settings_path, icon) = context;
    let mut events = vec![inbox.event.as_raw_handle(), show.event.as_raw_handle()];
    if let Some((_, taskbar)) = icon {
        events.push(taskbar.event().as_raw_handle());
    }
    let mut main_focus = None;
    loop {
        // SAFETY: Inbox 与交接槽持有事件直到循环结束；消息、回复、唤起与显隐共享一次阻塞等待，空闲不耗 CPU。
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(
                events.len() as u32,
                events.as_ptr(),
                INFINITE,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if wait == WAIT_FAILED {
            return Err(error());
        }
        if let Some((tray, taskbar)) = icon {
            tray.sync(taskbar)?;
        }
        match show.receiver.try_recv() {
            Ok(panel) => present(main, settings, editor, panel),
            Err(TryRecvError::Empty) => {}
            // 后台停止：放弃窗口，线程退出由调用方 join。
            Err(TryRecvError::Disconnected) => return Ok(()),
        }
        loop {
            match inbox.receiver.try_recv() {
                Ok(reply) => {
                    let (before, colors) = (editor.model.options, editor.colors.clone());
                    let lookup = matches!(reply, Reply::Lookup(_));
                    // 补存在途期间改过的设置优先，其回复后再按当前查询框补查。
                    let next = editor.reply(Ok(reply));
                    // 回复与在途操作不符：工作线程契约已失效，不再消费后续回复。
                    if editor.fatal {
                        return Err(editor.model.notice.clone());
                    }
                    // Saved 回复也可能补处理空查询或超长查询，模型清空时必须同步撤下旧字块。
                    if lookup || editor.model.result.is_none() {
                        show_page(main, editor);
                    }
                    if !lookup {
                        sync_settings(settings, editor, Some(&colors));
                        settings.layout(editor.model.options.candidate_font)?;
                    }
                    if editor.model.options != before {
                        apply_look(main, settings, &editor.model.options)?;
                    }
                    if let Some(request) = next {
                        send(requests, editor, request);
                    }
                    refresh(main, settings, editor);
                    if let Some(id) = main_focus.take() {
                        main.restore_focus(id);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err("查字工作线程已退出，请重新启动工具".into());
                }
            }
        }
        let mut message = MSG::default();
        // 每轮最多处理 64 条系统消息，避免持续消息流饿死工作线程回复。
        for _ in 0..64 {
            // SAFETY: MSG 栈对象完整有效，消息仅从当前线程提取。
            if unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } == 0 {
                break;
            }
            match message.message {
                WM_QUIT => {
                    return if message.wParam == 0 {
                        Ok(())
                    } else {
                        Err("窗口消息投递失败".into())
                    };
                }
                // 后台常驻：关闭主窗只隐藏（设置窗一并收起，未填完的色值丢弃），保留查询结果，等下次唤起。
                CLOSE if message.hwnd == main.handle => {
                    main_focus = None;
                    editor.close_settings();
                    sync_settings(settings, editor, None);
                    // SAFETY: 两个窗口仅隐藏，所有权继续由本函数持有。
                    unsafe {
                        ShowWindow(settings.handle, SW_HIDE);
                        ShowWindow(main.handle, SW_HIDE);
                    }
                }
                CLOSE if message.hwnd == settings.handle => {
                    editor.close_settings();
                    sync_settings(settings, editor, None);
                    post(settings.handle, LAYOUT, 0);
                    // SAFETY: 设置窗仅隐藏，所有权继续由本函数持有；单独打开时查字窗仍隐藏，不去激活它。
                    unsafe {
                        ShowWindow(settings.handle, SW_HIDE);
                        if IsWindowVisible(main.handle) != 0 {
                            SetFocus(main.control(QUERY));
                        }
                    }
                }
                LAYOUT => {
                    let face = editor.model.options.candidate_font;
                    if message.hwnd == main.handle {
                        main.layout(face)?;
                    } else if message.hwnd == settings.handle {
                        settings.layout(face)?;
                    }
                }
                ACTION if message.hwnd == settings.handle => {
                    let id = message.wParam & 0xffff;
                    let notification = (message.wParam >> 16) as u32;
                    if let Some(request) =
                        settings_action(settings, editor, id, notification, settings_path)?
                    {
                        send(requests, editor, request);
                    }
                    refresh(main, settings, editor);
                }
                ACTION => {
                    let id = message.wParam & 0xffff;
                    let notification = (message.wParam >> 16) as u32;
                    let action = match (id, notification) {
                        (QUERY, EN_CHANGE) | (SEARCH, BN_CLICKED) => {
                            // Enter 只是立即按当前文本再查一次，与边输边查同一路径。
                            edit(main, editor, requests);
                            refresh(main, settings, editor);
                            continue;
                        }
                        (LIST, LBN_SELCHANGE) => {
                            let index = main.send(LIST, LB_GETCURSEL, 0, 0);
                            match usize::try_from(index) {
                                Ok(index) => Action::Select(index),
                                Err(_) => continue,
                            }
                        }
                        // 键盘 Enter 也会投递按钮命令；停用的按钮不执行。
                        (PREVIOUS | NEXT | COPY, BN_CLICKED) if !view::enabled(editor, id) => {
                            continue;
                        }
                        (PREVIOUS, BN_CLICKED) => Action::Previous,
                        (NEXT, BN_CLICKED) => Action::Next,
                        (COPY, BN_CLICKED) => Action::Copy,
                        (SETTINGS, BN_CLICKED) => Action::Settings,
                        _ => continue,
                    };
                    match editor.act(action) {
                        Some(Task::Send(request)) => {
                            if send(requests, editor, request) {
                                main_focus = main.focused_control();
                            }
                        }
                        Some(Task::ShowSettings) => {
                            present(main, settings, editor, Panel::Settings)
                        }
                        Some(Task::Copy(text)) => {
                            // 只在用户点击时委托标准 EDIT 复制当前字资料，不读取或监视剪贴板。
                            main.text(CLIP, &text);
                            main.send(CLIP, EM_SETSEL, 0, -1);
                            main.send(CLIP, WM_COPY, 0, 0);
                        }
                        _ => {}
                    }
                    if id == LIST {
                        show_detail(main, editor);
                    }
                    refresh(main, settings, editor);
                }
                // 通知区图标的单击或菜单选择。
                tray::OPEN => present(main, settings, editor, tray::decode(message.wParam)),
                _ => keyboard(&message, main, settings),
            }
        }
    }
}

/// 查询框改动即查；在途时由共用状态机记下待补查。未发请求且结果已清空时立即重画。
fn edit(main: &Window, editor: &mut Editor, requests: &SyncSender<Request>) {
    match editor.act(Action::Query(query(main))) {
        Some(Task::Send(request)) => {
            send(requests, editor, request);
        }
        _ if !editor.model.busy => show_page(main, editor),
        _ => {}
    }
}

/// 通道满或断开：在途状态作废并提示，草稿退回已确认设置；不重试，下次操作照常再发。
fn send(requests: &SyncSender<Request>, editor: &mut Editor, request: Request) -> bool {
    match requests.try_send(request) {
        Ok(()) => true,
        Err(error) => {
            editor.failed(format!("操作未送达：{error}"));
            false
        }
    }
}
