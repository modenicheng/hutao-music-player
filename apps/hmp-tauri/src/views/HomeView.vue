<script setup lang="ts">
import { computed, inject, onMounted, ref } from "vue";
import { api } from "../lib/api/index.ts";
import type { RecommendFeed } from "../lib/api/types.ts";
import { playerKey } from "../lib/player.ts";
import { songToQueueItem } from "../lib/browserPlayerBridge.ts";
import AppIcon from "../components/AppIcon.vue";
import CoverCard from "../components/CoverCard.vue";
import SectionHeader from "../components/SectionHeader.vue";
import TrackTable from "../components/TrackTable.vue";
import playIcon from "../assets/icons/play_arrow-rounded.svg?raw";
import pauseIcon from "../assets/icons/pause-rounded.svg?raw";

// player 由 App 提供，inject 可能拿不到（单测/隔离渲染），播放入口全部兜底
const player = inject(playerKey);

const feed = ref<RecommendFeed | null>(null);
const loading = ref(true);
const failed = ref(false);

onMounted(async () => {
  try {
    feed.value = await api.recommend.feed();
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
});

const greeting = computed(() => {
  const hour = new Date().getHours();
  if (hour < 5) return "夜深了";
  if (hour < 11) return "早上好";
  if (hour < 14) return "中午好";
  if (hour < 18) return "下午好";
  return "晚上好";
});

const guessTracks = computed(() => feed.value?.guessYouLike.slice(0, 6) ?? []);
const playlists = computed(() => feed.value?.playlists.slice(0, 8) ?? []);
const currentTrack = computed(() => player?.state.currentTrack ?? null);

function continuePlay() {
  if (!player) return;
  if (player.state.queue.length > 0) {
    player.togglePlay();
    return;
  }
  if (feed.value) {
    player.playTracks(feed.value.guessYouLike.map(songToQueueItem), 0);
  }
}

function formatCount(value: number): string {
  const trimOne = (n: number) => {
    const fixed = n.toFixed(1);
    return fixed.endsWith(".0") ? fixed.slice(0, -2) : fixed;
  };
  if (value >= 10_000) return `${trimOne(value / 10_000)}万`;
  return String(value);
}
</script>

<template>
  <div class="home-view">
    <div v-if="loading" class="state-hint">加载中…</div>
    <div v-else-if="failed" class="state-hint">加载失败</div>

    <template v-else-if="feed">
      <!-- 问候 + 日期 -->
      <header class="home-head">
        <h1 class="home-greeting">{{ greeting }}</h1>
        <p class="home-date">{{ feed.daily.date }} · 每日推荐已就绪</p>
      </header>

      <!-- 继续播放：接上次听到的地方；没有在播曲目时一键开播猜你喜欢 -->
      <section class="continue-card" aria-label="继续播放">
        <template v-if="currentTrack">
          <img
            v-if="currentTrack.coverUrl"
            class="continue-cover"
            :src="currentTrack.coverUrl"
            :alt="currentTrack.title"
          />
          <div class="continue-copy">
            <span class="continue-label">继续播放</span>
            <span class="continue-title">{{ currentTrack.title }}</span>
            <span class="continue-artists">{{ currentTrack.artists.join(" / ") }}</span>
          </div>
          <button
            class="continue-play"
            :title="player?.state.playing ? '暂停' : '播放'"
            @click="player?.togglePlay"
          >
            <AppIcon v-if="player" :src="player.state.playing ? pauseIcon : playIcon" />
          </button>
        </template>
        <template v-else>
          <div class="continue-copy">
            <span class="continue-label">开始聆听</span>
            <span class="continue-title">{{ feed.guessYouLike[0]?.title ?? "暂无推荐" }}</span>
            <span class="continue-artists">来自「猜你喜欢」的第一首</span>
          </div>
          <button class="continue-play" title="播放" @click="continuePlay">
            <AppIcon :src="playIcon" />
          </button>
        </template>
      </section>

      <section class="section">
        <SectionHeader title="猜你喜欢" more-to="/discover" />
        <TrackTable :tracks="guessTracks" :player="player" show-header />
      </section>

      <section class="section">
        <SectionHeader title="推荐歌单" more-to="/discover" />
        <div class="playlist-grid">
          <CoverCard
            v-for="playlist in playlists"
            :key="playlist.id"
            :cover-url="playlist.coverUrl"
            :title="playlist.name"
            :subtitle="`${formatCount(playlist.playCount)}次播放`"
            :to="`/playlist/${playlist.id}`"
          />
        </div>
      </section>
    </template>
  </div>
</template>

<style scoped>
.home-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
}

.state-hint {
  padding: var(--space-12) 0;
  text-align: center;
  color: var(--muted-foreground);
}

.home-head {
  margin-bottom: var(--space-6);
}

.home-greeting {
  font-size: clamp(1.6rem, 3vw, 2.1rem);
  font-weight: 700;
}

.home-date {
  margin-top: 0.3rem;
  font-size: 0.9rem;
  color: var(--muted-foreground);
  font-variant-numeric: tabular-nums;
}

/* —— 继续播放卡：中性表面卡，主题色只留给右侧播放键 —— */
.continue-card {
  display: flex;
  align-items: center;
  gap: var(--space-4);
  padding: var(--space-4) var(--space-5);
  background: var(--surface-3);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-sm);
}

.continue-cover {
  width: 4.5rem;
  height: 4.5rem;
  flex: 0 0 auto;
  object-fit: cover;
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-sm);
}

.continue-copy {
  display: grid;
  min-width: 0;
  flex: 1;
}

.continue-label {
  font-size: 0.78rem;
  color: var(--muted-foreground);
  font-weight: 600;
}

.continue-title {
  margin-top: 0.15rem;
  font-size: 1.15rem;
  font-weight: 650;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.continue-artists {
  font-size: 0.82rem;
  color: var(--muted-foreground);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.continue-play {
  display: grid;
  place-items: center;
  width: 3rem;
  height: 3rem;
  flex: 0 0 auto;
  color: var(--track-on-accent);
  background: var(--track-accent);
  border-radius: var(--radius-full);
  box-shadow: var(--shadow-sm);
  transition:
    transform var(--duration-fast) var(--ease-standard),
    box-shadow var(--duration-fast) var(--ease-standard);
}

.continue-play:hover {
  transform: scale(1.05);
  box-shadow: var(--shadow-md);
}

.continue-play .app-icon {
  width: 1.35rem;
  height: 1.35rem;
}

.section {
  margin-top: var(--space-8);
}

.section > .section-header {
  margin-bottom: var(--space-4);
}

.playlist-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(9rem, 1fr));
  gap: var(--space-4);
}
</style>
