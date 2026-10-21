//! 启动阶段的进程/Job 边界；不允许在 TSF 按键回调中创建进程或等待回收。

use crate::Result;
use std::{
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr,
    time::{Duration, Instant},
};
use tigerrime_ipc::pipe::Peer;
use windows_sys::Win32::{
    Foundation::{
        DUPLICATE_SAME_ACCESS, DuplicateHandle, GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT,
        WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Security::SECURITY_ATTRIBUTES,
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
        Threading::{
            CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateEventW, CreateProcessW,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
            GetExitCodeProcess, GetProcessId, InitializeProcThreadAttributeList,
            LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, ResetEvent, ResumeThread,
            STARTF_USESTDHANDLES, STARTUPINFOEXW, SetEvent, TerminateProcess,
            UpdateProcThreadAttribute, WaitForMultipleObjects, WaitForSingleObject,
        },
    },
};

fn wide(value: &OsStr) -> Result<Vec<u16>> {
    let mut value: Vec<_> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err("启动参数含 NUL".into());
    }
    value.push(0);
    Ok(value)
}

fn command_line(args: &[OsString]) -> Result<Vec<u16>> {
    let mut command = Vec::new();
    for arg in args {
        if !command.is_empty() {
            command.push(b' ' as u16);
        }
        command.push(b'"' as u16);
        let mut slashes = 0;
        for unit in arg.encode_wide() {
            if unit == 0 {
                return Err("启动参数含 NUL".into());
            }
            if unit == b'\\' as u16 {
                slashes += 1;
                continue;
            }
            // Windows CRT 只在引号前将反斜杠成对解释；普通路径分隔符保持原样。
            let count = if unit == b'"' as u16 {
                slashes * 2 + 1
            } else {
                slashes
            };
            command.extend(std::iter::repeat_n(b'\\' as u16, count));
            command.push(unit);
            slashes = 0;
        }
        command.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
        command.push(b'"' as u16);
        if command.len() >= 32_767 {
            return Err("启动命令行长度超限".into());
        }
    }
    command.push(0);
    Ok(command)
}

fn inherited_copy(handle: HANDLE, access: Option<u32>) -> Result<OwnedHandle> {
    let mut copy = ptr::null_mut();
    // SAFETY: 只复制调用方仍持有的句柄；继承权限由本次显式 HANDLE_LIST 再限制。
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle,
            GetCurrentProcess(),
            &mut copy,
            access.unwrap_or(0),
            1,
            if access.is_some() {
                0
            } else {
                DUPLICATE_SAME_ACCESS
            },
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: 成功返回的新句柄由本对象独占回收。
    Ok(unsafe { OwnedHandle::from_raw_handle(copy) })
}

struct Attributes(Vec<u64>);
impl Attributes {
    fn new(handles: &[HANDLE]) -> Result<Self> {
        let mut size = 0;
        // SAFETY: 第一次调用仅查询长度，未提供输出区域。
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
        }
        if size == 0 || size > 64 * 1024 {
            return Err("启动属性长度不符".into());
        }
        let mut storage = vec![0u64; size.div_ceil(size_of::<u64>())];
        // SAFETY: 缓冲区按 8 字节对齐，覆盖系统要求的完整长度。
        if unsafe {
            InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 1, 0, &mut size)
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        let mut attributes = Self(storage);
        // SAFETY: handles 和对应的拥有者持续存活到 CreateProcess 返回；不继承其他句柄。
        if unsafe {
            UpdateProcThreadAttribute(
                attributes.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                size_of_val(handles),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(attributes)
    }

    fn pointer(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.0.as_mut_ptr().cast()
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: 只对成功初始化的属性列表执行配对销毁。
        unsafe {
            DeleteProcThreadAttributeList(self.pointer());
        }
    }
}

fn wait_millis(deadline: Instant) -> Result<u32> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))?;
    let millis = u32::try_from(remaining.as_nanos().div_ceil(1_000_000))?;
    if millis == u32::MAX {
        return Err("不允许 INFINITE 等待".into());
    }
    Ok(millis)
}

fn wait_handle(handle: HANDLE, deadline: Instant) -> Result<()> {
    let millis = wait_millis(deadline)?;
    // SAFETY: 调用方持有可等待句柄；等待有统一绝对期限，不忙轮询。
    match unsafe { WaitForSingleObject(handle, millis) } {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err(io::Error::from(io::ErrorKind::TimedOut).into()),
        _ => Err(io::Error::last_os_error().into()),
    }
}

pub struct JobChild {
    job: OwnedHandle,
    process: OwnedHandle,
    event: OwnedHandle,
}

impl AsHandle for JobChild {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.process.as_handle()
    }
}

impl JobChild {
    /// 追加父进程/事件两个继承句柄参数。调用方先验证数据目录，日志仅 create_new。
    pub fn spawn(
        binary: &Path,
        args: &[OsString],
        cwd: &Path,
        stdout: &Path,
        stderr: &Path,
    ) -> Result<Self> {
        if !binary.is_absolute() || !cwd.is_absolute() {
            return Err("启动路径必须是绝对路径".into());
        }
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(stdout)?;
        let errors = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(stderr)?;
        let input = File::open("NUL")?;
        let output = inherited_copy(output.as_raw_handle(), None)?;
        let errors = inherited_copy(errors.as_raw_handle(), None)?;
        let input = inherited_copy(input.as_raw_handle(), None)?;
        // SAFETY: 当前进程伪句柄仅用来产生可继承的最小查询权限副本，不转移伪句柄所有权。
        let parent = inherited_copy(
            unsafe { GetCurrentProcess() },
            Some(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE),
        )?;
        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            bInheritHandle: 1,
            lpSecurityDescriptor: ptr::null_mut(),
        };
        // SAFETY: 未命名自动复位事件只在父子之间继承，不允许第三方按名称接管。
        let event = unsafe { CreateEventW(&security, 0, 0, ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: 成功创建的事件由 OwnedHandle 管理。
        let event = unsafe { OwnedHandle::from_raw_handle(event) };
        // SAFETY: 创建不继承、未命名的专用 Job，避免第三方保留它而阻止父退出回收。
        let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: 成功创建的 Job 由当前对象独占持有。
        let job = unsafe { OwnedHandle::from_raw_handle(job) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: 仅设置生命周期标志；尚无完整资源基线，不伪造内存预算。
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        let handles = [
            parent.as_raw_handle(),
            event.as_raw_handle(),
            input.as_raw_handle(),
            output.as_raw_handle(),
            errors.as_raw_handle(),
        ];
        let mut attributes = Attributes::new(&handles)?;
        let mut startup = STARTUPINFOEXW {
            lpAttributeList: attributes.pointer(),
            ..Default::default()
        };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = input.as_raw_handle();
        startup.StartupInfo.hStdOutput = output.as_raw_handle();
        startup.StartupInfo.hStdError = errors.as_raw_handle();
        let mut full_args = vec![binary.as_os_str().to_owned()];
        full_args.extend_from_slice(args);
        full_args.extend([
            OsString::from((parent.as_raw_handle() as usize).to_string()),
            OsString::from((event.as_raw_handle() as usize).to_string()),
        ]);
        let mut command = command_line(&full_args)?;
        let binary = wide(binary.as_os_str())?;
        let cwd = wide(cwd.as_os_str())?;
        let mut info = PROCESS_INFORMATION::default();
        // SAFETY: 显式程序路径和有限 HANDLE_LIST；所有参数内存在同步调用期间有效。
        if unsafe {
            CreateProcessW(
                binary.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                CREATE_SUSPENDED | CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                ptr::null(),
                cwd.as_ptr(),
                &startup.StartupInfo,
                &mut info,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: 成功返回的进程和主线程句柄各由一个 RAII 所有者接管。
        let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
        // SAFETY: 主线程仍暂停，直到加入 Job 后才恢复。
        let thread = unsafe { OwnedHandle::from_raw_handle(info.hThread) };
        let child = Self {
            job,
            process,
            event,
        };
        // SAFETY: 必须在主线程恢复前加入 Job，防止启动到管理之间的逃逸窗口。
        if unsafe {
            AssignProcessToJobObject(child.job.as_raw_handle(), child.process.as_raw_handle())
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: 只恢复自己创建的主线程一次；异常时 child 的 Drop 终止仍暂停的进程。
        let previous = unsafe { ResumeThread(thread.as_raw_handle()) };
        if previous == u32::MAX {
            return Err(io::Error::last_os_error().into());
        }
        if previous != 1 {
            return Err("新建主线程的暂停计数异常".into());
        }
        Ok(child)
    }

    /// 自动复位：第一次是引擎就绪，测试故障触发时可再次通知，不混淆两个阶段。
    pub fn wait_signal(&self, deadline: Instant) -> Result<()> {
        let handles = [self.event.as_raw_handle(), self.process.as_raw_handle()];
        // SAFETY: 同时等就绪事件和进程退出；事件优先，保留故障点通知后立即退出的证据。
        match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, wait_millis(deadline)?) } {
            WAIT_OBJECT_0 => Ok(()),
            value if value == WAIT_OBJECT_0 + 1 => {
                Err(format!("子进程在发出信号前退出（代码 {}）", self.wait(deadline)?).into())
            }
            WAIT_TIMEOUT => Err(io::Error::from(io::ErrorKind::TimedOut).into()),
            _ => Err(io::Error::last_os_error().into()),
        }
    }

    pub fn wait(&self, deadline: Instant) -> Result<u32> {
        wait_handle(self.process.as_raw_handle(), deadline)?;
        let mut code = 0;
        // SAFETY: 已确认进程结束，保留实际退出码。
        if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(code)
    }

    pub fn terminate(&self) -> Result<()> {
        // SAFETY: 仅终止本对象创建的专用 Job，不枚举或操作用户其他进程。
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
}

impl Drop for JobChild {
    fn drop(&mut self) {
        // SAFETY: Job 包括所有子孙；直接终止兜住 Assign 失败但主线程仍暂停的路径。
        unsafe {
            TerminateJobObject(self.job.as_raw_handle(), 1);
            if WaitForSingleObject(self.process.as_raw_handle(), 0) != WAIT_OBJECT_0 {
                TerminateProcess(self.process.as_raw_handle(), 1);
                WaitForSingleObject(self.process.as_raw_handle(), 2_000);
            }
        }
    }
}

pub struct Bootstrap {
    pub parent: Peer,
    pub event: OwnedHandle,
}
impl Bootstrap {
    /// 内部子进程模式专用；只有确认继承标志、进程/事件类型后才接管原始句柄。
    /// # Safety
    /// 有效参数必须来自当前子进程的显式继承列表，且尚未交给其他 Rust 句柄所有者。
    pub unsafe fn take(parent: usize, event: usize) -> Result<Self> {
        if parent == event
            || [parent, event]
                .iter()
                .any(|value| *value == 0 || *value == usize::MAX)
        {
            return Err("无效的启动句柄".into());
        }
        for value in [parent, event] {
            let mut flags = 0;
            // SAFETY: Windows 对原始数值执行内核句柄校验，不在用户态解引用。
            if unsafe { GetHandleInformation(value as HANDLE, &mut flags) } == 0 {
                return Err(io::Error::last_os_error().into());
            }
            if flags & HANDLE_FLAG_INHERIT == 0 {
                return Err("启动句柄不是继承句柄".into());
            }
        }
        // SAFETY: 检查句柄类型；不把任意整数直接包装成 BorrowedHandle。
        let id = unsafe { GetProcessId(parent as HANDLE) };
        if id == 0 || id == std::process::id() {
            return Err("启动父进程身份不符".into());
        }
        // SAFETY: ResetEvent 同时验证事件类型/权限；事件初始为无信号，父端不会主动置位。
        if unsafe { ResetEvent(event as HANDLE) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: 这两个已验证的继承句柄只由内部入口接管一次，数值也已检查互不相同。
        let parent = unsafe { OwnedHandle::from_raw_handle(parent as HANDLE) };
        // SAFETY: 同上，事件句柄单独拥有，不与其他 Rust 对象共享所有权。
        let event = unsafe { OwnedHandle::from_raw_handle(event as HANDLE) };
        Ok(Self {
            parent: Peer::from_process(parent.as_handle())?,
            event,
        })
    }
}

pub fn signal(event: &OwnedHandle) -> Result<()> {
    // SAFETY: 调用方持有私有自动复位事件；SetEvent 验证类型，不传递输入内容。
    if unsafe { SetEvent(event.as_raw_handle()) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

/// 限时父测试的无 DLL 等待模式，只验证启动继承与 Job 回收。
/// # Safety
/// 仅在子进程入口调用一次；有效的原始句柄必须满足 Bootstrap::take 的所有权约束。
pub unsafe fn probe_idle(parent: usize, event: usize) -> Result<()> {
    // SAFETY: 子进程入口保证继承句柄尚无 Rust 所有者；take 进一步校验类型和继承标志。
    let bootstrap = unsafe { Bootstrap::take(parent, event)? };
    signal(&bootstrap.event)?;
    std::thread::sleep(Duration::from_secs(60));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_preserves_quotes_spaces_unicode_and_trailing_slashes() {
        let args: Vec<OsString> = ["a b", "", "中😀", "x\"y", "D:\\dir\\"]
            .into_iter()
            .map(Into::into)
            .collect();
        let value = command_line(&args).unwrap();
        assert_eq!(
            String::from_utf16(&value[..value.len() - 1]).unwrap(),
            "\"a b\" \"\" \"中😀\" \"x\\\"y\" \"D:\\dir\\\\\""
        );
        assert!(command_line(&[OsString::from("bad\0arg")]).is_err());
        assert!(command_line(&[OsString::from("a".repeat(32_767))]).is_err());
    }

    #[test]
    fn bootstrap_rejects_missing_duplicate_or_pseudo_handles() {
        for (parent, event) in [(0, 1), (1, 1), (usize::MAX, 1)] {
            // SAFETY: 这些合成无效值都会在接管任何句柄之前被拒绝。
            assert!(unsafe { Bootstrap::take(parent, event) }.is_err());
        }
    }
}
