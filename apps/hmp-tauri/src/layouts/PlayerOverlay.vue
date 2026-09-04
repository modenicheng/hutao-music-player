<script setup lang="ts">
import { onMounted, onUnmounted } from "vue";
import NowPlayingBody from "../components/nowplaying/NowPlayingBody.vue";
import AppIcon from "../components/AppIcon.vue";
import closeIcon from "../assets/icons/expand-less-rounded.svg?raw";
import type { PlayerController } from "../lib/player";

/**
 * 全屏播放层（DESIGN.md §3.1）：slide-bottom 入场由 MainLayout 的
 * Transition 提供；ESC 或收起按钮关闭。
 */
const props = defineProps<{
  player: PlayerController;
}>();

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    props.player.hideOverlay();
  }
}

onMounted(() => window.addEventListener("keydown", onKeydown));
onUnmounted(() => window.removeEventListener("keydown", onKeydown));
</script>

<template>
  <div class="player-overlay">
    <button class="overlay-collapse" title="收起播放页 (ESC)" @click="player.hideOverlay">
      <AppIcon :src="closeIcon" />
    </button>
    <NowPlayingBody :player="player" />
  </div>
</template>

<style scoped>
.player-overlay {
  position: fixed;
  inset: 0;
  z-index: var(--z-overlay);
  width: 100%;
  height: 100%;
  color: var(--foreground);
  background: var(--surface-2);
  box-shadow: var(--shadow-lg);
  will-change: transform;
}

.overlay-collapse {
  position: absolute;
  top: 1rem;
  left: 1rem;
  z-index: 2;
  display: grid;
  place-items: center;
  width: 2.25rem;
  height: 2.25rem;
  color: var(--foreground);
  background: color-mix(in srgb, var(--surface-3) 72%, transparent);
  border-radius: var(--radius-full);
  box-shadow: var(--shadow-sm);
  backdrop-filter: blur(8px);
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.overlay-collapse:hover {
  background: var(--surface-3);
}

.overlay-collapse .app-icon {
  width: 1.25rem;
  height: 1.25rem;
}
</style>
