// 可选精修：加权 k-means。默认关闭，`refine: "kmeans"` 时启用。
// 只对二分产出的叶（而非全部像素）做加权迭代：每叶权重 = 像素数 count，
// 计算量从 O(像素数 × 质心数) 降到 O(叶数 × 质心数)，
// 而二分质心本身已经落在密度脊线附近，两轮足以让质心吸附到密度峰。
import type { QuantizedLeaf } from "./binary-split";

export const DEFAULT_REFINE_ITERATIONS = 2;

export const refine = (
  centroids: readonly QuantizedLeaf[],
  points: readonly QuantizedLeaf[],
  iterations: number = DEFAULT_REFINE_ITERATIONS,
): QuantizedLeaf[] => {
  if (centroids.length === 0) return [];
  let centers: QuantizedLeaf[] = centroids.map((centroid) => ({ ...centroid }));

  for (let round = 0; round < Math.max(0, iterations); round += 1) {
    const sums = centers.map(() => ({ l: 0, a: 0, b: 0, count: 0 }));
    for (const point of points) {
      let nearest = 0;
      let nearestDistance = Infinity;
      for (let i = 0; i < centers.length; i += 1) {
        const distance = squaredDistance(point, centers[i]);
        if (distance < nearestDistance) {
          nearestDistance = distance;
          nearest = i;
        }
      }
      const sum = sums[nearest];
      const weight = point.count;
      sum.l += point.l * weight;
      sum.a += point.a * weight;
      sum.b += point.b * weight;
      sum.count += weight;
    }
    centers = sums.map((sum, i) =>
      sum.count > 0
        ? { l: sum.l / sum.count, a: sum.a / sum.count, b: sum.b / sum.count, count: sum.count }
        : // 空簇保留原质心：调色板叶数不缩水，评分阶段自然会淘汰它
          centers[i],
    );
  }

  return centers;
};

const squaredDistance = (a: QuantizedLeaf, b: QuantizedLeaf): number => {
  const dl = a.l - b.l;
  const da = a.a - b.a;
  const db = a.b - b.b;
  return dl * dl + da * da + db * db;
};
