//! 输入法只用安装器发布的系统字体，启动时核对；进程私有注册只给打包绘图标与预览用，
//! 这些场景不能要求机器先装好字体，且不让 CoreText 再按可变路径读取。
use crate::font_file::{LEARNING_FAMILY, LEARNING_ROOTS};
use objc2_app_kit::NSFont;
use objc2_foundation::NSString;
use std::{ffi::c_void, path::Path, ptr};
use tigertype_backend::{Result, resources, storage};

/// 接 IMK 回调前核对文楷与关键字根：字根是私用区字形，任何替代字体都只显示方块。
pub fn ensure_learning_font() -> Result<()> {
    let name = NSString::from_str(LEARNING_FAMILY);
    let rooted = NSFont::fontWithName_size(&name, 20.0).is_some_and(|font| {
        let covered = font.coveredCharacterSet();
        LEARNING_ROOTS
            .iter()
            .all(|root| covered.longCharacterIsMember(*root))
    });
    if rooted {
        return Ok(());
    }
    Err(format!("系统缺少含虎码字根的 {LEARNING_FAMILY}，请重新运行安装").into())
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> *const c_void;
    fn CFRelease(value: *const c_void);
    fn CFErrorGetCode(error: *const c_void) -> isize;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDataProviderCreateWithCFData(data: *const c_void) -> *const c_void;
    fn CGDataProviderRelease(provider: *const c_void);
    fn CGFontCreateWithDataProvider(provider: *const c_void) -> *const c_void;
    fn CGFontRelease(font: *const c_void);
}
#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTFontManagerRegisterGraphicsFont(font: *const c_void, error: *mut *const c_void) -> bool;
    fn CTFontManagerUnregisterGraphicsFont(font: *const c_void, error: *mut *const c_void) -> bool;
}
pub struct PrivateFont(GraphicsFont);
/// 只解码同一份字节，不注册字体；安装判重和私有注册共用这个加载边界。
struct GraphicsFont(*const c_void);
impl GraphicsFont {
    fn new(bytes: &[u8]) -> Result<Self> {
        // SAFETY: CFData 复制有界字节；provider/font 逐层持有引用，各失败路径释放已创建对象。
        unsafe {
            let data = CFDataCreate(ptr::null(), bytes.as_ptr(), bytes.len().try_into()?);
            if data.is_null() {
                return Err("无法创建字体数据".into());
            }
            let provider = CGDataProviderCreateWithCFData(data);
            CFRelease(data);
            if provider.is_null() {
                return Err("无法创建字体数据源".into());
            }
            let font = CGFontCreateWithDataProvider(provider);
            CGDataProviderRelease(provider);
            if font.is_null() {
                return Err("字体无法加载".into());
            }
            Ok(Self(font))
        }
    }
}
impl Drop for GraphicsFont {
    fn drop(&mut self) {
        // SAFETY: 只释放本对象持有的创建引用，未注册字体也走同一释放路径。
        unsafe { CGFontRelease(self.0) };
    }
}
pub(crate) fn validate(bytes: &[u8]) -> Result<()> {
    GraphicsFont::new(bytes).map(|_| ())
}
impl PrivateFont {
    fn new(bytes: &[u8]) -> Result<Self> {
        let font = GraphicsFont::new(bytes)?;
        // SAFETY: 解码对象存活至注册完成；失败自动释放，成功由 PrivateFont 持有。
        unsafe {
            let mut error = ptr::null();
            if !CTFontManagerRegisterGraphicsFont(font.0, &mut error) {
                let code = if error.is_null() {
                    0
                } else {
                    let code = CFErrorGetCode(error);
                    CFRelease(error);
                    code
                };
                return Err(format!(
                    "进程私有字体注册失败（CoreText {code}），不使用同名系统字体替代"
                )
                .into());
            }
            Ok(Self(font))
        }
    }
}
impl Drop for PrivateFont {
    fn drop(&mut self) {
        // SAFETY: 成功注册的字体仅注销一次；显示组件先销毁，随后释放自有 CGFont 引用。
        unsafe {
            CTFontManagerUnregisterGraphicsFont(self.0.0, ptr::null_mut());
        }
    }
}
pub fn load(directory: &Path) -> Result<Vec<PrivateFont>> {
    let mut fonts = Vec::new();
    for entry in crate::bundle::locked()?
        .into_iter()
        .filter(|e| e.path.starts_with("Contents/Resources/Fonts/"))
    {
        let name = Path::new(&entry.path).file_name().ok_or("字体名称缺失")?;
        let bytes = storage::read_file(&directory.join(name), entry.size)?;
        if bytes.len() as u64 != entry.size || resources::sha256_bytes(&bytes) != entry.sha256 {
            return Err(format!("随包字体校验失败：{}", entry.path).into());
        }
        fonts.push(PrivateFont::new(&bytes)?);
    }
    // 在接收 IMK/UI 回调前确认 AppKit 能选择它，不把注册成功等同于名称可用。
    if NSFont::fontWithName_size(&NSString::from_str(LEARNING_FAMILY), 20.0).is_none() {
        return Err("私有文楷已注册但 AppKit 无法选择".into());
    }
    Ok(fonts)
}
