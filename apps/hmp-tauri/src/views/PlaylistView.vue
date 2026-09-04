<script setup lang="ts">
import { inject, onMounted, ref, watch } from "vue";
import { api } from "../lib/api/index.ts";
import type { PlaylistDetail } from "../lib/api/types.ts";
import { playerKey } from "../lib/player.ts";
import { songToQueueItem } from "../lib/browserPlayerBridge.ts";
import AppIcon from "../components/AppIcon.vue";
import Button from "../components/Button.vue";
import PageHeader from "../components/PageHeader.vue";
import TrackTable from "../components/TrackTable.vue";
import playIcon from "../assets/icons/play_arrow-rounded.svg?raw";

// 路由以 props: true 传入 :id；同组件在不同歌单间复用时靠 watch 重新拉数据
const props = defineProps<{ id: string }>();

// player 可能拿不到：隐藏"播放全部"入口，页面其余部分照常渲染
const player = inject(playerKey);

const detail = ref<PlaylistDetail | null>(null);
const loading = ref(true);
const failed = ref(false);

async function load() {
  loading.value = true;
  failed.value = false;
  try {
    detail.value = await api.playlist.detail(props.id);
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
}

onMounted(load);
watch(() => props.id, load);

// —— 简介折叠（与专辑页同一交互：超过 2 行给"展开"） ——
const descExpanded = ref(false);
const DESC_FOLD_THRESHOLD = 60;

function playAll() {
  if (!player || !detail.value) return;
  player.playTracks(
    detail.value.songs.map(songToQueueItem),
    0,
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
  <div class="playlist-view">
    <div v-if="loading" class="state-hint">加载中…</div>
    <div v-else-if="failed" class="state-hint">加载失败</div>

    <div v-else-if="detail" class="playlist-layout">
      <img class="playlist-cover" :src="detail.coverUrl" :alt="detail.name" />

      <div class="playlist-body">
        <PageHeader :title="detail.name">
          <template #meta>
            <span class="meta-creator">
              <img class="creator-avatar" :src="detail.creator.avatarUrl" :alt="detail.creator.name" />
              {{ detail.creator.name }}
            </span>
            <span aria-hidden="true">·</span>
            <span
              v-for="tag in detail.tags"
              :key="tag"
              class="tag-chip"
            >{{ tag }}</span>
            <span aria-hidden="true">·</span>
            <span>{{ formatCount(detail.playCount) }}次播放</span>
            <span aria-hidden="true">·</span>
            <span>{{ detail.songs.length }}首</span>
          </template>
        </PageHeader>

        <p class="playlist-desc" :class="{ 'is-clamped': !descExpanded }">{{ detail.desc }}</p>
        <button
          v-if="detail.desc.length > DESC_FOLD_THRESHOLD"
          class="desc-toggle"
          @click="descExpanded = !descExpanded"
        >
          {{ descExpanded ? "收起" : "展开" }}
        </button>

        <!-- 播放全部：整单替换队列并从头播；无 player 时隐藏入口 -->
        <div class="playlist-actions">
          <Button v-if="player" variant="default" size="sm" @click="playAll">
            <AppIcon :src="playIcon" class="play-all-icon" />
            播放全部
          </Button>
        </div>

        <TrackTable class="playlist-songs" :tracks="detail.songs" :player="player" show-header />
      </div>
    </div>
  </div>
</template>

<style scoped>
.playlist-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
}

.state-hint {
  padding: var(--space-12) 0;
  text-align: center;
  color: var(--muted-foreground);
}

.playlist-layout {
  display: flex;
  align-items: flex-start;
  gap: var(--space-8);
}

.playlist-cover {
  width: clamp(11rem, 20vw, 15rem);
  aspect-ratio: 1 / 1;
  flex: 0 0 auto;
  object-fit: cover;
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
}

.playlist-body {
  flex: 1;
  min-width: 0;
}

.playlist-body :deep(.page-header) {
  padding-top: 0;
}

.meta-creator {
  display: inline-flex;
  align-items: center;
  gap: 0.4em;
}

.creator-avatar {
  width: 1.4em;
  height: 1.4em;
  border-radius: var(--radius-full);
  object-fit: cover;
}

/* 标签 chips：与 TrackTable 音质徽章同族（accent-soft 底），视觉语言一致 */
.tag-chip {
  padding: 0.05rem 0.5rem;
  font-size: 0.72rem;
  color: var(--track-accent);
  background: var(--track-accent-soft);
  border-radius: var(--radius-full);
  white-space: nowrap;
}

.playlist-desc {
  color: var(--muted-foreground);
  font-size: 0.9rem;
  line-height: 1.7;
}

.playlist-desc.is-clamped {
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.desc-toggle {
  margin-top: var(--space-1);
  font-size: 0.82rem;
  color: var(--muted-foreground);
  transition: color var(--duration-fast) var(--ease-standard);
}

.desc-toggle:hover {
  color: var(--foreground);
}

.playlist-actions {
  display: flex;
  align-items: center;
  margin-top: var(--space-4);
}

.play-all-icon {
  width: 1.1rem;
  height: 1.1rem;
}

.playlist-songs {
  margin-top: var(--space-6);
}
</style>
