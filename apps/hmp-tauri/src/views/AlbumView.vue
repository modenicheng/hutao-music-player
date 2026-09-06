<script setup lang="ts">
import { inject, onMounted, ref, watch } from "vue";
import { api } from "../lib/api/index.ts";
import type { AlbumDetail } from "../lib/api/types.ts";
import { playerKey } from "../lib/player.ts";
import Button from "../components/Button.vue";
import PageHeader from "../components/PageHeader.vue";
import TrackTable from "../components/TrackTable.vue";

// 路由以 props: true 传入 :id；同组件在不同专辑间复用时靠 watch 重新拉数据
const props = defineProps<{ id: string }>();

// player 可能拿不到（inject 无默认值）：播放交给 TrackTable 内部兜底，页面照常渲染
const player = inject(playerKey);

const detail = ref<AlbumDetail | null>(null);
const loading = ref(true);
const failed = ref(false);

async function load() {
  loading.value = true;
  failed.value = false;
  try {
    detail.value = await api.album.detail(props.id);
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
}

onMounted(load);
watch(() => props.id, load);

// —— 简介折叠 ——
// mock 文案长度不定：超过 2 行才给"展开"按钮（阈值按中文字符数粗估，两行约 60 字）
const descExpanded = ref(false);
const DESC_FOLD_THRESHOLD = 60;

// —— 收藏（mock：仅本地 toggle，接账号体系后换成接口调用） ——
const faved = ref(false);

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
  <div class="album-view">
    <div v-if="loading" class="state-hint">加载中…</div>
    <div v-else-if="failed" class="state-hint">加载失败</div>

    <div v-else-if="detail" class="album-content">
      <div class="album-head">
        <img class="album-cover" :src="detail.picUrl" :alt="detail.name" />

        <div class="album-info">
          <PageHeader :title="detail.name">
            <template #meta>
              <RouterLink
                class="meta-link text-link"
                :to="`/artist/${detail.artist.mid}`"
              >{{ detail.artist.name }}</RouterLink>
              <span aria-hidden="true">·</span>
              <span>{{ detail.releaseDate }}</span>
              <span aria-hidden="true">·</span>
              <span>{{ detail.company }}</span>
              <span aria-hidden="true">·</span>
              <span>{{ detail.songs.length }}首</span>
            </template>
          </PageHeader>

          <p class="album-desc" :class="{ 'is-clamped': !descExpanded }">{{ detail.desc }}</p>
          <button
            v-if="detail.desc.length > DESC_FOLD_THRESHOLD"
            class="desc-toggle text-link"
            @click="descExpanded = !descExpanded"
          >
            {{ descExpanded ? "收起" : "展开" }}
          </button>

          <div class="album-actions">
            <Button
              variant="outline"
              size="sm"
              :class="{ 'is-faved': faved }"
              @click="faved = !faved"
            >
              {{ faved ? "已收藏" : "收藏专辑" }}
            </Button>
            <span class="fav-count">{{ formatCount(detail.favCount) }} 人收藏</span>
          </div>
        </div>
      </div>

      <!-- 专辑内曲目同属一张专辑，专辑列冗余，关掉让标题列更宽；表格通栏全宽 -->
      <TrackTable
        class="album-songs"
        :tracks="detail.songs"
        :player="player"
        :show-album="false"
        show-header
      />
    </div>
  </div>
</template>

<style scoped>
.album-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
}

.state-hint {
  padding: var(--space-12) 0;
  text-align: center;
  color: var(--muted-foreground);
}

/* 两段式：头部（封面 | 信息）与曲目表分离，表格通栏占满内容列 */
.album-head {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  gap: var(--space-5) var(--space-8);
  align-items: center;
}

.album-cover {
  width: clamp(11rem, 18vw, 14rem);
  aspect-ratio: 1 / 1;
  object-fit: cover;
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
}

.album-info {
  min-width: 0;
}

.album-info :deep(.page-header) {
  padding: 0 0 var(--space-3);
}

/* 窄窗（niri ⅓ 宽）：头部竖排，列表保持全宽 */
@media (max-width: 48rem) {
  .album-head {
    grid-template-columns: 1fr;
    gap: var(--space-4);
  }

  .album-cover {
    width: clamp(10rem, 36vw, 13rem);
  }
}

/* 简介折叠：clamp 两行，展开后完整显示 */
.album-desc {
  color: var(--muted-foreground);
  font-size: 0.9rem;
  line-height: 1.7;
}

.album-desc.is-clamped {
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.desc-toggle {
  margin-top: var(--space-1);
  font-size: 0.82rem;
}

.album-actions {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-top: var(--space-3);
}

/* 已收藏态：中性选中底，主题色不进信息区 */
.album-actions .is-faved {
  color: var(--foreground);
  background: var(--muted);
  border-color: transparent;
}

.fav-count {
  font-size: 0.85rem;
  color: var(--muted-foreground);
  font-variant-numeric: tabular-nums;
}

.album-songs {
  margin-top: var(--space-6);
}
</style>
