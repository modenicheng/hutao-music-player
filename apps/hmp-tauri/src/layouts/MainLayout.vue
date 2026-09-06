<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import Sidebar from "./Sidebar.vue";
import PlayerBar from "./PlayerBar.vue";
import PlayerOverlay from "./PlayerOverlay.vue";
import QueueDrawer from "../components/QueueDrawer.vue";
import Scroll from "../components/Scroll.vue";
import type { PlayerController } from "../lib/player.ts";

defineProps<{
  player: PlayerController;
}>();

// niri 等平铺 WM 下窗口可能被压到 ⅓ 宽：≤52rem 自动收成图标栏。
// 用户手动切换后以手动为准；跨越断点时清除手动值回到自动。
const narrowQuery = window.matchMedia("(max-width: 52rem)");
const isNarrow = ref(narrowQuery.matches);
const manualCollapsed = ref<boolean | null>(null);
const sidebarCollapsed = computed(() => manualCollapsed.value ?? isNarrow.value);

function onNarrowChange(event: MediaQueryListEvent) {
  isNarrow.value = event.matches;
  manualCollapsed.value = null;
}

onMounted(() => narrowQuery.addEventListener("change", onNarrowChange));
onUnmounted(() => narrowQuery.removeEventListener("change", onNarrowChange));
</script>

<template>
  <div
    class="app-layout"
    :class="{ 'is-sidebar-collapsed': sidebarCollapsed }"
  >
    <!-- <TopBar class="top-bar" /> -->

    <Sidebar
      :collapsed="sidebarCollapsed"
      @update:collapsed="manualCollapsed = $event"
      class="sidebar"
    />

    <main class="content">
      <!-- 页面滚动统一走自绘 Scroll 组件，不用原生滚动条 -->
      <Scroll>
        <RouterView />
      </Scroll>
    </main>

    <PlayerBar
      :player="player"
      :on-toggle-queue="player.toggleQueue"
      class="player-bar"
    />
  </div>

  <QueueDrawer
    :player="player"
    :open="player.state.queueVisible"
    :themed="player.state.overlayVisible"
    @close="player.hideQueue"
  />

  <Transition name="slide-bottom">
    <PlayerOverlay v-if="player.state.overlayVisible" :player="player" />
  </Transition>
</template>

<style scoped>
.app-layout {
  width: 100%;
  height: 100%;
  display: grid;
  gap: var(--layout-gap);
  padding: var(--layout-gap);
  grid-template-columns: var(--sidebar-current-width) minmax(0, 1fr);
  grid-template-rows: minmax(0, 1fr) var(--player-bar-height);
  background: var(--surface-1);
  --sidebar-current-width: var(--sidebar-width);
  transition: grid-template-columns var(--duration-slow) var(--ease-standard);
}

.app-layout.is-sidebar-collapsed {
  --sidebar-current-width: 4rem;
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

  /* 滚动交给内部的 Scroll 组件，这里只做裁剪 */
  overflow: hidden;

  background: var(--surface-2);
  color: var(--foreground);
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
