<script setup lang="ts">
import { onMounted, onUnmounted } from "vue";
import AppIcon from "./AppIcon.vue";
import Scroll from "./Scroll.vue";
import closeIcon from "../assets/icons/close-rounded.svg?raw";
import { PlayerControlStatus, type PlayerController } from "../lib/player";

/**
 * 播放列表抽屉（DESIGN.md §3.1.7）：面板底色跟随曲目调色板，
 * 当前行 accent 高亮 + 三根跳动条。数据来自 PlayerBridge 队列扩展。
 */
const props = defineProps<{
  player: PlayerController;
  open: boolean;
}>();

const emit = defineEmits<{ close: [] }>();

function formatDuration(ms: number) {
  const totalSeconds = Math.max(0, Math.round(ms / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = String(totalSeconds % 60).padStart(2, "0");
  return `${minutes}:${seconds}`;
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape" && props.open) {
    emit("close");
  }
}

onMounted(() => window.addEventListener("keydown", onKeydown));
onUnmounted(() => window.removeEventListener("keydown", onKeydown));

function isCurrent(index: number) {
  return props.player.state.currentTrack?.mid === props.player.state.queue[index]?.mid;
}

function playingNow() {
  return (
    props.player.state.playing &&
    props.player.state.controlStatus !== PlayerControlStatus.dragging
  );
}
</script>

<template>
  <Teleport to="body">
    <Transition name="drawer">
      <section
        v-if="open"
        class="queue-drawer"
        role="dialog"
        aria-label="播放列表"
      >
        <header class="drawer-head">
          <h2 class="drawer-title">播放列表 · {{ player.state.queue.length }} 首</h2>
          <button class="drawer-close" title="关闭" @click="emit('close')">
            <AppIcon :src="closeIcon" />
          </button>
        </header>

        <Scroll direction="vertical" class="drawer-list">
          <ul class="queue-rows">
            <li
              v-for="(item, index) in player.state.queue"
              :key="`${item.mid}-${index}`"
              class="queue-row"
              :class="{ 'is-current': isCurrent(index) }"
            >
              <button
                class="queue-main"
                :title="`播放《${item.title}》`"
                @click="player.playAt(index)"
              >
                <span class="row-index">
                  <span v-if="isCurrent(index)" class="equalizer" :class="{ 'is-paused': !playingNow() }" aria-hidden="true">
                    <i></i><i></i><i></i>
                  </span>
                  <template v-else>{{ index + 1 }}</template>
                </span>
                <span class="row-cover">
                  <img v-if="item.coverUrl" :src="item.coverUrl" :alt="item.title" loading="lazy" />
                </span>
                <span class="row-copy">
                  <span class="row-title">{{ item.title }}</span>
                  <span class="row-artists">{{ item.artists.join(" / ") }}</span>
                </span>
                <span class="row-duration">{{ formatDuration(item.durationMs) }}</span>
              </button>
              <button
                class="row-remove"
                title="从列表移除"
                @click="player.removeAt(index)"
              >
                <AppIcon :src="closeIcon" />
              </button>
            </li>
          </ul>
        </Scroll>
      </section>
    </Transition>
  </Teleport>
</template>

<style scoped>
.queue-drawer {
  position: fixed;
  top: 0;
  right: 0;
  /* 高于播放页 overlay（z-overlay），全局唯一实例挂在 MainLayout */
  z-index: var(--z-modal);
  display: flex;
  flex-direction: column;
  width: 380px;
  max-width: 90vw;
  padding: var(--space-4) var(--space-4) 0;
  color: var(--foreground);
  /* 面板底色跟随曲目调色板（亮=soft 混表面，暗=deep） */
  background: linear-gradient(180deg, var(--track-accent-soft), var(--surface-2) 60%);
  backdrop-filter: blur(24px);
  border-left: 1px solid var(--border);
  box-shadow: var(--shadow-lg);
}

:global(.dark) .queue-drawer {
  background: linear-gradient(180deg, var(--track-deep), var(--surface-1) 70%);
  color: var(--track-deep-fg);
}

.drawer-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  padding-bottom: var(--space-3);
  border-bottom: 1px solid var(--border);
}

.drawer-title {
  font-size: 1rem;
  font-weight: 650;
}

.drawer-close {
  display: grid;
  place-items: center;
  width: 2rem;
  height: 2rem;
  border-radius: var(--radius-full);
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.drawer-close:hover {
  background: var(--track-accent-soft);
}

.drawer-close .app-icon {
  width: 1.1rem;
  height: 1.1rem;
}

.drawer-list {
  flex: 1;
  min-height: 0;
  margin: 0 calc(-1 * var(--space-4));
}

.queue-rows {
  padding: var(--space-2) var(--space-2) var(--space-6);
}

.queue-row {
  display: flex;
  align-items: center;
  border-radius: var(--radius-md);
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.queue-row:hover {
  background: var(--track-accent-soft);
}

.queue-row.is-current .row-title {
  color: var(--track-accent);
  font-weight: 650;
}

.queue-main {
  display: flex;
  flex: 1;
  align-items: center;
  gap: var(--space-3);
  min-width: 0;
  padding: 0.4rem 0.5rem;
  text-align: left;
}

.row-index {
  display: grid;
  place-items: center;
  width: 1.6rem;
  flex: 0 0 auto;
  font-size: 0.82rem;
  color: var(--muted-foreground);
  font-variant-numeric: tabular-nums;
}

.row-cover {
  width: 2.25rem;
  height: 2.25rem;
  flex: 0 0 auto;
  border-radius: var(--radius-sm);
  overflow: hidden;
  background: var(--muted);
}

.row-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.row-copy {
  display: grid;
  min-width: 0;
  flex: 1;
  line-height: 1.3;
}

.row-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 0.9rem;
}

.row-artists {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 0.76rem;
  color: var(--muted-foreground);
}

.row-duration {
  flex: 0 0 auto;
  font-size: 0.78rem;
  color: var(--muted-foreground);
  font-variant-numeric: tabular-nums;
}

.row-remove {
  display: grid;
  place-items: center;
  width: 1.8rem;
  height: 1.8rem;
  margin-right: 0.35rem;
  flex: 0 0 auto;
  color: var(--muted-foreground);
  border-radius: var(--radius-full);
  opacity: 0;
  transition:
    opacity var(--duration-fast) var(--ease-standard),
    background-color var(--duration-fast) var(--ease-standard);
}

.queue-row:hover .row-remove,
.row-remove:focus-visible {
  opacity: 1;
}

.row-remove:hover {
  background: var(--track-accent-soft);
  color: var(--foreground);
}

.row-remove .app-icon {
  width: 0.85rem;
  height: 0.85rem;
}

/* 三根跳动条：正在播放的行替代序号 */
.equalizer {
  display: flex;
  align-items: flex-end;
  gap: 2px;
  height: 0.85rem;
}

.equalizer i {
  width: 3px;
  background: var(--track-equalizer);
  border-radius: 1px;
  animation: eq-bounce 0.9s ease-in-out infinite;
}

.equalizer i:nth-child(1) {
  height: 60%;
  animation-delay: 0s;
}

.equalizer i:nth-child(2) {
  height: 100%;
  animation-delay: 0.25s;
}

.equalizer i:nth-child(3) {
  height: 45%;
  animation-delay: 0.5s;
}

.equalizer.is-paused i {
  animation-play-state: paused;
}

@keyframes eq-bounce {
  0%,
  100% {
    transform: scaleY(0.55);
  }
  50% {
    transform: scaleY(1);
  }
}

@media (prefers-reduced-motion: reduce) {
  .equalizer i {
    animation: none;
  }
}

/* 抽屉进出场 */
.drawer-enter-active,
.drawer-leave-active {
  transition: transform var(--duration-normal) var(--ease-enter), opacity var(--duration-normal) var(--ease-enter);
}

.drawer-enter-from,
.drawer-leave-to {
  transform: translateX(100%);
  opacity: 0;
}
</style>
