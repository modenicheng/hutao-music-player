<script setup lang="ts">
import AppIcon from "./AppIcon.vue";
import Equalizer from "./Equalizer.vue";
import HoverGroup from "./HoverGroup.vue";
import HoverItem from "./HoverItem.vue";
import playIcon from "../assets/icons/play_arrow-rounded.svg?raw";
import type { PlayerController } from "../lib/player";
import { songToQueueItem } from "../lib/browserPlayerBridge.ts";
import type { SongRef } from "../lib/api/types.ts";

/**
 * 曲目表（DESIGN.md §3.0）：序号/标题+歌手/专辑/时长。
 * 整行点击 → 替换队列并从该行播放；当前行显示跳动条并高亮；
 * 行 hover 用 HoverGroup 滑动高亮块（与侧栏导航同款物理效果，中性底）。
 * 播放图标挂在 HoverGroup 的 indicator 槽，随高亮块一同纵向滑入序号位：
 * 跳动条优先级最高（正在播放行图标让位），其次播放图标，序号最低。
 */
const props = withDefaults(
  defineProps<{
    tracks: SongRef[];
    /** 传入则启用当前行高亮与点击播放 */
    player?: PlayerController;
    showAlbum?: boolean;
    showHeader?: boolean;
    /** 外部已在播放的 mid（可选覆盖） */
    activeMid?: string | null;
  }>(),
  { showAlbum: true, showHeader: false, activeMid: null },
);

const emit = defineEmits<{ play: [song: SongRef, index: number] }>();

function isCurrent(song: SongRef) {
  const mid = props.activeMid ?? props.player?.state.currentTrack?.mid ?? null;
  return mid !== null && mid === song.mid;
}

function playingNow() {
  return props.player?.state.playing ?? false;
}

function formatDuration(ms: number) {
  const totalSeconds = Math.max(0, Math.round(ms / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = String(totalSeconds % 60).padStart(2, "0");
  return `${minutes}:${seconds}`;
}

function play(song: SongRef, index: number) {
  emit("play", song, index);
  if (props.player?.playTracks) {
    props.player.playTracks(
      props.tracks.map(songToQueueItem),
      index,
    );
  }
}
</script>

<template>
  <div class="track-table" :class="{ 'with-album': showAlbum }">
    <div v-if="showHeader" class="table-row table-head" aria-hidden="true">
      <span class="cell-index">#</span>
      <span class="cell-title">标题</span>
      <span v-if="showAlbum" class="cell-album">专辑</span>
      <span class="cell-duration">时长</span>
    </div>

    <!-- hover 高亮用中性底，与侧栏同款；主题色只留给正在播放行 -->
    <HoverGroup class="track-rows" highlight-color="var(--neutral-200)" :highlight-opacity="0.6">
      <HoverItem v-for="(song, index) in tracks" :key="song.mid">
        <button
          class="table-row"
          :class="{ 'is-current': isCurrent(song) }"
          :title="`播放《${song.title}》`"
          @click="play(song, index)"
        >
          <span class="cell-index">
            <Equalizer v-if="isCurrent(song)" :paused="!playingNow()" />
            <template v-else>{{ index + 1 }}</template>
          </span>
          <span class="cell-title">
            <span class="song-title">{{ song.title }}</span>
            <span class="song-artists">
              <template v-for="(artist, i) in song.artists" :key="artist.mid">
                <span v-if="i > 0" class="artist-sep">/</span>
                <RouterLink
                  class="artist-link text-link"
                  :to="`/artist/${artist.mid}`"
                  @click.stop
                >{{ artist.name }}</RouterLink>
              </template>
            </span>
          </span>
          <RouterLink
            v-if="showAlbum"
            class="cell-album text-link"
            :to="`/album/${song.album.mid}`"
            :title="song.album.name"
            @click.stop
          >{{ song.album.name }}</RouterLink>
          <span class="cell-duration">{{ formatDuration(song.durationMs) }}</span>
        </button>
      </HoverItem>
      <!-- 播放图标：随滑动高亮一同纵向运动（不逐行 fade），停在序号位 -->
      <template #indicator>
        <span class="cell-play-float"><AppIcon :src="playIcon" /></span>
      </template>
    </HoverGroup>

    <div v-if="tracks.length === 0" class="table-empty">这里还没有歌曲</div>
  </div>
</template>

<style scoped>
.track-table {
  display: grid;
  gap: 2px;
  /* 负外边距放在表头/行共同的父级：两者同处外扩坐标系，
     表头内容才能与行内容两缘对齐（缺了它 "#" 与序号、时长与时长列错位 0.75rem）。
     外扩的 0.75rem 与行内边距互相抵消，内容两缘位置不变 */
  margin-inline: -0.75rem;
}

/* 行容器：HoverGroup 滑动高亮块接管行 hover（与侧栏导航同款）。
   外扩由父级 .track-table 统一负责，这里只管布局 */
.track-rows {
  display: grid;
  gap: 2px;
}

.track-rows :deep(.hover-item) {
  display: grid;
  border-radius: var(--radius-md);
}

.track-rows :deep(.hover-item > .table-row) {
  height: 100%;
}

.table-row {
  display: grid;
  grid-template-columns: 2.5rem minmax(0, 1fr) 4rem;
  grid-template-areas: "index title duration";
  align-items: center;
  gap: var(--space-4);
  width: 100%;
  /* 行内边距 0.75rem 让高亮带包住内容留出呼吸空隙；
     内容两缘位置由 .track-rows 的负外边距补回（见下），仍与节标题/更多对齐 */
  padding: 0.45rem 0.75rem;
  text-align: left;
  color: var(--foreground);
  border-radius: var(--radius-md);
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.track-table.with-album .table-row {
  grid-template-columns: 2.5rem minmax(0, 1.4fr) minmax(0, 0.8fr) 4rem;
  grid-template-areas: "index title album duration";
}

.cell-album {
  display: none;
}

.track-table.with-album .cell-album {
  display: block;
}

.table-head {
  color: var(--muted-foreground);
  font-size: 0.78rem;
  padding-bottom: 0.3rem;
}

.table-row.is-current .song-title {
  color: var(--track-accent);
  font-weight: 650;
}

.cell-index {
  grid-area: index;
  display: grid;
  place-items: center start;
  color: var(--muted-foreground);
  font-size: 0.85rem;
  font-variant-numeric: tabular-nums;
  transition: opacity var(--duration-fast) var(--ease-standard);
}

/* hover 时序号让位：图标由滑动块带过来（正在播放行是跳动条，不让位） */
.track-rows .table-row:not(.is-current):hover .cell-index {
  opacity: 0;
}

.cell-title {
  grid-area: title;
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
}

.song-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 0.95rem;
}

.song-artists {
  display: inline-flex;
  align-items: center;
  gap: 0.35em;
  overflow: hidden;
  white-space: nowrap;
  font-size: 0.8rem;
  color: var(--muted-foreground);
  flex: 0 1 auto;
  min-width: 0;
}

.artist-sep {
  opacity: 0.6;
}

.cell-album {
  grid-area: album;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 0.84rem;
}

.cell-duration {
  grid-area: duration;
  text-align: right;
  color: var(--muted-foreground);
  font-size: 0.84rem;
  font-variant-numeric: tabular-nums;
}

/* 播放图标层：随 HoverGroup 高亮块滑动，与序号同位（行内距 0.75rem）1:1 替换 */
.track-rows :deep(.hover-indicator) {
  left: 0.75rem;
  width: 2.5rem;
  display: grid;
  place-items: center start;
}

.cell-play-float {
  display: grid;
  place-items: center start;
  width: 100%;
  height: 100%;
  color: var(--foreground);
  transition: opacity var(--duration-fast) var(--ease-standard);
}

/* EQ 优先：滑到正在播放行上时图标淡出让位（进出列表的显隐由 indicator 外层负责） */
.track-rows:has(.table-row.is-current:hover) .cell-play-float {
  opacity: 0;
}

.cell-play-float .app-icon {
  width: 1rem;
  height: 1rem;
}

.table-empty {
  padding: var(--space-8) 0;
  text-align: center;
  color: var(--muted-foreground);
}

/* 窄窗（niri ⅓ 宽）：专辑列让位，保住标题可读 */
@media (max-width: 42rem) {
  .track-table.with-album .table-row {
    grid-template-columns: 2.5rem minmax(0, 1fr) 4rem;
    grid-template-areas: "index title duration";
  }

  .track-table.with-album .cell-album {
    display: none;
  }
}
</style>
