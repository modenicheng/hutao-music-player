// 二分切分量化（median cut 的"更快"变体）。
// 策略：每次全局挑"跨度最大"的桶，沿其跨度最大的 L/a/b 轴按中位数对分。
// 沿最宽轴切的原因：每次都消掉当前最大的方差方向，叶体积收缩最快，
// 在同等叶数下方差最小；中位对分保证两侧像素数均衡，
// 大面积背景不会被切成一堆碎片，小而鲜艳的区域也能保住自己的叶。
import type { Oklab } from "./oklab";

export interface QuantizedLeaf {
  // 叶内像素均值（OKLab）
  l: number;
  a: number;
  b: number;
  // 叶内像素数，评分时作为 coverage 的权重
  count: number;
}

// 叶内像素低于该值就不再对分：继续切只会放大噪声
const MIN_SPLIT_PIXELS = 4;

type Axis = 0 | 1 | 2; // 0=L, 1=a, 2=b

export const binarySplit = (
  pixels: readonly Oklab[],
  maxLeaves = 10,
): QuantizedLeaf[] => {
  if (pixels.length === 0) return [];
  const limit = Math.max(1, maxLeaves);
  // 不改动调用方数组，所有排序都发生在内部拷贝上
  const buckets: Oklab[][] = [Array.from(pixels)];

  while (buckets.length < limit) {
    let target = -1;
    let widest = 0;
    let axis: Axis = 0;
    for (let i = 0; i < buckets.length; i += 1) {
      const bucket = buckets[i];
      if (bucket.length < MIN_SPLIT_PIXELS) continue;
      const span = widestAxisSpan(bucket);
      if (span.span > widest) {
        widest = span.span;
        axis = span.axis;
        target = i;
      }
    }
    // 所有桶都退化成单点（纯色图、极小图）时提前收工
    if (target < 0 || widest <= 0) break;
    const bucket = buckets[target];
    bucket.sort(compareByAxis(axis));
    const median = bucket.length >> 1;
    buckets.splice(target, 1, bucket.slice(0, median), bucket.slice(median));
  }

  return buckets.map(leafMean);
};

const widestAxisSpan = (bucket: readonly Oklab[]): { axis: Axis; span: number } => {
  let minL = Infinity;
  let maxL = -Infinity;
  let minA = Infinity;
  let maxA = -Infinity;
  let minB = Infinity;
  let maxB = -Infinity;
  for (const pixel of bucket) {
    if (pixel.l < minL) minL = pixel.l;
    if (pixel.l > maxL) maxL = pixel.l;
    if (pixel.a < minA) minA = pixel.a;
    if (pixel.a > maxA) maxA = pixel.a;
    if (pixel.b < minB) minB = pixel.b;
    if (pixel.b > maxB) maxB = pixel.b;
  }
  const spanL = maxL - minL;
  const spanA = maxA - minA;
  const spanB = maxB - minB;
  if (spanB >= spanA && spanB >= spanL) return { axis: 2, span: spanB };
  if (spanA >= spanL) return { axis: 1, span: spanA };
  return { axis: 0, span: spanL };
};

const compareByAxis = (axis: Axis) => {
  if (axis === 0) return (p: Oklab, q: Oklab) => p.l - q.l;
  if (axis === 1) return (p: Oklab, q: Oklab) => p.a - q.a;
  return (p: Oklab, q: Oklab) => p.b - q.b;
};

const leafMean = (bucket: readonly Oklab[]): QuantizedLeaf => {
  let l = 0;
  let a = 0;
  let b = 0;
  for (const pixel of bucket) {
    l += pixel.l;
    a += pixel.a;
    b += pixel.b;
  }
  const count = bucket.length;
  return { l: l / count, a: a / count, b: b / count, count };
};
