//! 打包时在本机绘制虎码印章；AppKit 使用 CoreGraphics/CoreText，不调用 iconutil 或外部脚本。
use objc2::{AnyThread, MainThreadMarker, runtime::AnyObject};
use objc2_app_kit::*;
use objc2_foundation::{
    NSAttributedString, NSDictionary, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use std::path::Path;
use tigertype_backend::{Result, resources, storage};
#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTFontManagerRegisterFontsForURL(
        url: *const AnyObject,
        scope: u32,
        error: *mut *const std::ffi::c_void,
    ) -> bool;
}
struct Graphics;
impl Drop for Graphics {
    fn drop(&mut self) {
        NSGraphicsContext::restoreGraphicsState_class();
    }
}
pub fn render(repository: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    let mtm = MainThreadMarker::new().ok_or("图标绘制须在主线程")?;
    let _app = NSApplication::sharedApplication(mtm);
    let font = crate::bundle::locked()?
        .into_iter()
        .find(|f| f.path.ends_with("LXGWWenKaiGBScreen.ttf"))
        .ok_or("缺少锁定文楷")?;
    let path = repository.join("resources/fonts/LXGWWenKaiGBScreen.ttf");
    let bytes = storage::read_file(&path, font.size)?;
    if bytes.len() as u64 != font.size || resources::sha256_bytes(&bytes) != font.sha256 {
        return Err("图标字体校验失败".into());
    }
    let url = NSURL::fileURLWithPath(&NSString::from_str(
        path.to_str().ok_or("字体路径不是 UTF-8")?,
    ));
    // SAFETY: NSURL 与 CFURL toll-free bridged；scope=1 仅本进程，错误不请求写回，不全机安装字体。
    unsafe {
        CTFontManagerRegisterFontsForURL((&*url as *const NSURL).cast(), 1, std::ptr::null_mut());
    }
    let mut body = Vec::new();
    for (size, kind) in [
        (16, *b"icp4"),
        (32, *b"icp5"),
        (64, *b"icp6"),
        (128, *b"ic07"),
        (256, *b"ic08"),
        (512, *b"ic09"),
        (1024, *b"ic10"),
    ] {
        let png = png(size, false)?;
        body.extend_from_slice(&kind);
        body.extend_from_slice(&u32::try_from(png.len() + 8)?.to_be_bytes());
        body.extend_from_slice(&png);
    }
    let mut icon = b"icns".to_vec();
    icon.extend_from_slice(&u32::try_from(body.len() + 8)?.to_be_bytes());
    icon.extend(body);
    Ok((icon, png(36, true)?))
}
fn png(size: usize, template: bool) -> Result<Vec<u8>> {
    let mut pixels = vec![0u8; size * size * 4];
    let mut plane = pixels.as_mut_ptr();
    // SAFETY: RGBA 缓冲区大小精确，位图释放早于 pixels；系统不接管外部内存。
    let bitmap = unsafe { NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(NSBitmapImageRep::alloc(), &mut plane, size as isize, size as isize, 8, 4, true, false, NSDeviceRGBColorSpace, (size * 4) as isize, 32) }.ok_or("无法创建图标位图")?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)
        .ok_or("无法创建图标绘制上下文")?;
    NSGraphicsContext::saveGraphicsState_class();
    let _restore = Graphics;
    NSGraphicsContext::setCurrentContext(Some(&context));
    let n = size as f64;
    let face = NSFont::fontWithName_size(&NSString::from_str("LXGW WenKai GB Screen"), n * 0.76)
        .ok_or("随包文楷未注册，拒绝以其他字形打包")?;
    let background = NSColor::colorWithSRGBRed_green_blue_alpha(0.76, 0.41, 0.12, 1.0);
    let foreground = if template {
        NSColor::blackColor()
    } else {
        NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.96, 0.86, 1.0)
    };
    let rect = NSRect::new(
        NSPoint::new(n * 0.05, n * 0.05),
        NSSize::new(n * 0.9, n * 0.9),
    );
    let border = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, n * 0.18, n * 0.18);
    if template {
        foreground.set();
        border.setLineWidth(n * 0.04);
        border.stroke();
    } else {
        background.set();
        border.fill();
    }
    // SAFETY: 字体和颜色分别对应系统属性键，字典由属性字符串复制持有。
    let text = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str("虎"),
            Some(&NSDictionary::from_slices(
                &[NSFontAttributeName, NSForegroundColorAttributeName],
                &[face.as_ref(), foreground.as_ref()],
            )),
        )
    };
    let extent = text.size();
    text.drawAtPoint(NSPoint::new(
        (n - extent.width) / 2.0,
        (n - extent.height) / 2.0,
    ));
    // SAFETY: PNG 使用空属性字典，编码在像素和位图仍存活时完成。
    Ok(unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .ok_or("图标 PNG 编码失败")?
    .to_vec())
}
