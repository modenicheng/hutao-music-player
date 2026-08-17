<script setup lang="ts">
import type { PlayerController } from "../lib/player";

defineProps<{
  player: PlayerController;
}>();
</script>

<template>
  <div class="player-container" @click="player.hideOverlay"></div>
</template>

<style scoped>
.player-container {
  position: fixed;
  inset: 0;
  z-index: var(--z-overlay);
  width: 100%;
  height: 100%;
  color: var(--foreground);
  background: var(--surface-3);
  box-shadow: var(--shadow-lg);
  will-change: transform;
}
</style>
