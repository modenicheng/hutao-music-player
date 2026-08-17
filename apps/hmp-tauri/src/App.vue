<script setup lang="ts">
import { onMounted, onUnmounted } from "vue";
import MainLayout from "./layouts/MainLayout.vue";
import { PlayerController } from "./lib/player.ts";
import { tauriPlayerBridge } from "./lib/tauriPlayerBridge.ts";

const player = new PlayerController(tauriPlayerBridge);

onMounted(player.mount);
onUnmounted(player.unmount);
</script>

<template>
  <MainLayout :player="player" />
</template>
