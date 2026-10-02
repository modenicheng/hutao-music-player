//! 程序化确定性占位封面 / 头像生成。
//!
//! 为什么必须确定性：同一实体在页面各处拿到同一张图、取色模块（M6）输入稳定、
//! 不依赖网络。一切"随机量"只从 seed 经 FNV-1a 哈希派生，禁止时间/真随机。
//!
//! 占位封面语义（2026-09-30 改版）：真图缺失/未到达时的中性占位——
//! 设计系统中性色底（--neutral-400 系）+ 资源类型图标（Material Symbols
//! Rounded，与 ui/assets/icons 同源 path），按 seed 前缀选形：
//! `playlist:` → library-music、`artist:` → person、其余（专辑/曲目）→ album。
//! 不再做全彩渐变构图：列表页铺满彩色块喧宾夺主，且与"氛围色不进列表"
//! （DESIGN.md §1.2 v0.4）相悖。
//!
//! SVG 字符串确定性生成；栅格化经 resvg（slint svg 特性同款依赖），
//! 结果按占位图标类型缓存为 [`slint::Image`]（至多 3 条，见 `cover_image`）。
//! tiny-skia 像素是预乘 alpha，Slint 需要直 alpha，拷贝时反预乘。

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

// —— 中性占位色板（apps/hmp-tauri/src/styles/index.css 浅色套）———
// 底 = --neutral-400 → 加深一档的竖向微渐变；图标 = --neutral-600。
// 中间调在浅色页底（neutral-50/100）与深色页底（#131312/#1D1D1B）上都不刺眼。
const TILE_BG_TOP: (u8, u8, u8) = (0xB6, 0xAE, 0xAC);
const TILE_BG_BOTTOM: (u8, u8, u8) = (0xA5, 0x9C, 0x99);
const TILE_ICON: (u8, u8, u8) = (0x75, 0x67, 0x64);

/// Material Symbols Rounded 24×24 图标 path（与 ui/assets/icons/*.svg 同源）。
const ICON_ALBUM: &str = "M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm0 14.5c-2.49 0-4.5-2.01-4.5-4.5S9.51 7.5 12 7.5s4.5 2.01 4.5 4.5-2.01 4.5-4.5 4.5zm0-5.5c-.55 0-1 .45-1 1s.45 1 1 1 1-.45 1-1-.45-1-1-1z";
const ICON_PLAYLIST: &str = "M12.5 15q1.05 0 1.775-.725T15 12.5V7h2q.425 0 .713-.288T18 6t-.288-.712T17 5h-2q-.425 0-.712.288T14 6v4.5q-.325-.25-.7-.375T12.5 10q-1.05 0-1.775.725T10 12.5t.725 1.775T12.5 15M8 18q-.825 0-1.412-.587T6 16V4q0-.825.588-1.412T8 2h12q.825 0 1.413.588T22 4v12q0 .825-.587 1.413T20 18zm-4 4q-.825 0-1.412-.587T2 20V7q0-.425.288-.712T3 6t.713.288T4 7v13h13q.425 0 .713.288T18 21t-.288.713T17 22z";
const ICON_ARTIST: &str = "M12 12c2.21 0 4-1.79 4-4s-1.79-4-4-4-4 1.79-4 4 1.79 4 4 4zm0 2c-2.67 0-8 1.34-8 4v2h16v-2c0-2.66-5.33-4-8-4z";

/// 渐变方向轴（avatar_svg 头像渐变仍在用）。
const GRADIENT_AXES: [(f64, f64, f64, f64); 4] = [
    (0.0, 0.0, 1.0, 1.0),
    (1.0, 0.0, 0.0, 1.0),
    (0.0, 0.0, 0.0, 1.0),
    (0.0, 1.0, 1.0, 0.0),
];

/// seed 前缀 → 资源类型图标（歌单/歌手/专辑，专辑为默认兜底）。
fn icon_path_for(seed: &str) -> &'static str {
    if seed.starts_with("playlist:") {
        ICON_PLAYLIST
    } else if seed.starts_with("artist:") {
        ICON_ARTIST
    } else {
        ICON_ALBUM
    }
}

/// 生成确定性中性占位封面 SVG（600×600 viewBox：中性底渐变 + 居中类型图标）。
/// 同类实体同图（同一性由 seed 前缀保证），不同实体不再做色彩区分。
pub fn cover_svg(seed: &str) -> String {
    let icon = icon_path_for(seed);
    let (r1, g1, b1) = TILE_BG_TOP;
    let (r2, g2, b2) = TILE_BG_BOTTOM;
    let (ir, ig, ib) = TILE_ICON;
    // 24 单元图标放大 9 倍（216px，36% 视宽），居中
    let inset = (600.0 - 24.0 * 9.0) / 2.0;
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="600" height="600" viewBox="0 0 600 600"><defs><linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="rgb({r1},{g1},{b1})"/><stop offset="1" stop-color="rgb({r2},{g2},{b2})"/></linearGradient></defs><rect width="600" height="600" fill="url(#bg)"/><path transform="translate({inset} {inset}) scale(9)" d="{icon}" fill="rgb({ir},{ig},{ib})"/></svg>"#,
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

/// `file://` URI → 本地路径（两种在库形态都收）：
/// - 规范 URL 形态 `file:///C:/a/b.jpg`（Url::to_file_path 产物）：剥前缀后
///   以 `/` 开头且次字符是盘符冒号 → 去.protocol 斜杠、`/`→`\`；
///   Unix 规范形态 `/home/...` 原样；
/// - 宽容形态 `file://C:\a\b.jpg`（persist_cover 的 `format!("file://{}")`）：
///   剥前缀即本地路径。
///
/// 裸剥前缀的旧写法会把规范形态解析成 `/C:/...`（Windows 读不到），是
/// "封面文件在盘上却显示占位"的根因之一。不做 percent 解码（写入方均为
/// Path::display 形态，无转义字符）。
pub fn file_uri_to_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    if let Some(win) = rest.strip_prefix('/') {
        // 规范形态：file:///C:/... → C:\...（Windows 盘符才转义）
        if win.as_bytes().get(1) == Some(&b':') {
            return Some(win.replace('/', "\\"));
        }
    }
    // 宽容形态（file://C:\...）与 Unix 规范形态（/home/...）：剥前缀即路径
    Some(rest.to_string())
}

// slint::Image 非 Send/Sync：缓存放 thread_local（UI 消费全程在主线程）。
// key = 占位图标类型（三类 &'static str），条目封顶 3（理由见 cover_image doc）。
thread_local! {
    static COVER_CACHE: RefCell<HashMap<&'static str, Image>> = RefCell::new(HashMap::new());
}

/// 确定性封面图（seed 同 covers.ts：专辑用 `album:{mid}`）。
///
/// 缓存 key = 占位**图标类型**（`icon_path_for` 的 playlist/artist/album 三类
/// `&'static str`），不是 seed：2026-09-30 改版后同类占位像素逐字节相同
/// （`cover_svg_is_deterministic_and_typed` 断言 album 前缀下不同 seed 同图），
/// 按 seed 缓存是纯重复。内存审计：占位封面按曲目 mid 播种，同像素位图
/// 逐曲复制（256×256 RGBA = 256KB/条，1 万曲 ≈ 2.5GB）；按类型缓存后条目
/// 封顶 3。对外签名 `cover_image(seed)` 不变，调用点零改动。
pub fn cover_image(seed: &str) -> Image {
    let kind = icon_path_for(seed);
    COVER_CACHE.with(|cache| {
        if let Some(image) = cache.borrow().get(kind) {
            return image.clone();
        }
        let image = svg_to_image(&cover_svg(seed));
        cache.borrow_mut().insert(kind, image.clone());
        image
    })
}

/// 当前线程 [`COVER_CACHE`] 的条目数（测试观测「条目 ≤ 3」用，仅测试构建）。
#[cfg(test)]
fn cover_cache_len() -> usize {
    COVER_CACHE.with(|cache| cache.borrow().len())
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
    fn cover_svg_is_deterministic_and_typed() {
        let a = cover_svg("album:al01");
        let b = cover_svg("album:al01");
        assert_eq!(a, b);
        assert!(a.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg""#));
        assert!(a.contains("linearGradient"));
        // 同类实体同图（中性占位不做色彩区分）；不同类型图标不同
        assert_eq!(cover_svg("album:al02"), a);
        assert_eq!(cover_svg("album:完全不同"), a);
        let playlist = cover_svg("playlist:al01");
        let artist = cover_svg("artist:al01");
        assert_ne!(playlist, a);
        assert_ne!(artist, a);
        assert_ne!(playlist, artist);
    }

    #[test]
    fn icon_kind_follows_seed_prefix() {
        assert_eq!(icon_path_for("playlist:123"), ICON_PLAYLIST);
        assert_eq!(icon_path_for("artist:周杰伦"), ICON_ARTIST);
        assert_eq!(icon_path_for("album:al01"), ICON_ALBUM);
        assert_eq!(icon_path_for("liked:so001"), ICON_ALBUM, "无前缀兜底专辑盘");
    }

    #[test]
    fn file_uri_to_path_handles_both_forms() {
        // 规范 URL 形态（Url::to_file_path 产物）：Windows 盘符
        assert_eq!(
            file_uri_to_path("file:///C:/Users/cheng/AppData/Local/hmp/covers/a.jpg").as_deref(),
            Some(r"C:\Users\cheng\AppData\Local\hmp\covers\a.jpg")
        );
        // 宽容形态（persist_cover 的 file://{display}）
        assert_eq!(
            file_uri_to_path(r"file://C:\Users\cheng\covers\a.jpg").as_deref(),
            Some(r"C:\Users\cheng\covers\a.jpg")
        );
        // Unix 规范形态
        assert_eq!(
            file_uri_to_path("file:///home/u/covers/a.jpg").as_deref(),
            Some("/home/u/covers/a.jpg")
        );
        // 非 file:// 与空串：None
        assert_eq!(file_uri_to_path("https://y.gtimg.cn/a.jpg"), None);
        assert_eq!(file_uri_to_path(""), None);
    }

    #[test]
    fn cover_image_renders_opaque() {
        let image = cover_image("album:al01");
        assert!(image.size().width > 0);
    }

    #[test]
    fn cover_image_cache_is_capped_by_icon_kind() {
        // 大量不同 seed 灌入：缓存按图标类型而非 seed，条目数封顶 3
        for i in 0..64 {
            let _ = cover_image(&format!("album:gen{i}"));
            let _ = cover_image(&format!("playlist:gen{i}"));
            let _ = cover_image(&format!("artist:gen{i}"));
            let _ = cover_image(&format!("liked:gen{i}")); // 无 playlist/artist 前缀 → album 类
        }
        let len = cover_cache_len();
        assert!(len <= 3, "缓存条目应封顶 3，实际 {len}");
        // 四种 seed 形态恰好落到三类图标上
        assert_eq!(len, 3);
    }
}
