<script setup lang="ts">
import { onMounted, onUnmounted } from "vue";
import NowPlayingBody from "../components/nowplaying/NowPlayingBody.vue";
import AppIcon from "../components/AppIcon.vue";
import collapseIcon from "../assets/icons/expand-more-rounded.svg?raw";
import type { PlayerController } from "../lib/player";
import { trackPaletteVars } from "../lib/trackTheme.ts";

/**
 * 全屏播放层（DESIGN.md §3.1）：slide-bottom 入场由 MainLayout 的
 * Transition 提供；ESC 或收起按钮关闭。
 * 专辑取色的 --track-* 变量只注入这一层子树（DESIGN.md §1.2 v0.3）。
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
  <div class="player-overlay" :style="trackPaletteVars ?? undefined">
    <button class="overlay-collapse" title="收起播放页 (ESC)" @click="player.hideOverlay">
      <AppIcon :src="collapseIcon" />
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

/* 裸图标收起键：无底无框，墨色深浅由环境层顶部背景自动决定
   （--track-on-ambient 按渐变起点对比度计算）；idle 略收，hover 提满 */
.overlay-collapse {
  position: absolute;
  top: 1rem;
  left: 1rem;
  z-index: 2;
  display: grid;
  place-items: center;
  width: 2.25rem;
  height: 2.25rem;
  color: color-mix(in srgb, var(--track-on-ambient, var(--foreground)) 75%, transparent);
  transition: color var(--duration-fast) var(--ease-standard);
}

.overlay-collapse:hover {
  color: var(--track-on-ambient, var(--foreground));
}

.overlay-collapse .app-icon {
  width: 1.4rem;
  height: 1.4rem;
}
</style>
