<script setup lang="ts">
import { onMounted, onUnmounted, provide } from "vue";
import MainLayout from "./layouts/MainLayout.vue";
import { PlayerController, playerKey } from "./lib/player.ts";
import { tauriPlayerBridge } from "./lib/tauriPlayerBridge.ts";
import { BrowserPlayerBridge } from "./lib/browserPlayerBridge.ts";
import { isTauriRuntime } from "./lib/runtime.ts";
import { applyTrackTheme } from "./lib/trackTheme.ts";

// 浏览器开发模式跑模拟桥；Tauri 壳内走真实 daemon 控制
const player = new PlayerController(
  isTauriRuntime() ? tauriPlayerBridge : new BrowserPlayerBridge(),
);

provide(playerKey, player);

// 曲目层调色全局生效（任何页面的 --track-* 消费方都跟随当前曲目）
const stopTrackTheme = applyTrackTheme(player);

onMounted(player.mount);
onUnmounted(() => {
  stopTrackTheme();
  player.unmount();
});
</script>

<template>
  <MainLayout :player="player" />
</template>
