// 聚类评分、accent 挑选与颜色整形。
// 一切判断都在 OKLab/OKLch 中进行：L 直接对应明度观感，
// C 直接对应"艳不艳"，这里是整条取色管线的品味所在。
import type { QuantizedLeaf } from "./binary-split";
import {
  oklabToLinearRgb,
  oklchToHex,
  oklchToOklab,
  parseHexToRgb,
  srgbToLinear,
  type Oklab,
  type Oklch,
} from "./oklab";

export type ColorMode = "light" | "dark";

// 叶的 L 合法窗：近黑近白当主色既不可读也不耐看，直接淘汰
export const LEAF_LIGHTNESS_RANGE = { min: 0.2, max: 0.9 } as const;
// C 低于该值视为无彩色（灰阶封面），整族走回退
export const ACHROMATIC_CHROMA = 0.04;
// accent 目标亮度：亮色模式略暗（白底上稳）、暗色模式略亮（黑底上跳）
export const ACCENT_TARGET_L: Record<ColorMode, number> = { light: 0.6, dark: 0.68 };
export const ACCENT_L_TOLERANCE = 0.03;
// deep 深色面板底的亮度目标
export const DEEP_TARGET_L: Record<ColorMode, number> = { light: 0.35, dark: 0.22 };
const DEEP_CHROMA_RATIO = 0.8;
// coverage 在评分中的权重（chroma 权重恒为 1）
const COVERAGE_WEIGHT = 0.5;
// WCAG 2.1 AA 级正文对比阈值
const WCAG_AA_CONTRAST = 4.5;
const WHITE = "#FFFFFF";
const BLACK = "#000000";

export const scoreLeaf = (leaf: QuantizedLeaf, totalPixels: number): number => {
  // chroma = "音乐性"；coverage 取平方根，防止大面积背景（黑边、白墙）
  // 靠像素数垄断评分，把小而鲜艳的主体挤出局
  const chroma = Math.hypot(leaf.a, leaf.b);
  const coverage = totalPixels > 0 ? Math.sqrt(leaf.count / totalPixels) : 0;
  return chroma + COVERAGE_WEIGHT * coverage;
};

// 返回胜出叶；候选为空或胜出叶无彩色时返回 null（调用方走回退）
export const selectAccent = (leaves: readonly QuantizedLeaf[]): QuantizedLeaf | null => {
  const total = leaves.reduce((sum, leaf) => sum + leaf.count, 0);
  let best: QuantizedLeaf | null = null;
  let bestScore = -Infinity;
  for (const leaf of leaves) {
    if (leaf.l < LEAF_LIGHTNESS_RANGE.min || leaf.l > LEAF_LIGHTNESS_RANGE.max) continue;
    const score = scoreLeaf(leaf, total);
    if (score > bestScore) {
      bestScore = score;
      best = leaf;
    }
  }
  if (!best) return null;
  if (Math.hypot(best.a, best.b) < ACHROMATIC_CHROMA) return null;
  return best;
};

// 把胜出色的 L 拉进目标窗（只夹 L，不动 C/h，色相保持不变），再收缩进 sRGB 色域
export const shapeAccent = (accent: Oklch, mode: ColorMode): Oklch => {
  const target = ACCENT_TARGET_L[mode];
  const l = Math.min(
    target + ACCENT_L_TOLERANCE,
    Math.max(target - ACCENT_L_TOLERANCE, accent.l),
  );
  return fitIntoSrgbGamut({ l, c: accent.c, h: accent.h });
};

// 派生 deep 面板色：亮度压到面板目标、保留 80% 彩度维持与 accent 的血缘
export const deepFromAccent = (accent: Oklch, mode: ColorMode): string => {
  const deep = fitIntoSrgbGamut({
    l: DEEP_TARGET_L[mode],
    c: accent.c * DEEP_CHROMA_RATIO,
    h: accent.h,
  });
  return oklchToHex(deep.l, deep.c, deep.h);
};

// 色域收缩只压 chroma：L（可读性目标）与 h（色相恒定）一个都不动。
// 直接裁剪线性通道会在饱和色上把 L 拉离目标窗，这里用二分保证不越界。
export const fitIntoSrgbGamut = (color: Oklch): Oklch => {
  if (isInSrgbGamut(oklchToOklab(color.l, color.c, color.h))) return color;
  let low = 0; // 同亮度灰永远在色域内
  let high = color.c;
  for (let i = 0; i < 24; i += 1) {
    const mid = (low + high) / 2;
    if (isInSrgbGamut(oklchToOklab(color.l, mid, color.h))) {
      low = mid;
    } else {
      high = mid;
    }
  }
  return { l: color.l, c: low, h: color.h };
};

const isInSrgbGamut = (oklab: Oklab): boolean => {
  const { r, g, b } = oklabToLinearRgb(oklab.l, oklab.a, oklab.b);
  const epsilon = 1e-4;
  return (
    r >= -epsilon && r <= 1 + epsilon &&
    g >= -epsilon && g <= 1 + epsilon &&
    b >= -epsilon && b <= 1 + epsilon
  );
};

// WCAG 2.1 相对亮度：sRGB 先展开成线性再加权
export const wcagRelativeLuminance = (hex: string): number => {
  const { r, g, b } = parseHexToRgb(hex);
  return (
    0.2126 * srgbToLinear(r / 255) +
    0.7152 * srgbToLinear(g / 255) +
    0.0722 * srgbToLinear(b / 255)
  );
};

export const contrastRatio = (a: string, b: string): number => {
  const la = wcagRelativeLuminance(a);
  const lb = wcagRelativeLuminance(b);
  const lighter = Math.max(la, lb);
  const darker = Math.min(la, lb);
  return (lighter + 0.05) / (darker + 0.05);
};

// accent 上的前景色二选一：白优先（品牌观感）。
// 数学上任意颜色与黑白的对比度最大值恒 ≥ ~4.58，
// 所以白不达标时黑必然 ≥ 4.5:1，不存在两者都不达标的颜色。
export const pickOnAccent = (accentHex: string): string =>
  contrastRatio(accentHex, WHITE) >= WCAG_AA_CONTRAST ? WHITE : BLACK;

// OKLab 线性插值：感知均匀空间里的中点才是"看起来"的中点
export const mix = (from: Oklab, to: Oklab, t: number): Oklab => {
  const k = Math.min(1, Math.max(0, t));
  return {
    l: from.l + (to.l - from.l) * k,
    a: from.a + (to.a - from.a) * k,
    b: from.b + (to.b - from.b) * k,
  };
};
