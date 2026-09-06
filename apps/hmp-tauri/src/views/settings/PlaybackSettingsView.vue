<script setup lang="ts">
import { computed, inject } from "vue";
import PageHeader from "../../components/PageHeader.vue";
import SettingsNav from "./SettingsNav.vue";
import { playerKey } from "../../lib/player.ts";
import {
  QUALITY_TIERS,
  qualityState,
  selectQuality,
} from "../../lib/qualityStore.ts";

/** 播放设置：音质与音量真实生效；行为开关未接线，禁用展示 */
const player = inject(playerKey);

const volumePercent = computed(() => Math.round((player?.state.volume ?? 1) * 100));

function onVolumeInput(event: Event) {
  const input = event.target as HTMLInputElement;
  player?.setVolume(Number(input.value) / 100);
}
</script>

<template>
  <div class="playback-settings-view">
    <PageHeader title="播放设置" back-to="/settings" />
    <SettingsNav />

    <section class="settings-group" aria-labelledby="quality-title">
      <h2 id="quality-title" class="group-title">音质偏好</h2>

      <div class="quality-options" role="radiogroup" aria-label="音质偏好">
        <button
          v-for="tier in QUALITY_TIERS"
          :key="tier.id"
          type="button"
          role="radio"
          class="quality-option"
          :class="{ 'is-active': qualityState.selected === tier.id }"
          :aria-checked="qualityState.selected === tier.id"
          @click="selectQuality(tier.id)"
        >
          <span class="quality-copy">
            <span class="quality-label">{{ tier.label }}</span>
            <span class="quality-detail">{{ tier.detail }}</span>
          </span>
          <span class="quality-dot" aria-hidden="true"></span>
        </button>
      </div>

      <p class="group-caption">
        实际播放档位取偏好与曲目最高档中较低的一档，与 PlayerBar / 播放页的音质徽章联动
      </p>
    </section>

    <section v-if="player" class="settings-group" aria-labelledby="volume-title">
      <h2 id="volume-title" class="group-title">音量</h2>

      <div class="setting-row">
        <div class="setting-copy">
          <span class="setting-label">默认音量</span>
          <span class="setting-desc">与底部播放条的音量控制实时同步</span>
        </div>
        <div class="volume-control">
          <input
            class="volume-slider"
            type="range"
            min="0"
            max="100"
            step="1"
            :value="volumePercent"
            aria-label="音量"
            @input="onVolumeInput"
          />
          <span class="volume-value">{{ volumePercent }}%</span>
        </div>
      </div>
    </section>

    <section class="settings-group" aria-labelledby="behavior-title">
      <h2 id="behavior-title" class="group-title">播放行为</h2>

      <div class="setting-row">
        <div class="setting-copy">
          <span class="setting-label">启动后恢复上次队列</span>
          <span class="setting-desc">打开应用时接续上次听到的位置</span>
        </div>
        <span class="switch" aria-hidden="true"></span>
      </div>

      <div class="setting-row">
        <div class="setting-copy">
          <span class="setting-label">歌曲间淡入淡出</span>
          <span class="setting-desc">切换曲目时做短交叉淡化</span>
        </div>
        <span class="switch" aria-hidden="true"></span>
      </div>

      <p class="group-caption">播放行为依赖播放守护进程的对应能力，即将上线</p>
    </section>
  </div>
</template>

<style scoped>
.playback-settings-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
  max-width: 56rem;
}

.settings-group {
  padding: var(--space-4) var(--space-5);
  background: var(--surface-3);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-sm);
}

.settings-group + .settings-group {
  margin-top: var(--space-4);
}

.group-title {
  padding: var(--space-1) var(--space-2) var(--space-3);
  font-size: 1rem;
  font-weight: 650;
}

.group-caption {
  padding: var(--space-2) var(--space-2) var(--space-1);
  font-size: 0.8rem;
  color: var(--muted-foreground);
}

.setting-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-6);
  padding: var(--space-3) var(--space-2);
}

.setting-row + .setting-row {
  border-top: 1px solid var(--border);
}

.setting-copy {
  display: grid;
  min-width: 0;
}

.setting-label {
  font-size: 0.92rem;
  font-weight: 550;
}

.setting-desc {
  margin-top: 0.15rem;
  font-size: 0.8rem;
  color: var(--muted-foreground);
}

/* —— 音质四选：整行可点的单选卡，选中 = muted 底 —— */
.quality-options {
  display: grid;
  gap: var(--space-2);
}

.quality-option {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  padding: 0.6rem 0.75rem;
  text-align: left;
  color: var(--foreground);
  background: transparent;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  transition:
    background-color var(--duration-fast) var(--ease-standard),
    border-color var(--duration-fast) var(--ease-standard);
}

.quality-option:hover:not(.is-active) {
  background: var(--muted);
}

.quality-option.is-active {
  background: var(--muted);
  border-color: var(--border-strong);
}

.quality-copy {
  display: grid;
  min-width: 0;
}

.quality-label {
  font-size: 0.92rem;
  font-weight: 550;
}

.quality-detail {
  margin-top: 0.1rem;
  font-size: 0.78rem;
  color: var(--muted-foreground);
  font-variant-numeric: tabular-nums;
}

/* 单选圆点：选中实心 */
.quality-dot {
  width: 1rem;
  height: 1rem;
  flex: 0 0 auto;
  border: 1.5px solid var(--border-strong);
  border-radius: var(--radius-full);
  transition:
    background-color var(--duration-fast) var(--ease-standard),
    box-shadow var(--duration-fast) var(--ease-standard);
}

.quality-option.is-active .quality-dot {
  background: var(--primary);
  border-color: var(--primary);
  box-shadow: inset 0 0 0 2.5px var(--surface-3);
}

/* —— 音量横条：全局已统一 accent 色，这里只管布局 —— */
.volume-control {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  flex: 0 0 auto;
}

.volume-slider {
  width: 11rem;
}

.volume-value {
  min-width: 2.8rem;
  text-align: right;
  font-size: 0.85rem;
  color: var(--muted-foreground);
  font-variant-numeric: tabular-nums;
}

/* —— 行为开关（禁用态）：轨道 + 滑块，一律灰 —— */
.switch {
  position: relative;
  width: 2.25rem;
  height: 1.25rem;
  flex: 0 0 auto;
  background: var(--muted);
  border: 1px solid var(--border);
  border-radius: var(--radius-full);
  opacity: 0.55;
}

.switch::after {
  content: "";
  position: absolute;
  top: 50%;
  left: 0.15rem;
  width: 0.95rem;
  height: 0.95rem;
  background: var(--surface-3);
  border-radius: var(--radius-full);
  box-shadow: var(--shadow-sm);
  transform: translateY(-50%);
}

/* 窄窗：控制组换行到标签下方 */
@media (max-width: 42rem) {
  .setting-row {
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-3);
  }
}
</style>
