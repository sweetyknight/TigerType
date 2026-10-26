//! 虎码印章图标：琥珀圆角方块、两侧虎纹、米白霞鹜文楷“虎”字（字体缺失时退到楷体）。
//! 只用 GDI：在 4 倍超采样画布上分别画出底板、虎纹与字形的覆盖率，缩小后合成非预乘 ARGB；
//! 运行时取 HICON（通知区、窗口图标），打包时写 .ico（输入法配置图标、开始菜单）。
use super::*;

const AMBER: u32 = 0xB45309;
const STRIPE: u32 = 0x93400A;
const CREAM: u32 = 0xFFF8EE;
const SUPERSAMPLE: i32 = 4;
/// .ico 收录的尺寸；任务栏、开始菜单与资源管理器按 DPI 取用。
const SIZES: [i32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];
/// 虎纹：100 单位坐标，起点加两段三次贝塞尔围成尖条；右三条、左两条，与设计稿一致。
const STRIPES: [[(f32, f32); 7]; 5] = [
    [
        (100.0, 15.0),
        (91.0, 18.0),
        (84.0, 23.0),
        (79.0, 30.0),
        (86.0, 28.0),
        (93.0, 27.5),
        (100.0, 28.5),
    ],
    [
        (100.0, 42.0),
        (90.0, 44.0),
        (82.0, 48.5),
        (76.0, 55.0),
        (84.0, 53.5),
        (92.0, 53.0),
        (100.0, 54.5),
    ],
    [
        (100.0, 70.0),
        (92.0, 72.0),
        (86.0, 76.0),
        (82.0, 81.5),
        (88.0, 80.0),
        (94.0, 79.5),
        (100.0, 80.5),
    ],
    [
        (0.0, 28.0),
        (8.0, 30.0),
        (14.0, 34.0),
        (19.0, 40.0),
        (13.0, 38.5),
        (6.0, 38.0),
        (0.0, 38.5),
    ],
    [
        (0.0, 57.0),
        (9.0, 59.0),
        (16.0, 63.0),
        (21.0, 69.5),
        (14.0, 68.0),
        (7.0, 67.5),
        (0.0, 68.0),
    ],
];
const FACES: [&str; 3] = ["LXGW WenKai GB Screen", "KaiTi", UI_FACE];

/// 32 位自上而下 DIB 画布；Drop 时配对释放。
struct Canvas {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    side: i32,
}
impl Canvas {
    fn new(side: i32) -> Option<Self> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: side,
                // 负高度为自上而下的行序。
                biHeight: -side,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: 新建内存 DC 与 DIB，失败时释放已建对象；像素缓冲归 DIB，随其释放。
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return None;
            }
            let mut bits = null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if bitmap.is_null() || bits.is_null() {
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
                DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bitmap);
            Some(Self {
                dc,
                bitmap,
                old,
                bits: bits.cast(),
                side,
            })
        }
    }
    fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: DIB 像素缓冲有 side² 个 32 位像素，在本对象存活期间有效且只经此借用。
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.side * self.side) as usize) }
    }
    /// 清成黑底，用白色画刷、无画笔执行 draw，返回每像素覆盖率（取最亮通道）。
    fn pass(&mut self, draw: impl FnOnce(HDC)) -> Vec<u8> {
        self.pixels().fill(0);
        // SAFETY: 只选入库存画刷/画笔，绘制后还原；GdiFlush 保证读像素前绘制完成。
        unsafe {
            SetDCBrushColor(self.dc, 0x00FF_FFFF);
            let brush = SelectObject(self.dc, GetStockObject(DC_BRUSH));
            let pen = SelectObject(self.dc, GetStockObject(NULL_PEN));
            draw(self.dc);
            GdiFlush();
            SelectObject(self.dc, pen);
            SelectObject(self.dc, brush);
        }
        self.pixels()
            .iter()
            .map(|pixel| {
                (pixel >> 16 & 0xFF)
                    .max(pixel >> 8 & 0xFF)
                    .max(pixel & 0xFF) as u8
            })
            .collect()
    }
}
impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: 还原原位图后删除自建位图与 DC。
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

/// 灰度抗锯齿字体（ClearType 彩边会混进覆盖率）；按优先顺序核对实际字体名。
fn glyph_font(dc: HDC, pixels: i32) -> HFONT {
    for (index, face) in FACES.iter().enumerate() {
        // SAFETY: 名称 NUL 结尾；字体归调用方释放。
        let font = unsafe {
            CreateFontW(
                -pixels,
                0,
                0,
                0,
                FW_NORMAL as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                ANTIALIASED_QUALITY as u32,
                0,
                wide(face).as_ptr(),
            )
        };
        if !font.is_null() && (index + 1 == FACES.len() || face_is(dc, font, face)) {
            return font;
        }
        if !font.is_null() {
            // SAFETY: 字体只在 face_is 内临时选入并已还原。
            unsafe { DeleteObject(font) };
        }
    }
    null_mut()
}

/// 按字形墨迹框居中，返回基线起点：GDI 的垂直居中按字符单元算，汉字会偏下。
/// 取不到字形度量时退回按字身估算。
/// SAFETY: 调用方保证 dc 有效且已选入目标字体。
pub(super) unsafe fn centered(dc: HDC, glyph: u16, side: i32) -> (i32, i32) {
    let one = FIXED { fract: 0, value: 1 };
    let zero = FIXED { fract: 0, value: 0 };
    let identity = MAT2 {
        eM11: one,
        eM12: zero,
        eM21: zero,
        eM22: one,
    };
    // SAFETY: GLYPHMETRICS 为纯数据，全零合法。
    let mut metrics: GLYPHMETRICS = unsafe { std::mem::zeroed() };
    // SAFETY: 只取度量（GGO_METRICS 不写缓冲），输出结构为本地值。
    let read = unsafe {
        GetGlyphOutlineW(
            dc,
            glyph.into(),
            GGO_METRICS,
            &mut metrics,
            0,
            null_mut(),
            &identity,
        )
    };
    if read == GDI_ERROR as u32 {
        return (side / 8, side * 7 / 8);
    }
    let (width, height) = (metrics.gmBlackBoxX as i32, metrics.gmBlackBoxY as i32);
    let origin = metrics.gmptGlyphOrigin;
    (
        (side - width) / 2 - origin.x,
        (side - height) / 2 + origin.y,
    )
}

/// 逐通道线性混合，t 为 0–255；两端无蓝则结果无蓝。
const fn blend(from: u32, to: u32, t: u32) -> u32 {
    let mut rgb = 0;
    let mut shift = 0;
    while shift < 24 {
        let (a, b) = (from >> shift & 0xFF, to >> shift & 0xFF);
        rgb |= ((a * (255 - t) + b * t) / 255) << shift;
        shift += 8;
    }
    rgb
}

/// 渲染 side×side 的非预乘 ARGB（自上而下）；小尺寸不画虎纹、字身加大，保证 16px 仍认得出。
fn render(side: i32) -> Option<Vec<u32>> {
    let big = side * SUPERSAMPLE;
    let mut canvas = Canvas::new(big)?;
    let radius = big * if side <= 20 { 18 } else { 22 } / 100;
    // SAFETY: 画布 DC 有效；圆角矩形多给 1 像素抵消无画笔时少画的右/下边。
    let shape = canvas.pass(|dc| unsafe {
        RoundRect(dc, 0, 0, big + 1, big + 1, radius * 2, radius * 2);
    });
    let stripes = if side >= 48 {
        let scale = |(x, y): (f32, f32)| POINT {
            x: (x * big as f32 / 100.0).round() as i32,
            y: (y * big as f32 / 100.0).round() as i32,
        };
        // SAFETY: 路径在本次调用内开始、结束并填充；点数组为本地值。
        canvas.pass(|dc| unsafe {
            BeginPath(dc);
            for stripe in STRIPES {
                let start = scale(stripe[0]);
                MoveToEx(dc, start.x, start.y, null_mut());
                let curve: Vec<POINT> = stripe[1..].iter().map(|&p| scale(p)).collect();
                PolyBezierTo(dc, curve.as_ptr(), curve.len() as u32);
                CloseFigure(dc);
            }
            EndPath(dc);
            FillPath(dc);
        })
    } else {
        vec![0; (big * big) as usize]
    };
    let ratio = match side {
        48.. => 64,
        32..48 => 72,
        _ => 80,
    };
    let glyph = canvas.pass(|dc| {
        let font = glyph_font(dc, big * ratio / 100);
        if font.is_null() {
            return;
        }
        let text = [0x864E_u16];
        // SAFETY: 字体选入后还原并删除；文字缓冲为本地值。
        unsafe {
            let old = SelectObject(dc, font);
            SetBkMode(dc, TRANSPARENT as _);
            SetTextColor(dc, 0x00FF_FFFF);
            let (x, y) = centered(dc, text[0], big);
            SetTextAlign(dc, TA_BASELINE | TA_LEFT);
            TextOutW(dc, x, y, text.as_ptr(), 1);
            SelectObject(dc, old);
            DeleteObject(font);
        }
    });
    let average = |plane: &[u8], x: i32, y: i32| {
        let mut sum = 0u32;
        for dy in 0..SUPERSAMPLE {
            let row = ((y * SUPERSAMPLE + dy) * big + x * SUPERSAMPLE) as usize;
            sum += plane[row..row + SUPERSAMPLE as usize]
                .iter()
                .map(|&v| u32::from(v))
                .sum::<u32>();
        }
        sum / (SUPERSAMPLE * SUPERSAMPLE) as u32
    };
    let mut pixels = Vec::with_capacity((side * side) as usize);
    for y in 0..side {
        for x in 0..side {
            let alpha = average(&shape, x, y);
            let color = blend(
                blend(AMBER, STRIPE, average(&stripes, x, y)),
                CREAM,
                average(&glyph, x, y),
            );
            pixels.push(if alpha == 0 { 0 } else { alpha << 24 | color });
        }
    }
    Some(pixels)
}

/// 运行时图标；HICON 归调用方 DestroyIcon。
pub(super) fn icon(side: i32) -> Option<HICON> {
    let pixels = render(side)?;
    let mut canvas = Canvas::new(side)?;
    canvas.pixels().copy_from_slice(&pixels);
    // 单色掩码全零：透明度完全由彩色位图的 alpha 决定；行按 16 位对齐。
    let mask_bits = vec![0u8; (side as usize).div_ceil(16) * 2 * side as usize];
    // SAFETY: 掩码位图在本函数内创建并删除；CreateIconIndirect 复制两张位图，之后即可释放。
    unsafe {
        let mask = CreateBitmap(side, side, 1, 1, mask_bits.as_ptr().cast());
        if mask.is_null() {
            return None;
        }
        // 位图选在内存 DC 里时不能交给 CreateIconIndirect，先换回原位图。
        SelectObject(canvas.dc, canvas.old);
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: canvas.bitmap,
        });
        SelectObject(canvas.dc, canvas.bitmap);
        DeleteObject(mask);
        (!icon.is_null()).then_some(icon)
    }
}

/// 安装包用的 .ico：各尺寸为 32 位 DIB（自下而上行序、全零 AND 掩码），透明度在 alpha。
pub fn ico_file() -> Result<Vec<u8>, String> {
    let mut images = Vec::new();
    for side in SIZES {
        let pixels = render(side).ok_or_else(error)?;
        let mask_row = (side as usize).div_ceil(32) * 4;
        let mut image = Vec::new();
        for value in [40u32, side as u32, side as u32 * 2] {
            image.extend_from_slice(&value.to_le_bytes());
        }
        image.extend_from_slice(&1u16.to_le_bytes());
        image.extend_from_slice(&32u16.to_le_bytes());
        // 压缩方式、图像大小、分辨率与调色板全为 0。
        image.extend_from_slice(&[0; 24]);
        for row in pixels.chunks(side as usize).rev() {
            for pixel in row {
                image.extend_from_slice(&pixel.to_le_bytes());
            }
        }
        image.resize(image.len() + mask_row * side as usize, 0);
        images.push((side, image));
    }
    let mut file = Vec::new();
    for value in [0u16, 1, images.len() as u16] {
        file.extend_from_slice(&value.to_le_bytes());
    }
    let mut offset = 6 + 16 * images.len();
    for (side, image) in &images {
        // 256 在目录项里记作 0。
        let edge = if *side >= 256 { 0 } else { *side as u8 };
        file.extend_from_slice(&[edge, edge, 0, 0]);
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&32u16.to_le_bytes());
        file.extend_from_slice(&(image.len() as u32).to_le_bytes());
        file.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += image.len();
    }
    for (_, image) in images {
        file.extend_from_slice(&image);
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigertype_contracts::without_blue;

    #[test]
    fn the_seal_is_rounded_blue_free_and_carries_a_cream_glyph() {
        for side in [16, 48] {
            let pixels = render(side).unwrap();
            let at = |x: i32, y: i32| pixels[(y * side + x) as usize];
            // 圆角处几近透明（16px 的角像素只盖住一点），中心不透明。
            assert!(at(0, 0) >> 24 < 96, "{side}");
            assert_eq!(at(side / 2, side / 2) >> 24, 255, "{side}");
            for pixel in &pixels {
                assert!(without_blue(pixel & 0xFF_FFFF), "{side} {pixel:08X}");
            }
            let creamy = pixels
                .iter()
                .any(|&p| p >> 24 == 255 && (p >> 8 & 0xFF) > 0xD0);
            assert!(creamy, "{side} 须有米白字形");
        }
        // 大尺寸带虎纹：右缘中段有比底板更深的像素。
        let pixels = render(64).unwrap();
        let darker = (40..64).any(|y| {
            let pixel = pixels[(y * 64 + 62) as usize];
            pixel >> 24 > 200 && (pixel >> 16 & 0xFF) < 0xB0
        });
        assert!(darker, "大图标须画出虎纹");
    }

    #[test]
    fn the_icon_file_lists_every_size_with_consistent_offsets() {
        let file = ico_file().unwrap();
        let word = |at: usize| u16::from_le_bytes([file[at], file[at + 1]]) as usize;
        let dword = |at: usize| u32::from_le_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        assert_eq!((word(0), word(2), word(4)), (0, 1, SIZES.len()));
        let mut expected = 6 + 16 * SIZES.len();
        for (index, side) in SIZES.into_iter().enumerate() {
            let entry = 6 + 16 * index;
            assert_eq!(file[entry] as i32, side % 256);
            assert_eq!(dword(entry + 12), expected);
            assert_eq!(dword(expected + 4), side as usize);
            expected += dword(entry + 8);
        }
        assert_eq!(expected, file.len());
        let icon = icon(32).unwrap();
        // SAFETY: 销毁本测试刚建的图标。
        unsafe { DestroyIcon(icon) };
    }
}
