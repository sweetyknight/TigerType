//! AppKit 主线程和固定后台线程的唯一组装点；不注册、启用或切换系统输入法。
use crate::runtime::Runtime;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use std::{cell::RefCell, path::PathBuf, rc::Rc};
use tigertype_macos_input_method::appkit::Server;
use tigertype_macos_ui::appkit::Views;

pub fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = match args.as_slice() {
        [] => run(),
        [command] if command == "install" => {
            std::env::current_exe().map_err(Into::into).and_then(|exe| {
                let bundle = exe
                    .parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.parent())
                    .ok_or("程序不在应用包内")?;
                crate::install::install(bundle)
            })
        }
        [command] if command == "uninstall" => crate::install::uninstall(),
        _ => Err("用法：tigertype [install|uninstall]；不带参数由系统作为输入法启动".into()),
    };
    result.map_err(|e| e.to_string())
}
fn run() -> tigertype_backend::Result<()> {
    // SAFETY: 仅读取进程身份；输入法不能以安装器的提权身份处理个人数据。
    if unsafe { libc::geteuid() } == 0 {
        return Err("输入法须以普通用户运行".into());
    }
    let mtm = MainThreadMarker::new().ok_or("输入法须在主线程启动")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let executable = std::env::current_exe()?;
    let contents = executable
        .parent()
        .and_then(|p| p.parent())
        .ok_or("程序不在应用包内")?;
    // 先核对系统字体再启动 IMK，不留下拆分显示方块的半就绪输入法。
    crate::fonts::ensure_learning_font()?;
    let root = PathBuf::from(std::env::var_os("HOME").ok_or("缺少 HOME")?)
        .join("Library/Application Support/TigerType");
    let config = root.join("config/settings.toml");
    let runtime = Runtime::start(contents.join("Resources/tiger"), root)?;
    let status = runtime.ready.clone();
    let options = status.clone();
    let worker = runtime.worker.clone();
    let connector = runtime.connector.clone();
    let alive = connector.clone();
    let tools = Rc::new(RefCell::new(None::<Views>));
    let views = tools.clone();
    let server = Server::start(
        mtm,
        move || Box::new(connector.link()),
        move |panel| {
            let Some(Ok(options)) = options.get() else {
                return;
            };
            if views.borrow().is_none() {
                let worker = worker.clone();
                match Views::new(mtm, *options, config.clone(), move |request, completion| {
                    worker.submit(request, move |reply| completion.finish(reply));
                }) {
                    Ok(value) => *views.borrow_mut() = Some(value),
                    Err(error) => {
                        eprintln!("查字/设置初始化失败：{error}");
                        return;
                    }
                }
            }
            if let Some(views) = views.borrow().as_ref() {
                views.show(panel);
            }
        },
        move || match status.get() {
            None => Some("词表正在加载".into()),
            Some(Err(error)) => Some(error.clone()),
            Some(Ok(_)) if alive.stopped() => Some("引擎已停止，请重新启动输入法".into()),
            Some(Ok(_)) => None,
        },
    );
    match server {
        Ok(server) => {
            app.run();
            drop(server);
        }
        Err(error) => {
            runtime.shutdown();
            return Err(error.into());
        }
    }
    drop(tools);
    runtime.shutdown();
    Ok(())
}
