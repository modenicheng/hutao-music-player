<script setup lang="ts">
import { computed, onMounted, onUnmounted } from "vue";
import AppIcon from "./AppIcon.vue";
import Equalizer from "./Equalizer.vue";
import HoverGroup from "./HoverGroup.vue";
import HoverItem from "./HoverItem.vue";
import Scroll from "./Scroll.vue";
import closeIcon from "../assets/icons/close-rounded.svg?raw";
import { PlayerControlStatus, type PlayerController } from "../lib/player";
import { trackPaletteVars } from "../lib/trackTheme.ts";

/**
 * 播放列表抽屉（DESIGN.md §3.1.7）：上下文各有一套确定的高度锚定，
 * 长队列交给内部 Scroll 滚动，抽屉本身不随内容增高——
 * - 主界面（非 themed）：右上半径浮板，底边停在 PlayerBar 上方，
 *   圆角 + 全边框与内容区 / PlayerBar 的浮板语言一致；
 * - themed（播放页 overlay 打开）：贴 overlay 右缘通高，底边停在
 *   底部控制台上方（--np-console-clearance），左侧圆角。
 * 行 hover 中性滑动高亮；当前行 accent 高亮 + 三根跳动条。
 * themed = overlay 打开中：抽屉属于播放页上下文，整只跟随专辑色；
 * 否则（从 PlayerBar 打开）用品牌层中性面板，两套上下文互不串色。
 * 弹出时全屏遮罩压暗其余页面，点击遮罩任意处关闭（ESC / 关闭按钮 /
 * PlayerBar 与控制台的播放列表按钮——后者为开关语义——均可收起）。
 */
const props = defineProps<{
  player: PlayerController;
  open: boolean;
  themed?: boolean;
}>();

const emit = defineEmits<{ close: [] }>();

function formatDuration(ms: number) {
  const totalSeconds = Math.max(0, Math.round(ms / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = String(totalSeconds % 60).padStart(2, "0");
  return `${minutes}:${seconds}`;
}

/** 队列总时长：一小时内"46 分钟"，跨小时"1 小时 24 分钟" */
const totalLabel = computed(() => {
  const totalMinutes = Math.round(
    props.player.state.queue.reduce((acc, item) => acc + item.durationMs, 0) /
      60_000,
  );
  if (totalMinutes <= 0) return "";
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  return hours > 0 ? `${hours} 小时 ${minutes} 分钟` : `${minutes} 分钟`;
});

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
    <!-- 遮罩：压暗抽屉外的页面；夹在 overlay(--z-overlay) 与抽屉(--z-modal)之间 -->
    <Transition name="queue-mask">
      <div
        v-if="open"
        class="queue-mask"
        aria-hidden="true"
        @click="emit('close')"
      />
    </Transition>
    <Transition name="drawer">
      <section
        v-if="open"
        class="queue-drawer"
        :class="{ themed: props.themed }"
        role="dialog"
        aria-label="播放列表"
        :style="props.themed ? (trackPaletteVars ?? undefined) : undefined"
      >
        <header class="drawer-head">
          <div class="drawer-copy">
            <h2 class="drawer-title">播放列表</h2>
            <p class="drawer-meta">
              {{ player.state.queue.length }} 首
              <template v-if="totalLabel"> · 总时长 {{ totalLabel }}</template>
            </p>
          </div>
          <div class="drawer-actions">
            <button
              v-if="player.state.queue.length > 0"
              class="drawer-clear"
              title="清空播放列表"
              @click="player.clearQueue()"
            >
              清空
            </button>
            <button class="drawer-close" title="关闭" @click="emit('close')">
              <AppIcon :src="closeIcon" />
            </button>
          </div>
        </header>

        <Scroll
          v-if="player.state.queue.length > 0"
          direction="vertical"
          class="drawer-list"
          width="auto"
        >
          <HoverGroup
            class="queue-rows"
            highlight-color="var(--queue-hover)"
            :highlight-opacity="1"
          >
            <HoverItem v-for="(item, index) in player.state.queue" :key="`${item.mid}-${index}`">
              <div
                class="queue-row"
                :class="{ 'is-current': isCurrent(index) }"
              >
                <button
                  class="queue-main"
                  :title="`播放《${item.title}》`"
                  @click="player.playAt(index)"
                >
                  <span class="row-index">
                    <Equalizer v-if="isCurrent(index)" :paused="!playingNow()" />
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
              </div>
            </HoverItem>
          </HoverGroup>
        </Scroll>

        <div v-else class="drawer-empty">
          <p class="empty-title">播放列表已空</p>
          <p class="empty-hint">从发现、搜索或歌单里挑几首吧</p>
        </div>
      </section>
    </Transition>
  </Teleport>
</template>

<style scoped>
.queue-drawer {
  /* 主界面上下文：中性表面面板 + 中性 hover；--queue-hover 由 .themed 覆写 */
  --queue-hover: var(--muted);
  position: fixed;
  /* 高度两端锚定（PlayerBar 上方），抽屉不随内容增高，长队列交给内部 Scroll */
  top: var(--layout-gap);
  right: var(--layout-gap);
  bottom: calc(var(--player-bar-height) + var(--layout-gap) * 2);
  z-index: var(--z-modal);
  display: flex;
  flex-direction: column;
  width: 380px;
  max-width: 90vw;
  padding: var(--space-4) var(--space-4) var(--space-3);
  color: var(--foreground);
  background: linear-gradient(180deg, var(--surface-3), var(--surface-2) 60%);
  backdrop-filter: blur(24px);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  overflow: hidden;
}

:global(.dark) .queue-drawer:not(.themed) {
  background: linear-gradient(180deg, var(--surface-2), var(--surface-1) 70%);
}

/* overlay 上下文（themed）：整只抽屉跟随专辑色，贴右缘、停在底部控制台上方 */
.queue-drawer.themed {
  --queue-hover: var(--track-accent-soft);
  top: 0;
  right: 0;
  bottom: var(--np-console-clearance);
  padding-bottom: var(--space-4);
  background: linear-gradient(180deg, var(--track-accent-soft), var(--surface-2) 60%);
  border-color: var(--border);
  border-radius: var(--radius-lg) 0 0 var(--radius-lg);
}

:global(.dark) .queue-drawer.themed {
  background: linear-gradient(180deg, var(--track-deep), var(--surface-1) 70%);
  color: var(--track-deep-fg);
}

/* 遮罩：比抽屉低一级（DOM 顺序也在其前），罩住主界面/overlay 全部内容 */
.queue-mask {
  position: fixed;
  inset: 0;
  z-index: calc(var(--z-modal) - 1);
  background: rgb(0 0 0 / 0.2);
}

:global(.dark) .queue-mask {
  background: rgb(0 0 0 / 0.45);
}

/* 压暗要能看出渐变过程：用两端缓的 S 曲线，避开前载曲线的“闪现”感；
   入场稍慢于退场（消失比出现利落） */
.queue-mask-enter-active {
  transition: opacity var(--duration-slow) cubic-bezier(0.4, 0, 0.2, 1);
}

.queue-mask-leave-active {
  transition: opacity var(--duration-normal) cubic-bezier(0.4, 0, 0.2, 1);
}

.queue-mask-enter-from,
.queue-mask-leave-to {
  opacity: 0;
}

.drawer-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  padding-bottom: var(--space-3);
  border-bottom: 1px solid var(--border);
}

.drawer-copy {
  display: grid;
  min-width: 0;
}

.drawer-title {
  font-size: 1rem;
  font-weight: 650;
}

.drawer-meta {
  margin-top: 0.1rem;
  font-size: 0.78rem;
  color: var(--muted-foreground);
  font-variant-numeric: tabular-nums;
}

.drawer-actions {
  display: flex;
  align-items: center;
  gap: var(--space-1);
  flex: 0 0 auto;
}

.drawer-clear {
  padding: 0.3rem 0.6rem;
  font-size: 0.82rem;
  color: var(--muted-foreground);
  border-radius: var(--radius-full);
  transition:
    background-color var(--duration-fast) var(--ease-standard),
    color var(--duration-fast) var(--ease-standard);
}

.drawer-clear:hover {
  color: var(--foreground);
  background: var(--muted);
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
  background: var(--muted);
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

/* 行 hover 用 HoverGroup 滑动高亮块，圆角与行一致 */
.queue-rows :deep(.hover-item) {
  border-radius: var(--radius-md);
}

.queue-row {
  position: relative;
  border-radius: var(--radius-md);
}

.queue-row.is-current .row-title {
  color: var(--track-accent);
  font-weight: 650;
}

.queue-main {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  min-width: 0;
  width: 100%;
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
  transition: opacity var(--duration-fast) var(--ease-standard);
}

/* 悬停时时长让位给移除按钮（原位浮现），不常驻预留列宽——
   否则行尾会拖出一条 ~60px 的死空间 */
.queue-row:hover .row-duration,
.queue-row:focus-within .row-duration {
  opacity: 0;
}

.row-remove {
  position: absolute;
  top: 50%;
  right: 0.3rem;
  transform: translateY(-50%);
  display: grid;
  place-items: center;
  width: 1.8rem;
  height: 1.8rem;
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
  background: var(--queue-hover);
  color: var(--foreground);
}

.row-remove .app-icon {
  width: 0.85rem;
  height: 0.85rem;
}

/* —— 空态 —— */
.drawer-empty {
  display: grid;
  flex: 1;
  place-content: center;
  justify-items: center;
  gap: var(--space-1);
  text-align: center;
}

.empty-title {
  font-size: 0.95rem;
  font-weight: 600;
}

.empty-hint {
  font-size: 0.82rem;
  color: var(--muted-foreground);
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
