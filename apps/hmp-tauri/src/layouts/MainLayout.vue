<script setup lang="ts">
import Sidebar from "./Sidebar.vue";
import PlayerBar from "./PlayerBar.vue";
import PlayerOverlay from "./PlayerOverlay.vue";

import { ref } from "vue";

const playerOverlay = ref(false);
</script>

<template>
  <div class="app-layout">
    <!-- <TopBar class="top-bar" /> -->

    <Sidebar class="sidebar" />

    <main class="content">
      <RouterView />
    </main>

    <PlayerBar
      class="player-bar"
      @click="
        () => {
          playerOverlay = !playerOverlay;
        }
      "
    />
  </div>

  <Transition name="slide-bottom">
    <PlayerOverlay v-if="playerOverlay" @close="playerOverlay = false" />
  </Transition>
</template>

<style scoped>
.app-layout {
  width: 100%;
  height: 100%;
  display: grid;
  gap: var(--layout-gap);
  padding: var(--layout-gap);
  grid-template-columns: var(--sidebar-width) minmax(0, 1fr);
  grid-template-rows: minmax(0, 1fr) var(--player-bar-height);
  background: var(--background);
}

/*
.top-bar {
  grid-column: 2 / -1;
  grid-row: 1;
} */

.sidebar {
  grid-column: 1;
  grid-row: 1 / -1;
}

.content {
  grid-column: 2;
  grid-row: 1;

  min-width: 0;
  min-height: 0;

  overflow: auto;

  background: var(--surface-1);
  color: var(--foreground);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
}

.player-bar {
  grid-column: 2 / -1;
  grid-row: 2;
}

/* PlayerOverlay 入场/退场动画 */
.slide-bottom-enter-active {
  transition: transform var(--duration-normal) var(--ease-enter);
}

.slide-bottom-leave-active {
  transition: transform var(--duration-fast) var(--ease-exit);
}

.slide-bottom-enter-from,
.slide-bottom-leave-to {
  transform: translateY(100%);
}
</style>
