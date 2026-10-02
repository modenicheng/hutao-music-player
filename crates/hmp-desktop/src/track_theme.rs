//! OKLab 主色提取管线（移植自 apps/hmp-tauri/src/lib/color，DESIGN.md §2 取色管线）。
//!
//! 管线：RGBA 像素 → OKLab → 二分切分量化 → 评分挑 accent →
//! 整形（L 目标窗 + 色域收缩）→ 派生整族 track-* 颜色。
//! 无封面 / 灰阶封面 / 空输入时整族回退 walnut 品牌色，UI 永不出现无色状态。
//!
//! 与 TS 版的移植约定：
//! - 逐函数忠实移植（oklab.ts / binary-split.ts / score.ts / index.ts），全程
//!   f64；TS 的 hex 串中间值在 Rust 里改用 `[u8; 3]` 直存（省字符串往返），
//!   数值路径不变：accent 先量化到字节再回 OKLab，与 TS 的
//!   oklchToHex → hexToOklab 严格等价。
//! - 取整语义：TS `Math.round` = Rust `f64::round`（半值远离零）。
//! - 二分切分的排序必须稳定（JS Array.sort 稳定）：用 `slice::sort_by`，
//!   禁止 `sort_unstable_by`。
//! - oklch 色相归一到 [0, 2π)；fitIntoSrgbGamut 固定 24 次二分，epsilon 1e-4。
//! - kmeans.ts 不移植（Vue 默认 refine:"none"，adapter 不传该选项）。
//!
//! 另含环境层底图预烘焙（[`ambient_image`]）：Slint 无 CSS blur/saturate 滤镜，
//! 用「最长边 ≤128 面积平均降采样 + 3 趟盒滤波(半径 5) + 饱和度 ×1.2」近似
//! Vue 播放页 `blur(80px) saturate(1.2)` 环境层。
//!
//! UI 接线（[`apply_cover`]/[`apply_fallback`]/[`reapply_for_theme`]，全部
//! UI 线程调用）：对应 Vue adapter.ts（64×64 canvas 采样）+ trackTheme.ts
//! 的 `watch([coverUrl, theme.resolved])` —— 取色随封面与主题亮暗重算，
//! 结果整族写入 TrackPalette global（M6 播放页 overlay 消费），环境层底图
//! 写入 Player.ambient-cover。
//!
//! 内存纪律：每次换曲全图只物化一次 —— decode_cover_bounded 把 to_rgba8
//! 的完整位图立即压到最长边 ≤480 的 RGBA 缓冲，环境层底图与取色采样都从
//! 这份缓冲派生；Applied 状态只保留去重 key 与 64×64 采样（≤16KB），
//! 不再常驻全尺寸封面位图（旧实现 `sample_pixels` 与 `ambient_image` 各做
//! 一次全图 to_rgba8，且 Applied 持有整张封面 Image 副本 4-36MB）。

use std::cell::RefCell;

use slint::{Color, Global, Image, Rgba8Pixel, SharedPixelBuffer, Weak};

use crate::{AppWindow, Player, Theme};
// 与本模块的取色结果结构体 TrackPalette 同名的 slint global，别名引入
use crate::TrackPalette as TrackPaletteGlobal;

const TWO_PI: f64 = std::f64::consts::PI * 2.0;

/// 与 TS ColorMode（"light" | "dark"）对应；作为逐模式常量数组的下标（Light=0/Dark=1）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Light = 0,
    Dark = 1,
}

/// 与 Vue TrackPalette 接口对应；颜色一律直存 slint::Color
#[derive(Clone, Copy)]
pub struct TrackPalette {
    /// 播放态强调色
    pub accent: Color,
    /// accent 上的文字/图标，黑或白，保证 WCAG ≥ 4.5:1
    pub on_accent: Color,
    /// hover/选中底：亮色模式 accent @ 0.12，暗色模式 @ 0.20（from_argb_u8）
    pub accent_soft: Color,
    /// 播放页/播放列表深色面板底
    pub deep: Color,
    /// deep 面板上的正文暖白
    pub deep_fg: Color,
    /// 播放页环境渐变起点
    pub grad_from: Color,
    /// 播放页环境渐变终点
    pub grad_to: Color,
    /// 环境层顶部浮动元素（收起键等）的墨色：按 grad_from 对比度自动取深/浅
    pub on_ambient: Color,
    /// 是否为回退调色板（无输入 / 无彩色封面）
    pub is_fallback: bool,
}

// ---------------------------------------------------------------------------
// 回退与整形常量（与 index.ts / score.ts 逐值一致）
// ---------------------------------------------------------------------------

/// 与 styles/index.css 的 --accent 一致：回退即品牌胡桃木
const FALLBACK_ACCENT: [[u8; 3]; 2] = [[0xB3, 0x4A, 0x3A], [0xD0, 0x64, 0x52]]; // light #B34A3A / dark #D06452
const DEEP_FOREGROUND: [u8; 3] = [0xF7, 0xF0, 0xEA]; // #F7F0EA
const GRAD_TO: [[u8; 3]; 2] = [[0xFA, 0xF9, 0xF8], [0x18, 0x14, 0x12]]; // light #FAF9F8 / dark #181412
/// 渐变起点 = accent 朝中性端混合：亮色拉向白、暗色压向暖黑
const GRAD_FROM_ANCHOR: [[u8; 3]; 2] = [[0xFF, 0xFF, 0xFF], [0x1F, 0x1B, 0x17]]; // #FFFFFF / #1F1B17
const GRAD_FROM_RATIO: [f64; 2] = [0.82, 0.75];
const SOFT_ALPHA: [f64; 2] = [0.12, 0.2];
/// 环境层墨色两极：暖黑与亮色主题 foreground 同源，暖白与渐变终点同源
const AMBIENT_INK_DARK: [u8; 3] = [0x34, 0x28, 0x27]; // #342827
const AMBIENT_INK_LIGHT: [u8; 3] = [0xFA, 0xF9, 0xF8]; // #FAF9F8
/// alpha 低于该阈值的像素视为透明（画布留白），不参与取色
const MIN_PIXEL_ALPHA: u8 = 16;

/// 叶的 L 合法窗（min, max）：近黑近白当主色既不可读也不耐看，直接淘汰
const LEAF_LIGHTNESS_RANGE: (f64, f64) = (0.2, 0.9);
/// C 低于该值视为无彩色（灰阶封面），整族走回退
const ACHROMATIC_CHROMA: f64 = 0.04;
/// accent 目标亮度：亮色模式略暗（白底上稳）、暗色模式略亮（黑底上跳）
const ACCENT_TARGET_L: [f64; 2] = [0.6, 0.68];
const ACCENT_L_TOLERANCE: f64 = 0.03;
/// deep 深色面板底的亮度目标
const DEEP_TARGET_L: [f64; 2] = [0.35, 0.22];
/// deep 保留 80% 彩度维持与 accent 的血缘
const DEEP_CHROMA_RATIO: f64 = 0.8;
/// coverage 在评分中的权重（chroma 权重恒为 1）
const COVERAGE_WEIGHT: f64 = 0.5;
/// WCAG 2.1 AA 级正文对比阈值
const WCAG_AA_CONTRAST: f64 = 4.5;
const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];
const BLACK: [u8; 3] = [0x00, 0x00, 0x00];

/// 叶内像素低于该值就不再对分：继续切只会放大噪声
const MIN_SPLIT_PIXELS: usize = 4;
/// 二分切分的叶子数上限（Vue adapter 用默认值）
const MAX_LEAVES: usize = 10;

// ---------------------------------------------------------------------------
// oklab.ts —— 色彩空间转换（Björn Ottosson 标准矩阵，公有领域）
// ---------------------------------------------------------------------------

/// sRGB 传递函数的分段点（IEC 61966-2-1）
const SRGB_TO_LINEAR_THRESHOLD: f64 = 0.04045;
const LINEAR_TO_SRGB_THRESHOLD: f64 = 0.0031308;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Oklab {
    l: f64,
    a: f64,
    b: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Oklch {
    l: f64,
    c: f64,
    h: f64,
}

fn srgb_to_linear(channel: f64) -> f64 {
    if channel <= SRGB_TO_LINEAR_THRESHOLD {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(channel: f64) -> f64 {
    let sign = if channel < 0.0 { -1.0 } else { 1.0 };
    let value = channel.abs();
    let encoded = if value <= LINEAR_TO_SRGB_THRESHOLD {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    sign * encoded
}

fn linear_rgb_to_oklab(r: f64, g: f64, b: f64) -> Oklab {
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    Oklab {
        l: 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        a: 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        b: 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    }
}

fn oklab_to_linear_rgb(color: Oklab) -> [f64; 3] {
    let l_ = color.l + 0.3963377774 * color.a + 0.2158037573 * color.b;
    let m_ = color.l - 0.1055613458 * color.a - 0.0638541728 * color.b;
    let s_ = color.l - 0.0894841775 * color.a - 1.2914855480 * color.b;
    let ll = l_ * l_ * l_;
    let mm = m_ * m_ * m_;
    let ss = s_ * s_ * s_;
    [
        4.0767416621 * ll - 3.3077115913 * mm + 0.2309699292 * ss,
        -1.2684380046 * ll + 2.6097574011 * mm - 0.3413193965 * ss,
        -0.0041960863 * ll - 0.7034186147 * mm + 1.7076147010 * ss,
    ]
}

fn oklab_to_oklch(color: Oklab) -> Oklch {
    let raw_hue = color.b.atan2(color.a);
    // 色相归一到 [0, 2π)，方便外部直接比较
    let hue = if raw_hue < 0.0 {
        raw_hue + TWO_PI
    } else {
        raw_hue
    };
    Oklch {
        l: color.l,
        c: color.a.hypot(color.b),
        h: hue,
    }
}

fn oklch_to_oklab(color: Oklch) -> Oklab {
    Oklab {
        l: color.l,
        a: color.c * color.h.cos(),
        b: color.c * color.h.sin(),
    }
}

/// 便捷入口：0-255 的 sRGB 像素 → OKLab（TS pixelToOklab）
fn pixel_to_oklab(r: u8, g: u8, b: u8) -> Oklab {
    linear_rgb_to_oklab(
        srgb_to_linear(f64::from(r) / 255.0),
        srgb_to_linear(f64::from(g) / 255.0),
        srgb_to_linear(f64::from(b) / 255.0),
    )
}

fn linear_channel_to_byte(linear: f64) -> u8 {
    // 越界通道直接裁剪：取色场景不需要完整的 gamut mapping，
    // 派生色对色域边界的敏感度由整形步骤（fit_into_srgb_gamut）兜底
    let clamped = linear.clamp(0.0, 1.0);
    (linear_to_srgb(clamped) * 255.0).round() as u8
}

/// TS oklabToHex 的字节版（hex 串往返省去，数值口径一致）
fn oklab_to_rgb(color: Oklab) -> [u8; 3] {
    let [r, g, b] = oklab_to_linear_rgb(color);
    [
        linear_channel_to_byte(r),
        linear_channel_to_byte(g),
        linear_channel_to_byte(b),
    ]
}

/// TS oklchToHex 的字节版
fn oklch_to_rgb(color: Oklch) -> [u8; 3] {
    oklab_to_rgb(oklch_to_oklab(color))
}

// ---------------------------------------------------------------------------
// binary-split.ts —— 二分切分量化（median cut 的“更快”变体）
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct QuantizedLeaf {
    /// 叶内像素均值（OKLab）
    l: f64,
    a: f64,
    b: f64,
    /// 叶内像素数，评分时作为 coverage 的权重
    count: usize,
}

/// 每次全局挑“跨度最大”的桶，沿其跨度最大的 L/a/b 轴按中位数对分。
/// 沿最宽轴切：每次消掉当前最大的方差方向，叶体积收缩最快；
/// 中位对分保证两侧像素数均衡，大面积背景不会被切成碎片。
fn binary_split(pixels: &[Oklab], max_leaves: usize) -> Vec<QuantizedLeaf> {
    if pixels.is_empty() {
        return Vec::new();
    }
    let limit = max_leaves.max(1);
    // 不改动调用方数组，所有排序都发生在内部拷贝上
    let mut buckets: Vec<Vec<Oklab>> = vec![pixels.to_vec()];

    while buckets.len() < limit {
        let mut target: Option<usize> = None;
        let mut widest = 0.0;
        let mut axis = 0u8;
        for (i, bucket) in buckets.iter().enumerate() {
            if bucket.len() < MIN_SPLIT_PIXELS {
                continue;
            }
            let (span_axis, span) = widest_axis_span(bucket);
            if span > widest {
                widest = span;
                axis = span_axis;
                target = Some(i);
            }
        }
        // 所有桶都退化成单点（纯色图、极小图）时提前收工
        let target = match target {
            Some(t) => t,
            None => break,
        };
        if widest <= 0.0 {
            break;
        }
        let mut bucket = buckets.remove(target);
        // JS Array.sort 是稳定排序 → 必须用稳定排序的 sort_by
        bucket.sort_by(|p, q| compare_by_axis(axis, p, q));
        let median = bucket.len() >> 1;
        let tail = bucket.split_off(median);
        buckets.insert(target, bucket);
        buckets.insert(target + 1, tail);
    }

    buckets.into_iter().map(|b| leaf_mean(&b)).collect()
}

/// 返回 (axis, span)；轴优先级 B ≥ A ≥ L（tie-break 与 TS 分支顺序一致）
fn widest_axis_span(bucket: &[Oklab]) -> (u8, f64) {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for pixel in bucket {
        for axis in 0..3 {
            let v = match axis {
                0 => pixel.l,
                1 => pixel.a,
                _ => pixel.b,
            };
            if v < min[axis] {
                min[axis] = v;
            }
            if v > max[axis] {
                max[axis] = v;
            }
        }
    }
    let span_l = max[0] - min[0];
    let span_a = max[1] - min[1];
    let span_b = max[2] - min[2];
    if span_b >= span_a && span_b >= span_l {
        (2, span_b)
    } else if span_a >= span_l {
        (1, span_a)
    } else {
        (0, span_l)
    }
}

fn compare_by_axis(axis: u8, p: &Oklab, q: &Oklab) -> std::cmp::Ordering {
    match axis {
        0 => p.l.total_cmp(&q.l),
        1 => p.a.total_cmp(&q.a),
        _ => p.b.total_cmp(&q.b),
    }
}

fn leaf_mean(bucket: &[Oklab]) -> QuantizedLeaf {
    let mut l = 0.0;
    let mut a = 0.0;
    let mut b = 0.0;
    for pixel in bucket {
        l += pixel.l;
        a += pixel.a;
        b += pixel.b;
    }
    let count = bucket.len();
    QuantizedLeaf {
        l: l / count as f64,
        a: a / count as f64,
        b: b / count as f64,
        count,
    }
}

// ---------------------------------------------------------------------------
// score.ts —— 聚类评分、accent 挑选与颜色整形
// ---------------------------------------------------------------------------

fn score_leaf(leaf: &QuantizedLeaf, total_pixels: usize) -> f64 {
    // chroma = “音乐性”；coverage 取平方根，防止大面积背景（黑边、白墙）
    // 靠像素数垄断评分，把小而鲜艳的主体挤出局
    let chroma = leaf.a.hypot(leaf.b);
    let coverage = if total_pixels > 0 {
        (leaf.count as f64 / total_pixels as f64).sqrt()
    } else {
        0.0
    };
    chroma + COVERAGE_WEIGHT * coverage
}

/// 返回胜出叶；候选为空或胜出叶无彩色时返回 None（调用方走回退）
fn select_accent(leaves: &[QuantizedLeaf]) -> Option<QuantizedLeaf> {
    let total: usize = leaves.iter().map(|leaf| leaf.count).sum();
    let mut best: Option<&QuantizedLeaf> = None;
    let mut best_score = f64::NEG_INFINITY;
    for leaf in leaves {
        if leaf.l < LEAF_LIGHTNESS_RANGE.0 || leaf.l > LEAF_LIGHTNESS_RANGE.1 {
            continue;
        }
        let score = score_leaf(leaf, total);
        if score > best_score {
            best_score = score;
            best = Some(leaf);
        }
    }
    let best = best?;
    if best.a.hypot(best.b) < ACHROMATIC_CHROMA {
        return None;
    }
    Some(*best)
}

/// 把胜出色的 L 拉进目标窗（只夹 L，不动 C/h，色相保持不变），再收缩进 sRGB 色域
fn shape_accent(accent: Oklch, mode: Mode) -> Oklch {
    let target = ACCENT_TARGET_L[mode as usize];
    let l = (target + ACCENT_L_TOLERANCE).min((target - ACCENT_L_TOLERANCE).max(accent.l));
    fit_into_srgb_gamut(Oklch {
        l,
        c: accent.c,
        h: accent.h,
    })
}

/// 派生 deep 面板色：亮度压到面板目标、保留 80% 彩度维持与 accent 的血缘
fn deep_from_accent(accent: Oklch, mode: Mode) -> [u8; 3] {
    let deep = fit_into_srgb_gamut(Oklch {
        l: DEEP_TARGET_L[mode as usize],
        c: accent.c * DEEP_CHROMA_RATIO,
        h: accent.h,
    });
    oklch_to_rgb(deep)
}

/// 色域收缩只压 chroma：L（可读性目标）与 h（色相恒定）一个都不动。
/// 直接裁剪线性通道会在饱和色上把 L 拉离目标窗，这里用二分保证不越界。
const GAMUT_BISECTION_STEPS: usize = 24;

fn fit_into_srgb_gamut(color: Oklch) -> Oklch {
    if is_in_srgb_gamut(oklch_to_oklab(color)) {
        return color;
    }
    let mut low = 0.0; // 同亮度灰永远在色域内
    let mut high = color.c;
    for _ in 0..GAMUT_BISECTION_STEPS {
        let mid = (low + high) / 2.0;
        if is_in_srgb_gamut(oklch_to_oklab(Oklch {
            l: color.l,
            c: mid,
            h: color.h,
        })) {
            low = mid;
        } else {
            high = mid;
        }
    }
    Oklch {
        l: color.l,
        c: low,
        h: color.h,
    }
}

fn is_in_srgb_gamut(oklab: Oklab) -> bool {
    let [r, g, b] = oklab_to_linear_rgb(oklab);
    let epsilon = 1e-4;
    r >= -epsilon
        && r <= 1.0 + epsilon
        && g >= -epsilon
        && g <= 1.0 + epsilon
        && b >= -epsilon
        && b <= 1.0 + epsilon
}

/// WCAG 2.1 相对亮度：sRGB 先展开成线性再加权（与 TS 同式，供测试复用）
pub fn wcag_relative_luminance(rgb: [u8; 3]) -> f64 {
    0.2126 * srgb_to_linear(f64::from(rgb[0]) / 255.0)
        + 0.7152 * srgb_to_linear(f64::from(rgb[1]) / 255.0)
        + 0.0722 * srgb_to_linear(f64::from(rgb[2]) / 255.0)
}

/// WCAG 2.1 对比度（供测试复用）
pub fn contrast_ratio(a: [u8; 3], b: [u8; 3]) -> f64 {
    let la = wcag_relative_luminance(a);
    let lb = wcag_relative_luminance(b);
    let lighter = la.max(lb);
    let darker = la.min(lb);
    (lighter + 0.05) / (darker + 0.05)
}

/// accent 上的前景色二选一：白优先（品牌观感）。
/// 数学上任意颜色与黑白的对比度最大值恒 ≥ ~4.58，
/// 所以白不达标时黑必然 ≥ 4.5:1，不存在两者都不达标的颜色。
fn pick_on_accent(accent: [u8; 3]) -> [u8; 3] {
    if contrast_ratio(accent, WHITE) >= WCAG_AA_CONTRAST {
        WHITE
    } else {
        BLACK
    }
}

/// OKLab 线性插值：感知均匀空间里的中点才是“看起来”的中点
fn mix(from: Oklab, to: Oklab, t: f64) -> Oklab {
    let k = t.clamp(0.0, 1.0);
    Oklab {
        l: from.l + (to.l - from.l) * k,
        a: from.a + (to.a - from.a) * k,
        b: from.b + (to.b - from.b) * k,
    }
}

// ---------------------------------------------------------------------------
// index.ts —— 管线编排 + 派生色
// ---------------------------------------------------------------------------

/// RGBA8 四通道平铺像素（任意长度，len%4==0）→ 调色板。
/// alpha < 16 的像素跳过（MIN_PIXEL_ALPHA）。空输入/无彩色 → 整族品牌回退。
/// dark=false 对应 Vue mode:"light"。
pub fn extract(pixels: &[u8], dark: bool) -> TrackPalette {
    let mode = if dark { Mode::Dark } else { Mode::Light };
    match select_accent_rgb(pixels, mode) {
        Some(accent) => build_palette(accent, mode, false),
        None => build_palette(FALLBACK_ACCENT[mode as usize], mode, true),
    }
}

fn select_accent_rgb(pixels: &[u8], mode: Mode) -> Option<[u8; 3]> {
    let points = to_oklab_points(pixels);
    if points.is_empty() {
        return None;
    }
    let leaves = binary_split(&points, MAX_LEAVES);
    let winner = select_accent(&leaves)?;
    let accent = oklab_to_oklch(Oklab {
        l: winner.l,
        a: winner.a,
        b: winner.b,
    });
    let shaped = shape_accent(accent, mode);
    Some(oklch_to_rgb(shaped))
}

fn to_oklab_points(pixels: &[u8]) -> Vec<Oklab> {
    let mut points = Vec::with_capacity(pixels.len() / 4);
    for px in pixels.chunks_exact(4) {
        if px[3] < MIN_PIXEL_ALPHA {
            continue;
        }
        points.push(pixel_to_oklab(px[0], px[1], px[2]));
    }
    points
}

/// accent 与全部派生色共用一条管线，保证回退色与胜出色产出同一形状的调色板；
/// 回退色的 accent 保持原字节值（与 --accent 严格一致），派生色照常从它推导。
fn build_palette(accent_rgb: [u8; 3], mode: Mode, is_fallback: bool) -> TrackPalette {
    let m = mode as usize;
    let accent_oklab = pixel_to_oklab(accent_rgb[0], accent_rgb[1], accent_rgb[2]);
    let anchor = GRAD_FROM_ANCHOR[m];
    let grad_from_oklab = mix(
        accent_oklab,
        pixel_to_oklab(anchor[0], anchor[1], anchor[2]),
        GRAD_FROM_RATIO[m],
    );
    let grad_from_rgb = oklab_to_rgb(grad_from_oklab);
    let accent_oklch = oklab_to_oklch(accent_oklab);
    // 环境层顶部明暗随专辑走：谁与背景对比度更高用谁
    let on_ambient = if contrast_ratio(grad_from_rgb, AMBIENT_INK_LIGHT)
        >= contrast_ratio(grad_from_rgb, AMBIENT_INK_DARK)
    {
        AMBIENT_INK_LIGHT
    } else {
        AMBIENT_INK_DARK
    };

    TrackPalette {
        accent: rgb_color(accent_rgb),
        on_accent: rgb_color(pick_on_accent(accent_rgb)),
        accent_soft: Color::from_argb_u8(
            (SOFT_ALPHA[m] * 255.0).round() as u8,
            accent_rgb[0],
            accent_rgb[1],
            accent_rgb[2],
        ),
        deep: rgb_color(deep_from_accent(accent_oklch, mode)),
        deep_fg: rgb_color(DEEP_FOREGROUND),
        grad_from: rgb_color(grad_from_rgb),
        grad_to: rgb_color(GRAD_TO[m]),
        on_ambient: rgb_color(on_ambient),
        is_fallback,
    }
}

fn rgb_color(rgb: [u8; 3]) -> Color {
    Color::from_rgb_u8(rgb[0], rgb[1], rgb[2])
}

// ---------------------------------------------------------------------------
// 采样与环境层底图（对应 Vue adapter 的 canvas 路径；Slint 侧为像素级重写）
// ---------------------------------------------------------------------------

/// 换曲路径唯一一次全图物化的产物：to_rgba8 解出的完整位图被立即压到
/// 最长边 ≤ AMBIENT_MAX_SIDE 的 RGBA 缓冲，全尺寸副本随即释放。
/// 环境层底图与取色采样都从这份缓冲派生（旧实现两处各自物化一次全图）。
struct DecodedCover {
    width: u32,
    height: u32,
    /// RGBA8 平铺像素，len == width * height * 4
    rgba: Vec<u8>,
}

/// 封面 Image → 有界解码缓冲（全图 to_rgba8 只发生在这里）。
/// 非 RGBA8 嵌入位图（to_rgba8() 拿不到缓冲）或空图返回 None，调用方走回退。
fn decode_cover_bounded(image: &Image) -> Option<DecodedCover> {
    let full = image.to_rgba8()?;
    let (w, h) = (full.width(), full.height());
    if w == 0 || h == 0 {
        return None;
    }
    let (dw, dh) = ambient_target_size(w, h);
    let rgba = resample_area_average(full.as_bytes(), w, h, dw, dh);
    Some(DecodedCover {
        width: dw,
        height: dh,
        rgba,
    })
}

/// 把有界解码缓冲压到 size×size 的 RGBA8 平铺像素（面积平均降采样，
/// 纵横比不保持——对应 Vue adapter 的 canvas drawImage(img,0,0,64,64)）。
/// 从 480px 源取 64×64 采样对调色板质量绰绰有余。
fn sample_pixels(source: &DecodedCover, size: u32) -> Vec<u8> {
    resample_area_average(&source.rgba, source.width, source.height, size, size)
}

/// 环境层模糊底图：输入是有界解码缓冲（解码时已面积平均降采样到最长边 ≤ 480），
/// 3 趟盒滤波（半径随源宽等比 ~5%）近似高斯，饱和度 ×1.2（对应 CSS blur(80px) saturate(1.2)
/// 的预烘焙替代——Slint 无滤镜），输出 RGBA8 slint::Image。
/// 源图刻意偏大：显示区 ~1600px、软件渲染器（离屏 QA 宿主）是最近邻采样，
/// 480 源把最近邻块压到 ~3px、GPU 双线性则完全连续（128 源在最近邻下呈 12px 块）。
fn ambient_image(source: &DecodedCover) -> Image {
    let (w, h) = (source.width as usize, source.height as usize);
    let mut data: Vec<f64> = source.rgba.iter().map(|v| f64::from(*v)).collect();
    // 模糊半径随源宽等比（≈5%）：对应 CSS blur(80px) 在 ~1600px 显示区的相对强度，
    // 源图大小不一（程序化 256 / 真图 ≤480）时观感一致
    let radius = ((source.width as f64) * AMBIENT_BLUR_RATIO).round() as usize;
    let radius = radius.clamp(AMBIENT_BLUR_MIN_RADIUS, AMBIENT_MAX_SIDE as usize);
    for _ in 0..AMBIENT_BLUR_PASSES {
        box_blur_pass(&mut data, w, h, radius, false);
        box_blur_pass(&mut data, w, h, radius, true);
    }
    saturate_gamma(&mut data, AMBIENT_SATURATION);

    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(source.width, source.height);
    for (out, v) in buffer.make_mut_bytes().iter_mut().zip(data) {
        *out = v.round().clamp(0.0, 255.0) as u8;
    }
    Image::from_rgba8(buffer)
}

const AMBIENT_MAX_SIDE: u32 = 480;
/// 模糊半径 = 源宽 × 该比例（≈ CSS blur(80px)/1600px 显示区），下限保底
const AMBIENT_BLUR_RATIO: f64 = 0.05;
const AMBIENT_BLUR_MIN_RADIUS: usize = 4;
const AMBIENT_BLUR_PASSES: usize = 3;
/// 对应 CSS saturate(1.2)：c' = clamp(c + (c - luma) * 0.2)（gamma 域简单版）
const AMBIENT_SATURATION: f64 = 0.2;

/// 最长边 ≤ AMBIENT_MAX_SIDE（已小于则原样，不做放大）
fn ambient_target_size(w: u32, h: u32) -> (u32, u32) {
    let longest = w.max(h);
    if longest <= AMBIENT_MAX_SIDE {
        return (w, h);
    }
    let scale = f64::from(AMBIENT_MAX_SIDE) / f64::from(longest);
    let scaled = |v: u32| ((f64::from(v) * scale).round() as u32).max(1);
    (scaled(w), scaled(h))
}

/// 面积平均降采样：目标像素的盒与源像素的重叠面积作权重，四通道独立。
/// 目标与源同尺寸时退化为恒等映射（纯色图模糊后仍是同色的依据）。
fn resample_area_average(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    debug_assert_eq!(src.len(), (sw as usize) * (sh as usize) * 4);
    let mut out = vec![0u8; (dw as usize) * (dh as usize) * 4];
    let x_scale = f64::from(sw) / f64::from(dw);
    let y_scale = f64::from(sh) / f64::from(dh);
    for ty in 0..dh {
        let y0 = f64::from(ty) * y_scale;
        let y1 = f64::from(ty + 1) * y_scale;
        let sy0 = y0.floor() as u32;
        let sy1 = (y1.ceil() as u32).min(sh); // 不含
        for tx in 0..dw {
            let x0 = f64::from(tx) * x_scale;
            let x1 = f64::from(tx + 1) * x_scale;
            let sx0 = x0.floor() as u32;
            let sx1 = (x1.ceil() as u32).min(sw); // 不含
            let mut acc = [0.0f64; 4];
            let mut weight = 0.0;
            for sy in sy0..sy1 {
                let oy = y1.min(f64::from(sy + 1)) - y0.max(f64::from(sy));
                for sx in sx0..sx1 {
                    let ox = x1.min(f64::from(sx + 1)) - x0.max(f64::from(sx));
                    let w = ox * oy;
                    let i = ((sy * sw + sx) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += f64::from(src[i + c]) * w;
                    }
                    weight += w;
                }
            }
            let o = ((ty * dw + tx) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / weight).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// 单趟盒滤波（水平或垂直分离），边缘 clamp；window = 2*radius+1 个样本
fn box_blur_pass(data: &mut [f64], w: usize, h: usize, radius: usize, vertical: bool) {
    let window = (radius * 2 + 1) as f64;
    let mut tmp = vec![0.0; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let mut sums = [0.0f64; 4];
            for d in -(radius as isize)..=(radius as isize) {
                let (sx, sy) = if vertical {
                    (x, (y as isize + d).clamp(0, h as isize - 1) as usize)
                } else {
                    ((x as isize + d).clamp(0, w as isize - 1) as usize, y)
                };
                let i = (sy * w + sx) * 4;
                for c in 0..4 {
                    sums[c] += data[i + c];
                }
            }
            let o = (y * w + x) * 4;
            for c in 0..4 {
                tmp[o + c] = sums[c] / window;
            }
        }
    }
    data.copy_from_slice(&tmp);
}

/// gamma 域饱和度提升：luma = 0.2126r+0.7152g+0.0722b，c' = clamp(c + (c-luma)*amount)
fn saturate_gamma(data: &mut [f64], amount: f64) {
    for px in data.chunks_exact_mut(4) {
        let luma = 0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2];
        for c in &mut px[..3] {
            *c = (*c + (*c - luma) * amount).clamp(0.0, 255.0);
        }
    }
}

// ---------------------------------------------------------------------------
// UI 接线（player_bridge / bridge 调用；对应 Vue adapter.ts + trackTheme.ts）
// ---------------------------------------------------------------------------

/// Vue adapter 的采样边长：64×64 足以保住主色结构，量化成本压到常数级
const SAMPLE_SIZE: u32 = 64;

/// 最近一次写入 TrackPalette global 的取色来源（去重 + 主题重算的依据）
enum Applied {
    /// 尚未写过（启动空态：global 的 slint 默认值仍由 Theme.dark 三元绑定驱动）
    None,
    /// 品牌回退族（无曲 / 无彩色封面）
    Fallback,
    /// 胜出封面（key = `mid|url` 或 `mid|prog`）。只留 64×64 取色采样
    /// （≤16KB，主题翻转免解码重算整族）——不再常驻全尺寸封面位图；
    /// 环境层底图与亮暗无关，由 Player.ambient-cover 属性持有即可。
    Cover { key: String, sample: Vec<u8> },
}

thread_local! {
    /// 取色应用状态（来源 + 应用时的亮暗档）；slint::Image 非 Send/Sync，
    /// 只在 UI 线程读写（与 covers.rs 缓存同纪律）
    static APPLIED: RefCell<(Applied, bool)> = const { RefCell::new((Applied::None, false)) };
}

/// 封面 → 曲目层调色板 + 环境层模糊底图（player_bridge 每帧推送调用）。
/// 按键去重：10Hz 状态推送同曲同主题零重算；QQ 远程封面先吃程序化占位取色，
/// 真图回包后按 `mid|url` 键重算（见 player_bridge::spawn_cover_fetch）。
pub fn apply_cover(ui_weak: &Weak<AppWindow>, cover_key: &str, cover: &Image) {
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    let dark = Theme::get(&ui).get_dark();
    let skip = APPLIED.with(|cell| {
        let state = cell.borrow();
        matches!(&state.0, Applied::Cover { key, .. } if key == cover_key) && state.1 == dark
    });
    if skip {
        return;
    }

    // 每次换曲只解码一次：全图物化 → 立即压到 ≤480px 有界缓冲，环境层
    // 底图与取色采样都从它派生。解码失败（非 RGBA8 嵌入位图，对应 Vue
    // adapter canvas 采样的跨域/纹理失败）→ 回退族 + 空环境层
    let source = decode_cover_bounded(cover);
    let sample = source.as_ref().map(|s| sample_pixels(s, SAMPLE_SIZE));
    let palette = match &sample {
        Some(pixels) => extract(pixels, dark),
        None => extract(&[], dark),
    };
    let ambient = source.as_ref().map(ambient_image).unwrap_or_default();
    write_palette(&ui, &palette);
    Player::get(&ui).set_ambient_cover(ambient);
    APPLIED.with(|cell| {
        *cell.borrow_mut() = (
            Applied::Cover {
                key: cover_key.to_owned(),
                sample: sample.unwrap_or_default(),
            },
            dark,
        );
    });
}

/// 无当前曲：曲目层整族回落品牌胡桃木、环境层清空（player_bridge 空态调用）。
pub fn apply_fallback(ui: &AppWindow) {
    let dark = Theme::get(ui).get_dark();
    let skip = APPLIED.with(|cell| {
        let state = cell.borrow();
        matches!(state.0, Applied::Fallback) && state.1 == dark
    });
    if skip {
        return;
    }

    write_palette(ui, &extract(&[], dark));
    Player::get(ui).set_ambient_cover(Image::default());
    APPLIED.with(|cell| *cell.borrow_mut() = (Applied::Fallback, dark));
}

/// 主题亮暗翻转 → 按当前来源整族重算（bridge 的 cycle/set-mode 与系统偏好
/// 漂移回调）。无取色时空操作（global 默认值的三元绑定自随主题）。
pub fn reapply_for_theme(ui: &AppWindow) {
    let dark = Theme::get(ui).get_dark();
    let already = APPLIED.with(|cell| {
        let state = cell.borrow();
        !matches!(state.0, Applied::None) && state.1 == dark
    });
    if already {
        return;
    }
    let taken = APPLIED.with(|cell| std::mem::replace(&mut cell.borrow_mut().0, Applied::None));
    match taken {
        Applied::None => {}
        Applied::Fallback => apply_fallback(ui),
        Applied::Cover { key, sample } => {
            // 环境层底图与亮暗无关（Player.ambient-cover 保持现值），
            // 只需从缓存的 64×64 采样免解码重算调色板整族
            let palette = extract(&sample, dark);
            write_palette(ui, &palette);
            APPLIED.with(|cell| {
                *cell.borrow_mut() = (Applied::Cover { key, sample }, dark);
            });
        }
    }
}

fn write_palette(ui: &AppWindow, palette: &TrackPalette) {
    let global = TrackPaletteGlobal::get(ui);
    global.set_accent(palette.accent);
    global.set_on_accent(palette.on_accent);
    global.set_accent_soft(palette.accent_soft);
    global.set_deep(palette.deep);
    global.set_deep_fg(palette.deep_fg);
    global.set_grad_from(palette.grad_from);
    global.set_grad_to(palette.grad_to);
    global.set_on_ambient(palette.on_ambient);
    global.set_is_fallback(palette.is_fallback);
}

// ---------------------------------------------------------------------------
// 测试（镜像 apps/hmp-tauri/src/lib/color/color.test.ts）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // 16×16 = 256 像素（2 的幂，叶均值无浮点累积误差）
    const SIZE: usize = 16;
    // hex 只有 8bit 精度，通道各差 1/255 会带来 ~0.002 的 L 漂移，断言按此放宽
    const QUANTIZATION_SLACK: f64 = 0.011;

    fn solid_pixels(r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut pixels = vec![0u8; SIZE * SIZE * 4];
        for px in pixels.chunks_exact_mut(4) {
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = 255;
        }
        pixels
    }

    fn half_red_half_blue() -> Vec<u8> {
        let mut pixels = vec![0u8; SIZE * SIZE * 4];
        let half = pixels.len() / 2;
        for (i, px) in pixels.chunks_exact_mut(4).enumerate() {
            let is_red = i * 4 < half;
            px[0] = if is_red { 255 } else { 0 };
            px[2] = if is_red { 0 } else { 255 };
            px[3] = 255;
        }
        pixels
    }

    fn solid_image(w: u32, h: u32, r: u8, g: u8, b: u8) -> Image {
        let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
        for px in buffer.make_mut_bytes().chunks_exact_mut(4) {
            px.copy_from_slice(&[r, g, b, 255]);
        }
        Image::from_rgba8(buffer)
    }

    fn color_rgb(color: Color) -> [u8; 3] {
        let c = color.to_argb_u8();
        [c.red, c.green, c.blue]
    }

    fn accent_oklch(rgb: [u8; 3]) -> Oklch {
        oklab_to_oklch(pixel_to_oklab(rgb[0], rgb[1], rgb[2]))
    }

    fn hue_distance(a: f64, b: f64) -> f64 {
        let diff = (a - b).abs() % TWO_PI;
        diff.min(TWO_PI - diff)
    }

    #[test]
    fn solid_red_keeps_hue_and_hits_lightness_window() {
        let palette = extract(&solid_pixels(255, 0, 0), false);
        let accent = accent_oklch(color_rgb(palette.accent));
        let pure_red = accent_oklch([255, 0, 0]);

        assert!(!palette.is_fallback);
        assert!(hue_distance(accent.h, pure_red.h) <= 0.05);
        assert!(accent.l >= 0.6 - 0.03 - QUANTIZATION_SLACK);
        assert!(accent.l <= 0.6 + 0.03 + QUANTIZATION_SLACK);
    }

    #[test]
    fn half_red_half_blue_winner_is_one_of_them() {
        let palette = extract(&half_red_half_blue(), false);
        let accent = accent_oklch(color_rgb(palette.accent));
        let red = accent_oklch([255, 0, 0]);
        let blue = accent_oklch([0, 0, 255]);

        let distance = hue_distance(accent.h, red.h).min(hue_distance(accent.h, blue.h));
        assert!(distance <= 0.05);
    }

    #[test]
    fn gray_image_falls_back_to_brand_walnut() {
        let palette = extract(&solid_pixels(128, 128, 128), false);

        assert!(palette.is_fallback);
        assert_eq!(color_rgb(palette.accent), [0xB3, 0x4A, 0x3A]);
    }

    #[test]
    fn empty_input_returns_fallback_family() {
        let light = extract(&[], false);
        let dark = extract(&[], true);

        assert!(light.is_fallback);
        assert!(dark.is_fallback);
        assert_eq!(color_rgb(light.accent), [0xB3, 0x4A, 0x3A]);
        assert_eq!(color_rgb(dark.accent), [0xD0, 0x64, 0x52]);
        // deep_fg 非零（TS 断言字符串非空）
        let fg = light.deep_fg.to_argb_u8();
        assert!(fg.alpha != 0 || fg.red != 0 || fg.green != 0 || fg.blue != 0);
    }

    #[test]
    fn near_transparent_pixels_are_skipped() {
        // alpha < 16（MIN_PIXEL_ALPHA）不参与取色 → 整族回退
        let mut pixels = solid_pixels(255, 0, 0);
        for px in pixels.chunks_exact_mut(4) {
            px[3] = MIN_PIXEL_ALPHA - 1;
        }
        let palette = extract(&pixels, false);
        assert!(palette.is_fallback);
    }

    #[test]
    fn on_accent_contrast_meets_wcag_aa() {
        // 饱和红在亮暗两种整形后都可能偏向白不可达的一侧，两种模式都验证
        for dark in [false, true] {
            let palette = extract(&solid_pixels(255, 0, 0), dark);
            assert!(contrast_ratio(color_rgb(palette.accent), color_rgb(palette.on_accent)) >= 4.5);
        }
        let fallback = extract(&[], false);
        assert!(contrast_ratio(color_rgb(fallback.accent), color_rgb(fallback.on_accent)) >= 4.5);
    }

    #[test]
    fn accent_lightness_per_mode_window() {
        let light = accent_oklch(color_rgb(extract(&solid_pixels(255, 0, 0), false).accent));
        let dark = accent_oklch(color_rgb(extract(&solid_pixels(255, 0, 0), true).accent));

        assert!(light.l >= 0.6 - 0.03 - QUANTIZATION_SLACK);
        assert!(light.l <= 0.6 + 0.03 + QUANTIZATION_SLACK);
        assert!(dark.l >= 0.68 - 0.03 - QUANTIZATION_SLACK);
        assert!(dark.l <= 0.68 + 0.03 + QUANTIZATION_SLACK);
    }

    #[test]
    fn derived_fields_follow_conventions() {
        let light = extract(&solid_pixels(30, 120, 200), false);
        let dark = extract(&solid_pixels(30, 120, 200), true);

        let soft = light.accent_soft.to_argb_u8();
        assert_eq!(soft.alpha, (0.12f64 * 255.0).round() as u8); // 31
        assert_eq!([soft.red, soft.green, soft.blue], color_rgb(light.accent));
        let soft = dark.accent_soft.to_argb_u8();
        assert_eq!(soft.alpha, (0.2f64 * 255.0).round() as u8); // 51

        assert_eq!(color_rgb(light.grad_to), [0xFA, 0xF9, 0xF8]);
        assert_eq!(color_rgb(dark.grad_to), [0x18, 0x14, 0x12]);
        assert_eq!(color_rgb(light.deep_fg), [0xF7, 0xF0, 0xEA]);
        assert_eq!(color_rgb(dark.deep_fg), [0xF7, 0xF0, 0xEA]);
    }

    #[test]
    fn fallback_grad_from_matches_ts_anchor() {
        // node 按 TS 公式手算（跨语言一致的锚点）：
        //   light = oklabToHex(mix(hexToOklab("#B34A3A"), hexToOklab("#FFFFFF"), 0.82)) = #F4DEDA
        let light = extract(&[], false);
        assert_eq!(color_rgb(light.grad_from), [244, 222, 218]);
        //   dark  = oklabToHex(mix(hexToOklab("#D06452"), hexToOklab("#1F1B17"), 0.75)) = #472D25
        let dark = extract(&[], true);
        assert_eq!(color_rgb(dark.grad_from), [71, 45, 37]);
    }

    #[test]
    fn sample_pixels_downscales_solid() {
        let image = solid_image(8, 8, 200, 60, 40);
        let source = decode_cover_bounded(&image).expect("embedded rgba8 is decodable");
        let pixels = sample_pixels(&source, 4);

        assert_eq!(pixels.len(), 4 * 4 * 4);
        for px in pixels.chunks_exact(4) {
            assert_eq!(px, [200u8, 60, 40, 255].as_slice());
        }
    }

    #[test]
    fn decode_cover_bounded_caps_longest_side() {
        // 960×480 → 最长边压到 480（面积平均等比）；≤480 的源原样保留（不放大）
        let big = solid_image(960, 480, 10, 20, 30);
        let source = decode_cover_bounded(&big).expect("embedded rgba8 is decodable");
        assert_eq!((source.width, source.height), (480, 240));
        assert_eq!(source.rgba.len(), 480 * 240 * 4);

        let small = solid_image(8, 8, 10, 20, 30);
        let source = decode_cover_bounded(&small).expect("embedded rgba8 is decodable");
        assert_eq!((source.width, source.height), (8, 8));
    }

    #[test]
    fn ambient_image_blurs_solid_to_same_color() {
        // 纯红：盒滤波对均匀图是恒等（clamp 语义），饱和度 ×1.2 后
        // r 越界 clamp 回 255、g/b 负值 clamp 回 0 → 仍是纯红
        let image = solid_image(8, 8, 255, 0, 0);
        let source = decode_cover_bounded(&image).expect("embedded rgba8 is decodable");
        let ambient = ambient_image(&source);

        let size = ambient.size();
        assert!(size.width > 0 && size.width <= 128);
        assert!(size.height > 0 && size.height <= 128);
        let buffer = ambient.to_rgba8().expect("ambient is rgba8");
        assert_eq!(buffer.as_bytes().len() % 4, 0);
        for px in buffer.as_bytes().chunks_exact(4) {
            assert_eq!(px, [255u8, 0, 0, 255].as_slice());
        }
    }
}
