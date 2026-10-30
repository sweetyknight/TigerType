//! 系统模态循环的事件唤醒；登记、注销与回调寿命保持配对，不轮询。
use super::*;

thread_local! {
    // 唤起邮箱与等待登记只在本 UI 线程访问；回调不访问业务状态。
    static WAKE: Cell<HANDLE> = const { Cell::new(null_mut()) };
    static WAIT: Cell<HANDLE> = const { Cell::new(null_mut()) };
}

/// 登记唤起邮箱：菜单（托盘右键、窗口系统菜单、色板）与移动/缩放都是系统模态循环，期间事件循环不运行；
/// 来了唤起或后台停止就收起模态交回事件循环，否则停止时 UI 线程会一直等用户操作，join 不返回。
/// 邮箱须活过本线程的全部窗口；传 None 取消登记。
pub(super) fn watch(show: Option<&Inbox<Panel>>) {
    WAKE.set(show.map_or(null_mut(), |show| show.event.as_raw_handle()));
}
// SAFETY: 线程池线程调用；上下文是进入模态的窗口句柄（值），只投递消息，不碰 UI 状态。
unsafe extern "system" fn woken(context: *mut c_void, _: bool) {
    // SAFETY: 同上；窗口已销毁时投递只返回失败。
    unsafe { PostMessageW(context as HWND, UNBLOCK, 0, 0) };
}
/// WM_ENTERMENULOOP/WM_ENTERSIZEMOVE：登记一次性等待；已登记（嵌套）或未登记邮箱时不动。登记失败只是退回旧行为。
/// 返回本次是否登记，嵌套的调用方据此只注销自己登记的等待。
pub(super) fn modal_enter(hwnd: HWND) -> bool {
    let event = WAKE.get();
    if event.is_null() || !WAIT.get().is_null() {
        return false;
    }
    let mut wait = null_mut();
    // SAFETY: 事件由登记的邮箱持有；上下文按值传窗口句柄，不指向栈或堆对象。
    if unsafe {
        RegisterWaitForSingleObject(
            &mut wait,
            event,
            Some(woken),
            hwnd as *const c_void,
            INFINITE,
            WT_EXECUTEONLYONCE,
        )
    } != 0
    {
        WAIT.set(wait);
        return true;
    }
    false
}
/// WM_EXITMENULOOP/WM_EXITSIZEMOVE：阻塞注销到回调结束（若已开始）。
pub(super) fn modal_exit() {
    let wait = WAIT.replace(null_mut());
    if !wait.is_null() {
        // SAFETY: wait 由 modal_enter 登记，只注销一次；回调只投递消息，阻塞等待不会死锁。
        unsafe { UnregisterWaitEx(wait, INVALID_HANDLE_VALUE) };
    }
}
/// UNBLOCK：自动复位事件已被等待消耗，先重新置上让事件循环照常取唤起或发现停止；
/// 仍在模态中才收起菜单并取消移动/缩放（迟到的 UNBLOCK 不打断之后的操作）。
pub(super) fn unblock(hwnd: HWND) {
    // SAFETY: 事件由登记的邮箱持有；EndMenu 与 WM_CANCELMODE 只作用于本线程的模态循环与鼠标捕获。
    unsafe {
        SetEvent(WAKE.get());
        if !WAIT.get().is_null() {
            EndMenu();
            SendMessageW(hwnd, WM_CANCELMODE, 0, 0);
        }
    }
}
