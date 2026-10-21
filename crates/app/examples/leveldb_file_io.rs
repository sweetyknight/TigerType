//! 在合成目录复现 LevelDB 1.23 的 CURRENT 写入/替换，不加载或修改 rime.dll。
//! leveldb 模式对应官方 env_windows.cc；move 模式仅用于定位 API 差异，不能替代生产修复。

#[cfg(windows)]
fn main() -> tigerrime_app::Result<()> {
    use std::{
        ffi::CString,
        fs::File,
        io::Write,
        os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
        path::Path,
    };
    use windows_sys::Win32::{
        Foundation::GetLastError,
        Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MoveFileA, MoveFileExW, REPLACEFILE_IGNORE_MERGE_ERRORS,
            ReplaceFileA,
        },
    };

    let mode = std::env::args().nth(1).unwrap_or_else(|| "leveldb".into());
    if !matches!(mode.as_str(), "leveldb" | "move") {
        return Err("模式必须为 leveldb 或 move".into());
    }
    let iterations: u32 = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "1000".into())
        .parse()?;
    if !(1..=100_000).contains(&iterations) {
        return Err("循环次数必须为 1..100000".into());
    }
    let location = std::env::args().nth(3).unwrap_or_else(|| "project".into());
    let parent = match location.as_str() {
        "project" => tigerrime_app::project_root()?.join("artifacts"),
        "target" => tigerrime_app::project_root()?.join("target"),
        "cache" => tigerrime_app::project_root()?.join(".cache"),
        "temp" => std::env::temp_dir(),
        _ => return Err("位置只能为 project/target/cache/temp，不能指定现用数据目录".into()),
    };
    let work = tigerrime_app::resources::new_directory(&parent, "file-io")?;
    let current = work.join("CURRENT");
    eprintln!("诊断目录: {}，模式: {mode}", work.display());

    fn narrow(path: &Path) -> tigerrime_app::Result<CString> {
        let value = path.to_str().ok_or("诊断路径不是 UTF-8")?;
        Ok(CString::new(value.strip_prefix(r"\\?\").unwrap_or(value))?)
    }

    for iteration in 1..=iterations {
        let temporary = work.join(format!("{iteration:06}.dbtmp"));
        // 与 LevelDB 一样：独占写入、落盘、关闭句柄后才替换 CURRENT。
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(&temporary)?;
        writeln!(file, "MANIFEST-{iteration:06}")?;
        file.sync_all()?;
        drop(file);
        let error = if mode == "leveldb" {
            let from = narrow(&temporary)?;
            let to = narrow(&current)?;
            // SAFETY: 两个 C 字符串保持有效；操作仅限本次创建的合成目录。
            if unsafe { MoveFileA(from.as_ptr().cast(), to.as_ptr().cast()) } != 0 {
                continue;
            }
            // SAFETY: 必须立即保存上一次调用的错误，后续系统调用会覆盖它。
            let move_error = unsafe { GetLastError() };
            // SAFETY: 对应 LevelDB 1.23 的替换路径，不提供备份或回调指针。
            if unsafe {
                ReplaceFileA(
                    to.as_ptr().cast(),
                    from.as_ptr().cast(),
                    std::ptr::null(),
                    REPLACEFILE_IGNORE_MERGE_ERRORS,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            } != 0
            {
                continue;
            }
            // SAFETY: 获取紧邻失败调用的线程错误码，不依赖日志中的问号文本。
            let replace_error = unsafe { GetLastError() };
            eprintln!("iteration={iteration} MoveFileA={move_error} ReplaceFileA={replace_error}");
            if matches!(replace_error, 2 | 3) {
                move_error
            } else {
                replace_error
            }
        } else {
            let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = current.as_os_str().encode_wide().chain(Some(0)).collect();
            // SAFETY: 对照实验仅替换同目录文件，两个 UTF-16 路径以 NUL 结尾。
            if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_REPLACE_EXISTING) } != 0 {
                continue;
            }
            // SAFETY: 保存紧邻失败调用的错误码。
            unsafe { GetLastError() }
        };
        let message = format!(
            "mode={mode} iteration={iteration} code={error}: {}",
            std::io::Error::from_raw_os_error(error as i32)
        );
        eprintln!(
            "CURRENT metadata: {:?}; temporary metadata: {:?}",
            std::fs::metadata(&current),
            std::fs::metadata(&temporary)
        );
        eprintln!("CURRENT 占用查询（只读）: {:?}", lockers(&current));
        std::fs::write(work.join("failure.txt"), &message)?;
        return Err(message.into());
    }
    assert_eq!(
        std::fs::read_to_string(current)?,
        format!("MANIFEST-{iterations:06}\n")
    );
    println!("{mode}: {iterations} 次写入/替换通过");
    Ok(())
}

#[cfg(windows)]
fn lockers(path: &std::path::Path) -> tigerrime_app::Result<Vec<(u32, String)>> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::RestartManager::{
        RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources, RmStartSession,
    };
    struct Session(u32);
    impl Drop for Session {
        fn drop(&mut self) {
            // SAFETY: 只结束本工具的只读查询会话；不调用 RmShutdown / RmRestart。
            unsafe {
                RmEndSession(self.0);
            }
        }
    }
    let mut handle = 0;
    let mut key = [0_u16; 33];
    // SAFETY: 输出指针和 33 字符会话键缓冲区有效。
    let error = unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) };
    if error != 0 {
        return Err(std::io::Error::from_raw_os_error(error as i32).into());
    }
    let session = Session(handle);
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let names = [path.as_ptr()];
    // SAFETY: 仅注册本次合成 CURRENT 文件；路径内存在调用期间存活。
    let error = unsafe {
        RmRegisterResources(
            session.0,
            1,
            names.as_ptr(),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
        )
    };
    if error != 0 {
        return Err(std::io::Error::from_raw_os_error(error as i32).into());
    }
    let mut records = [RM_PROCESS_INFO::default(); 32];
    let mut needed = 0;
    let mut count = records.len() as u32;
    let mut reasons = 0;
    // SAFETY: 有界输出数组足够容纳 count 项；超出容量直接报告，不循环重试。
    let error = unsafe {
        RmGetList(
            session.0,
            &mut needed,
            &mut count,
            records.as_mut_ptr(),
            &mut reasons,
        )
    };
    if error != 0 {
        return Err(std::io::Error::from_raw_os_error(error as i32).into());
    }
    Ok(records[..count as usize]
        .iter()
        .map(|record| {
            let length = record
                .strAppName
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(record.strAppName.len());
            (
                record.Process.dwProcessId,
                String::from_utf16_lossy(&record.strAppName[..length]),
            )
        })
        .collect())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("此诊断仅支持 Windows");
}
