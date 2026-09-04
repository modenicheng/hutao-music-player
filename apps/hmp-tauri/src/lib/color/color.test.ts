import { describe, expect, it } from "vitest";
import { extractPalette } from "./index";
import { hexToOklab, oklabToOklch } from "./oklab";
import { contrastRatio } from "./score";

// 16×16 = 256 像素（2 的幂，叶均值无浮点累积误差）
const SIZE = 16;
// hex 只有 8bit 精度，通道各差 1/255 会带来 ~0.002 的 L 漂移，断言按此放宽
const QUANTIZATION_SLACK = 0.011;

const solidImage = (r: number, g: number, b: number): Uint8ClampedArray => {
  const pixels = new Uint8ClampedArray(SIZE * SIZE * 4);
  for (let i = 0; i < pixels.length; i += 4) {
    pixels[i] = r;
    pixels[i + 1] = g;
    pixels[i + 2] = b;
    pixels[i + 3] = 255;
  }
  return pixels;
};

const halfRedHalfBlue = (): Uint8ClampedArray => {
  const pixels = new Uint8ClampedArray(SIZE * SIZE * 4);
  for (let i = 0; i < pixels.length; i += 4) {
    const isRed = i < pixels.length / 2;
    pixels[i] = isRed ? 255 : 0;
    pixels[i + 2] = isRed ? 0 : 255;
    pixels[i + 3] = 255;
  }
  return pixels;
};

const accentOklch = (hex: string) => {
  const oklab = hexToOklab(hex);
  return oklabToOklch(oklab.l, oklab.a, oklab.b);
};

const hueDistance = (a: number, b: number): number => {
  const diff = Math.abs(a - b) % (Math.PI * 2);
  return Math.min(diff, Math.PI * 2 - diff);
};

describe("extractPalette", () => {
  it("纯红色图的 accent 保持红相且亮度落在目标窗", () => {
    const palette = extractPalette(solidImage(255, 0, 0));
    const accent = accentOklch(palette.accent);
    const pureRed = accentOklch("#FF0000");

    expect(palette.isFallback).toBe(false);
    expect(hueDistance(accent.h, pureRed.h)).toBeLessThanOrEqual(0.05);
    expect(accent.l).toBeGreaterThanOrEqual(0.6 - 0.03 - QUANTIZATION_SLACK);
    expect(accent.l).toBeLessThanOrEqual(0.6 + 0.03 + QUANTIZATION_SLACK);
  });

  it("红蓝各半时胜出者必为其中之一", () => {
    const palette = extractPalette(halfRedHalfBlue());
    const accent = accentOklch(palette.accent);
    const red = accentOklch("#FF0000");
    const blue = accentOklch("#0000FF");

    const distance = Math.min(hueDistance(accent.h, red.h), hueDistance(accent.h, blue.h));
    expect(distance).toBeLessThanOrEqual(0.05);
  });

  it("灰阶图整族回退且 accent 等于品牌胡桃木色", () => {
    const palette = extractPalette(solidImage(128, 128, 128));

    expect(palette.isFallback).toBe(true);
    expect(palette.accent).toBe("#B34A3A");
  });

  it("空输入不崩溃并返回回退调色板", () => {
    const light = extractPalette(new Uint8ClampedArray(0));
    const dark = extractPalette(new Uint8ClampedArray(0), { mode: "dark" });

    expect(light.isFallback).toBe(true);
    expect(light.accent).toBe("#B34A3A");
    expect(dark.accent).toBe("#D06452");
    expect(light.deepFg.length).toBeGreaterThan(0);
  });

  it("onAccent 与 accent 的 WCAG 对比度至少 4.5:1", () => {
    // 饱和红在亮暗两种整形后都可能偏向白不可达的一侧，两种模式都验证
    const light = extractPalette(solidImage(255, 0, 0), { mode: "light" });
    const dark = extractPalette(solidImage(255, 0, 0), { mode: "dark" });
    const fallback = extractPalette(new Uint8ClampedArray(0));

    expect(contrastRatio(light.accent, light.onAccent)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(dark.accent, dark.onAccent)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(fallback.accent, fallback.onAccent)).toBeGreaterThanOrEqual(4.5);
  });

  it("亮暗两模式的 accent 亮度分别落在各自目标窗", () => {
    const light = accentOklch(extractPalette(solidImage(255, 0, 0), { mode: "light" }).accent);
    const dark = accentOklch(extractPalette(solidImage(255, 0, 0), { mode: "dark" }).accent);

    expect(light.l).toBeGreaterThanOrEqual(0.6 - 0.03 - QUANTIZATION_SLACK);
    expect(light.l).toBeLessThanOrEqual(0.6 + 0.03 + QUANTIZATION_SLACK);
    expect(dark.l).toBeGreaterThanOrEqual(0.68 - 0.03 - QUANTIZATION_SLACK);
    expect(dark.l).toBeLessThanOrEqual(0.68 + 0.03 + QUANTIZATION_SLACK);
  });

  it("kmeans 精修不改变纯色图的取色结果", () => {
    const plain = extractPalette(solidImage(255, 0, 0));
    const refined = extractPalette(solidImage(255, 0, 0), { refine: "kmeans" });
    const pureRed = accentOklch("#FF0000");

    expect(refined.accent).toBe(plain.accent);
    expect(refined.isFallback).toBe(false);
    expect(hueDistance(accentOklch(refined.accent).h, pureRed.h)).toBeLessThanOrEqual(0.05);
  });

  it("派生字段遵循约定格式", () => {
    const light = extractPalette(solidImage(30, 120, 200));
    const dark = extractPalette(solidImage(30, 120, 200), { mode: "dark" });

    expect(light.accentSoft).toMatch(/^rgba\(\d{1,3}, \d{1,3}, \d{1,3}, 0\.12\)$/);
    expect(dark.accentSoft).toMatch(/^rgba\(\d{1,3}, \d{1,3}, \d{1,3}, 0\.2\)$/);
    expect(light.deepFg).toBe("#F7F0EA");
    expect(light.gradTo).toBe("#FAF9F8");
    expect(dark.gradTo).toBe("#181412");
    expect(light.accent).toMatch(/^#[0-9A-F]{6}$/);
    expect(light.deep).toMatch(/^#[0-9A-F]{6}$/);
    expect(light.gradFrom).toMatch(/^#[0-9A-F]{6}$/);
  });
});
