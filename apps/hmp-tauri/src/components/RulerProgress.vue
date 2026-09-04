<script setup lang="ts">
import { computed, ref } from "vue";

/**
 * 刻度进度条（DESIGN.md §3.1 签名组件）
 * 静息 3px、悬停/拖拽 6px；每 10% 一根刻度，四分之一处加高；
 * 拖拽时 thumb 上方浮出时间气泡；纯百分比进出，时间换算由调用方完成。
 */
const props = withDefaults(
  defineProps<{
    /** 0..1 */
    progress: number;
    /** 用于时间显示；null 时隐藏时间相关 UI */
    durationMs?: number | null;
    disabled?: boolean;
    ariaLabel?: string;
  }>(),
  { durationMs: null, disabled: false, ariaLabel: "播放进度" },
);

const emit = defineEmits<{ seek: [percent: number] }>();

const dragging = ref(false);
const dragValue = ref(0);
const hovering = ref(false);

/** 展示值：拖拽时跟手，其余时候跟播放状态 */
const value = computed(() => (dragging.value ? dragValue.value : clamp(props.progress)));

const minorTicks = [10, 20, 30, 40, 60, 70, 80, 90];
const quarterTicks = [25, 50, 75];

function clamp(v: number) {
  return Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0;
}

function percentFromEvent(event: PointerEvent, target: HTMLElement) {
  const rect = target.getBoundingClientRect();
  if (rect.width <= 0) return 0;
  return clamp((event.clientX - rect.left) / rect.width);
}

function handlePointerDown(event: PointerEvent) {
  if (props.disabled) return;
  const target = event.currentTarget as HTMLElement;
  dragging.value = true;
  dragValue.value = percentFromEvent(event, target);
  target.setPointerCapture(event.pointerId);
}

function handlePointerMove(event: PointerEvent) {
  if (!dragging.value || props.disabled) return;
  dragValue.value = percentFromEvent(event, event.currentTarget as HTMLElement);
}

function handlePointerUp(event: PointerEvent) {
  if (!dragging.value) return;
  const target = event.currentTarget as HTMLElement;
  const percent = percentFromEvent(event, target);
  dragging.value = false;
  emit("seek", percent);
}

function handleKeydown(event: KeyboardEvent) {
  if (props.disabled) return;
  const step = event.key === "ArrowLeft" ? -0.02 : event.key === "ArrowRight" ? 0.02 : 0;
  if (step === 0) return;
  event.preventDefault();
  emit("seek", clamp(props.progress + step));
}

function formatTime(ms: number | null) {
  if (ms === null || !Number.isFinite(ms)) return "--:--";
  const totalSeconds = Math.max(0, Math.round(ms / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = String(totalSeconds % 60).padStart(2, "0");
  return `${minutes}:${seconds}`;
}

const elapsedText = computed(() =>
  formatTime(props.durationMs === null ? null : value.value * props.durationMs),
);
const remainingText = computed(() =>
  props.durationMs === null
    ? "--:--"
    : `-${formatTime(Math.max(0, (1 - value.value) * props.durationMs))}`,
);
const bubbleText = computed(() =>
  props.durationMs === null ? `${Math.round(value.value * 100)}%` : formatTime(value.value * props.durationMs),
);
</script>

<template>
  <div class="ruler-row">
    <span class="time time-elapsed" aria-hidden="true">{{ elapsedText }}</span>

    <div
      class="ruler"
      :class="{ 'is-active': hovering || dragging, 'is-dragging': dragging, 'is-disabled': disabled }"
      role="slider"
      tabindex="0"
      :aria-label="ariaLabel"
      :aria-valuemin="0"
      :aria-valuemax="100"
      :aria-valuenow="Math.round(value * 100)"
      :aria-disabled="disabled"
      @pointerdown="handlePointerDown"
      @pointermove="handlePointerMove"
      @pointerup="handlePointerUp"
      @pointercancel="dragging = false"
      @mouseenter="hovering = true"
      @mouseleave="hovering = false"
      @keydown="handleKeydown"
    >
      <span
        v-for="tick in minorTicks"
        :key="`m${tick}`"
        class="tick tick-minor"
        :style="{ left: `${tick}%` }"
        aria-hidden="true"
      ></span>
      <span
        v-for="tick in quarterTicks"
        :key="`q${tick}`"
        class="tick tick-quarter"
        :style="{ left: `${tick}%` }"
        aria-hidden="true"
      ></span>

      <div class="rail">
        <div class="rail-played" :style="{ transform: `scaleX(${value})` }"></div>
      </div>

      <div
        v-if="!disabled && durationMs !== null && dragging"
        class="thumb-bubble"
        :style="{ left: `${value * 100}%` }"
      >
        {{ bubbleText }}
      </div>
      <span
        v-if="!disabled"
        class="thumb"
        :style="{ left: `${value * 100}%` }"
        aria-hidden="true"
      ></span>
    </div>

    <span class="time time-remaining" aria-hidden="true">{{ remainingText }}</span>
  </div>
</template>

<style scoped>
.ruler-row {
  display: flex;
  align-items: center;
  gap: var(--space-4);
  width: 100%;
}

.time {
  flex: 0 0 auto;
  min-width: 3.25rem;
  font-size: 0.82rem;
  color: var(--muted-foreground);
  font-variant-numeric: tabular-nums;
}

.time-remaining {
  text-align: right;
}

/* 命中区比视觉条高，保证好拖 */
.ruler {
  position: relative;
  flex: 1;
  height: 1.75rem;
  cursor: pointer;
  border-radius: var(--radius-full);
}

.ruler:focus-visible {
  outline: 2px solid var(--ring);
  outline-offset: 2px;
}

.ruler.is-disabled {
  cursor: default;
}

/* 刻度：1px 细线，四分之一处加高 */
.tick {
  position: absolute;
  top: 50%;
  width: 1px;
  transform: translate(-50%, -50%);
  background: var(--foreground);
  opacity: 0.16;
  transition: height var(--duration-fast) var(--ease-standard);
}

.tick-minor {
  height: 0.5rem;
}

.tick-quarter {
  height: 0.75rem;
}

/* 轨道：静息 3px，悬停/拖拽增粗到 6px */
.rail {
  position: absolute;
  top: 50%;
  left: 0;
  right: 0;
  height: 3px;
  transform: translateY(-50%);
  background: var(--track);
  border-radius: var(--radius-full);
  overflow: hidden;
  transition: height var(--duration-normal) var(--ease-standard);
}

.ruler.is-active .rail {
  height: 6px;
}

.rail-played {
  width: 100%;
  height: 100%;
  background: var(--track-accent);
  border-radius: var(--radius-full);
  transform-origin: left;
  will-change: transform;
}

/* thumb：仅悬停/拖拽出现 */
.thumb {
  position: absolute;
  top: 50%;
  width: 0.75rem;
  height: 0.75rem;
  border-radius: 50%;
  background: var(--surface-3);
  border: 2px solid var(--track-accent);
  box-shadow: var(--shadow-sm);
  transform: translate(-50%, -50%);
  opacity: 0;
  pointer-events: none;
  transition: opacity var(--duration-fast) var(--ease-standard);
}

.ruler.is-active .thumb {
  opacity: 1;
}

.ruler.is-dragging .thumb {
  opacity: 1;
  transform: translate(-50%, -50%) scale(1.15);
}

.thumb-bubble {
  position: absolute;
  bottom: 100%;
  transform: translateX(-50%);
  padding: 0.1rem 0.45rem;
  background: var(--popover);
  color: var(--popover-foreground);
  border-radius: var(--radius-sm);
  box-shadow: var(--shadow-md);
  font-size: 0.75rem;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  pointer-events: none;
  z-index: var(--z-dropdown);
}
</style>
