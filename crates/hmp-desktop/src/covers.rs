//! 程序化确定性封面 / 头像生成（移植自 apps/hmp-tauri/src/lib/api/covers.ts）。
//!
//! 为什么必须确定性：同一实体在页面各处拿到同一张图、取色模块（M6）输入稳定、
//! 不依赖网络。一切"随机量"只从 seed 经 FNV-1a 哈希派生，禁止时间/真随机。
//!
//! SVG 字符串与 TS 版逐字段一致；栅格化经 resvg（slint svg 特性同款依赖），
//! 结果按 seed 缓存为 [`slint::Image`]。tiny-skia 像素是预乘 alpha，
//! Slint 需要直 alpha，拷贝时反预乘。

use std::cell::RefCell;
use std::collections::HashMap;

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

const FNV_OFFSET_BASIS: u32 = 0x811c_9dc5;
const FNV_PRIME: u32 = 0x0100_0193;

/// FNV-1a 32 位哈希（与 covers.ts hashSeed 逐字节一致，程序化封面的确定性地基）
pub fn hash_seed(seed: &str) -> u32 {
    let mut hash = FNV_OFFSET_BASIS;
    for ch in seed.chars() {
        hash ^= u32::from(ch);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// 在基础哈希上叠加盐值派生子随机数（与 covers.ts derive 一致）
fn derive(base: u32, salt: &str) -> u32 {
    hash_seed(&format!("{salt}:{}", radix36(base)))
}

fn radix36(mut value: u32) -> String {
    if value == 0 {
        return "0".into();
    }
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buf = Vec::new();
    while value > 0 {
        buf.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    buf.reverse();
    String::from_utf8(buf).expect("radix36 digits are ascii")
}

struct Palette {
    hue: u32,
    hue_analog: u32,
    hue_accent: u32,
    s1: u32,
    s2: u32,
    l1: u32,
    l2: u32,
}

/// 色板规则与 covers.ts paletteFor 一致：色相覆盖全环，
/// 封面内部只用 "主色 + 邻近色 + 低透明对侧 accent"，S 45–75%、L 35–65%。
fn palette_for(seed: &str) -> (u32, Palette) {
    let base = hash_seed(&format!("cover:{seed}"));
    let hue = base % 360;
    let p = Palette {
        hue,
        hue_analog: (hue + 14 + (derive(base, "analog") % 44)) % 360,
        hue_accent: (hue + 150 + (derive(base, "accent") % 60)) % 360,
        s1: 45 + (derive(base, "s1") % 31),
        s2: 45 + (derive(base, "s2") % 31),
        l1: 35 + (derive(base, "l1") % 31),
        l2: 35 + (derive(base, "l2") % 31),
    };
    (base, p)
}

const GRADIENT_AXES: [(f64, f64, f64, f64); 4] = [
    (0.0, 0.0, 1.0, 1.0),
    (1.0, 0.0, 0.0, 1.0),
    (0.0, 0.0, 0.0, 1.0),
    (0.0, 1.0, 1.0, 0.0),
];

/// 生成确定性封面 SVG（与 covers.ts coverUrl 的 SVG 串一致；600×600 viewBox）
pub fn cover_svg(seed: &str) -> String {
    let (base, p) = palette_for(seed);
    let axis = GRADIENT_AXES[(derive(base, "axis") % 4) as usize];
    let layout = (derive(base, "layout") % 4) as usize;
    let hsl = |h: u32, s: u32, l: u32| format!("hsl({h}, {s}%, {l}%)");
    let hsla = |h: u32, s: u32, l: u32, a: f64| format!("hsla({h}, {s}%, {l}%, {a})");

    let shapes = match layout {
        // 轨道：大行星 + 细轨道环 + 卫星点
        0 => format!(
            r#"<circle cx="432" cy="176" r="196" fill="{}"/><circle cx="150" cy="452" r="118" fill="none" stroke="{}" stroke-width="3"/><circle cx="150" cy="452" r="26" fill="{}"/>"#,
            hsla(p.hue_accent, p.s1, p.l1, 0.2),
            hsla(p.hue_analog, p.s2, p.l2, 0.55),
            hsla(p.hue_analog, p.s2, 74, 0.5),
        ),
        // 山脊：两座错落三角 + 低悬的"太阳"
        1 => format!(
            r#"<path d="M0 600 L230 210 L460 600 Z" fill="{}"/><path d="M210 600 L420 300 L620 600 Z" fill="{}"/><circle cx="438" cy="150" r="64" fill="{}"/>"#,
            hsla(p.hue_analog, p.s2, p.l2, 0.28),
            hsla(p.hue_accent, p.s1, p.l1, 0.22),
            hsla(p.hue_accent, p.s2, 72, 0.42),
        ),
        // 声波：自下而上的三道同心弧
        2 => format!(
            r#"<path d="M0 760 A300 300 0 0 1 600 760" fill="none" stroke="{}" stroke-width="44"/><path d="M-120 760 A420 420 0 0 1 720 760" fill="none" stroke="{}" stroke-width="30"/><path d="M-240 760 A540 540 0 0 1 840 760" fill="none" stroke="{}" stroke-width="20"/>"#,
            hsla(p.hue_analog, p.s2, p.l2, 0.2),
            hsla(p.hue_accent, p.s1, p.l1, 0.14),
            hsla(p.hue, p.s1, 70, 0.1),
        ),
        // 斜切：左上大圆 + 右下旋转菱形 + 一道对角细线
        _ => format!(
            r#"<circle cx="120" cy="96" r="210" fill="{}"/><rect x="380" y="330" width="260" height="260" transform="rotate(45 510 460)" fill="{}"/><path d="M60 540 L540 60" stroke="{}" stroke-width="3"/>"#,
            hsla(p.hue_analog, p.s2, p.l2, 0.3),
            hsla(p.hue_accent, p.s1, p.l1, 0.24),
            hsla(p.hue, p.s1, 82, 0.5),
        ),
    };

    let l1_glow = (p.l1 + 12).min(70); // TS: Math.min(70, l1 + 12)
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="600" height="600" viewBox="0 0 600 600"><defs><linearGradient id="bg" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}"><stop offset="0" stop-color="{c1}"/><stop offset="1" stop-color="{c2}"/></linearGradient><radialGradient id="glow" cx="0.5" cy="0.36" r="0.75"><stop offset="0" stop-color="{c3}" stop-opacity="0.35"/><stop offset="1" stop-color="{c4}" stop-opacity="0"/></radialGradient></defs><rect width="600" height="600" fill="url(#bg)"/><rect width="600" height="600" fill="url(#glow)"/>{shapes}</svg>"#,
        x1 = axis.0,
        y1 = axis.1,
        x2 = axis.2,
        y2 = axis.3,
        c1 = hsl(p.hue, p.s1, p.l1),
        c2 = hsl(p.hue_analog, p.s2, p.l2),
        c3 = hsl(p.hue_accent, p.s1, l1_glow.min(70)),
        c4 = hsl(p.hue_accent, p.s1, p.l1),
    )
}

/// 生成确定性头像 SVG（同色相纯渐变圆；与 covers.ts avatarUrl 一致）
pub fn avatar_svg(seed: &str) -> String {
    let base = hash_seed(&format!("avatar:{seed}"));
    let hue = base % 360;
    let s1 = 45 + (derive(base, "s1") % 31);
    let s2 = 40 + (derive(base, "s2") % 26);
    let l_light = 62 + (derive(base, "light") % 18);
    let l_dark = 34 + (derive(base, "dark") % 16);
    let axis = GRADIENT_AXES[(derive(base, "axis") % 2) as usize];
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96" viewBox="0 0 96 96"><defs><linearGradient id="g" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}"><stop offset="0" stop-color="hsl({hue}, {s1}%, {l_light}%)"/><stop offset="1" stop-color="hsl({hue}, {s2}%, {l_dark}%)"/></linearGradient></defs><circle cx="48" cy="48" r="48" fill="url(#g)"/></svg>"#,
        x1 = axis.0,
        y1 = axis.1,
        x2 = axis.2,
        y2 = axis.3,
    )
}

const RENDER_SIZE: u32 = 256;

fn svg_to_image(svg: &str) -> Image {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_str(svg, &opt).expect("mock svg parses");
    let mut pixmap = resvg::tiny_skia::Pixmap::new(RENDER_SIZE, RENDER_SIZE).expect("pixmap");
    let scale = RENDER_SIZE as f32 / tree.size().width();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(RENDER_SIZE, RENDER_SIZE);
    let src = pixmap.data();
    let dst = buffer.make_mut_bytes();
    for (px, out) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        // tiny-skia 预乘 alpha → Slint 直 alpha
        let a = u32::from(px[3]);
        // tiny-skia 预乘 → 直 alpha；a==255 快路径（封面全不透明）
        let unpremultiply = |v: u8| -> u8 {
            if a == 255 {
                v
            } else {
                (u32::from(v) * 255 / a.max(1)).min(255) as u8
            }
        };
        out[0] = unpremultiply(px[0]);
        out[1] = unpremultiply(px[1]);
        out[2] = unpremultiply(px[2]);
        out[3] = px[3];
    }
    Image::from_rgba8(buffer)
}

/// 确定性封面图（seed 同 covers.ts：专辑用 `album:{mid}`）。
/// slint::Image 非 Send/Sync：缓存放 thread_local（UI 消费全程在主线程）
pub fn cover_image(seed: &str) -> Image {
    thread_local! {
        static CACHE: RefCell<HashMap<String, Image>> = RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        if let Some(image) = cache.borrow().get(seed) {
            return image.clone();
        }
        let image = svg_to_image(&cover_svg(seed));
        cache.borrow_mut().insert(seed.to_owned(), image.clone());
        image
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_ts_reference() {
        // node 端用同一实现算出的参考值（跨语言确定性）
        assert_eq!(hash_seed("liked:so001"), 1_875_290_365);
        assert_eq!(hash_seed("cover:al01"), 681_060_008);
        assert_eq!(hash_seed("local:so001"), 1_657_473_683);
        assert_eq!(hash_seed("recent:so002"), 2_118_552_908);
        assert_eq!(hash_seed("bought:so001"), 1_856_879_493);
    }

    #[test]
    fn cover_svg_is_deterministic_and_wellformed() {
        let a = cover_svg("album:al01");
        let b = cover_svg("album:al01");
        assert_eq!(a, b);
        assert!(a.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg""#));
        assert!(a.contains("linearGradient"));
        assert_ne!(cover_svg("album:al02"), a);
    }

    #[test]
    fn cover_image_renders_opaque() {
        let image = cover_image("album:al01");
        assert!(image.size().width > 0);
    }
}
