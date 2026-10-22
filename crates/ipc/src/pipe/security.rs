//! 本机管道只授权当前登录 SID，并通过存活进程句柄核对预期对端。

use crate::{Error, Result};
use std::{
    ffi::c_void,
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
    },
    ptr,
};
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE, LocalFree, WAIT_TIMEOUT},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        },
        GetTokenInformation, SID_AND_ATTRIBUTES, TOKEN_GROUPS, TOKEN_QUERY, TokenGroups,
        TokenSessionId,
    },
    Storage::FileSystem::{FILE_APPEND_DATA, FILE_GENERIC_READ, FILE_GENERIC_WRITE},
    System::{
        Pipes::{
            GetNamedPipeClientProcessId, GetNamedPipeClientSessionId, GetNamedPipeServerProcessId,
            GetNamedPipeServerSessionId,
        },
        SystemServices::SE_GROUP_LOGON_ID,
        Threading::{
            GetCurrentProcess, GetProcessId, OpenProcess, OpenProcessToken,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
            WaitForSingleObject,
        },
    },
};

// FILE_APPEND_DATA 与 FILE_CREATE_PIPE_INSTANCE 同值；客户端不能创建额外服务端实例。
pub(super) const CLIENT_ACCESS: u32 = (FILE_GENERIC_READ | FILE_GENERIC_WRITE) & !FILE_APPEND_DATA;

pub(super) struct LocalMemory(pub *mut c_void);
impl Drop for LocalMemory {
    fn drop(&mut self) {
        // SAFETY: 仅接管 SID/SDDL 转换 API 成功分配的 LocalAlloc 内存。
        unsafe {
            LocalFree(self.0);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Identity {
    sid: String,
    session: u32,
}

impl Identity {
    pub fn from_process(process: HANDLE) -> Result<Self> {
        let mut token = ptr::null_mut();
        // SAFETY: process 是调用方持有的进程句柄；成功后 token 交给 RAII。
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: API 成功返回新的非继承 token 句柄。
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut size = 0;
        // SAFETY: 首次仅查询缓冲区长度，不提供输出内存。
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenGroups,
                ptr::null_mut(),
                0,
                &mut size,
            );
        }
        if io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
            || size as usize > 64 * 1024
            || (size as usize) < size_of::<TOKEN_GROUPS>()
        {
            return Err(Error::Invalid("登录组信息长度不符"));
        }
        // usize 数组满足 TOKEN_GROUPS/SID_AND_ATTRIBUTES 在 x64/x86 上的对齐。
        let mut storage = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        let capacity = storage.len() * size_of::<usize>();
        // SAFETY: 输出区域可写、已对齐，长度不超过已分配容量。
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenGroups,
                storage.as_mut_ptr().cast(),
                capacity as u32,
                &mut size,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: API 成功返回至少一个 TOKEN_GROUPS 头；下面先验证变长数组边界。
        let groups = unsafe { &*storage.as_ptr().cast::<TOKEN_GROUPS>() };
        let offset = std::mem::offset_of!(TOKEN_GROUPS, Groups);
        if (size as usize) < size_of::<TOKEN_GROUPS>()
            || size as usize > capacity
            || groups.GroupCount as usize
                > (size as usize - offset) / size_of::<SID_AND_ATTRIBUTES>()
        {
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
        let mut sid = ptr::null_mut();
        // SAFETY: SID 来自刚取得的 token 组；输出在成功后由 LocalMemory 释放。
        if unsafe { ConvertSidToStringSidW(group.Sid, &mut sid) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        let _sid_memory = LocalMemory(sid.cast());
        let mut length = 0;
        // SAFETY: API 返回有效 NUL 结尾 SID 字符串，且其长度受 Windows SID 格式限制。
        while unsafe { *sid.add(length) } != 0 {
            length += 1;
        }
        // SAFETY: 已找到同一分配内的终止符，仅复制有效字符。
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(sid, length) })
            .map_err(|_| Error::Invalid("登录 SID 不是有效 UTF-16"))?;
        let mut session = 0u32;
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
        Ok(Self { sid, session })
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
        Ok(
            format!(r"\\.\pipe\TigerType-{}-{}-{label}", self.session, self.sid)
                .encode_utf16()
                .chain(Some(0))
                .collect(),
        )
    }

    pub fn descriptor(&self) -> Result<LocalMemory> {
        let sddl: Vec<_> = format!("D:P(A;;0x{CLIENT_ACCESS:08x};;;{})", self.sid)
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
}

pub struct Peer {
    process: OwnedHandle,
    id: u32,
    identity: Identity,
}

impl AsHandle for Peer {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.process.as_handle()
    }
}

impl Peer {
    pub fn same_process(&self, other: &Peer) -> Result<bool> {
        Ok(self.id == other.id
            && self.identity == other.identity
            && self.is_alive()?
            && other.is_alive()?)
    }
    pub fn verify_image(&self, expected: &std::path::Path) -> Result<()> {
        if !expected.is_absolute() {
            return Err(Error::Invalid("后台映像路径必须为绝对路径"));
        }
        let expected: Vec<_> = expected.as_os_str().encode_wide().collect();
        let mut image = vec![0u16; 32768];
        let mut length = image.len() as u32;
        // SAFETY: 仅查询已固定进程对象的映像路径；不按输入内容打开文件或信任自报 PID。
        if unsafe {
            QueryFullProcessImageNameW(
                self.process.as_raw_handle(),
                0,
                image.as_mut_ptr(),
                &mut length,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: 两个 UTF-16 缓冲区和长度匹配；按 Windows 序号忽略大小写比较，不使用语言相关折叠。
        if unsafe {
            windows_sys::Win32::Globalization::CompareStringOrdinal(
                image.as_ptr(),
                length as i32,
                expected.as_ptr(),
                expected.len() as i32,
                1,
            )
        } != windows_sys::Win32::Globalization::CSTR_EQUAL
        {
            return Err(Error::Invalid("后台进程映像不是预期程序"));
        }
        Ok(())
    }
    pub(super) fn from_pipe(pipe: HANDLE, local: &Identity, server: bool) -> Result<Self> {
        let mut pid = 0;
        // SAFETY: PID 只来自内核的管道对端查询，不来自消息或环境变量。
        let ok = unsafe {
            if server {
                GetNamedPipeClientProcessId(pipe, &mut pid)
            } else {
                GetNamedPipeServerProcessId(pipe, &mut pid)
            }
        };
        if ok == 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: 立即固定内核报告的存活进程；随后再次核对管道、登录 SID 和会话。
        let process = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if process.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: OpenProcess 的新句柄由唯一 RAII 所有者接管。
        let process = unsafe { OwnedHandle::from_raw_handle(process) };
        let peer = Self::from_process(process.as_handle())?;
        peer.verify(pipe, local, server)?;
        Ok(peer)
    }
    /// app 的宿主表只接受已取得可信进程句柄的同登录进程；不接受消息自报 PID。
    pub fn require_local(&self) -> Result<()> {
        // SAFETY: 只查询当前进程令牌，伪句柄不转移所有权。
        let local = Identity::from_process(unsafe { GetCurrentProcess() })?;
        if self.identity != local || !self.is_alive()? {
            return Err(Error::Invalid("宿主不是同登录的存活进程"));
        }
        Ok(())
    }

    pub fn is_alive(&self) -> Result<bool> {
        // SAFETY: 所有权句柄防止 PID 复用；一次查询而非忙轮询。
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
            WAIT_TIMEOUT => Ok(true),
            windows_sys::Win32::Foundation::WAIT_OBJECT_0 => Ok(false),
            _ => Err(io::Error::last_os_error().into()),
        }
    }

    /// 必须借用可信启动信息持有的进程句柄（如 Child），不能凭正文自报 PID 打开。
    /// 复制句柄而非稍后按 PID 重开，避免原进程已退出后 PID 被复用。
    pub fn from_process(process: BorrowedHandle<'_>) -> Result<Self> {
        let process = process.try_clone_to_owned()?;
        // SAFETY: 仅查询持有的进程对象；调用方需提供查询和 SYNCHRONIZE 权限。
        let id = unsafe { GetProcessId(process.as_raw_handle()) };
        if id == 0 {
            return Err(io::Error::last_os_error().into());
        }
        let identity = Identity::from_process(process.as_raw_handle())?;
        Ok(Self {
            process,
            id,
            identity,
        })
    }

    pub(super) fn verify(&self, pipe: HANDLE, local: &Identity, server: bool) -> Result<()> {
        let mut id = 0;
        let mut session = 0;
        // SAFETY: 管道与预期进程均由调用方持有；查询内核记录，不接受对端自报身份。
        let valid = unsafe {
            let queried = if server {
                GetNamedPipeClientProcessId(pipe, &mut id) != 0
                    && GetNamedPipeClientSessionId(pipe, &mut session) != 0
            } else {
                GetNamedPipeServerProcessId(pipe, &mut id) != 0
                    && GetNamedPipeServerSessionId(pipe, &mut session) != 0
            };
            if !queried {
                return Err(io::Error::last_os_error().into());
            }
            WaitForSingleObject(self.process.as_raw_handle(), 0) == WAIT_TIMEOUT
        };
        if !valid || id != self.id || session != local.session || self.identity != *local {
            return Err(Error::Invalid("管道对端不是预期的存活登录进程"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::Runtime;
    use super::*;
    use windows_sys::Win32::{
        Foundation::{ERROR_ACCESS_DENIED, INVALID_HANDLE_VALUE},
        Security::{
            ACCESS_ALLOWED_ACE,
            Authorization::{ConvertStringSidToSidW, GetSecurityInfo, SE_KERNEL_OBJECT},
            DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetSecurityDescriptorControl,
            SE_DACL_PROTECTED,
        },
        Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
        },
        System::{
            Pipes::{CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS},
            SystemServices::ACCESS_ALLOWED_ACE_TYPE,
        },
    };

    #[test]
    fn kernel_acl_only_grants_logon_sid_and_excludes_extra_server_instances() -> Result<()> {
        let runtime = Runtime::new()?;
        let label = format!("acl-{}", std::process::id());
        let listener = runtime.bind(&label)?;
        let mut descriptor = ptr::null_mut();
        let mut acl = ptr::null_mut();
        // SAFETY: 查询本测试创建的管道，不修改 ACL；成功返回的描述符由 LocalFree 回收。
        let error = unsafe {
            GetSecurityInfo(
                listener.0.io.handle.as_raw_handle(),
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut acl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32).into());
        }
        let _descriptor = LocalMemory(descriptor);
        assert!(!acl.is_null());
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: 描述符和 ACL 均由内核返回且 guard 尚未释放。
        unsafe {
            assert_ne!(
                GetSecurityDescriptorControl(descriptor, &mut control, &mut revision),
                0
            );
            assert_ne!(control & SE_DACL_PROTECTED, 0);
            assert_eq!((*acl).AceCount, 1);
        }
        let mut entry = ptr::null_mut();
        // SAFETY: ACL 至少含一个 ACE；GetAce 返回该描述符内部的有效指针。
        assert_ne!(unsafe { GetAce(acl, 0, &mut entry) }, 0);
        // SAFETY: 创建时仅放入 ACCESS_ALLOWED_ACE，先检查类型后访问允许 ACE 字段。
        let ace = unsafe { &*entry.cast::<ACCESS_ALLOWED_ACE>() };
        assert_eq!(ace.Header.AceType as u32, ACCESS_ALLOWED_ACE_TYPE);
        assert_eq!(ace.Mask, CLIENT_ACCESS);
        let text: Vec<_> = runtime
            .0
            .identity
            .sid
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut expected = ptr::null_mut();
        assert_ne!(
            // SAFETY: 期待 SID 来自本机令牌，转换输出单独回收。
            unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut expected) },
            0
        );
        let _expected = LocalMemory(expected);
        assert_ne!(
            // SAFETY: SidStart 是变长 SID 的起点；两份 SID 分配在调用期间都有效。
            unsafe { EqualSid((&ace.SidStart as *const u32).cast_mut().cast(), expected) },
            0
        );

        let name = runtime
            .0
            .identity
            .name(&format!("acl-instance-{}", std::process::id()))?;
        let descriptor = runtime.0.identity.descriptor()?;
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
}
