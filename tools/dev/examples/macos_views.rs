//! 独立 Mac UI 预览：只用合成资料与内存设置，不注册输入法、不读用户目录、不创建输入线程。
#[cfg(target_os = "macos")]
fn main() {
    use objc2::{MainThreadMarker, MainThreadOnly, sel};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSMenu, NSMenuItem, NSScreen,
    };
    use objc2_foundation::NSString;
    use std::{cell::RefCell, rc::Rc};
    use tigertype_contracts::{
        Candidate, Composition, InputState, Key, LearningInfo, LookupItem, LookupPage, Options,
        protocol::Panel,
        ui::{Reply, Request},
    };
    use tigertype_macos_input_method::{
        Action, Location, MenuAction, NativeRect,
        appkit::{CandidatePanel, Menu},
    };
    let mtm = MainThreadMarker::new().expect("预览必须在主线程启动");
    let app = NSApplication::sharedApplication(mtm);
    // 预览与产品共用校验/私有注册入口，不要求系统事先安装字体。
    let _fonts = tigertype_macos_app::fonts::load(
        &tigertype_dev::project_root()
            .unwrap()
            .join("resources/fonts"),
    )
    .unwrap();
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let options = Rc::new(RefCell::new(Options::default()));
    let saved = options.clone();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("tigertype-preview-{}-{stamp}", std::process::id()));
    std::fs::create_dir(&directory).expect("创建合成预览目录失败");
    let config = directory.join("settings.toml");
    // 不创建假配置文件；“打开”明确失败，保存/载入仅操作这个合成进程内的设置。
    let views = Rc::new(
        tigertype_macos_ui::appkit::Views::new(
            mtm,
            Options::default(),
            config,
            move |request, completion| {
                let reply = match request {
                    Request::Lookup { query, page } => Reply::Lookup(Ok(LookupPage {
                        items: if query.is_empty() {
                            vec![]
                        } else {
                            (0..9)
                                .map(|i| LookupItem {
                                    text: ["拉", "隹", "𠀀", "虎码"][i % 4].into(),
                                    learning: LearningInfo {
                                        codes: vec!["ui".into(), "uil".into()],
                                        full_codes: vec!["uil".into()],
                                        readings: vec!["lā".into()],
                                        decomposition: Some("扌立".into()),
                                    },
                                })
                                .collect()
                        },
                        has_more: page == 0,
                    })),
                    Request::SaveSettings(next) => {
                        *saved.borrow_mut() = next;
                        Reply::Saved(Ok(next))
                    }
                    Request::ReloadSettings => Reply::Saved(Ok(*saved.borrow())),
                };
                completion.finish(reply);
            },
        )
        .unwrap(),
    );
    let state = Rc::new(RefCell::new(InputState {
        composing: true,
        composition: Composition {
            text: "ui".into(),
            cursor: 2,
            selection: 2..2,
        },
        revision: 1,
        highlighted: Some(0),
        is_last_page: false,
        candidates: [("拉", "〔扌立 · uil〕"), ("隹", "〔隹 · ui / uii〕")]
            .into_iter()
            .enumerate()
            .map(|(i, (text, comment))| Candidate {
                text: text.into(),
                comment: comment.into(),
                label: (i + 1).to_string(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }));
    let tools = views.clone();
    let input = state.clone();
    let action: Rc<dyn Fn(Action)> = Rc::new(move |action| match action {
        Action::Select(selected) => eprintln!("合成候选点击：{selected:?}（不会上屏）"),
        Action::Menu {
            action: MenuAction::Show(panel),
            ..
        } => tools.show(panel),
        Action::Menu {
            action: MenuAction::Key(key),
            ..
        } => {
            let mut s = input.borrow_mut();
            match key {
                Key::ToggleAscii => s.ascii_mode = !s.ascii_mode,
                Key::ToggleFullShape => s.full_shape = !s.full_shape,
                Key::TogglePunctuation => s.ascii_punctuation = !s.ascii_punctuation,
                _ => {}
            }
            eprintln!(
                "合成状态：中英={} 全角={} 英文标点={}",
                s.ascii_mode, s.full_shape, s.ascii_punctuation
            );
        }
    });
    let candidate = CandidatePanel::new(mtm, action.clone());
    let mut location = Location::default();
    let screen = NSScreen::mainScreen(mtm)
        .expect("需要图形登录会话")
        .visibleFrame();
    candidate
        .show(
            1,
            state.borrow().clone(),
            &mut location,
            Some(NativeRect {
                x: screen.origin.x + 40.0,
                y: screen.origin.y + screen.size.height - 100.0,
                width: 1.0,
                height: 20.0,
            }),
            None,
        )
        .unwrap();
    let menu = Menu::new(mtm, 1, &state.borrow(), None, action);
    let bar = NSMenu::new(mtm);
    let root = NSMenuItem::new(mtm);
    root.setTitle(&NSString::from_str("TigerType 预览"));
    root.setSubmenu(Some(menu.native()));
    bar.addItem(&root);
    // SAFETY: terminate: 是 NSApplication 的系统动作，应用由运行循环持有。
    unsafe {
        let quit = NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("退出预览"),
            Some(sel!(terminate:)),
            &NSString::from_str("q"),
        );
        quit.setTarget(Some(&app));
        menu.native().addItem(&quit);
    }
    app.setMainMenu(Some(&bar));
    eprintln!(
        "仅 M2/M3 合成预览：无 IMK、无上屏、无磁盘保存。菜单状态是创建时快照；不代表 M1 接线。⌘Q 退出。"
    );
    views.show(Panel::Lookup);
    views.show(Panel::Settings);
    for window in app.windows() {
        let title = window.title().to_string();
        if !title.is_empty() {
            window.setTitle(&NSString::from_str(&format!(
                "{title} · 合成预览（保存仅在内存）"
            )));
        }
    }
    app.run();
    candidate.hide();
    views.close();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("此预览只能在 macOS 图形登录会话运行；没有启动或安装输入法。");
    std::process::exit(1);
}
