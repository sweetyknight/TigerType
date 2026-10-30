//! 菜单测试共享的串行与超时边界；不依赖具体窗口/托盘测试，避免测试模块互相引用。
use super::*;

// SAFETY: 系统在本线程菜单模态循环内调用；只收起本线程菜单。
unsafe extern "system" fn give_up(_: HWND, _: u32, _: usize, _: u32) {
    // SAFETY: 同上。
    unsafe { EndMenu() };
}
// 系统同一时刻只允许一个弹出菜单：别的线程弹菜单会立即关掉本线程的，弹真实菜单的测试经此串行。
static MENU: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// 在本线程运行一个模态循环，取得锁后才起表：约 300ms 时另一线程断开唤起邮箱（即后台停止）。
/// 修复失效时由 5 秒定时器收起菜单，让测试以超时失败而不是挂住；模态须在 3 秒内被收起，
/// 不足 200ms 说明模态根本没展开（例如被别处的菜单顶掉）。
pub(super) fn bounded(sender: crate::ReplySender<Panel>, modal: impl FnOnce()) {
    let _only = MENU.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        drop(sender);
    });
    let started = std::time::Instant::now();
    // SAFETY: 定时器属于本线程，用后注销。
    let timer = unsafe { SetTimer(null_mut(), 0, 5000, Some(give_up)) };
    modal();
    // 只计模态本身：回收停止线程要等满 300ms，计入会让没展开的菜单也显得够久。
    let elapsed = started.elapsed();
    // SAFETY: 同上。
    unsafe { KillTimer(null_mut(), timer) };
    stopper.join().unwrap();
    assert!(elapsed < std::time::Duration::from_secs(3), "{elapsed:?}");
    assert!(
        elapsed > std::time::Duration::from_millis(200),
        "模态未展开：{elapsed:?}"
    );
}
