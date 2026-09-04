<script setup lang="ts">
import AppIcon from "./AppIcon.vue";
import playIcon from "../assets/icons/play_arrow-rounded.svg?raw";
import type { PlayerController } from "../lib/player";
import { songToQueueItem } from "../lib/browserPlayerBridge.ts";
import type { SongRef } from "../lib/api/types.ts";

/**
 * 曲目表（DESIGN.md §3.0）：序号/标题+歌手/专辑/操作/时长。
 * 整行点击 → 替换队列并从该行播放；当前行显示跳动条并高亮。
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

    <button
      v-for="(song, index) in tracks"
      :key="song.mid"
      class="table-row"
      :class="{ 'is-current': isCurrent(song) }"
      :title="`播放《${song.title}》`"
      @click="play(song, index)"
    >
      <span class="cell-index">
        <span v-if="isCurrent(song)" class="equalizer" :class="{ 'is-paused': !playingNow() }" aria-hidden="true">
          <i></i><i></i><i></i>
        </span>
        <template v-else>{{ index + 1 }}</template>
      </span>
      <span class="cell-title">
        <span class="song-title">{{ song.title }}</span>
        <span class="song-artists">
          <template v-for="(artist, i) in song.artists" :key="artist.mid">
            <span v-if="i > 0" class="artist-sep">/</span>
            <RouterLink
              class="artist-link"
              :to="`/artist/${artist.mid}`"
              @click.stop
            >{{ artist.name }}</RouterLink>
          </template>
        </span>
        <span v-if="song.quality" class="quality-badge">{{ song.quality }}</span>
      </span>
      <RouterLink
        v-if="showAlbum"
        class="cell-album"
        :to="`/album/${song.album.mid}`"
        :title="song.album.name"
        @click.stop
      >{{ song.album.name }}</RouterLink>
      <span class="cell-duration">{{ formatDuration(song.durationMs) }}</span>
      <span class="cell-hover-play" aria-hidden="true">
        <AppIcon :src="playIcon" />
      </span>
    </button>

    <div v-if="tracks.length === 0" class="table-empty">这里还没有歌曲</div>
  </div>
</template>

<style scoped>
.track-table {
  display: grid;
  gap: 2px;
}

.table-row {
  display: grid;
  grid-template-columns: 2.5rem minmax(0, 1fr) 4rem 1.5rem;
  grid-template-areas: "index title duration hover";
  align-items: center;
  gap: var(--space-4);
  width: 100%;
  padding: 0.45rem 0.75rem;
  text-align: left;
  color: var(--foreground);
  border-radius: var(--radius-md);
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.track-table.with-album .table-row {
  grid-template-columns: 2.5rem minmax(0, 1.4fr) minmax(0, 0.8fr) 4rem 1.5rem;
  grid-template-areas: "index title album duration hover";
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

.table-row:not(.table-head):hover {
  background: var(--track-accent-soft);
}

.table-row.is-current .song-title {
  color: var(--track-accent);
  font-weight: 650;
}

.cell-index {
  grid-area: index;
  display: grid;
  place-items: center;
  color: var(--muted-foreground);
  font-size: 0.85rem;
  font-variant-numeric: tabular-nums;
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

.artist-link:hover {
  color: var(--foreground);
  text-decoration: underline;
}

.quality-badge {
  flex: 0 0 auto;
  padding: 0.05rem 0.4rem;
  font-size: 0.68rem;
  color: var(--track-accent);
  background: var(--track-accent-soft);
  border-radius: var(--radius-full);
  white-space: nowrap;
}

.cell-album {
  grid-area: album;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 0.84rem;
  color: var(--muted-foreground);
}

.cell-album:hover {
  color: var(--foreground);
  text-decoration: underline;
}

.cell-duration {
  grid-area: duration;
  text-align: right;
  color: var(--muted-foreground);
  font-size: 0.84rem;
  font-variant-numeric: tabular-nums;
}

/* hover 时序号让位给播放图标 */
.cell-hover-play {
  grid-area: hover;
  display: grid;
  place-items: center;
  color: var(--foreground);
  opacity: 0;
  transition: opacity var(--duration-fast) var(--ease-standard);
}

.cell-hover-play .app-icon {
  width: 1rem;
  height: 1rem;
}

.table-row:not(.table-head):hover .cell-hover-play {
  opacity: 1;
}

.table-empty {
  padding: var(--space-8) 0;
  text-align: center;
  color: var(--muted-foreground);
}

/* 三根跳动条 */
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

.equalizer i:nth-child(1) { height: 60%; animation-delay: 0s; }
.equalizer i:nth-child(2) { height: 100%; animation-delay: 0.25s; }
.equalizer i:nth-child(3) { height: 45%; animation-delay: 0.5s; }

.equalizer.is-paused i {
  animation-play-state: paused;
}

@keyframes eq-bounce {
  0%, 100% { transform: scaleY(0.55); }
  50% { transform: scaleY(1); }
}

@media (prefers-reduced-motion: reduce) {
  .equalizer i { animation: none; }
}
</style>
