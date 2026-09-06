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

// dev 测试钩子：QA 脚本灌长队列 / 驱动播放器用，不进生产产物
if (import.meta.env.DEV) {
  (window as unknown as Record<string, unknown>).__hmpPlayer = player;
}

// 曲目层调色 App 级预热；动态变量只注入播放页 overlay（其余界面用品牌回退色）
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
