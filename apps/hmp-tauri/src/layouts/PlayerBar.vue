<script setup lang="ts">
import { PlayerControlStatus, type PlayerController } from "../lib/player";
import AppIcon from "../components/AppIcon.vue";
import playIcon from "../assets/icons/play_arrow-rounded.svg?raw";
import pauseIcon from "../assets/icons/pause-rounded.svg?raw";
import skipNextIcon from "../assets/icons/skip-next-rounded.svg?raw";
import skipPreviousIcon from "../assets/icons/skip-previous-rounded.svg?raw";
import queueIcon from "../assets/icons/queue-music-rounded.svg?raw";

const props = defineProps<{
  player: PlayerController;
  /** 打开播放列表抽屉（桥支持队列时显示按钮） */
  onOpenQueue?: () => void;
}>();

function artistsText() {
  return props.player.state.artists.join(" / ");
}
</script>

<template>
  <footer class="player-bar">
    <!-- 长进度条：横贯全宽（保留原型拖拽机制） -->
    <div
      :ref="player.captureProgressBar"
      :class="[
        'progress-bar',
        player.state.controlStatus === PlayerControlStatus.dragging
          ? 'progress-bar-hover'
          : '',
      ]"
      @mousedown="player.startDragging"
      @mouseup="player.setProgress"
    >
      <div
        class="progress"
        :style="{
          transform: `scaleX(${player.state.progress})`,
          transformOrigin: `left`,
        }"
      ></div>
    </div>
    <div class="status-card">
      <div
        class="now-playing"
        :title="player.state.title ? '打开播放页' : undefined"
        @click="player.state.title && player.showOverlay()"
      >
        <img
          v-if="player.state.currentTrack?.coverUrl"
          class="now-cover"
          :src="player.state.currentTrack.coverUrl"
          :alt="player.state.currentTrack.title"
        />
        <div v-else class="now-cover now-cover-empty" aria-hidden="true"></div>
        <div class="now-copy">
          <span class="now-title">{{ player.state.title ?? "未在播放" }}</span>
          <span v-if="artistsText()" class="now-artists">{{ artistsText() }}</span>
        </div>
      </div>

      <div class="controls">
        <button
          class="control-button"
          title="上一曲"
          :disabled="!player.state.canGoPrevious"
          @click="player.previous"
        >
          <AppIcon :src="skipPreviousIcon" />
        </button>
        <button class="control-button control-play" :title="player.state.playing ? '暂停' : '播放'" @click="player.togglePlay">
          <AppIcon :src="player.state.playing ? pauseIcon : playIcon" />
        </button>
        <button
          class="control-button"
          title="下一曲"
          :disabled="!player.state.canGoNext"
          @click="player.next"
        >
          <AppIcon :src="skipNextIcon" />
        </button>
      </div>

      <div class="side-controls">
        <input
          class="volume"
          type="range"
          min="0"
          max="1"
          step="0.01"
          aria-label="音量"
          :value="player.state.volume"
          @input="player.setVolume(($event.target as HTMLInputElement).valueAsNumber)"
        />
        <button
          v-if="onOpenQueue && player.state.queue.length > 0"
          class="control-button"
          title="播放列表"
          @click="onOpenQueue"
        >
          <AppIcon :src="queueIcon" />
        </button>
      </div>
    </div>
  </footer>
</template>

<style lang="css" scoped>
.player-bar {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  min-height: var(--player-bar-height);
  color: var(--foreground);
}

.progress-bar {
  width: 100%;
  height: var(--control-height-xs);
  background: var(--track);
  border-radius: var(--radius-full);
  transform: scaleY(1);
  transition: transform var(--duration-fast);
}

.progress {
  width: 100%;
  height: 100%;
  background: var(--track-accent);
  border-radius: var(--radius-full);
}
.progress-bar-hover {
  transform: scaleY(1.5);
  transition: transform var(--duration-fast);
}

.progress-bar:hover {
  transform: scaleY(1.5);
  transition: transform var(--duration-fast);
}

.status-card {
  display: flex;
  align-items: center;
  gap: var(--space-4);
  padding: 0 var(--space-4);
  width: 100%;
  flex: 1;
  min-height: 0;
  background: var(--surface-2);
  border-radius: var(--radius-lg);
}

.now-playing {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  min-width: 0;
  flex: 1;
  cursor: pointer;
  border-radius: var(--radius-md);
}

.now-cover {
  width: 2.75rem;
  height: 2.75rem;
  flex: 0 0 auto;
  border-radius: var(--radius-md);
  object-fit: cover;
  box-shadow: var(--shadow-sm);
}

.now-cover-empty {
  background: var(--muted);
}

.now-copy {
  display: grid;
  min-width: 0;
  line-height: 1.3;
}

.now-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 0.92rem;
  font-weight: 550;
}

.now-artists {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 0.78rem;
  color: var(--muted-foreground);
}

.controls {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.control-button {
  display: grid;
  place-items: center;
  width: 2.25rem;
  height: 2.25rem;
  color: var(--foreground);
  border-radius: var(--radius-full);
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.control-button .app-icon {
  width: 1.3rem;
  height: 1.3rem;
}

.control-button:hover:not(:disabled) {
  background: var(--muted);
}

.control-button:disabled {
  opacity: 0.35;
  cursor: default;
}

/* 主播放键：略大 + 品牌色圆底 */
.control-play {
  width: 2.6rem;
  height: 2.6rem;
  color: var(--primary-foreground);
  background: var(--primary);
}

.control-play .app-icon {
  width: 1.55rem;
  height: 1.55rem;
}

.control-play:hover:not(:disabled) {
  background: var(--primary-hover);
}

.side-controls {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--space-2);
  flex: 1;
}

.volume {
  width: 8rem;
}
</style>
