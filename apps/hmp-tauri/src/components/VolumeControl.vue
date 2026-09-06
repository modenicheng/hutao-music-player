<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import AppIcon from "./AppIcon.vue";
import type { PlayerController } from "../lib/player";
import volumeIcon from "../assets/icons/volume-up-rounded.svg?raw";
import volumeOffIcon from "../assets/icons/volume-off-rounded.svg?raw";

/**
 * 音量控件（QQ/网易云式）：静息只显示图标，悬浮/点击向上展开竖向滑条。
 * 手感对齐手写的播放条控制：pointerdown 捕获 → move 跟手 → up 提交；
 * 展开动画与数值平滑都由 requestAnimationFrame 驱动，不依赖 CSS 过渡的节拍。
 */
const props = defineProps<{ player: PlayerController }>();

const rootRef = ref<HTMLElement | null>(null);
const railRef = ref<HTMLElement | null>(null);

// —— 展开状态：hover 暂开（离开有宽限），点击图标钉住，点外部/Esc 收起 ——
const hoverOpen = ref(false);
const pinned = ref(false);
const visible = computed(() => hoverOpen.value || pinned.value);

let closeTimer = 0;
function handleEnter() {
  window.clearTimeout(closeTimer);
  hoverOpen.value = true;
}
function handleLeave() {
  window.clearTimeout(closeTimer);
  closeTimer = window.setTimeout(() => (hoverOpen.value = false), 180);
}
function handleDocPointerDown(event: PointerEvent) {
  if (!rootRef.value?.contains(event.target as Node)) pinned.value = false;
}

// —— 展开动画：rAF 推进 0..1，高度/透明度/位移全部由它派生 ——
const POPOP_FULL_PX = 160; // 弹层满高（px）：pad 8.8 + 数值 11.5 + gap 4.8 + 轨 124 + pad 10.4
const EXPAND_MS = 200;
const expand = ref(0);
let expandRaf = 0;
let expandFrom = 0;
let expandStart = 0;
let expandTarget = 0;

const easeOutCubic = (t: number) => 1 - Math.pow(1 - t, 3);

watch(visible, (now) => {
  expandFrom = expand.value;
  expandTarget = now ? 1 : 0;
  expandStart = performance.now();
  cancelAnimationFrame(expandRaf);
  const step = (timestamp: number) => {
    const t = Math.min(1, (timestamp - expandStart) / EXPAND_MS);
    expand.value = expandFrom + (expandTarget - expandFrom) * easeOutCubic(t);
    if (t < 1) expandRaf = requestAnimationFrame(step);
  };
  expandRaf = requestAnimationFrame(step);
});

// —— 数值平滑：桥每 250ms 推一次音量，显示值用 rAF 收敛过去，扫条不跳变 ——
const displayed = ref(props.player.state.volume);
let valueRaf = 0;
let valueTarget = displayed.value;

watch(
  () => props.player.state.volume,
  (volume) => {
    valueTarget = volume;
    cancelAnimationFrame(valueRaf);
    const step = () => {
      const diff = valueTarget - displayed.value;
      if (Math.abs(diff) < 0.002) {
        displayed.value = valueTarget;
        return;
      }
      displayed.value += diff * 0.3;
      valueRaf = requestAnimationFrame(step);
    };
    valueRaf = requestAnimationFrame(step);
  },
);

// —— 拖拽（竖向）：与播放条拖拽同一套路 ——
// rail 只有 4px 宽，pointerdown 后把 move/up 挂到 window 上：
// 光标滑出细轨也持续跟手，且不依赖 setPointerCapture（部分环境对注入指针会抛 NotFoundError）。
const dragging = ref(false);
const dragValue = ref(0);

function clamp(value: number) {
  return Number.isFinite(value) ? Math.min(1, Math.max(0, value)) : 0;
}

function volumeFromEvent(event: PointerEvent) {
  const rect = railRef.value?.getBoundingClientRect();
  if (!rect || rect.height <= 0) return 0;
  return clamp(1 - (event.clientY - rect.top) / rect.height);
}

function applyVolume(value: number) {
  props.player.setVolume(value);
}

function handleRailPointerDown(event: PointerEvent) {
  dragging.value = true;
  dragValue.value = volumeFromEvent(event);
  applyVolume(dragValue.value);
  window.addEventListener("pointermove", handleWindowPointerMove);
  window.addEventListener("pointerup", handleWindowPointerUp);
  window.addEventListener("pointercancel", handleWindowPointerUp);
}

function handleWindowPointerMove(event: PointerEvent) {
  if (!dragging.value) return;
  dragValue.value = volumeFromEvent(event);
  applyVolume(dragValue.value);
}

function handleWindowPointerUp(event: PointerEvent) {
  if (!dragging.value) return;
  dragValue.value = volumeFromEvent(event);
  dragging.value = false;
  window.removeEventListener("pointermove", handleWindowPointerMove);
  window.removeEventListener("pointerup", handleWindowPointerUp);
  window.removeEventListener("pointercancel", handleWindowPointerUp);
}

function handleRailKeydown(event: KeyboardEvent) {
  const step = event.key === "ArrowUp" ? 0.05 : event.key === "ArrowDown" ? -0.05 : 0;
  if (step !== 0) {
    event.preventDefault();
    applyVolume(clamp(props.player.state.volume + step));
    return;
  }
  if (event.key === "Escape") pinned.value = false;
}

function handleWheel(event: WheelEvent) {
  event.preventDefault();
  applyVolume(clamp(props.player.state.volume + (event.deltaY < 0 ? 0.05 : -0.05)));
}

// —— 图标：点击 = 静音切换，同时钉住弹层便于继续细调 ——
const lastAudible = ref(props.player.state.volume > 0 ? props.player.state.volume : 0.8);
const muted = computed(() => props.player.state.volume <= 0);

function handleTriggerClick() {
  if (muted.value) {
    props.player.setVolume(lastAudible.value || 0.8);
  } else {
    lastAudible.value = props.player.state.volume;
    props.player.setVolume(0);
  }
  pinned.value = true;
}

const fill = computed(() => (dragging.value ? dragValue.value : displayed.value));
const percentText = computed(() => `${Math.round(fill.value * 100)}%`);
const popStyle = computed<{ height: string; opacity: string; transform: string; pointerEvents: "auto" | "none" }>(() => ({
  height: `${expand.value * POPOP_FULL_PX}px`,
  opacity: String(expand.value),
  transform: `translateY(${(1 - expand.value) * 8}px)`,
  pointerEvents: expand.value > 0.5 ? "auto" : "none",
}));

onBeforeUnmount(() => {
  window.clearTimeout(closeTimer);
  cancelAnimationFrame(expandRaf);
  cancelAnimationFrame(valueRaf);
  document.removeEventListener("pointerdown", handleDocPointerDown, true);
  window.removeEventListener("pointermove", handleWindowPointerMove);
  window.removeEventListener("pointerup", handleWindowPointerUp);
  window.removeEventListener("pointercancel", handleWindowPointerUp);
});

watch(pinned, (pinnedNow) => {
  if (pinnedNow) document.addEventListener("pointerdown", handleDocPointerDown, true);
  else document.removeEventListener("pointerdown", handleDocPointerDown, true);
});
</script>

<template>
  <div
    ref="rootRef"
    class="volume"
    @mouseenter="handleEnter"
    @mouseleave="handleLeave"
    @focusin="handleEnter"
    @focusout="handleLeave"
  >
    <button
      class="volume-trigger"
      :title="muted ? '取消静音' : '静音'"
      :aria-label="muted ? '取消静音' : '静音'"
      @click="handleTriggerClick"
    >
      <AppIcon class="volume-icon" :src="muted ? volumeOffIcon : volumeIcon" />
    </button>

    <div class="volume-pop" :style="popStyle" :aria-hidden="expand < 0.5">
      <span class="volume-value" aria-hidden="true">{{ percentText }}</span>
      <div
        ref="railRef"
        class="volume-rail"
        :class="{ 'is-dragging': dragging }"
        role="slider"
        tabindex="0"
        aria-label="音量"
        :aria-valuemin="0"
        :aria-valuemax="100"
        :aria-valuenow="Math.round(fill * 100)"
        @pointerdown="handleRailPointerDown"
        @wheel="handleWheel"
        @keydown="handleRailKeydown"
      >
        <div class="volume-track">
          <div class="volume-fill" :style="{ height: `${fill * 100}%` }"></div>
        </div>
        <span class="volume-thumb" :style="{ bottom: `${fill * 100}%` }" aria-hidden="true"></span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.volume {
  position: relative;
  display: grid;
  place-items: center;
}

.volume-trigger {
  display: grid;
  place-items: center;
  width: 2.25rem;
  height: 2.25rem;
  color: var(--muted-foreground);
  border-radius: var(--radius-full);
  transition:
    color var(--duration-fast) var(--ease-standard),
    background-color var(--duration-fast) var(--ease-standard);
}

.volume-trigger:hover,
.volume-trigger:focus-visible {
  color: var(--foreground);
  background: var(--track-accent-soft);
}

.volume-icon {
  position: relative;
  width: 1.2rem;
  height: 1.2rem;
}

/* —— 竖向弹层：细长窄条，只包住数值 + 细轨 —— */
.volume-pop {
  position: absolute;
  bottom: calc(100% + 0.6rem);
  left: 50%;
  margin-left: -1.375rem;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.3rem;
  width: 2.75rem;
  padding: 0.55rem 0.2rem 0.65rem;
  background: var(--popover);
  color: var(--popover-foreground);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-md);
  overflow: hidden;
  will-change: height, opacity, transform;
  z-index: var(--z-dropdown);
}

.volume-value {
  font-size: 0.72rem;
  line-height: 1;
  color: var(--muted-foreground);
  font-variant-numeric: tabular-nums;
}

/* 命中区比视觉轨道宽，保证好拖 */
.volume-rail {
  position: relative;
  display: grid;
  place-items: center;
  width: 100%;
  height: 7.75rem;
  cursor: pointer;
  border-radius: var(--radius-full);
}

.volume-rail:focus-visible {
  outline: 2px solid var(--ring);
  outline-offset: 2px;
}

.volume-track {
  position: relative;
  width: 4px;
  height: 100%;
  background: var(--track);
  border-radius: var(--radius-full);
  overflow: hidden;
}

.volume-fill {
  position: absolute;
  bottom: 0;
  left: 0;
  width: 100%;
  background: var(--track-accent);
  border-radius: var(--radius-full);
  will-change: height;
}

.volume-thumb {
  position: absolute;
  left: 50%;
  width: 0.7rem;
  height: 0.7rem;
  border-radius: 50%;
  background: var(--surface-3);
  border: 2px solid var(--track-accent);
  box-shadow: var(--shadow-sm);
  transform: translate(-50%, 50%);
  opacity: 0;
  pointer-events: none;
  transition: opacity var(--duration-fast) var(--ease-standard);
}

.volume-rail:hover .volume-thumb,
.volume-rail.is-dragging .volume-thumb {
  opacity: 1;
}
</style>
