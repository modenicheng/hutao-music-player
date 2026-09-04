<script setup lang="ts">
import { inject } from "vue";
import NowPlayingBody from "../components/nowplaying/NowPlayingBody.vue";
import { playerKey } from "../lib/player.ts";

// 路由直达播放页与全屏 overlay 共用同一主体组件
const player = inject(playerKey);
</script>

<template>
  <div v-if="player" class="now-playing-view">
    <NowPlayingBody :player="player" />
  </div>
</template>

<style scoped>
.now-playing-view {
  height: 100%;
  min-height: 0;
}
</style>
