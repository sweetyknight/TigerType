//! 独立 Linux 查字/设置预览：只用合成资料与内存设置，不启动输入法框架、不读用户目录、不创建工作线程。
//! backend 尚无 Linux 存储边界，预览因此放在本组件而非开发工具；需要图形会话（Wayland 或 X11）。
#[cfg(target_os = "linux")]
#[path = "../tests/support/fonts.rs"]
mod fonts;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    use gtk4::{glib, prelude::*};
    use std::{cell::Cell, rc::Rc};
    use tigertype_contracts::{
        LearningInfo, LookupItem, LookupPage, Options,
        protocol::Panel,
        ui::{Reply, Request},
    };
    let resources =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../resources/fonts");
    // 预览显式读入仓库锁定字体，不要求系统事先安装。
    if let Some(code) = fonts::relaunch(&resources) {
        return code;
    }
    if let Err(error) = gtk4::init() {
        eprintln!("需要图形登录会话：{error}");
        return std::process::ExitCode::FAILURE;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("tigertype-preview-{}-{stamp}", std::process::id()));
    std::fs::create_dir(&directory).expect("创建合成预览目录失败");
    // 不创建假配置文件；“打开”明确失败，保存/载入仅操作这个合成进程内的设置。
    let saved = Rc::new(Cell::new(Options::default()));
    let store = saved.clone();
    let views = tigertype_linux_ui::gtk::Views::new(
        Options::default(),
        directory.join("settings.toml"),
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
                                roots: vec![vec![('扌', "us".into()), ('立', "il".into())]],
                            })
                            .collect()
                    },
                    has_more: page == 0,
                })),
                Request::SaveSettings(next) => {
                    store.set(next);
                    Reply::Saved(Ok(next))
                }
                Request::ReloadSettings => Reply::Saved(Ok(store.get())),
            };
            completion.finish(reply);
        },
    )
    .unwrap();
    views.show(Panel::Lookup, "");
    views.show(Panel::Settings, "");
    let windows = gtk4::Window::toplevels();
    // 顶层窗口列表的元素类型是 GObject，逐个转换。
    let all = windows.iter::<glib::Object>().flatten();
    for window in all.filter_map(|w| w.downcast::<gtk4::Window>().ok()) {
        if let Some(title) = window.title() {
            window.set_title(Some(&format!("{title} · 合成预览（保存仅在内存）")));
        }
    }
    // 两个窗口都关闭即退出，不用计时器轮询。
    let main = glib::MainLoop::new(None, false);
    let quit = main.clone();
    windows.connect_items_changed(move |list, _, _, _| {
        if list.n_items() == 0 {
            quit.quit();
        }
    });
    eprintln!("合成预览：无输入法框架、无上屏、无磁盘保存。关闭两个窗口即退出。");
    main.run();
    views.close();
    std::fs::remove_dir_all(&directory).ok();
    std::process::ExitCode::SUCCESS
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("此预览只能在 Linux 图形登录会话运行；没有启动或安装输入法。");
    std::process::exit(1);
}
