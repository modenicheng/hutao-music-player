//! 二维码终端渲染（spec §4.3 `qr_ascii.rs`）。
//!
//! 解码 → 缩放（Nearest）→ 每字符 2×2 像素 → 半块 Unicode 字符。

use image::imageops::FilterType;

/// 渲染错误。
#[derive(Debug, thiserror::Error)]
pub enum QrRenderError {
    #[error("图像解码失败: {0}")]
    Decode(String),
    #[error("图像尺寸无效")]
    InvalidSize,
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

/// 渲染灰度/黑白图（已按 width 缩放）为半块字符。
fn render_img(img: &image::DynamicImage, width_chars: usize) -> Result<String, QrRenderError> {
    let w = width_chars.max(1);
    if img.width() == 0 || img.height() == 0 {
        return Err(QrRenderError::InvalidSize);
    }
    // 缩放为 w × w 像素（QR 方形），Nearest 保持硬边
    let small = img
        .resize_exact(w as u32, w as u32, FilterType::Nearest)
        .to_luma8();
    let mut out = String::new();
    for r in (0..small.height() as usize).step_by(2) {
        for c in 0..small.width() as usize {
            let top = small.get_pixel(c as u32, r as u32).0[0] < 128;
            let bottom = if r + 1 < small.height() as usize {
                small.get_pixel(c as u32, (r + 1) as u32).0[0] < 128
            } else {
                false
            };
            let ch = match (top, bottom) {
                (false, false) => ' ',
                (true, false) => '▀',
                (false, true) => '▄',
                (true, true) => '█',
            };
            out.push(ch);
        }
        out.push('\n');
    }
    Ok(out)
}

/// 解码二维码图像字节并渲染为 ASCII 艺术字符串。
pub fn render_qr(data: &[u8], width_chars: usize) -> Result<String, QrRenderError> {
    let img = image::load_from_memory(data).map_err(|e| QrRenderError::Decode(e.to_string()))?;
    render_img(&img, width_chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_2x2_block_map() {
        // 2x2 像素：左列上黑下白（▀），右列全黑（█）
        let img = image::RgbaImage::from_fn(2, 2, |x, y| {
            let dark = match (x, y) {
                (0, 0) => true,  // 左列上：黑
                (0, 1) => false, // 左列下：白 → 左字符 ▀
                _ => true,       // 右列全黑 → 右字符 █
            };
            if dark {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        });
        let s = render_img(&image::DynamicImage::ImageRgba8(img), 2).unwrap();
        assert_eq!(s, "▀█\n"); // 每字符 2 行 × 1 列像素；宽 2 字符 → 1 行输出
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
        assert!(s.contains('█'));
    }
}
