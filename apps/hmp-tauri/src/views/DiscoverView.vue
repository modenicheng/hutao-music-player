<script setup lang="ts">
import { inject, onMounted, ref } from "vue";
import { api } from "../lib/api/index.ts";
import type { RecommendFeed } from "../lib/api/types.ts";
import { playerKey } from "../lib/player.ts";
import { songToQueueItem } from "../lib/browserPlayerBridge.ts";
import CoverCard from "../components/CoverCard.vue";
import Scroll from "../components/Scroll.vue";
import SectionHeader from "../components/SectionHeader.vue";
import TrackTable from "../components/TrackTable.vue";

// player 由 App 提供，inject 可能拿不到（单测/隔离渲染），所有播放入口都要兜底
const player = inject(playerKey);

const feed = ref<RecommendFeed | null>(null);
const loading = ref(true);
const failed = ref(false);

onMounted(async () => {
  try {
    feed.value = await api.recommend.feed();
  } catch {
    // mock 层 reject 与未来真实接口 5xx 走同一文案
    failed.value = true;
  } finally {
    loading.value = false;
  }
});

/** 每日推荐 hero：把 "2026-09-05" 拆成年份 + 月日两段做大小字排印 */
function splitDailyDate(date: string) {
  const [year = "", month = "", day = ""] = date.split("-");
  return { year, monthDay: month && day ? `${month}.${day}` : date };
}

function playNewSong(index: number) {
  if (!player || !feed.value) return;
  player.playTracks(
    feed.value.newSongs.map(songToQueueItem),
    index,
  );
}

function formatCount(value: number): string {
  const trimOne = (n: number) => {
    const fixed = n.toFixed(1);
    return fixed.endsWith(".0") ? fixed.slice(0, -2) : fixed;
  };
  if (value >= 100_000_000) return `${trimOne(value / 100_000_000)}亿`;
  if (value >= 10_000) return `${trimOne(value / 10_000)}万`;
  return String(value);
}
</script>

<template>
  <div class="discover-view">
    <div v-if="loading" class="state-hint">加载中…</div>
    <div v-else-if="failed" class="state-hint">加载失败</div>

    <template v-else-if="feed">
      <!-- 每日推荐 hero：日期大字排印；本期不做点击动作，故渲染为纯卡片 -->
      <section class="daily-hero" aria-label="每日推荐">
        <img class="daily-cover" :src="feed.daily.coverUrl" :alt="feed.daily.title" />
        <div class="daily-scrim" aria-hidden="true"></div>
        <div class="daily-copy">
          <div class="daily-title">{{ feed.daily.title }}</div>
          <div class="daily-date">
            <span class="daily-year">{{ splitDailyDate(feed.daily.date).year }}</span>
            <span class="daily-month-day">{{ splitDailyDate(feed.daily.date).monthDay }}</span>
          </div>
        </div>
      </section>

      <section class="section">
        <SectionHeader title="猜你喜欢" />
        <TrackTable :tracks="feed.guessYouLike" :player="player" show-header />
      </section>

      <section class="section">
        <SectionHeader title="新歌速递" />
        <Scroll direction="horizontal" height="15rem" class="new-song-scroll">
          <div class="new-song-row">
            <CoverCard
              v-for="(song, index) in feed.newSongs"
              :key="song.mid"
              class="new-song-card"
              :cover-url="song.album.picUrl"
              :title="song.title"
              :subtitle="song.artists.map((artist) => artist.name).join(' / ')"
              playable
              @play="playNewSong(index)"
            />
          </div>
        </Scroll>
      </section>

      <section class="section">
        <SectionHeader title="排行榜精选" more-to="/top" />
        <div class="top-grid">
          <RouterLink
            v-for="chart in feed.topCharts"
            :key="chart.id"
            :to="`/top/${chart.id}`"
            class="top-card"
          >
            <span class="top-cover">
              <img :src="chart.picUrl" :alt="chart.name" loading="lazy" />
            </span>
            <span class="top-name">{{ chart.name }}</span>
            <span class="top-meta">{{ chart.updateTime }} · {{ chart.trackCount }}首</span>
          </RouterLink>
        </div>
      </section>

      <section class="section">
        <SectionHeader title="推荐歌单" />
        <div class="playlist-grid">
          <CoverCard
            v-for="playlist in feed.playlists"
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
.discover-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
}

.state-hint {
  padding: var(--space-12) 0;
  text-align: center;
  color: var(--muted-foreground);
}

/* —— 每日推荐 hero —— */
.daily-hero {
  position: relative;
  display: flex;
  align-items: center;
  min-height: 11rem;
  margin-bottom: var(--space-8);
  border-radius: var(--radius-lg);
  overflow: hidden;
  box-shadow: var(--shadow-md);
}

.daily-cover {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

/* 左侧渐变遮罩：保证日期大字在任何封面上都可读 */
.daily-scrim {
  position: absolute;
  inset: 0;
  background: linear-gradient(90deg, var(--surface-2) 30%, transparent 78%);
}

.daily-copy {
  position: relative;
  padding: var(--space-6) var(--space-8);
}

.daily-title {
  font-size: 1rem;
  font-weight: 650;
  letter-spacing: 0.08em;
  color: var(--muted-foreground);
}

.daily-date {
  display: flex;
  align-items: baseline;
  gap: var(--space-3);
  margin-top: var(--space-2);
  color: var(--foreground);
}

.daily-year {
  font-size: clamp(1.2rem, 2.4vw, 1.6rem);
  font-weight: 550;
  color: var(--muted-foreground);
  font-variant-numeric: tabular-nums;
}

.daily-month-day {
  font-size: clamp(2.8rem, 6vw, 4rem);
  font-weight: 750;
  line-height: 1;
  letter-spacing: 0.02em;
  font-variant-numeric: tabular-nums;
}

/* —— 区块节奏 —— */
.section {
  margin-top: var(--space-8);
}

.section > .section-header {
  margin-bottom: var(--space-4);
}

/* —— 新歌速递横排 —— */
.new-song-row {
  display: grid;
  grid-auto-flow: column;
  grid-auto-columns: 9.5rem;
  gap: var(--space-4);
}

.new-song-scroll {
  margin-top: var(--space-4);
}

/* —— 榜单卡（允许矩形封面） —— */
.top-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(10rem, 1fr));
  gap: var(--space-4);
  margin-top: var(--space-4);
}

.top-card {
  display: block;
  min-width: 0;
  transition: transform var(--duration-normal) var(--ease-standard);
}

.top-card:hover {
  transform: translateY(-2px);
}

.top-cover {
  display: block;
  aspect-ratio: 4 / 3;
  border-radius: var(--radius-lg);
  overflow: hidden;
  box-shadow: var(--shadow-md);
  transition: box-shadow var(--duration-normal) var(--ease-standard);
}

.top-card:hover .top-cover {
  box-shadow: var(--shadow-lg);
}

.top-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.top-name {
  display: block;
  margin-top: 0.55rem;
  font-size: 0.92rem;
  font-weight: 550;
}

.top-meta {
  display: block;
  margin-top: 0.15rem;
  font-size: 0.8rem;
  color: var(--muted-foreground);
  font-variant-numeric: tabular-nums;
}

/* —— 推荐歌单网格 —— */
.playlist-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(9rem, 1fr));
  gap: var(--space-4);
  margin-top: var(--space-4);
}
</style>
