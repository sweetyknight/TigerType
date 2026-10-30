//! 专供限时子进程加载的 DLL 驻留探针；不注册 COM，不触碰真实输入宿主。
#![cfg(windows)]
use std::{
    panic::catch_unwind,
    time::{Duration, Instant},
};
use tigertype_windows_ipc::{Error, pipe::Runtime};

fn probe() -> Result<(), Box<dyn std::error::Error>> {
    // 默认入口即使模块已经 PIN 也不得悄悄放宽为 DLL 入口。
    if Runtime::new().is_ok() {
        return Err("默认 EXE 限制失效".into());
    }
    let runtime = Runtime::new_process_resident()?;
    for index in 0..16 {
        let listener = runtime.bind(&format!("dll-probe-{}-{index}", std::process::id()))?;
        // 探针不接收其他进程，等待故意在连接完成前到期。
        if !matches!(
            listener.accept(Instant::now() + Duration::from_millis(1)),
            Err(Error::TimedOut)
        ) {
            return Err("挂起 accept 未按期取消".into());
        }
    }
    runtime.shutdown(Instant::now() + Duration::from_secs(3))?;
    if !matches!(runtime.bind("must-remain-closed"), Err(Error::Closed)) {
        return Err("关闭后的运行时重新准入".into());
    }
    Ok(())
}

// SAFETY: 无参数、无跨 ABI 分配；错误和 panic 都转换为有限状态码，不在 DllMain 执行。
#[unsafe(no_mangle)]
pub extern "system" fn TigerTypeResidentProbe() -> u32 {
    match catch_unwind(probe) {
        Ok(Ok(())) => 0,
        Ok(Err(_)) => 1,
        Err(_) => 2,
    }
}
