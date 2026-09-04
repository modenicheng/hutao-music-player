// OKLab 色彩空间转换核心（Björn Ottosson 的标准矩阵，公有领域）。
// 为什么用 OKLab：它感知均匀，数值上的距离与人眼感知的色差一致，
// 量化、评分、插值在这一空间进行才稳定；HSV 在蓝紫区非线性失真严重，
// 会导致"看起来更艳的蓝"被系统性高估或错切色相。

export interface LinearRgb {
  r: number;
  g: number;
  b: number;
}

export interface Oklab {
  l: number;
  a: number;
  b: number;
}

export interface Oklch {
  l: number;
  c: number;
  h: number;
}

export interface Srgb {
  r: number;
  g: number;
  b: number;
}

// sRGB 传递函数的分段点（IEC 61966-2-1）
const SRGB_TO_LINEAR_THRESHOLD = 0.04045;
const LINEAR_TO_SRGB_THRESHOLD = 0.0031308;

export const srgbToLinear = (channel: number): number => {
  return channel <= SRGB_TO_LINEAR_THRESHOLD
    ? channel / 12.92
    : Math.pow((channel + 0.055) / 1.055, 2.4);
};

export const linearToSrgb = (channel: number): number => {
  const sign = channel < 0 ? -1 : 1;
  const value = Math.abs(channel);
  const encoded =
    value <= LINEAR_TO_SRGB_THRESHOLD
      ? value * 12.92
      : 1.055 * Math.pow(value, 1 / 2.4) - 0.055;
  return sign * encoded;
};

export const linearRgbToOklab = (r: number, g: number, b: number): Oklab => {
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return {
    l: 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
    a: 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
    b: 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
  };
};

export const oklabToLinearRgb = (l: number, a: number, b: number): LinearRgb => {
  const l_ = l + 0.3963377774 * a + 0.2158037573 * b;
  const m_ = l - 0.1055613458 * a - 0.0638541728 * b;
  const s_ = l - 0.0894841775 * a - 1.2914855480 * b;
  const ll = l_ * l_ * l_;
  const mm = m_ * m_ * m_;
  const ss = s_ * s_ * s_;
  return {
    r: 4.0767416621 * ll - 3.3077115913 * mm + 0.2309699292 * ss,
    g: -1.2684380046 * ll + 2.6097574011 * mm - 0.3413193965 * ss,
    b: -0.0041960863 * ll - 0.7034186147 * mm + 1.7076147010 * ss,
  };
};

export const oklabToOklch = (l: number, a: number, b: number): Oklch => {
  const rawHue = Math.atan2(b, a);
  // 色相归一到 [0, 2π)，方便外部直接比较
  const hue = rawHue < 0 ? rawHue + Math.PI * 2 : rawHue;
  return { l, c: Math.hypot(a, b), h: hue };
};

export const oklchToOklab = (l: number, c: number, h: number): Oklab => {
  return { l, a: c * Math.cos(h), b: c * Math.sin(h) };
};

// 便捷入口：0-255 的 sRGB 像素 → OKLab
export const pixelToOklab = (r: number, g: number, b: number): Oklab => {
  return linearRgbToOklab(
    srgbToLinear(r / 255),
    srgbToLinear(g / 255),
    srgbToLinear(b / 255),
  );
};

export const oklabToHex = (l: number, a: number, b: number): string => {
  const { r, g, b: blue } = oklabToLinearRgb(l, a, b);
  return rgbToHex(linearChannelToByte(r), linearChannelToByte(g), linearChannelToByte(blue));
};

export const oklchToHex = (l: number, c: number, h: number): string => {
  const oklab = oklchToOklab(l, c, h);
  return oklabToHex(oklab.l, oklab.a, oklab.b);
};

export const parseHexToRgb = (hex: string): Srgb => {
  const value = Number.parseInt(hex.replace("#", ""), 16);
  if (!Number.isFinite(value)) return { r: 0, g: 0, b: 0 };
  return { r: (value >> 16) & 0xff, g: (value >> 8) & 0xff, b: value & 0xff };
};

export const hexToOklab = (hex: string): Oklab => {
  const { r, g, b } = parseHexToRgb(hex);
  return pixelToOklab(r, g, b);
};

export const rgbToHex = (r: number, g: number, b: number): string => {
  const value = (1 << 24) | (r << 16) | (g << 8) | b;
  return `#${value.toString(16).slice(1).toUpperCase()}`;
};

const linearChannelToByte = (linear: number): number => {
  // 越界通道直接裁剪：取色场景不需要完整的 gamut mapping，
  // 派生色对色域边界的敏感度由 score.ts 的整形步骤兜底
  const clamped = Math.min(1, Math.max(0, linear));
  return Math.round(linearToSrgb(clamped) * 255);
};
