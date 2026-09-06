<script setup lang="ts">
import { computed, inject, onMounted, ref } from "vue";
import { api } from "../../lib/api/index.ts";
import type { PlaylistRef, SongRef } from "../../lib/api/types.ts";
import { songPool } from "../../lib/api/mock-data.ts";
import { playerKey } from "../../lib/player.ts";
import { songToQueueItem } from "../../lib/browserPlayerBridge.ts";
import AppIcon from "../../components/AppIcon.vue";
import Button from "../../components/Button.vue";
import CoverCard from "../../components/CoverCard.vue";
import PageHeader from "../../components/PageHeader.vue";
import SectionHeader from "../../components/SectionHeader.vue";
import TrackTable from "../../components/TrackTable.vue";
import favoriteIcon from "../../assets/icons/favorite-filled-rounded.svg?raw";
import playIcon from "../../assets/icons/play_arrow-rounded.svg?raw";

// player 可能拿不到：播放入口与播放全部按钮兜底隐藏，页面照常渲染
const player = inject(playerKey);

const liked = ref<SongRef[]>([]);
const created = ref<PlaylistRef[]>([]);
const favorited = ref<PlaylistRef[]>([]);
const loading = ref(true);
const failed = ref(false);

onMounted(async () => {
  try {
    // 三个请求相互独立，一并发起；任一失败整页报错（与首页同策略）
    const [likedRes, createdRes, favoritedRes] = await Promise.all([
      api.library.liked(),
      api.library.created(),
      api.library.favorited(),
    ]);
    liked.value = likedRes;
    created.value = createdRes;
    favorited.value = favoritedRes;
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
});

// 最近播放预览：后端 recent_plays 未接线，与最近播放页同源（总池前几首）
const RECENT_PREVIEW_SIZE = 5;
const recentPreview = songPool.slice(0, RECENT_PREVIEW_SIZE);

const playlistCount = computed(() => created.value.length + favorited.value.length);

function playLiked() {
  if (!player || liked.value.length === 0) return;
  player.playTracks(
    liked.value.map(songToQueueItem),
    0,
  );
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
  <div class="library-view">
    <div v-if="loading" class="state-hint">加载中…</div>
    <div v-else-if="failed" class="state-hint">加载失败</div>

    <template v-else>
      <PageHeader title="我喜欢">
        <template #meta>
          <span>{{ liked.length }} 首喜欢的歌</span>
          <span aria-hidden="true">·</span>
          <span>{{ playlistCount }} 个歌单</span>
        </template>
      </PageHeader>

      <!-- 我喜欢：hero 卡 + 喜欢列表 -->
      <section class="liked-section" aria-label="我喜欢">
        <div class="liked-card">
          <span class="liked-cover" aria-hidden="true">
            <AppIcon :src="favoriteIcon" />
          </span>
          <div class="liked-copy">
            <span class="liked-label">我喜欢</span>
            <span class="liked-title">私藏的心动瞬间</span>
            <span class="liked-meta">{{ liked.length }} 首</span>
          </div>
          <Button v-if="player && liked.length > 0" variant="default" size="sm" @click="playLiked">
            <AppIcon :src="playIcon" class="play-all-icon" />
            播放全部
          </Button>
        </div>

        <TrackTable v-if="liked.length > 0" class="liked-songs" :tracks="liked" :player="player" show-header />
      </section>

      <!-- 最近播放：预览 5 首，整页入口在最近播放页 -->
      <section v-if="recentPreview.length > 0" class="section" aria-label="最近播放">
        <SectionHeader title="最近播放" more-to="/library/recent" />
        <TrackTable :tracks="recentPreview" :player="player" show-header />
      </section>

      <!-- 创建的歌单 -->
      <section v-if="created.length > 0" class="section" aria-label="创建的歌单">
        <SectionHeader title="创建的歌单" />
        <div class="playlist-grid">
          <CoverCard
            v-for="playlist in created"
            :key="playlist.id"
            :cover-url="playlist.coverUrl"
            :title="playlist.name"
            :subtitle="`${formatCount(playlist.playCount)}次播放`"
            :to="`/playlist/${playlist.id}`"
          />
        </div>
      </section>

      <!-- 收藏的歌单 -->
      <section v-if="favorited.length > 0" class="section" aria-label="收藏的歌单">
        <SectionHeader title="收藏的歌单" />
        <div class="playlist-grid">
          <CoverCard
            v-for="playlist in favorited"
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
.library-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
}

.state-hint {
  padding: var(--space-12) 0;
  text-align: center;
  color: var(--muted-foreground);
}

/* —— 我喜欢 hero 卡：中性表面卡，主题色只留给播放键 —— */
.liked-card {
  display: flex;
  align-items: center;
  gap: var(--space-4);
  padding: var(--space-4) var(--space-5);
  background: var(--surface-3);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-sm);
}

/* 与侧栏"喜欢的收藏"封面同一渐变，保持身份一致 */
.liked-cover {
  display: grid;
  place-items: center;
  width: 4.5rem;
  height: 4.5rem;
  flex: 0 0 auto;
  color: var(--surface-3);
  background: linear-gradient(135deg, #7660a4, #ef9d9d);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-sm);
}

.liked-cover .app-icon {
  width: 1.6rem;
  height: 1.6rem;
}

.liked-copy {
  display: grid;
  min-width: 0;
  flex: 1;
}

.liked-label {
  font-size: 0.78rem;
  color: var(--muted-foreground);
  font-weight: 600;
}

.liked-title {
  margin-top: 0.15rem;
  font-size: 1.15rem;
  font-weight: 650;
}

.liked-meta {
  font-size: 0.82rem;
  color: var(--muted-foreground);
  font-variant-numeric: tabular-nums;
}

.play-all-icon {
  width: 1.1rem;
  height: 1.1rem;
}

.liked-songs {
  margin-top: var(--space-4);
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
