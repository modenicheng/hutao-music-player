// 主色提取模块唯一对外入口（DESIGN.md §2）。
// 管线：RGBA 像素 → OKLab → 二分切分量化（可选 k-means 精修）→
// 评分挑 accent → 整形（L 目标窗 + 色域收缩）→ 派生整族 --track-* 颜色。
// 无封面 / 灰阶封面 / 空输入时整族回退 walnut 品牌色，UI 永不出现无色状态。
import { binarySplit } from "./binary-split";
import { refine } from "./kmeans";
import {
  contrastRatio,
  deepFromAccent,
  mix,
  pickOnAccent,
  selectAccent,
  shapeAccent,
  type ColorMode,
} from "./score";
import {
  hexToOklab,
  oklabToHex,
  oklabToOklch,
  oklchToHex,
  parseHexToRgb,
  pixelToOklab,
  type Oklab,
} from "./oklab";

export interface TrackPalette {
  /** 播放态强调色（hex） */
  accent: string;
  /** accent 上的文字/图标，黑或白，保证 WCAG ≥ 4.5:1 */
  onAccent: string;
  /** hover/选中底：亮色模式 accent @ 0.12，暗色模式 @ 0.20 */
  accentSoft: string;
  /** 播放页/播放列表深色面板底（hex） */
  deep: string;
  /** deep 面板上的正文暖白 */
  deepFg: string;
  /** 播放页环境渐变起点（hex） */
  gradFrom: string;
  /** 播放页环境渐变终点（hex） */
  gradTo: string;
  /** 环境层顶部浮动元素（收起键等）的墨色：按 gradFrom 对比度自动取深/浅 */
  onAmbient: string;
  /** 是否为回退调色板（无输入 / 无彩色封面） */
  isFallback: boolean;
}

export interface ExtractOptions {
  /** 亮/暗模式，决定 accent 的目标亮度窗与透明度档位 */
  mode?: ColorMode;
  /** 可选的加权 k-means 精修，默认关闭 */
  refine?: "none" | "kmeans";
  /** 二分切分的叶子数上限，默认 10 */
  maxLeaves?: number;
}

// 与 styles/index.css 的 --accent 一致：回退即品牌胡桃木
const FALLBACK_ACCENT: Record<ColorMode, string> = { light: "#B34A3A", dark: "#D06452" };
const DEEP_FOREGROUND = "#F7F0EA";
const GRAD_TO: Record<ColorMode, string> = { light: "#FAF9F8", dark: "#181412" };
// 渐变起点 = accent 朝中性端混合：亮色拉向白、暗色压向暖黑
const GRAD_FROM: Record<ColorMode, { anchor: string; ratio: number }> = {
  light: { anchor: "#FFFFFF", ratio: 0.82 },
  dark: { anchor: "#1F1B17", ratio: 0.75 },
};
const SOFT_ALPHA: Record<ColorMode, number> = { light: 0.12, dark: 0.2 };
// 环境层墨色两极：暖黑与亮色主题 foreground 同源，暖白与渐变终点同源
const AMBIENT_INK_DARK = "#342827";
const AMBIENT_INK_LIGHT = "#FAF9F8";
// alpha 低于该阈值的像素视为透明（画布留白），不参与取色
const MIN_PIXEL_ALPHA = 16;

export const extractPalette = (
  pixels: Uint8ClampedArray,
  options: ExtractOptions = {},
): TrackPalette => {
  const mode = options.mode ?? "light";
  const accentHex = selectAccentHex(pixels, options, mode);
  if (accentHex !== null) return buildPalette(accentHex, mode, false);
  return buildPalette(FALLBACK_ACCENT[mode], mode, true);
};

const selectAccentHex = (
  pixels: Uint8ClampedArray,
  options: ExtractOptions,
  mode: ColorMode,
): string | null => {
  const points = toOklabPoints(pixels);
  if (points.length === 0) return null;
  let leaves = binarySplit(points, options.maxLeaves ?? 10);
  if (options.refine === "kmeans") leaves = refine(leaves, leaves);
  const winner = selectAccent(leaves);
  if (!winner) return null;
  const accent = oklabToOklch(winner.l, winner.a, winner.b);
  const shaped = shapeAccent(accent, mode);
  return oklchToHex(shaped.l, shaped.c, shaped.h);
};

const toOklabPoints = (pixels: Uint8ClampedArray): Oklab[] => {
  const points: Oklab[] = [];
  for (let i = 0; i + 3 < pixels.length; i += 4) {
    if (pixels[i + 3] < MIN_PIXEL_ALPHA) continue;
    points.push(pixelToOklab(pixels[i], pixels[i + 1], pixels[i + 2]));
  }
  return points;
};

// accent 与全部派生色共用一条管线，保证回退色与胜出色产出同一形状的调色板；
// 回退色的 accent 保持原 hex（与 --accent 严格一致），派生色照常从它推导。
const buildPalette = (accentHex: string, mode: ColorMode, isFallback: boolean): TrackPalette => {
  const accentOklab = hexToOklab(accentHex);
  const soft = parseHexToRgb(accentHex);
  const grad = GRAD_FROM[mode];
  const gradFrom = mix(accentOklab, hexToOklab(grad.anchor), grad.ratio);
  const accent = oklabToOklch(accentOklab.l, accentOklab.a, accentOklab.b);
  const gradFromHex = oklabToHex(gradFrom.l, gradFrom.a, gradFrom.b);
  return {
    accent: accentHex,
    onAccent: pickOnAccent(accentHex),
    accentSoft: `rgba(${soft.r}, ${soft.g}, ${soft.b}, ${SOFT_ALPHA[mode]})`,
    deep: deepFromAccent(accent, mode),
    deepFg: DEEP_FOREGROUND,
    gradFrom: gradFromHex,
    gradTo: GRAD_TO[mode],
    // 环境层顶部明暗随专辑走：谁与背景对比度更高用谁
    onAmbient:
      contrastRatio(gradFromHex, AMBIENT_INK_LIGHT) >=
      contrastRatio(gradFromHex, AMBIENT_INK_DARK)
        ? AMBIENT_INK_LIGHT
        : AMBIENT_INK_DARK,
    isFallback,
  };
};
