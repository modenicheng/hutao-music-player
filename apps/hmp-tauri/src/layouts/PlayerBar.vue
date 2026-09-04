<script setup lang="ts">
import { PlayerControlStatus, type PlayerController } from "../lib/player";

defineProps<{
  player: PlayerController;
}>();
</script>

<template>
  <footer class="player-bar">
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
      <div class="btn" @click="player.togglePlay">
        {{ player.state.playing ? `pause` : `play` }}
      </div>
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
  background: var(--primary);
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
.cursor {
  height: 0.8rem;
  width: 0.3rem;
  background-color: var(--primary);
}

.status-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 var(--space-4);
  width: 100%;
  flex: 1;
  min-height: 0;
  background: var(--surface-2);
  border-radius: var(--radius-lg);
}

.volume {
  width: 8rem;
}
</style>
