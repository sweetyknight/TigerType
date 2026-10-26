//! 沙箱宿主前提实测：AppContainer 与低完整性子进程可连接并核对后台映像，Untrusted 级被拒，
//! 三者都不能为同名管道抢建实例。系统要求 AppContainer 已注册配置（仅派生 SID 时 CreateProcessW 报
//! ERROR_FILE_NOT_FOUND），故 AC 用例会创建并删除临时配置（当前用户注册表与 Packages 目录），默认忽略，
//! 须经用户授权后显式运行。测试程序复制到新建临时目录，只授予当前用户与 ALL APPLICATION PACKAGES。

use super::{
    security::{
        Identity, LocalMemory, grant_app_container_query, sid, sid_string, token_information,
    },
    *,
};
use crate::Message;
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};
use tigertype_contracts::{
    InputState,
    protocol::{Command, Reply, Response, Route},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ACCESS_DENIED},
    Security::{
        DuplicateTokenEx, FreeSid, GetLengthSid,
        Isolation::{CreateAppContainerProfile, DeleteAppContainerProfile},
        SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES, SecurityImpersonation, SetTokenInformation,
        TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL,
        TOKEN_QUERY, TokenIntegrityLevel, TokenIsAppContainer, TokenPrimary,
    },
    Storage::FileSystem::CreateDirectoryW,
    System::{
        SystemServices::SE_GROUP_INTEGRITY,
        Threading::{
            CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, CreateProcessW,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
            GetExitCodeProcess, InitializeProcThreadAttributeList, OpenProcessToken,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, STARTUPINFOEXW,
            STARTUPINFOW, TerminateProcess, UpdateProcThreadAttribute,
        },
    },
};

const WORKER: &str = "pipe::sandbox_tests::sandbox_worker";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sandbox {
    AppContainer,
    Low,
    Untrusted,
}
impl Sandbox {
    fn name(self) -> &'static str {
        match self {
            Self::AppContainer => "app-container",
            Self::Low => "low",
            Self::Untrusted => "untrusted",
        }
    }
    fn integrity(self) -> &'static str {
        if self == Self::Untrusted {
            "S-1-16-0"
        } else {
            "S-1-16-4096"
        }
    }
}

fn sequence() -> usize {
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

/// 自建子进程的唯一所有者；断言失败或超时都终止并回收，不触碰其他进程。
struct Child(OwnedHandle);
impl Child {
    fn adopt(info: PROCESS_INFORMATION) -> Self {
        // SAFETY: CreateProcess* 成功返回的两个新句柄各自只关闭一次；线程句柄不再需要。
        unsafe {
            CloseHandle(info.hThread);
            Self(OwnedHandle::from_raw_handle(info.hProcess))
        }
    }
    fn finish(self) -> Result<u32> {
        // SAFETY: 等待自己创建的进程，最长 15 秒；超时后 Drop 负责终止。
        if unsafe { WaitForSingleObject(self.0.as_raw_handle(), 15_000) } != WAIT_OBJECT_0 {
            return Err(Error::TimedOut);
        }
        let mut code = 0;
        // SAFETY: 进程句柄有效且已退出。
        if unsafe { GetExitCodeProcess(self.0.as_raw_handle(), &mut code) } == 0 {
            return Err(std_io::Error::last_os_error().into());
        }
        Ok(code)
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: 只终止并等待本测试创建的子进程；已退出时调用无副作用。
        unsafe {
            TerminateProcess(self.0.as_raw_handle(), 1);
            WaitForSingleObject(self.0.as_raw_handle(), 5_000);
        }
    }
}

/// AppContainer 可读执行的新建目录；只删除本测试复制的程序和空目录，不递归删除。
struct Staging {
    directory: PathBuf,
    image: PathBuf,
}
impl Staging {
    fn new() -> Result<Self> {
        let directory = std::env::temp_dir().join(format!(
            "tigertype-sandbox-{}-{}",
            std::process::id(),
            sequence()
        ));
        if !directory.is_absolute() {
            return Err(Error::Invalid("临时目录必须为绝对路径"));
        }
        let user = Identity::current()?.user;
        let sddl = wide(format!("D:P(A;OICI;FA;;;{user})(A;OICI;0x1200a9;;;AC)").as_ref());
        let mut descriptor = ptr::null_mut();
        // SAFETY: SDDL 以 NUL 结尾；成功输出由 LocalMemory 释放。
        if unsafe {
            windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                windows_sys::Win32::Security::Authorization::SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(std_io::Error::last_os_error().into());
        }
        let descriptor = LocalMemory(descriptor);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        // SAFETY: 新建目录，已存在则失败；不修改任何现有对象的权限。
        if unsafe { CreateDirectoryW(wide(directory.as_os_str()).as_ptr(), &attributes) } == 0 {
            return Err(std_io::Error::last_os_error().into());
        }
        let image = directory.join("sandbox-worker.exe");
        let staging = Self { directory, image };
        // 新文件继承目录 DACL，不复制原测试程序的安全描述符。
        std::fs::copy(std::env::current_exe()?, &staging.image)?;
        Ok(staging)
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.image);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

fn environment(label: &str, kind: Sandbox) -> Result<Vec<u16>> {
    let mut variables: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(key, _)| !key.to_string_lossy().starts_with("TIGERTYPE_SANDBOX_"))
        .collect();
    variables.push(("TIGERTYPE_SANDBOX_LABEL".into(), label.into()));
    variables.push(("TIGERTYPE_SANDBOX_KIND".into(), kind.name().into()));
    variables.push((
        "TIGERTYPE_SANDBOX_IMAGE".into(),
        std::env::current_exe()?.into(),
    ));
    // Windows 环境块按键名不区分大小写排序，以双 NUL 结束。
    variables.sort_by_key(|(key, _)| key.to_string_lossy().to_uppercase());
    let mut block = Vec::new();
    for (key, value) in variables {
        block.extend(key.encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

fn command_line(image: &Path) -> Vec<u16> {
    let mut line = OsString::from("\"");
    line.push(image);
    line.push(format!("\" --ignored --exact {WORKER} --nocapture"));
    wide(&line)
}

/// 本测试新建的临时 AppContainer 配置；子进程退出后删除。同名已存在即失败，绝不删除他人配置。
struct Profile {
    name: Vec<u16>,
    sid: *mut std::ffi::c_void,
}
impl Profile {
    fn new() -> Result<Self> {
        let name = wide(format!("TigerType.Test.{}.{}", std::process::id(), sequence()).as_ref());
        let mut sid = ptr::null_mut();
        // SAFETY: 名称以 NUL 结尾、无能力项；成功输出由 Drop 中的 FreeSid 释放。
        let result = unsafe {
            CreateAppContainerProfile(
                name.as_ptr(),
                name.as_ptr(),
                name.as_ptr(),
                ptr::null(),
                0,
                &mut sid,
            )
        };
        if result < 0 {
            return Err(std_io::Error::from_raw_os_error(result).into());
        }
        Ok(Self { name, sid })
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        // SAFETY: SID 与配置均由 new 成功创建，各释放一次。
        unsafe {
            FreeSid(self.sid);
            DeleteAppContainerProfile(self.name.as_ptr());
        }
    }
}

fn spawn_app_container(image: &Path, environment: &[u16], profile: &Profile) -> Result<Child> {
    let capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: profile.sid,
        Capabilities: ptr::null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    let mut size = 0;
    // SAFETY: 首次只查询属性表长度。
    unsafe {
        InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
    }
    let mut list = vec![0usize; size.div_ceil(size_of::<usize>())];
    // SAFETY: 缓冲区按查询长度分配并对齐；成功后在本函数末尾删除。
    if unsafe { InitializeProcThreadAttributeList(list.as_mut_ptr().cast(), 1, 0, &mut size) } == 0
    {
        return Err(std_io::Error::last_os_error().into());
    }
    let result = (|| {
        // SAFETY: capabilities 与 SID 在 CreateProcessW 返回前一直存活。
        if unsafe {
            UpdateProcThreadAttribute(
                list.as_mut_ptr().cast(),
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                (&capabilities as *const SECURITY_CAPABILITIES).cast(),
                size_of::<SECURITY_CAPABILITIES>(),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(std_io::Error::last_os_error().into());
        }
        let startup = STARTUPINFOEXW {
            StartupInfo: STARTUPINFOW {
                cb: size_of::<STARTUPINFOEXW>() as u32,
                ..Default::default()
            },
            lpAttributeList: list.as_mut_ptr().cast(),
        };
        let mut line = command_line(image);
        let mut info = PROCESS_INFORMATION::default();
        // SAFETY: 所有指针在调用期间有效；不继承句柄，环境块为本函数构造的 UTF-16。
        if unsafe {
            CreateProcessW(
                wide(image.as_os_str()).as_ptr(),
                line.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
                environment.as_ptr().cast(),
                ptr::null(),
                &startup.StartupInfo,
                &mut info,
            )
        } == 0
        {
            return Err(std_io::Error::last_os_error().into());
        }
        Ok(Child::adopt(info))
    })();
    // SAFETY: 属性表已成功初始化，只删除一次。
    unsafe {
        DeleteProcThreadAttributeList(list.as_mut_ptr().cast());
    }
    result
}

fn spawn_integrity(kind: Sandbox, environment: &[u16]) -> Result<Child> {
    let mut token = ptr::null_mut();
    // SAFETY: 只复制当前进程令牌；成功后由 RAII 回收。
    if unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY,
            &mut token,
        )
    } == 0
    {
        return Err(std_io::Error::last_os_error().into());
    }
    // SAFETY: API 成功返回新的令牌句柄。
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut primary = ptr::null_mut();
    // SAFETY: 复制为主令牌，仅降低完整性，不添加任何权限或组。
    if unsafe {
        DuplicateTokenEx(
            token.as_raw_handle(),
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
            ptr::null(),
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        )
    } == 0
    {
        return Err(std_io::Error::last_os_error().into());
    }
    // SAFETY: API 成功返回新的令牌句柄。
    let primary = unsafe { OwnedHandle::from_raw_handle(primary) };
    let level = sid(kind.integrity())?;
    let label = TOKEN_MANDATORY_LABEL {
        Label: SID_AND_ATTRIBUTES {
            Sid: level.0,
            Attributes: SE_GROUP_INTEGRITY as u32,
        },
    };
    // SAFETY: 标签结构后紧跟的 SID 长度来自同一 SID；降低完整性无需特权。
    if unsafe {
        SetTokenInformation(
            primary.as_raw_handle(),
            TokenIntegrityLevel,
            (&label as *const TOKEN_MANDATORY_LABEL).cast(),
            size_of::<TOKEN_MANDATORY_LABEL>() as u32 + GetLengthSid(level.0),
        )
    } == 0
    {
        return Err(std_io::Error::last_os_error().into());
    }
    let image = std::env::current_exe()?;
    let mut line = command_line(&image);
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: 令牌是当前令牌的降级副本，无需分配主令牌特权；不继承句柄。
    if unsafe {
        CreateProcessAsUserW(
            primary.as_raw_handle(),
            wide(image.as_os_str()).as_ptr(),
            line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            environment.as_ptr().cast(),
            ptr::null(),
            &startup,
            &mut info,
        )
    } == 0
    {
        return Err(std_io::Error::last_os_error().into());
    }
    Ok(Child::adopt(info))
}

fn request() -> Request {
    Request {
        route: Route {
            generation: 5,
            request_id: 1,
            session_id: 1,
        },
        command: Command::Clear,
    }
}

fn run_sandbox(kind: Sandbox) -> Result<()> {
    // 前提二：AC 宿主只能凭后台进程 DACL 中追加的受限查询授权核对映像。
    grant_app_container_query()?;
    let runtime = Runtime::new()?;
    let label = format!("sandbox-{}-{}", std::process::id(), sequence());
    let listener = runtime.bind(&label)?;
    // 双实例对象只用于检查抢建权限，避免单实例上限掩盖 DACL 缺陷。
    let identity = Identity::current()?;
    let squat = identity.name(&format!("{label}-squat"))?;
    let descriptor = identity.descriptor()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // SAFETY: 新建本测试专用实例，不发起 I/O；句柄由 RAII 回收。
    let first = unsafe {
        CreateNamedPipeW(
            squat.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_REJECT_REMOTE_CLIENTS,
            2,
            4096,
            4096,
            0,
            &attributes,
        )
    };
    if first == INVALID_HANDLE_VALUE {
        return Err(std_io::Error::last_os_error().into());
    }
    // SAFETY: 成功创建的句柄只由此处回收。
    let _first = unsafe { OwnedHandle::from_raw_handle(first) };
    let environment = environment(&label, kind)?;
    // 子进程后声明、先析构：失败路径也先终止子进程，再删除临时配置和目录。
    let sandbox = if kind == Sandbox::AppContainer {
        Some((Staging::new()?, Profile::new()?))
    } else {
        None
    };
    let child = match &sandbox {
        Some((staging, profile)) => spawn_app_container(&staging.image, &environment, profile)?,
        None => spawn_integrity(kind, &environment)?,
    };
    if kind != Sandbox::Untrusted {
        let mut pipe = listener.accept(Instant::now() + Duration::from_secs(10))?;
        assert_eq!(
            decode(&pipe.receive_frame(Instant::now() + Duration::from_secs(5))?)?,
            Message::Request(request())
        );
        pipe.send_frame(
            &crate::encode(&Message::Response(Response {
                route: request().route,
                reply: Reply::Cleared(InputState::default()),
            }))?,
            Instant::now() + Duration::from_secs(5),
        )?;
    }
    assert_eq!(child.finish()?, 0, "{} 沙箱子进程断言失败", kind.name());
    Ok(())
}

#[test]
#[ignore = "创建并删除临时 AppContainer 配置（当前用户注册表与 Packages 目录），须经用户授权后显式运行"]
fn app_container_host_connects_with_logon_sid_and_limited_query_but_cannot_squat() -> Result<()> {
    run_sandbox(Sandbox::AppContainer)
}

#[test]
fn low_integrity_host_connects_but_cannot_squat() -> Result<()> {
    run_sandbox(Sandbox::Low)
}

#[test]
fn untrusted_host_is_denied_by_the_low_label_and_cannot_squat() -> Result<()> {
    run_sandbox(Sandbox::Untrusted)
}

#[test]
#[ignore = "只由限时父测试以沙箱令牌启动"]
fn sandbox_worker() -> Result<()> {
    let Some(label) = std::env::var_os("TIGERTYPE_SANDBOX_LABEL") else {
        return Ok(());
    };
    let label = label
        .into_string()
        .map_err(|_| Error::Invalid("标签非法"))?;
    let kind = match std::env::var("TIGERTYPE_SANDBOX_KIND").as_deref() {
        Ok("app-container") => Sandbox::AppContainer,
        Ok("low") => Sandbox::Low,
        Ok("untrusted") => Sandbox::Untrusted,
        _ => return Err(Error::Invalid("沙箱类型非法")),
    };
    let image = PathBuf::from(OsString::from_wide(
        &std::env::var_os("TIGERTYPE_SANDBOX_IMAGE")
            .ok_or(Error::Invalid("缺少后台映像"))?
            .encode_wide()
            .collect::<Vec<_>>(),
    ));
    let mut token = ptr::null_mut();
    assert_ne!(
        // SAFETY: 只查询本进程令牌，确认确实运行在预期沙箱中。
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) },
        0
    );
    // SAFETY: API 成功返回新的令牌句柄。
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let (value, _) = token_information(token.as_raw_handle(), TokenIsAppContainer)?;
    assert_eq!(value[0] as u32 & 1 == 1, kind == Sandbox::AppContainer);
    let (value, size) = token_information(token.as_raw_handle(), TokenIntegrityLevel)?;
    assert!(size >= size_of::<TOKEN_MANDATORY_LABEL>());
    // SAFETY: 长度已覆盖标签结构，SID 指向同一缓冲区。
    let level = sid_string(
        unsafe { &*value.as_ptr().cast::<TOKEN_MANDATORY_LABEL>() }
            .Label
            .Sid,
    )?;
    assert_eq!(level, kind.integrity());

    // 前提一：沙箱令牌含登录 SID，才能算出同一管道名并通过登录 SID ACE。
    let identity = Identity::current()?;
    let squat = identity.name(&format!("{label}-squat"))?;
    // SAFETY: 尝试为父进程的专用对象再建实例，预期被 DACL 拒绝；意外成功也会回收句柄。
    let stolen = unsafe {
        CreateNamedPipeW(
            squat.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_REJECT_REMOTE_CLIENTS,
            2,
            4096,
            4096,
            0,
            ptr::null(),
        )
    };
    let error = std_io::Error::last_os_error();
    if stolen != INVALID_HANDLE_VALUE {
        // SAFETY: 失败路径仍回收意外取得的句柄。
        drop(unsafe { OwnedHandle::from_raw_handle(stolen) });
        panic!("沙箱进程抢建了管道实例");
    }
    assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));

    let runtime = Runtime::new()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let connected = runtime.connect(&label, &image, deadline);
    if kind == Sandbox::Untrusted {
        assert!(
            matches!(&connected, Err(Error::Io(error)) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32)),
            "Untrusted 级应被低完整性标签拒绝"
        );
        return Ok(());
    }
    let mut pipe = connected?;
    let reply = pipe.transact(&mut ClientState::new(5)?, request(), deadline)?;
    assert_eq!(reply.reply, Reply::Cleared(InputState::default()));
    Ok(())
}
