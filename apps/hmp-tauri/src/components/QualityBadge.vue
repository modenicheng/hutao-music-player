<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import AppIcon from "./AppIcon.vue";
import {
  QUALITY_TIERS,
  TIER_RANK,
  effectiveTierId,
  qualityState,
  selectQuality,
  tierById,
  trackMaxTierId,
  type QualityTier,
} from "../lib/qualityStore";

// 资产库没有 check 图标，这里内联一枚 12px 勾
const checkIcon =
  '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4.5 12.6l5 5L19.5 7"/></svg>';

/**
 * 音质徽章（QQ 音乐式）：显示当前实际生效档位，点击弹出档位选择。
 * 生效档位 = min(用户偏好, 曲目最高档)；超出曲目提供的档位在弹层里禁用。
 */
const props = defineProps<{
  /** 当前曲目的音质文案（SongRef.quality），决定可用上限 */
  trackQuality?: string;
  /** 弹层向上展开（播放条/控制台都贴底） */
  align?: "up";
}>();

const rootRef = ref<HTMLElement | null>(null);
const open = ref(false);

function toggleOpen() {
  open.value = !open.value;
}
function handleDocPointerDown(event: PointerEvent) {
  if (!rootRef.value?.contains(event.target as Node)) open.value = false;
}
function handleKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") open.value = false;
}
watch(open, (now) => {
  if (now) document.addEventListener("pointerdown", handleDocPointerDown, true);
  else document.removeEventListener("pointerdown", handleDocPointerDown, true);
});
onBeforeUnmount(() => document.removeEventListener("pointerdown", handleDocPointerDown, true));

const maxTier = computed(() => trackMaxTierId(props.trackQuality));
const effective = computed(() => effectiveTierId(qualityState.selected, maxTier.value));
const effectiveTier = computed(() => tierById(effective.value) ?? QUALITY_TIERS[0]!);

function isDisabled(tier: QualityTier) {
  return TIER_RANK[tier.id] > TIER_RANK[maxTier.value];
}

function pick(tier: QualityTier) {
  if (isDisabled(tier)) return;
  selectQuality(tier.id);
  open.value = false;
}

const popStyle = computed(() => ({ bottom: props.align === "up" ? "calc(100% + 0.6rem)" : "calc(100% + 0.4rem)" }));
</script>

<template>
  <div ref="rootRef" class="quality" @keydown="handleKeydown">
    <button
      class="quality-badge"
      :title="`音质：${effectiveTier.label}（${effectiveTier.detail}），点击选择`"
      @click.stop="toggleOpen"
    >
      {{ effectiveTier.label }}
    </button>

    <div v-if="open" class="quality-pop" :style="popStyle" role="menu" aria-label="选择音质">
      <div class="quality-pop-title">选择音质</div>
      <button
        v-for="tier in QUALITY_TIERS"
        :key="tier.id"
        class="quality-option"
        :class="{ 'is-selected': tier.id === qualityState.selected, 'is-disabled': isDisabled(tier) }"
        role="menuitemradio"
        :aria-checked="tier.id === qualityState.selected"
        :disabled="isDisabled(tier)"
        @click="pick(tier)"
      >
        <span class="quality-option-copy">
          <span class="quality-option-label">{{ tier.label }}</span>
          <span class="quality-option-detail">{{ isDisabled(tier) ? "本曲未提供" : tier.detail }}</span>
        </span>
        <span v-if="tier.id === qualityState.selected" class="quality-option-check" aria-hidden="true">
          <AppIcon :src="checkIcon" />
        </span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.quality {
  position: relative;
}

.quality-badge {
  padding: 0.14rem 0.5rem;
  font-size: 0.72rem;
  font-weight: 600;
  letter-spacing: 0.02em;
  color: var(--track-accent);
  background: var(--track-accent-soft);
  border-radius: var(--radius-full);
  white-space: nowrap;
  transition:
    background-color var(--duration-fast) var(--ease-standard),
    color var(--duration-fast) var(--ease-standard);
}

.quality-badge:hover {
  color: var(--foreground);
  background: var(--muted);
}

/* —— 档位选择弹层 —— */
.quality-pop {
  position: absolute;
  left: 50%;
  transform: translateX(-50%);
  width: 13rem;
  padding: 0.5rem;
  background: var(--popover);
  color: var(--popover-foreground);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-md);
  z-index: var(--z-dropdown);
}

.quality-pop-title {
  padding: 0.25rem 0.5rem 0.45rem;
  font-size: 0.75rem;
  color: var(--muted-foreground);
}

.quality-option {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
  width: 100%;
  padding: 0.4rem 0.5rem;
  text-align: left;
  border-radius: var(--radius-md);
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.quality-option:hover:not(:disabled) {
  background: var(--track-accent-soft);
}

.quality-option.is-selected .quality-option-label {
  color: var(--track-accent);
  font-weight: 650;
}

.quality-option.is-disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.quality-option-copy {
  display: grid;
  min-width: 0;
}

.quality-option-label {
  font-size: 0.85rem;
}

.quality-option-detail {
  font-size: 0.72rem;
  color: var(--muted-foreground);
  font-variant-numeric: tabular-nums;
}

.quality-option-check {
  display: grid;
  place-items: center;
  width: 1rem;
  height: 1rem;
  color: var(--track-accent);
}

.quality-option-check .app-icon {
  width: 0.85rem;
  height: 0.85rem;
}
</style>
