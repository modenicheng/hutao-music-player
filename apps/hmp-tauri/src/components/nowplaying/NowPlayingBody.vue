<script setup lang="ts">
import { computed, ref, watch } from "vue";
import AppIcon from "../AppIcon.vue";
import RulerProgress from "../RulerProgress.vue";
import LyricsPane from "./LyricsPane.vue";
import CommentsSection from "./CommentsSection.vue";
import {
  PlayerControlStatus,
  type PlayerController,
} from "../../lib/player";
import { extractPaletteFromUrl } from "../../lib/color/adapter.ts";
import { themeState } from "../../lib/themeStore.ts";
import { api, type Lyrics } from "../../lib/api/index.ts";
import { findSong } from "../../lib/api/mock-data.ts";
import playIcon from "../../assets/icons/play_arrow-rounded.svg?raw";
import pauseIcon from "../../assets/icons/pause-rounded.svg?raw";
import skipNextIcon from "../../assets/icons/skip-next-rounded.svg?raw";
import skipPreviousIcon from "../../assets/icons/skip-previous-rounded.svg?raw";
import shuffleIcon from "../../assets/icons/shuffle-rounded.svg?raw";
import repeatIcon from "../../assets/icons/repeat-rounded.svg?raw";
import volumeIcon from "../../assets/icons/volume-up-rounded.svg?raw";
import queueIcon from "../../assets/icons/queue-music-rounded.svg?raw";
import favoriteIcon from "../../assets/icons/favorite-outline-rounded.svg?raw";
import commentIcon from "../../assets/icons/comment-rounded.svg?raw";

/**
 * 播放页主体（DESIGN.md §3.1）：环境层 + Hero + 刻度进度条 +
 * 控制排，向下滚动进入歌词区与网易云式评论区。
 * 全屏 overlay（PlayerOverlay）与路由页（NowPlayingView）共用本组件。
 */
const props = defineProps<{ player: PlayerController }>();

const theme = themeState();
const shuffleOn = ref(false);
const repeatOn = ref(false);
const lyrics = ref<Lyrics | null>(null);
const npRoot = ref<HTMLElement | null>(null);

const track = computed(() => props.player.state.currentTrack);
const songDetail = computed(() =>
  track.value ? findSong(track.value.mid) ?? null : null,
);

// —— 曲目层调色：封面变化 / 明暗主题切换时重取 ——
watch(
  () => [track.value?.coverUrl ?? null, theme.resolved] as const,
  async ([coverUrl, resolved]) => {
    const palette = coverUrl
      ? await extractPaletteFromUrl(coverUrl, { mode: resolved })
      : await extractPaletteFromUrl("", { mode: resolved });
    const rootStyle = document.documentElement.style;
    rootStyle.setProperty("--track-accent", palette.accent);
    rootStyle.setProperty("--track-on-accent", palette.onAccent);
    rootStyle.setProperty("--track-accent-soft", palette.accentSoft);
    rootStyle.setProperty("--track-deep", palette.deep);
    rootStyle.setProperty("--track-deep-fg", palette.deepFg);
    rootStyle.setProperty("--track-grad-from", palette.gradFrom);
    rootStyle.setProperty("--track-grad-to", palette.gradTo);
    rootStyle.setProperty("--track-equalizer", palette.accent);
  },
  { immediate: true },
);

// —— 歌词随曲目加载 ——
watch(
  () => track.value?.mid ?? null,
  async (mid) => {
    lyrics.value = null;
    if (!mid) return;
    try {
      lyrics.value = await api.lyrics.get(mid);
    } catch {
      lyrics.value = null;
    }
  },
  { immediate: true },
);

function scrollToLyrics() {
  npRoot.value
    ?.querySelector<HTMLElement>("#lyrics-anchor")
    ?.scrollIntoView({ behavior: "smooth", block: "start" });
}

function scrollToComments() {
  npRoot.value
    ?.querySelector<HTMLElement>("#comments-anchor")
    ?.scrollIntoView({ behavior: "smooth", block: "start" });
}
</script>

<template>
  <div ref="npRoot" class="now-playing">
    <!-- 环境层：渐变 + 封面放大模糊，光从音乐里透出来 -->
    <div class="ambient" aria-hidden="true">
      <img
        v-if="track?.coverUrl"
        class="ambient-cover"
        :src="track.coverUrl"
        alt=""
      />
      <div class="ambient-tint"></div>
    </div>

    <div class="np-scroll">
      <template v-if="track">
        <!-- Hero -->
        <section class="np-hero">
          <img
            v-if="track.coverUrl"
            class="np-cover"
            :src="track.coverUrl"
            :alt="`《${track.title}》专辑封面`"
          />
          <div class="np-meta">
            <h1 class="np-title">{{ track.title }}</h1>
            <p class="np-artists">
              <template v-for="(artist, i) in track.artists" :key="artist">
                <span v-if="i > 0" class="artist-sep">/</span>
                <span class="np-artist">{{ artist }}</span>
              </template>
            </p>
            <p v-if="songDetail" class="np-album">
              <RouterLink :to="`/album/${songDetail.album.mid}`" class="np-album-link">
                {{ songDetail.album.name }}
              </RouterLink>
              <span
                v-if="songDetail.quality"
                class="np-quality"
              >{{ songDetail.quality }}</span>
            </p>
            <div class="np-actions">
              <button class="np-action np-action-like" title="喜欢">
                <AppIcon :src="favoriteIcon" class="np-action-icon" />
                <span>喜欢</span>
              </button>
              <button class="np-action" title="收藏到歌单">
                <span>收藏</span>
              </button>
              <button class="np-action" title="下载">
                <span>下载</span>
              </button>
            </div>
          </div>
        </section>

        <!-- 刻度进度条（签名组件） -->
        <section class="np-ruler">
          <RulerProgress
            :progress="player.state.progress"
            :duration-ms="player.state.durationMs"
            :disabled="player.state.controlStatus === PlayerControlStatus.dragging"
            @seek="(percent) => player.seekToPercent(percent)"
          />
        </section>

        <!-- 控制排 -->
        <section class="np-controls">
          <div class="np-side np-side-left">
            <button
              class="mode-button"
              :class="{ 'is-on': shuffleOn }"
              :title="shuffleOn ? '随机播放：开' : '随机播放：关'"
              @click="shuffleOn = !shuffleOn"
            >
              <AppIcon :src="shuffleIcon" />
            </button>
          </div>

          <div class="np-main-controls">
            <button class="control-button" title="上一曲" @click="player.previous">
              <AppIcon :src="skipPreviousIcon" />
            </button>
            <button
              class="play-button"
              :title="player.state.playing ? '暂停' : '播放'"
              @click="player.togglePlay"
            >
              <AppIcon :src="player.state.playing ? pauseIcon : playIcon" />
            </button>
            <button class="control-button" title="下一曲" @click="player.next">
              <AppIcon :src="skipNextIcon" />
            </button>
          </div>

          <div class="np-side np-side-right">
            <button
              class="mode-button"
              :class="{ 'is-on': repeatOn }"
              :title="repeatOn ? '循环播放：开' : '循环播放：关'"
              @click="repeatOn = !repeatOn"
            >
              <AppIcon :src="repeatIcon" />
            </button>
          </div>
        </section>

        <section class="np-sub-controls">
          <div class="volume-group">
            <AppIcon :src="volumeIcon" class="volume-icon" />
            <input
              class="volume"
              type="range"
              min="0"
              max="1"
              step="0.01"
              aria-label="音量"
              :value="player.state.volume"
              @input="player.setVolume(($event.target as HTMLInputElement).valueAsNumber)"
            />
          </div>
          <div class="jump-group">
            <button class="jump-button" title="跳到歌词" @click="scrollToLyrics">
              歌词
            </button>
            <button class="jump-button" title="跳到评论" @click="scrollToComments">
              <AppIcon :src="commentIcon" class="jump-icon" />
              评论
            </button>
            <button class="jump-button" title="播放列表" @click="player.showQueue">
              <AppIcon :src="queueIcon" class="jump-icon" />
              列表
            </button>
          </div>
        </section>

        <!-- 歌词区 -->
        <section id="lyrics-anchor" class="np-lyrics">
          <LyricsPane
            :lyrics="lyrics"
            :position-ms="player.state.positionMs"
            @seek="(timeMs) => player.seek(timeMs)"
          />
        </section>

        <!-- 评论区（网易云式，向下滚动可见） -->
        <section id="comments-anchor" class="np-comments">
          <CommentsSection :mid="track.mid" />
        </section>
      </template>

      <div v-else class="np-empty">
        <p class="np-empty-title">还没有播放中的歌曲</p>
        <p class="np-empty-hint">从发现、搜索或歌单里挑一首开始吧</p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.now-playing {
  position: relative;
  height: 100%;
  min-height: 0;
  color: var(--foreground);
}

/* —— 环境层 —— */
.ambient {
  position: absolute;
  inset: 0;
  overflow: hidden;
  background: linear-gradient(180deg, var(--track-grad-from), var(--track-grad-to));
  transition: background var(--duration-slow) var(--ease-standard);
}

.ambient-cover {
  position: absolute;
  inset: -12%;
  width: 124%;
  height: 124%;
  object-fit: cover;
  filter: blur(80px) saturate(1.2);
  opacity: 0.25;
}

.ambient-tint {
  position: absolute;
  inset: 0;
  background: linear-gradient(180deg, transparent 40%, var(--track-grad-to) 100%);
}

.np-scroll {
  position: relative;
  z-index: 1;
  height: 100%;
  overflow-y: auto;
  scrollbar-width: thin;
}

/* —— Hero —— */
.np-hero {
  display: flex;
  align-items: center;
  gap: var(--space-8);
  max-width: 60rem;
  margin: 0 auto;
  padding: var(--space-10) var(--space-6) var(--space-6);
}

.np-cover {
  width: min(34vh, 320px);
  aspect-ratio: 1 / 1;
  flex: 0 0 auto;
  object-fit: cover;
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
}

.np-meta {
  min-width: 0;
}

.np-title {
  font-size: clamp(1.8rem, 4vw, 2.6rem);
  font-weight: 700;
  line-height: 1.25;
  overflow: hidden;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
}

.np-artists {
  margin-top: 0.5rem;
  font-size: 1.05rem;
  color: var(--muted-foreground);
}

.artist-sep {
  margin: 0 0.4em;
  opacity: 0.5;
}

.np-album {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--space-2);
  margin-top: 0.35rem;
  font-size: 0.92rem;
  color: var(--muted-foreground);
}

.np-album-link:hover {
  color: var(--foreground);
  text-decoration: underline;
}

.np-quality {
  padding: 0.05rem 0.5rem;
  font-size: 0.72rem;
  color: var(--track-accent);
  background: var(--track-accent-soft);
  border-radius: var(--radius-full);
  white-space: nowrap;
}

.np-actions {
  display: flex;
  gap: var(--space-2);
  margin-top: var(--space-5);
}

.np-action {
  display: inline-flex;
  align-items: center;
  gap: 0.35rem;
  padding: 0.4rem 1rem;
  font-size: 0.88rem;
  color: var(--foreground);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-full);
  transition:
    background-color var(--duration-fast) var(--ease-standard),
    border-color var(--duration-fast) var(--ease-standard);
}

.np-action-icon {
  width: 1rem;
  height: 1rem;
}

.np-action-like:hover {
  color: var(--track-accent);
}

/* —— 刻度进度条 —— */
.np-ruler {
  max-width: 60rem;
  margin: 0 auto;
  padding: var(--space-4) var(--space-6) 0;
}

/* —— 控制排 —— */
.np-controls {
  display: flex;
  align-items: center;
  justify-content: space-between;
  max-width: 60rem;
  margin: 0 auto;
  padding: var(--space-4) var(--space-6);
}

.np-side {
  flex: 1;
  display: flex;
}

.np-side-right {
  justify-content: flex-end;
}

.np-main-controls {
  display: flex;
  align-items: center;
  gap: var(--space-5);
}

.control-button {
  display: grid;
  place-items: center;
  width: 2.75rem;
  height: 2.75rem;
  color: var(--foreground);
  border-radius: var(--radius-full);
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.control-button .app-icon {
  width: 1.5rem;
  height: 1.5rem;
}

.control-button:hover {
  background: var(--track-accent-soft);
}

/* 主播放键：64px 圆，曲目层强调色 */
.play-button {
  display: grid;
  place-items: center;
  width: 4rem;
  height: 4rem;
  color: var(--track-on-accent);
  background: var(--track-accent);
  border-radius: var(--radius-full);
  box-shadow: var(--shadow-md);
  transition:
    transform var(--duration-fast) var(--ease-standard),
    box-shadow var(--duration-fast) var(--ease-standard),
    background-color var(--duration-normal) var(--ease-standard);
}

.play-button .app-icon {
  width: 1.75rem;
  height: 1.75rem;
}

.play-button:hover {
  transform: scale(1.04);
  box-shadow: var(--shadow-lg);
}

.play-button:active {
  transform: scale(0.98);
}

.mode-button {
  display: grid;
  place-items: center;
  width: 2.25rem;
  height: 2.25rem;
  color: var(--muted-foreground);
  border-radius: var(--radius-full);
  transition:
    color var(--duration-fast) var(--ease-standard),
    background-color var(--duration-fast) var(--ease-standard);
}

.mode-button .app-icon {
  width: 1.2rem;
  height: 1.2rem;
}

.mode-button:hover {
  color: var(--foreground);
  background: var(--track-accent-soft);
}

.mode-button.is-on {
  color: var(--track-accent);
}

/* —— 次级控制 —— */
.np-sub-controls {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  max-width: 60rem;
  margin: 0 auto;
  padding: 0 var(--space-6) var(--space-2);
}

.volume-group {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  color: var(--muted-foreground);
}

.volume-icon {
  width: 1.1rem;
  height: 1.1rem;
}

.volume {
  width: 8rem;
}

.jump-group {
  display: flex;
  gap: var(--space-2);
}

.jump-button {
  display: inline-flex;
  align-items: center;
  gap: 0.3rem;
  padding: 0.35rem 0.85rem;
  font-size: 0.84rem;
  color: var(--foreground);
  border-radius: var(--radius-full);
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.jump-icon {
  width: 1rem;
  height: 1rem;
}

.jump-button:hover {
  background: var(--track-accent-soft);
}

/* —— 歌词 / 评论 —— */
.np-lyrics {
  height: 72vh;
  margin-top: var(--space-4);
}

.np-comments {
  /* 评论区接在歌词之后，向下滚动自然抵达 */
  padding-top: var(--space-6);
  /* 让评论区落在中性底上，可读性优先 */
  background: var(--track-grad-to);
}

.np-empty {
  padding: var(--space-12) var(--space-6);
  text-align: center;
}

.np-empty-title {
  font-size: 1.3rem;
  font-weight: 650;
}

.np-empty-hint {
  margin-top: 0.5rem;
  color: var(--muted-foreground);
}

@media (max-width: 48rem) {
  .np-hero {
    flex-direction: column;
    text-align: center;
  }

  .np-album,
  .np-actions {
    justify-content: center;
  }
}
</style>
