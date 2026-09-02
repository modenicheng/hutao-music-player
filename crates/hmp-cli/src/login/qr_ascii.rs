//! 二维码终端渲染（spec §4.3 `qr_ascii.rs`）。
//!
//! 解码 → 灰度 → 原生分辨率半块渲染（每字符 2 行 × 1 列像素，` ▀▄█`）。
//!
//! 不缩放 → 模块网格保持完整：定位框内外方形严格同心、各模块位置精确。
//! 仅当图像宽于终端时，按图像尺寸的整数约数缩小（模块网格不被破坏）；
//! 找不到合适约数（或结果过小）才退化为 Nearest 整图缩放。

use image::GrayImage;
use image::imageops::FilterType;

/// 渲染错误。
#[derive(Debug, thiserror::Error)]
pub enum QrRenderError {
    #[error("图像解码失败: {0}")]
    Decode(String),
    #[error("图像尺寸无效")]
    InvalidSize,
}

/// QQ 登录二维码（`ptqrshow`）标准尺寸：33 模块 × 3px + 2 模块静区。
pub const EXPECTED_QR_SIZE: u32 = 111;

/// 渲染结果。
pub struct QrRender {
    /// 终端字符画。
    pub text: String,
    /// 图像尺寸是否等于 [`EXPECTED_QR_SIZE`]（尺寸异常时显示可能失真）。
    pub is_expected_size: bool,
    /// 实际图像尺寸。
    pub size: (u32, u32),
}

/// 终端宽度（`COLUMNS` 环境变量，钳位 32..=120，默认 60）。
pub fn terminal_width() -> usize {
    terminal_width_with(std::env::var("COLUMNS").ok().as_deref())
}

/// 供测试注入的宽度解析。
fn terminal_width_with(cols: Option<&str>) -> usize {
    let Some(v) = cols.and_then(|s| s.trim().parse::<usize>().ok()) else {
        return 60;
    };
    v.clamp(32, 120)
}

/// 半块字符表；索引 = 上行·1 + 下行·2（暗=1）。
const HALF_BLOCKS: [char; 4] = [' ', '▀', '▄', '█'];

/// `width` 的、不小于 `min` 的最小约数；不存在返回 1。
fn smallest_divisor_ge(width: u32, min: u32) -> u32 {
    (min..=width).find(|&f| width % f == 0).unwrap_or(1)
}

/// 半块渲染：每字符承载 2 行 × 1 列像素（原生分辨率）。
fn render_half_blocks(img: &GrayImage) -> String {
    let (width, height) = (img.width() as usize, img.height() as usize);
    let dark = |x: usize, y: usize| img.get_pixel(x as u32, y as u32).0[0] < 128;
    let mut out = String::with_capacity(width * (height / 2 + 1));
    for y in 0..height.div_ceil(2) {
        for x in 0..width {
            let top = dark(x, 2 * y);
            let bottom = if 2 * y + 1 < height {
                dark(x, 2 * y + 1)
            } else {
                false
            };
            let idx = (top as usize) | ((bottom as usize) << 1);
            out.push(HALF_BLOCKS[idx]);
        }
        out.push('\n');
    }
    out
}

/// 渲染灰度/黑白图为半块字符。
fn render_img(img: &image::DynamicImage, width_chars: usize) -> Result<String, QrRenderError> {
    let w = width_chars.max(1) as u32;
    if img.width() == 0 || img.height() == 0 {
        return Err(QrRenderError::InvalidSize);
    }
    let gray = img.to_luma8();
    let (width, height) = gray.dimensions();
    let small = if width > w {
        // 按整数约数缩小（保持模块网格）；结果过小（< 半宽）则 Nearest 兜底
        let f = smallest_divisor_ge(width, width.div_ceil(w));
        let resize = |nw: u32, nh: u32| {
            image::DynamicImage::ImageLuma8(gray.clone())
                .resize_exact(nw, nh, FilterType::Nearest)
                .to_luma8()
        };
        if f > 1 && width / f >= w / 2 {
            resize(width / f, height / f)
        } else {
            resize(w, w)
        }
    } else {
        gray
    };
    Ok(render_half_blocks(&small))
}

/// 解码二维码图像字节并渲染为终端字符画。
///
/// 同时校验图像尺寸是否为 [`EXPECTED_QR_SIZE`]（QQ 登录二维码标准格式），
/// 供调用方在尺寸异常时提示用户显示可能有误。
pub fn render_qr(data: &[u8], width_chars: usize) -> Result<QrRender, QrRenderError> {
    let img = image::load_from_memory(data).map_err(|e| QrRenderError::Decode(e.to_string()))?;
    let size = (img.width(), img.height());
    let is_expected_size = size == (EXPECTED_QR_SIZE, EXPECTED_QR_SIZE);
    let text = render_img(&img, width_chars)?;
    Ok(QrRender {
        text,
        is_expected_size,
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    /// 构造标准 QR 结构图：n 模块 × m 像素 + 四边 q 模块静区。
    fn make_qr_like(n: u32, m: u32, q: u32) -> GrayImage {
        let s = (n + 2 * q) * m;
        let mut img = GrayImage::new(s, s);
        let mod_dark = |i: i64, j: i64| -> bool {
            // 三个定位框（7×7：暗边框 + 亮间隔 + 暗 3×3 内核）
            for (ox, oy) in [(0i64, 0i64), (n as i64 - 7, 0), (0, n as i64 - 7)] {
                if (ox..ox + 7).contains(&i) && (oy..oy + 7).contains(&j) {
                    let (x, y) = (i - ox, j - oy);
                    let border = x == 0 || x == 6 || y == 0 || y == 6;
                    let inner = (2..5).contains(&x) && (2..5).contains(&y);
                    return border || inner;
                }
            }
            // 其余数据区：棋盘格
            (i + j) % 2 == 0
        };
        for y in 0..s {
            for x in 0..s {
                let (i, j) = (
                    x as i64 / m as i64 - q as i64,
                    y as i64 / m as i64 - q as i64,
                );
                let v =
                    if (0..n as i64).contains(&i) && (0..n as i64).contains(&j) && mod_dark(i, j) {
                        0
                    } else {
                        255
                    };
                img.put_pixel(x, y, Luma([v]));
            }
        }
        img
    }

    #[test]
    fn renders_2x2_block_map() {
        // 2x2 像素：左列上黑下白（▀），右列全黑（█）；原生分辨率不缩放
        let img = image::RgbaImage::from_fn(2, 2, |x, y| {
            let dark = match (x, y) {
                (0, 0) => true,  // 左上：黑
                (0, 1) => false, // 左下：白
                _ => true,       // 右列全黑
            };
            if dark {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        });
        let s = render_img(&image::DynamicImage::ImageRgba8(img), 2).unwrap();
        assert_eq!(s, "▀█\n");
    }

    #[test]
    fn width_is_clamped() {
        assert_eq!(terminal_width_with(Some("10")), 32);
        assert_eq!(terminal_width_with(Some("200")), 120);
        assert_eq!(terminal_width_with(None), 60);
    }

    #[test]
    fn decode_failure_returns_err() {
        assert!(render_qr(b"not an image", 60).is_err());
    }

    #[test]
    fn renders_real_png() {
        // 用 image crate 生成一张 21x21 纯黑 PNG 字节 → render_qr 成功且非空
        let mut img = image::RgbaImage::new(21, 21);
        for p in img.pixels_mut() {
            *p = image::Rgba([0, 0, 0, 255]);
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let s = render_qr(buf.get_ref(), 40).unwrap();
        assert!(s.text.contains('█'));
        // 21×21 非标准尺寸 → 标记尺寸异常
        assert!(!s.is_expected_size);
        assert_eq!(s.size, (21, 21));
    }

    #[test]
    fn smallest_divisor_works() {
        assert_eq!(smallest_divisor_ge(111, 2), 3); // 111 = 3×37
        assert_eq!(smallest_divisor_ge(116, 2), 2); // 116 = 4×29
        assert_eq!(smallest_divisor_ge(111, 30), 37);
        assert_eq!(smallest_divisor_ge(111, 112), 1); // 无约数 → 1
    }

    /// 原生分辨率渲染（与 QQ ptqrshow 相同结构：33 模块 × 3px + 2 模块静区 = 111×111）：
    /// 定位框 7 模块 = 21 字符宽，内外方形严格同心、模块与字符网格精确对齐。
    #[test]
    fn finder_patterns_are_native_aligned() {
        let img = make_qr_like(33, 3, 2);
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let r = render_qr(buf.get_ref(), 120).unwrap();
        assert!(r.is_expected_size); // 111×111 标准尺寸
        let lines: Vec<&str> = r.text.lines().collect();
        assert_eq!(lines.len(), 56); // 111 像素 / 2 → 56 字符行
        assert_eq!(lines[0].chars().count(), 111);
        // 模块 i 位于像素 [6+3i, 9+3i)；定位框 7 模块 → 字符列 6..27、字符行 3..14
        let row = |y: usize| -> String { lines[y].chars().skip(6).take(21).collect() };
        // 顶部边框行（像素行 6,7 = 模块行 0）：21 字符全暗
        assert_eq!(row(3), "█████████████████████");
        // 内核行（像素行 12,13 = 模块行 2,3）：边框+3×3 内核，左右对称 → 同心
        assert_eq!(row(6), "███   █████████   ███");
        // 内核底边（像素行 20,21 = 模块行 4,5）：内核暗、边框仍为暗边
        assert_eq!(row(10), "███   ▀▀▀▀▀▀▀▀▀   ███");
        // 静区为空白
        assert!(lines[3].chars().take(6).all(|c| c == ' '));
    }

    /// 超出终端宽度 → 按整数约数（÷3）缩小：模块 1px = 1 字符，
    /// 定位框 7 字符宽、内框 3 字符严格居中。
    #[test]
    fn downscales_by_integer_factor() {
        let img = make_qr_like(33, 3, 2); // 111×111
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let r = render_qr(buf.get_ref(), 60).unwrap();
        assert!(r.is_expected_size);
        let lines: Vec<&str> = r.text.lines().collect();
        assert_eq!(lines.len(), 19); // 37 像素 / 2 → 19 字符行
        assert_eq!(lines[0].chars().count(), 37);
        // 模块 i 位于像素 [2+i, 3+i)；定位框 7 模块 → 字符列 2..9
        let row = |y: usize| -> String { lines[y].chars().skip(2).take(7).collect() };
        // 像素行 4,5（模块行 2,3）：边框 + 3×3 内核，左右对称 → 同心
        assert_eq!(row(2), "█ ███ █");
        // 像素行 2,3（模块行 0,1）：顶边 + 间隔行（边框列保持暗）
        assert_eq!(row(1), "█▀▀▀▀▀█");
        // 像素行 6,7（模块行 4,5）：内核底边 + 间隔
        assert_eq!(row(3), "█ ▀▀▀ █");
        // 静区为空白
        assert!(lines[0].chars().all(|c| c == ' '));
    }

    /// 尺寸校验：111×111 标记正常，其他尺寸标记异常。
    #[test]
    fn reports_unexpected_size() {
        // 33 模块 × 3px + 2 模块静区 = 111×111 → 正常
        let img = make_qr_like(33, 3, 2);
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let r = render_qr(buf.get_ref(), 60).unwrap();
        assert!(r.is_expected_size);
        // 33 模块 × 3px 无静区 = 99×99 → 异常
        let img = make_qr_like(33, 3, 0);
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let r = render_qr(buf.get_ref(), 60).unwrap();
        assert!(!r.is_expected_size);
        assert_eq!(r.size, (99, 99));
    }
}
