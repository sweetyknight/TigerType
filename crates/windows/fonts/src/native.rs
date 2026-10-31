//! 固定目录和只读文件句柄，避免摘要校验与 GDI 按路径打开之间被替换。
use crate::{Result, locked};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Graphics::Gdi::*,
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
    },
};

fn information(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: 句柄由 File 持有，输出为本地完整结构。
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(info)
}
/// 仅本进程可见且不参与枚举；窗口/HFONT 必须先释放，再释放此注册引用。
pub struct PrivateFont {
    path: Vec<u16>,
    // 禁止字体和路径组件被改写/改名，Drop 注销仍能使用原路径；不阻止目录内其他文件升级。
    _file: File,
    _directories: Vec<File>,
}
impl PrivateFont {
    pub fn load(path: &Path) -> Result<Self> {
        let (size, hash) = locked()?;
        let loaded = Self::verified(path, size, &hash)?;
        // 注册后立刻验证可选性与关键字根，失败发生在初始化，而不是第一键绘制时。
        let family: Vec<_> = crate::FAMILY.encode_utf16().chain(Some(0)).collect();
        // SAFETY: 临时 DC/HFONT 不逃出本函数，所有分支配对还原、释放。
        let usable = unsafe {
            let dc = CreateCompatibleDC(ptr::null_mut());
            if dc.is_null() {
                return Err("无法校验私有字体".into());
            }
            let font = CreateFontW(
                -20,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                0,
                0,
                family.as_ptr(),
            );
            if font.is_null() {
                DeleteDC(dc);
                return Err("无法选择私有文楷".into());
            }
            let old = SelectObject(dc, font);
            let mut actual = [0; 64];
            let n = GetTextFaceW(dc, 64, actual.as_mut_ptr());
            let mut glyphs = [0; 3];
            let roots = [0xe4e4, 0xe46a, 0xe432];
            let valid = n > 0
                && String::from_utf16_lossy(&actual[..n as usize - 1]) == crate::FAMILY
                && GetGlyphIndicesW(
                    dc,
                    roots.as_ptr(),
                    3,
                    glyphs.as_mut_ptr(),
                    GGI_MARK_NONEXISTING_GLYPHS,
                ) != u32::MAX
                && glyphs.iter().all(|g| *g != 0 && *g != 0xffff);
            SelectObject(dc, old);
            DeleteObject(font);
            DeleteDC(dc);
            valid
        };
        if !usable {
            return Err("私有文楷不可用或字根缺失，拒绝采用系统替代字体".into());
        }
        Ok(loaded)
    }
    fn verified(path: &Path, size: u64, hash: &str) -> Result<Self> {
        if !path.is_absolute() {
            return Err("字体必须来自绝对路径".into());
        }
        let mut directories = Vec::new();
        for ancestor in path
            .parent()
            .ok_or("字体没有父目录")?
            .ancestors()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            let file = OpenOptions::new()
                .access_mode(0)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(ancestor)?;
            let info = information(&file)?;
            if info.dwFileAttributes & 0x400 != 0 || info.dwFileAttributes & 0x10 == 0 {
                return Err("字体目录不得包含链接或非目录组件".into());
            }
            directories.push(file);
        }
        let mut file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let info = information(&file)?;
        if info.dwFileAttributes & (0x400 | 0x10) != 0 || info.nNumberOfLinks != 1 {
            return Err("字体不得为链接、硬链接或目录".into());
        }
        let length = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
        if length != size {
            return Err("随包文楷大小不符".into());
        }
        // 固定小缓冲流式校验，不为每个宿主保留一份 26MB 字体副本。
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        if format!("{:x}", digest.finalize()) != hash {
            return Err("随包文楷 SHA256 不符".into());
        }
        let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: 校验后的文件及各级目录仍被句柄固定；私有字体优先于同名系统字体。
        if unsafe { AddFontResourceExW(path.as_ptr(), FR_PRIVATE | FR_NOT_ENUM, ptr::null()) } == 0
        {
            return Err("无法私有加载随包文楷".into());
        }
        Ok(Self {
            path,
            _file: file,
            _directories: directories,
        })
    }
}
impl Drop for PrivateFont {
    fn drop(&mut self) {
        // SAFETY: 与成功的 AddFontResourceExW 配对；文件/目录句柄此时尚未释放。
        unsafe { RemoveFontResourceExW(self.path.as_ptr(), FR_PRIVATE | FR_NOT_ENUM, ptr::null()) };
    }
}
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
