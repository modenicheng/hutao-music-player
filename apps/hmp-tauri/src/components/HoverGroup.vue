<script lang="ts" setup>
import { onBeforeUnmount, onMounted, ref } from "vue";

/** 高亮块外观：背景色 + 显示时的不透明度 + 形变强度 */
const props = withDefaults(
  defineProps<{
    /** 高亮块背景色，可传任意 CSS 颜色/变量 */
    highlightColor?: string;
    /** hover 显示时的不透明度（0~1） */
    highlightOpacity?: number;
    /** 黏滞形变强度 0~1：越大移动时被拉得越长、压得越扁 */
    viscosity?: number;
  }>(),
  {
    highlightColor: "var(--neutral-500)",
    highlightOpacity: 0.6,
    viscosity: 1,
  },
);

/**
 * 统一 hover 动画：容器内只有一个高亮块 (hover-block)，
 * 通过事件委托把块移动到当前被 hover 的 .hover-item 之上，
 * 并复制其几何 + 圆角，实现平滑滑动/形变。
 *
 * - 任意位置：块基于 getBoundingClientRect 读取目标真实边界框，
 *   因此同一容器内任意布局（列、行、网格、混合尺寸）都能平滑滑动。
 * - 圆角：直接复制目标元素的 border-radius（含四角/百分比/var()）。
 * - 黏滞形变：位置用个体 `translate` 属性 + 过渡；移动时对个体 `scale`
 *   施加一次瞬态动画：沿运动方向拉伸更大、垂直方向压缩更小；并把 `scale`
 *   原点设在运动“前缘”，让尾端被拖在后面、延迟回弹。
 *   —— `scale` 只在飞行中非 1，到位后回到 `1 1`，因此静止时圆角仍精确贴合。
 * - 性能：单块 + 事件委托（无逐 item 监听），无 mousemove 逐帧计算；
 *   位置/形变均为合成层，尺寸只在尺寸变化时触发一次小重排。
 * - 附加层（indicator 具名插槽）：可选的悬浮指示（如曲目表序号位的播放图标）。
 *   块的几何经容器上的 `--hover-x/y/w/h` 变量下发，指示层以同曲线 translate
 *   跟随滑动——只位移、不继承黏滞形变；显隐由 `.is-hovering` 驱动，
 *   首次定位与块一致：几何直接跳到位、只做淡入。
 */
const containerRef = ref<HTMLElement>();
const blockRef = ref<HTMLElement>();
/** indicator 插槽层（可选）：随块滑动的悬浮指示 */
const indicatorRef = ref<HTMLElement>();
/** 块当前是否停在某 item 上（驱动 indicator 显隐） */
const hovering = ref(false);

/** 当前被包裹的目标元素 */
let current: HTMLElement | null = null;
/** 块是否已首次定位（首次显示时不做滑动，直接原位淡入） */
let placed = false;
let rafId = 0;
/** 上一次放置的坐标，用于计算移动增量以决定拉伸方向/强度 */
let lastX = 0;
let lastY = 0;
/** 当前进行中的形变动画，避免叠加 */
let stretchAnim: Animation | null = null;

/** 把块精确放到 item 的几何上（相对容器的 padding-box 坐标） */
function placeBlock(item: HTMLElement, first = false) {
  const block = blockRef.value;
  const container = containerRef.value;
  if (!block || !container) return;

  const c = container.getBoundingClientRect();
  const r = item.getBoundingClientRect();

  // clientLeft/clientTop 抵消容器自身边框，得到 padding-box 原点。
  const x = r.left - c.left - container.clientLeft;
  const y = r.top - c.top - container.clientTop;

  const indicator = indicatorRef.value;
  if (first) {
    // 首次出现：先关掉过渡把几何“跳”到位，随后只做淡入，避免从角落飞入。
    block.style.transition = "none";
    if (indicator) indicator.style.transition = "none";
  }

  // indicator 与块消费同一份几何；变量改动经各层自己的 translate 过渡同帧滑动。
  if (indicator) {
    container.style.setProperty("--hover-x", `${x}px`);
    container.style.setProperty("--hover-y", `${y}px`);
    container.style.setProperty("--hover-w", `${r.width}px`);
    container.style.setProperty("--hover-h", `${r.height}px`);
  }

  if (first) {
    block.style.width = `${r.width}px`;
    block.style.height = `${r.height}px`;
    block.style.translate = `${x}px ${y}px`;
    block.style.borderRadius = getComputedStyle(item).borderRadius;
    void container.offsetWidth; // 强制重排，让上面的几何（含 indicator 变量）立即生效
    block.style.transition = "";
    if (indicator) indicator.style.transition = "";
  }

  block.style.width = `${r.width}px`;
  block.style.height = `${r.height}px`;
  block.style.translate = `${x}px ${y}px`;
  block.style.borderRadius = getComputedStyle(item).borderRadius;
  block.style.opacity = String(props.highlightOpacity);

  lastX = x;
  lastY = y;
}

/**
 * 黏滞形变：沿运动主轴拉伸、垂直轴压缩，然后回弹。
 * 用 Web Animations API 对个体 `scale` 施加瞬态动画，结束后回到 `1 1`。
 */
function triggerStretch(dx: number, dy: number) {
  const block = blockRef.value;
  if (!block) return;
  stretchAnim?.cancel();

  const horizontal = Math.abs(dx) >= Math.abs(dy);
  const dist = Math.hypot(dx, dy);
  const s = props.viscosity;

  // 仅沿运动方向（平行轴）拉伸；垂直运动方向（垂直分量）形变量为 0。
  const parallel = 1 + Math.min(dist / 110, 0.6) * s;
  const sx = horizontal ? parallel : 1;
  const sy = horizontal ? 1 : parallel;

  // 原点设在运动“前缘”：前缘贴住目标、尾端轻度拖拽，主体仍跟手。
  const originX = dx > 0 ? "100%" : dx < 0 ? "0%" : "50%";
  const originY = dy > 0 ? "100%" : dy < 0 ? "0%" : "50%";
  block.style.transformOrigin = `${originX} ${originY}`;

  stretchAnim = block.animate(
    [
      // 快速拉伸到峰 → 平滑追回，整体干脆不粘滞
      { offset: 0, scale: "1 1", easing: "cubic-bezier(0.22, 1, 0.36, 1)" },
      { offset: 0.3, scale: `${sx} ${sy}`, easing: "cubic-bezier(0.22, 1, 0.36, 1)" },
      { offset: 1, scale: "1 1" },
    ],
    { duration: 240, iterations: 1 },
  );
  stretchAnim.onfinish = () => (stretchAnim = null);
}

function showBlock(item: HTMLElement) {
  const prevX = lastX;
  const prevY = lastY;
  const wasPlaced = placed;

  current = item;
  placeBlock(item, !placed);
  placed = true;
  hovering.value = true;

  // 首次显示只做原位淡入；仅在 item 之间移动时触发形变。
  if (wasPlaced) {
    triggerStretch(lastX - prevX, lastY - prevY);
  }
}

function hideBlock() {
  current = null;
  hovering.value = false;
  stretchAnim?.cancel();
  stretchAnim = null;
  if (blockRef.value) blockRef.value.style.opacity = "0";
}

function onMouseOver(e: MouseEvent) {
  const item = (e.target as HTMLElement).closest<HTMLElement>(".hover-item");
  if (!item || item === current) return;
  showBlock(item);
}

function onMouseLeave() {
  hideBlock();
}

/** 容器滚动 / 窗口尺寸变化时，若正在 hover，重新锚定块位置 */
function reAnchor() {
  if (!current) return;
  cancelAnimationFrame(rafId);
  rafId = requestAnimationFrame(() => placeBlock(current!));
}

onMounted(() => {
  const container = containerRef.value;
  if (!container) return;
  // mouseover/mouseleave 委托：支持动态增删 item，无需逐项绑定。
  container.addEventListener("mouseover", onMouseOver);
  container.addEventListener("mouseleave", onMouseLeave);
  // 捕获滚动 & resize，保证列表滚动时块跟随。
  window.addEventListener("resize", reAnchor);
  window.addEventListener("scroll", reAnchor, true);
});

onBeforeUnmount(() => {
  const container = containerRef.value;
  container?.removeEventListener("mouseover", onMouseOver);
  container?.removeEventListener("mouseleave", onMouseLeave);
  window.removeEventListener("resize", reAnchor);
  window.removeEventListener("scroll", reAnchor, true);
  cancelAnimationFrame(rafId);
  stretchAnim?.cancel();
  stretchAnim = null;
});
</script>

<template>
  <div class="hover-group" ref="containerRef" :class="{ 'is-hovering': hovering }">
    <div
      class="hover-block"
      ref="blockRef"
      aria-hidden="true"
      :style="{ '--hover-bg': props.highlightColor }"
    ></div>
    <slot />
    <div v-if="$slots.indicator" ref="indicatorRef" class="hover-indicator" aria-hidden="true">
      <slot name="indicator" />
    </div>
  </div>
</template>

<style lang="css" scoped>
.hover-group {
  /* 成为绝对定位块的包含块，并创建独立层叠上下文 */
  position: relative;
  isolation: isolate;
  /* 裁掉形变时尾端被拖出的部分，保持列表边缘整洁 */
  overflow: hidden;
}

.hover-block {
  position: absolute;
  top: 0;
  left: 0;
  z-index: 0;
  pointer-events: none;
  background-color: var(--hover-bg, var(--neutral-500));
  opacity: 0;
  /* 圆角基线：让每次形变都能从 0 平滑过渡到目标圆角 */
  border-radius: 0;
  /* 个体变换属性：translate 管位置，scale 管形变（WAAPI 动画驱动，静止恒为 1 1） */
  translate: 0 0;
  scale: 1 1;
  will-change: translate, scale, opacity;
  transition:
    /* 显隐淡入淡出给足时长，避免出现/离开时接近硬切 */
    opacity var(--duration-normal) var(--ease-standard),
    /* 位置短促快出，紧跟指针；形变由 scale 动画负责黏滞 */
    translate 140ms cubic-bezier(0.22, 1, 0.36, 1),
    width 180ms cubic-bezier(0.22, 1, 0.36, 1),
    height 180ms cubic-bezier(0.22, 1, 0.36, 1),
    border-radius 180ms cubic-bezier(0.22, 1, 0.36, 1);
}

/* indicator：随块滑动的悬浮指示层，几何由 --hover-* 变量下发；
   只位移不参与黏滞形变，横向定位（left/width）交给消费方 */
.hover-indicator {
  position: absolute;
  top: 0;
  left: 0;
  z-index: 2;
  pointer-events: none;
  height: var(--hover-h, 0);
  translate: 0 var(--hover-y, 0);
  opacity: 0;
  will-change: translate, opacity;
  transition:
    opacity var(--duration-normal) var(--ease-standard),
    translate 140ms cubic-bezier(0.22, 1, 0.36, 1),
    height 180ms cubic-bezier(0.22, 1, 0.36, 1);
}

.hover-group.is-hovering .hover-indicator {
  opacity: 1;
}
</style>
