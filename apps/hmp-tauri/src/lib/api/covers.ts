// 程序化确定性封面 / 头像生成器（DESIGN.md §3.2：封面一律用程序化 SVG data-URL）。
//
// 为什么必须确定性：
// 1. 同一实体（同一专辑 / 歌手 / 榜单）在页面各处要拿到同一张图，视觉才一致；
// 2. 取色模块（src/lib/color/）的单测与截图回归需要稳定输入；
// 3. 不依赖网络，离线演示与测试都可运行。
//
// 因此一切"随机量"只允许从 seed 经纯函数哈希派生，禁止 Math.random / Date.now。

const FNV_OFFSET_BASIS = 0x811c9dc5;
const FNV_PRIME = 0x01000193;

/**
 * FNV-1a 32 位哈希。
 * 选它是因为实现短、跨平台逐字节稳定、雪崩效果对短字符串足够——
 * 同一个 seed 永远得到同一个数，这是整个 mock 层确定性的地基。
 * 该哈希也被 mock-data / client 复用（榜单热度、模拟延迟等），保证单一实现。
 */
export function hashSeed(seed: string): number {
  let hash = FNV_OFFSET_BASIS;
  for (let i = 0; i < seed.length; i += 1) {
    hash ^= seed.charCodeAt(i);
    hash = Math.imul(hash, FNV_PRIME);
  }
  return hash >>> 0;
}

/** 在基础哈希上叠加盐值派生子随机数，让色相 / 构图 / 饱和度等维度互不绑定 */
function derive(base: number, salt: string): number {
  return hashSeed(`${salt}:${base.toString(36)}`);
}

const hsl = (h: number, s: number, l: number): string =>
  `hsl(${h}, ${s}%, ${l}%)`;
const hsla = (h: number, s: number, l: number, a: number): string =>
  `hsla(${h}, ${s}%, ${l}%, ${a})`;

interface CoverPalette {
  hue: number;
  hueAnalog: number;
  hueAccent: number;
  s1: number;
  s2: number;
  l1: number;
  l2: number;
}

/**
 * 色板规则：色相覆盖整个色环（由 seed 决定），但同一张封面内部只用
 * "主色 + 邻近色 + 低透明度对侧 accent"，保证封面彼此不同却风格统一。
 * 饱和度 / 亮度压在舒服区间（S 45–75%、L 35–65%），避免刺眼或发灰。
 */
function paletteFor(seed: string): CoverPalette {
  const base = hashSeed(`cover:${seed}`);
  const hue = base % 360;
  return {
    hue,
    hueAnalog: (hue + 14 + (derive(base, "analog") % 44)) % 360,
    hueAccent: (hue + 150 + (derive(base, "accent") % 60)) % 360,
    s1: 45 + (derive(base, "s1") % 31),
    s2: 45 + (derive(base, "s2") % 31),
    l1: 35 + (derive(base, "l1") % 31),
    l2: 35 + (derive(base, "l2") % 31),
  };
}

type Layout = (palette: CoverPalette) => string;

// 4 种构图模板按 hash 轮换；每模板 3 个半透明几何形，风格一致但彼此可辨。
const LAYOUTS: Layout[] = [
  // 轨道：大行星 + 细轨道环 + 卫星点
  (p) =>
    `<circle cx="432" cy="176" r="196" fill="${hsla(p.hueAccent, p.s1, p.l1, 0.2)}"/>` +
    `<circle cx="150" cy="452" r="118" fill="none" stroke="${hsla(p.hueAnalog, p.s2, p.l2, 0.55)}" stroke-width="3"/>` +
    `<circle cx="150" cy="452" r="26" fill="${hsla(p.hueAnalog, p.s2, 74, 0.5)}"/>`,
  // 山脊：两座错落三角 + 低悬的"太阳"
  (p) =>
    `<path d="M0 600 L230 210 L460 600 Z" fill="${hsla(p.hueAnalog, p.s2, p.l2, 0.28)}"/>` +
    `<path d="M210 600 L420 300 L620 600 Z" fill="${hsla(p.hueAccent, p.s1, p.l1, 0.22)}"/>` +
    `<circle cx="438" cy="150" r="64" fill="${hsla(p.hueAccent, p.s2, 72, 0.42)}"/>`,
  // 声波：自下而上的三道同心弧
  (p) =>
    `<path d="M0 760 A300 300 0 0 1 600 760" fill="none" stroke="${hsla(p.hueAnalog, p.s2, p.l2, 0.2)}" stroke-width="44"/>` +
    `<path d="M-120 760 A420 420 0 0 1 720 760" fill="none" stroke="${hsla(p.hueAccent, p.s1, p.l1, 0.14)}" stroke-width="30"/>` +
    `<path d="M-240 760 A540 540 0 0 1 840 760" fill="none" stroke="${hsla(p.hue, p.s1, 70, 0.1)}" stroke-width="20"/>`,
  // 斜切：左上大圆 + 右下旋转菱形 + 一道对角细线
  (p) =>
    `<circle cx="120" cy="96" r="210" fill="${hsla(p.hueAnalog, p.s2, p.l2, 0.3)}"/>` +
    `<rect x="380" y="330" width="260" height="260" transform="rotate(45 510 460)" fill="${hsla(p.hueAccent, p.s1, p.l1, 0.24)}"/>` +
    `<path d="M60 540 L540 60" stroke="${hsla(p.hue, p.s1, 82, 0.5)}" stroke-width="3"/>`,
];

// 渐变轴向四选一，避免所有封面都是同一个走向
const GRADIENT_AXES = [
  { x1: 0, y1: 0, x2: 1, y2: 1 },
  { x1: 1, y1: 0, x2: 0, y2: 1 },
  { x1: 0, y1: 0, x2: 0, y2: 1 },
  { x1: 0, y1: 1, x2: 1, y2: 0 },
];

/**
 * 生成确定性封面：seed → 哈希 → HSL 色板 + 构图模板 → SVG data-URL。
 * 双层底（线性渐变 + 中心柔光）保证给取色模块的像素有层次，
 * 几何形提供次级聚类色，方便测试多主色场景。
 */
export function coverUrl(seed: string, size = 600): string {
  const palette = paletteFor(seed);
  const base = hashSeed(`cover:${seed}`);
  const axis = GRADIENT_AXES[derive(base, "axis") % GRADIENT_AXES.length];
  const layout = LAYOUTS[derive(base, "layout") % LAYOUTS.length];
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 600 600">` +
    `<defs>` +
    `<linearGradient id="bg" x1="${axis.x1}" y1="${axis.y1}" x2="${axis.x2}" y2="${axis.y2}">` +
    `<stop offset="0" stop-color="${hsl(palette.hue, palette.s1, palette.l1)}"/>` +
    `<stop offset="1" stop-color="${hsl(palette.hueAnalog, palette.s2, palette.l2)}"/>` +
    `</linearGradient>` +
    `<radialGradient id="glow" cx="0.5" cy="0.36" r="0.75">` +
    `<stop offset="0" stop-color="${hsl(palette.hueAccent, palette.s1, Math.min(70, palette.l1 + 12))}" stop-opacity="0.35"/>` +
    `<stop offset="1" stop-color="${hsl(palette.hueAccent, palette.s1, palette.l1)}" stop-opacity="0"/>` +
    `</radialGradient>` +
    `</defs>` +
    `<rect width="600" height="600" fill="url(#bg)"/>` +
    `<rect width="600" height="600" fill="url(#glow)"/>` +
    layout(palette) +
    `</svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

/**
 * 生成确定性头像：同色相的纯渐变圆（不做文字/首字母，展示交给外层组件）。
 * 亮端到暗端拉开明度差，圆上再无多余元素，缩小到 32px 也不糊。
 */
export function avatarUrl(seed: string, size = 96): string {
  const base = hashSeed(`avatar:${seed}`);
  const hue = base % 360;
  const s1 = 45 + (derive(base, "s1") % 31);
  const s2 = 40 + (derive(base, "s2") % 26);
  const lLight = 62 + (derive(base, "light") % 18); // 62–79
  const lDark = 34 + (derive(base, "dark") % 16); // 34–49
  const axis = GRADIENT_AXES[derive(base, "axis") % 2];
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 96 96">` +
    `<defs>` +
    `<linearGradient id="g" x1="${axis.x1}" y1="${axis.y1}" x2="${axis.x2}" y2="${axis.y2}">` +
    `<stop offset="0" stop-color="${hsl(hue, s1, lLight)}"/>` +
    `<stop offset="1" stop-color="${hsl(hue, s2, lDark)}"/>` +
    `</linearGradient>` +
    `</defs>` +
    `<circle cx="48" cy="48" r="48" fill="url(#g)"/>` +
    `</svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}
