<script setup lang="ts">
import { onMounted, onUnmounted } from "vue";
import MainLayout from "./layouts/MainLayout.vue";
import { PlayerController } from "./lib/player.ts";

const player = new PlayerController("Hope_is_the_thing_with_featers.flac");

onMounted(player.mount);
onUnmounted(player.unmount);
</script>

<template>
  <MainLayout :player="player" />
</template>
