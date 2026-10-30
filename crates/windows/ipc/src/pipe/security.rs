//! 管道 DACL 授予登录 SID 与 ALL APPLICATION PACKAGES，低完整性标签允许沙箱写入。
//! 两侧都不读取对方令牌：后台只核对同会话；宿主核对管道所有者、会话和后台映像。

use crate::{Error, Result};
use std::{
    ffi::c_void,
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE, LocalFree},
    Globalization::{CSTR_EQUAL, CompareStringOrdinal},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            ConvertStringSidToSidW, EXPLICIT_ACCESS_W, GRANT_ACCESS, GetSecurityInfo,
            NO_MULTIPLE_TRUSTEE, SDDL_REVISION_1, SE_KERNEL_OBJECT, SetEntriesInAclW,
            SetSecurityInfo, TRUSTEE_IS_SID, TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
        },
        DACL_SECURITY_INFORMATION, GetTokenInformation, NO_INHERITANCE, OWNER_SECURITY_INFORMATION,
        PSID, SID_AND_ATTRIBUTES, TOKEN_GROUPS, TOKEN_INFORMATION_CLASS, TOKEN_QUERY, TOKEN_USER,
        TokenGroups, TokenSessionId, TokenUser,
    },
    Storage::FileSystem::{FILE_APPEND_DATA, FILE_GENERIC_READ, FILE_GENERIC_WRITE},
    System::{
        Pipes::{
            GetNamedPipeClientProcessId, GetNamedPipeClientSessionId, GetNamedPipeServerProcessId,
            GetNamedPipeServerSessionId, PeekNamedPipe,
        },
        SystemServices::SE_GROUP_LOGON_ID,
        Threading::{
            GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
        },
    },
};

// FILE_APPEND_DATA 与 FILE_CREATE_PIPE_INSTANCE 同值；客户端不能创建额外服务端实例。
pub(super) const CLIENT_ACCESS: u32 = (FILE_GENERIC_READ | FILE_GENERIC_WRITE) & !FILE_APPEND_DATA;
// ALL APPLICATION PACKAGES；不授予 LPAC（ALL RESTRICTED APPLICATION PACKAGES）或 Everyone。
const APP_PACKAGES: &str = "S-1-15-2-1";

pub(super) struct LocalMemory(pub *mut c_void);
impl Drop for LocalMemory {
    fn drop(&mut self) {
        // SAFETY: 仅接管 SID/SDDL/ACL 转换 API 成功分配的 LocalAlloc 内存。
        unsafe {
            LocalFree(self.0);
        }
    }
}

pub(super) fn sid_string(sid: PSID) -> Result<String> {
    let mut text = ptr::null_mut();
    // SAFETY: SID 来自内核返回的令牌或描述符，调用期间其所有者存活；输出由 LocalMemory 释放。
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let _memory = LocalMemory(text.cast());
    let mut length = 0;
    // SAFETY: API 返回 NUL 结尾字符串，长度受 Windows SID 格式限制。
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: 已找到同一分配内的终止符，仅复制有效字符。
    String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
        .map_err(|_| Error::Invalid("SID 不是有效 UTF-16"))
}

pub(super) fn sid(text: &str) -> Result<LocalMemory> {
    let text: Vec<_> = text.encode_utf16().chain(Some(0)).collect();
    let mut sid = ptr::null_mut();
    // SAFETY: 固定 SID 字符串以 NUL 结尾；成功输出由 LocalMemory 释放。
    if unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut sid) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(LocalMemory(sid))
}

/// 返回按 usize 对齐的令牌信息缓冲及实际长度；长度上限防止异常令牌导致无界分配。
pub(super) fn token_information(
    token: HANDLE,
    class: TOKEN_INFORMATION_CLASS,
) -> Result<(Vec<usize>, usize)> {
    let mut size = 0;
    // SAFETY: 首次仅查询缓冲区长度，不提供输出内存。
    unsafe {
        GetTokenInformation(token, class, ptr::null_mut(), 0, &mut size);
    }
    if io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        || size as usize > 64 * 1024
    {
        return Err(Error::Invalid("令牌信息长度不符"));
    }
    // usize 数组满足 TOKEN_GROUPS/TOKEN_USER 在 x64/x86 上的对齐。
    let mut storage = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    let capacity = storage.len() * size_of::<usize>();
    // SAFETY: 输出区域可写、已对齐，长度不超过已分配容量。
    if unsafe {
        GetTokenInformation(
            token,
            class,
            storage.as_mut_ptr().cast(),
            capacity as u32,
            &mut size,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    if size as usize > capacity {
        return Err(Error::Invalid("令牌信息越界"));
    }
    Ok((storage, size as usize))
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Identity {
    pub logon: String,
    pub user: String,
    pub session: u32,
}

impl Identity {
    /// 只查询当前进程自己的令牌；AppContainer/低完整性宿主同样可以读取。
    pub fn current() -> Result<Self> {
        let mut token = ptr::null_mut();
        // SAFETY: 当前进程伪句柄只借用；成功后 token 交给 RAII。
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: API 成功返回新的非继承 token 句柄。
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let (storage, size) = token_information(token.as_raw_handle(), TokenGroups)?;
        let offset = std::mem::offset_of!(TOKEN_GROUPS, Groups);
        if size < size_of::<TOKEN_GROUPS>() {
            return Err(Error::Invalid("登录组信息长度不符"));
        }
        // SAFETY: 已确认至少含一个 TOKEN_GROUPS 头；下面先验证变长数组边界。
        let groups = unsafe { &*storage.as_ptr().cast::<TOKEN_GROUPS>() };
        if groups.GroupCount as usize > (size - offset) / size_of::<SID_AND_ATTRIBUTES>() {
            return Err(Error::Invalid("登录组数组越界"));
        }
        // SAFETY: 数量已按实际返回字节验证；SID 指针由受信内核填充，storage 尚存活。
        let entries = unsafe {
            std::slice::from_raw_parts(
                storage
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<SID_AND_ATTRIBUTES>(),
                groups.GroupCount as usize,
            )
        };
        let group = entries
            .iter()
            .find(|entry| entry.Attributes & SE_GROUP_LOGON_ID as u32 == SE_GROUP_LOGON_ID as u32)
            .ok_or(Error::Invalid("当前令牌没有登录 SID"))?;
        let logon = sid_string(group.Sid)?;
        let (storage, size) = token_information(token.as_raw_handle(), TokenUser)?;
        if size < size_of::<TOKEN_USER>() {
            return Err(Error::Invalid("用户信息长度不符"));
        }
        // SAFETY: 长度已确认覆盖 TOKEN_USER；SID 指向同一缓冲区。
        let user = sid_string(unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() }.User.Sid)?;
        let mut session = 0u32;
        let mut size = 0;
        // SAFETY: TokenSessionId 的输出是 DWORD；不读取或改变个人配置。
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenSessionId,
                (&mut session as *mut u32).cast(),
                4,
                &mut size,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self {
            logon,
            user,
            session,
        })
    }

    pub fn name(&self, label: &str) -> Result<Vec<u16>> {
        if label.is_empty()
            || label.len() > 64
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(Error::Invalid("管道标签只允许 1–64 字节字母、数字和连字符"));
        }
        Ok(format!(
            r"\\.\pipe\TigerType-{}-{}-{label}",
            self.session, self.logon
        )
        .encode_utf16()
        .chain(Some(0))
        .collect())
    }

    pub fn descriptor(&self) -> Result<LocalMemory> {
        // AC 满足沙箱双重访问检查；低完整性 NW 标签放行 AppContainer，Untrusted 仍被拒绝写入。
        let sddl: Vec<_> = format!(
            "D:P(A;;0x{CLIENT_ACCESS:08x};;;{})(A;;0x{CLIENT_ACCESS:08x};;;AC)S:(ML;;NW;;;LW)",
            self.logon
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: SDDL 来自内核 SID 和固定权限；受保护 DACL 不继承 Everyone/其他用户权限。
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(LocalMemory(descriptor))
    }

    /// 后台接纳宿主：DACL 与远程拒绝已由内核执行，这里只核对同会话，不打开宿主进程或令牌。
    pub fn verify_client(&self, pipe: HANDLE) -> Result<()> {
        let mut session = 0;
        // SAFETY: 管道由调用方持有；会话号来自内核记录而非对端自报。
        if unsafe { GetNamedPipeClientSessionId(pipe, &mut session) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        if session != self.session {
            return Err(Error::Invalid("宿主不在本登录会话"));
        }
        Ok(())
    }

    /// 宿主核对后台：所有者是本用户（拒绝其他用户抢注）、服务端同会话、进程映像为预期程序。
    pub fn verify_server(&self, pipe: HANDLE, image: &Path) -> Result<()> {
        let mut owner = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        // SAFETY: 客户端权限含 READ_CONTROL；所有者指针位于返回的描述符内，由 guard 回收。
        let error = unsafe {
            GetSecurityInfo(
                pipe,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32).into());
        }
        let _descriptor = LocalMemory(descriptor);
        if owner.is_null() || sid_string(owner)? != self.user {
            return Err(Error::Invalid("管道所有者不是当前用户"));
        }
        let mut session = 0;
        // SAFETY: 管道由调用方持有，会话号来自内核记录。
        if unsafe { GetNamedPipeServerSessionId(pipe, &mut session) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        if session != self.session {
            return Err(Error::Invalid("后台不在本登录会话"));
        }
        verify_peer_image(pipe, true, image)
    }
}

/// 停止命令等后台退出用：只要等待权限，调用方随后须确认管道仍连通（对端存活则 PID 未被复用）。
/// 后台就在本进程时返回 None：无从等待自身退出。
pub(super) fn open_server_process(pipe: HANDLE) -> Result<Option<OwnedHandle>> {
    let mut pid = 0;
    // SAFETY: PID 只来自内核的管道对端查询，不来自消息或环境变量。
    if unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    if pid == std::process::id() {
        return Ok(None);
    }
    // SAFETY: 只请求 SYNCHRONIZE，不能读写或终止对端。
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: OpenProcess 的新句柄由唯一 RAII 所有者接管。
    Ok(Some(unsafe { OwnedHandle::from_raw_handle(process) }))
}

/// 以受限查询打开对端核对映像，不读取其令牌；宿主发现后台与后台核对停止命令共用。
pub(super) fn verify_peer_image(pipe: HANDLE, server: bool, expected: &Path) -> Result<()> {
    if !expected.is_absolute() {
        return Err(Error::Invalid("预期映像路径必须为绝对路径"));
    }
    let mut pid = 0;
    // SAFETY: PID 只来自内核的管道对端查询，不来自消息或环境变量。
    let ok = unsafe {
        if server {
            GetNamedPipeServerProcessId(pipe, &mut pid)
        } else {
            GetNamedPipeClientProcessId(pipe, &mut pid)
        }
    };
    if ok == 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: 只请求受限查询；沙箱宿主依赖后台进程 DACL 中追加的 AC 受限查询授权。
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: OpenProcess 的新句柄由唯一 RAII 所有者接管。
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    let expected: Vec<_> = expected.as_os_str().encode_wide().collect();
    let mut image = vec![0u16; 32768];
    let mut length = image.len() as u32;
    // SAFETY: 仅查询已打开进程对象的映像路径；缓冲区长度与参数一致。
    if unsafe {
        QueryFullProcessImageNameW(process.as_raw_handle(), 0, image.as_mut_ptr(), &mut length)
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: 两个 UTF-16 缓冲区和长度匹配；按 Windows 序号忽略大小写比较，不使用语言相关折叠。
    if unsafe {
        CompareStringOrdinal(
            image.as_ptr(),
            length as i32,
            expected.as_ptr(),
            expected.len() as i32,
            1,
        )
    } != CSTR_EQUAL
    {
        return Err(Error::Invalid("对端进程映像不是预期程序"));
    }
    // 打开后管道仍连通说明原对端仍存活，存活进程的 PID 不会被复用，故核对的就是管道对端。
    // SAFETY: 只查询连接状态，不读取或消费数据。
    if unsafe {
        PeekNamedPipe(
            pipe,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

/// 后台启动时调用：只向自身进程 DACL 追加 AC 受限查询，供沙箱宿主核对映像；进程退出即失效。
pub fn grant_app_container_query() -> Result<()> {
    let packages = sid(APP_PACKAGES)?;
    let mut dacl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // SAFETY: 当前进程伪句柄拥有 READ_CONTROL；返回的 DACL 位于描述符内，由 guard 回收。
    let error = unsafe {
        GetSecurityInfo(
            GetCurrentProcess(),
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32).into());
    }
    let _descriptor = LocalMemory(descriptor);
    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: PROCESS_QUERY_LIMITED_INFORMATION,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: NO_INHERITANCE,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
            ptstrName: packages.0.cast(),
        },
    };
    let mut updated = ptr::null_mut();
    // SAFETY: 旧 DACL 与 SID 在调用期间有效；GRANT_ACCESS 与已有授权合并，重复调用幂等。
    let error = unsafe { SetEntriesInAclW(1, &access, dacl, &mut updated) };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32).into());
    }
    let _updated = LocalMemory(updated.cast());
    // SAFETY: 只改当前进程对象的 DACL，不涉及文件、注册表或其他进程。
    let error = unsafe {
        SetSecurityInfo(
            GetCurrentProcess(),
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            updated,
            ptr::null(),
        )
    };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::Runtime;
    use super::*;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::{
        Foundation::{ERROR_ACCESS_DENIED, INVALID_HANDLE_VALUE},
        Security::{
            ACCESS_ALLOWED_ACE, ACL, EqualSid, GetAce, GetSecurityDescriptorControl,
            LABEL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        },
        Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
        },
        System::{
            Pipes::{CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS},
            SystemServices::{
                ACCESS_ALLOWED_ACE_TYPE, SYSTEM_MANDATORY_LABEL_ACE_TYPE,
                SYSTEM_MANDATORY_LABEL_NO_WRITE_UP,
            },
        },
    };

    /// ACE 的（类型、掩码、SID 字符串）。
    type Ace = (u32, u32, String);

    /// 读取对象 DACL 或 SACL 中的全部 ACE 及描述符控制位。
    fn aces(handle: HANDLE, information: u32) -> Result<(Vec<Ace>, u16)> {
        let mut dacl: *mut ACL = ptr::null_mut();
        let mut sacl: *mut ACL = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        // SAFETY: 只读查询调用方持有的对象；描述符由 LocalMemory 回收。
        let error = unsafe {
            GetSecurityInfo(
                handle,
                SE_KERNEL_OBJECT,
                information,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut dacl,
                &mut sacl,
                &mut descriptor,
            )
        };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32).into());
        }
        let _descriptor = LocalMemory(descriptor);
        let mut control = 0;
        let mut revision = 0;
        assert_ne!(
            // SAFETY: 描述符由内核返回且 guard 尚未释放。
            unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
            0
        );
        let acl = if information == DACL_SECURITY_INFORMATION {
            dacl
        } else {
            sacl
        };
        assert!(!acl.is_null());
        let mut result = Vec::new();
        // SAFETY: ACL 来自同一描述符且 guard 尚未释放。
        let count = unsafe { (*acl).AceCount };
        for index in 0..count {
            let mut entry = ptr::null_mut();
            // SAFETY: 索引小于 AceCount；GetAce 返回描述符内部的有效指针。
            assert_ne!(unsafe { GetAce(acl, index as u32, &mut entry) }, 0);
            // SAFETY: 允许 ACE 与强制标签 ACE 的头、掩码和 SidStart 布局相同。
            let ace = unsafe { &*entry.cast::<ACCESS_ALLOWED_ACE>() };
            let sid = (&ace.SidStart as *const u32).cast_mut().cast();
            result.push((ace.Header.AceType as u32, ace.Mask, sid_string(sid)?));
        }
        Ok((result, control))
    }

    #[test]
    fn pipe_acl_grants_logon_sid_and_app_packages_with_low_label_and_no_extra_instances()
    -> Result<()> {
        let runtime = Runtime::new()?;
        let label = format!("acl-{}", std::process::id());
        let listener = runtime.bind(&label)?;
        let handle = listener.0.io.handle.as_raw_handle();
        let identity = &runtime.0.identity;
        let (dacl, control) = aces(handle, DACL_SECURITY_INFORMATION)?;
        assert_ne!(control & SE_DACL_PROTECTED, 0);
        assert_eq!(
            dacl,
            vec![
                (
                    ACCESS_ALLOWED_ACE_TYPE,
                    CLIENT_ACCESS,
                    identity.logon.clone()
                ),
                (ACCESS_ALLOWED_ACE_TYPE, CLIENT_ACCESS, APP_PACKAGES.into()),
            ]
        );
        let (sacl, _) = aces(handle, LABEL_SECURITY_INFORMATION)?;
        assert_eq!(
            sacl,
            vec![(
                SYSTEM_MANDATORY_LABEL_ACE_TYPE,
                SYSTEM_MANDATORY_LABEL_NO_WRITE_UP,
                "S-1-16-4096".into()
            )]
        );

        let name = identity.name(&format!("acl-instance-{}", std::process::id()))?;
        let descriptor = identity.descriptor()?;
        let attributes = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        // SAFETY: 专用测试对象允许两个实例，排除容量限制掩盖 ACL 缺陷；没有发起 I/O。
        let first = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_REJECT_REMOTE_CLIENTS,
                2,
                4096,
                4096,
                0,
                &attributes,
            )
        };
        assert_ne!(first, INVALID_HANDLE_VALUE);
        // SAFETY: 第一个实例成功后由 RAII 回收，无挂起操作。
        let _first = unsafe { OwnedHandle::from_raw_handle(first) };
        // SAFETY: 必须用 CreateNamedPipe 检查创建实例权限；CreateFile 的客户端打开不是该操作。
        let denied = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                PIPE_REJECT_REMOTE_CLIENTS,
                2,
                4096,
                4096,
                0,
                &attributes,
            )
        };
        let error = io::Error::last_os_error();
        if denied != INVALID_HANDLE_VALUE {
            // SAFETY: 若测试失败，仍回收意外取得的句柄。
            drop(unsafe { OwnedHandle::from_raw_handle(denied) });
            panic!("登录 SID 意外取得创建额外实例的权限");
        }
        assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
        Ok(())
    }

    #[test]
    fn server_owned_by_another_user_or_session_or_image_is_rejected() -> Result<()> {
        let runtime = Runtime::new()?;
        let label = format!("owner-{}", std::process::id());
        let _listener = runtime.bind(&label)?;
        let image = std::env::current_exe()?;
        let pipe = runtime.connect(&label, &image, Instant::now() + Duration::from_secs(5))?;
        let handle = pipe.io.handle.as_raw_handle();
        let local = Identity::current()?;
        local.verify_server(handle, &image)?;
        // 无法在测试中创建另一账户；以不同的“本用户”身份核对同一管道，覆盖抢注者所有者不符的拒绝分支。
        let other = Identity {
            user: "S-1-5-18".into(),
            ..Identity::current()?
        };
        assert!(matches!(
            other.verify_server(handle, &image),
            Err(Error::Invalid(_))
        ));
        let other = Identity {
            session: local.session.wrapping_add(1),
            ..Identity::current()?
        };
        assert!(matches!(
            other.verify_server(handle, &image),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            local.verify_server(handle, &image.with_file_name("not-the-server.exe")),
            Err(Error::Invalid(_))
        ));
        assert!(
            local
                .verify_server(handle, Path::new("relative.exe"))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn process_grant_adds_only_limited_query_for_app_packages_and_is_idempotent() -> Result<()> {
        grant_app_container_query()?;
        grant_app_container_query()?;
        // SAFETY: 只读查询当前进程伪句柄。
        let (dacl, _) = aces(unsafe { GetCurrentProcess() }, DACL_SECURITY_INFORMATION)?;
        let granted: Vec<_> = dacl
            .iter()
            .filter(|(_, _, sid)| sid == APP_PACKAGES)
            .collect();
        assert_eq!(
            granted,
            vec![&(
                ACCESS_ALLOWED_ACE_TYPE,
                PROCESS_QUERY_LIMITED_INFORMATION,
                APP_PACKAGES.to_string()
            )]
        );
        let expected = sid(APP_PACKAGES)?;
        let parsed = sid(&granted[0].2)?;
        // SAFETY: 两个 SID 分配在调用期间都有效。
        assert_ne!(unsafe { EqualSid(expected.0, parsed.0) }, 0);
        Ok(())
    }
}
