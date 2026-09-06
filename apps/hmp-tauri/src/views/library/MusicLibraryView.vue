<script setup lang="ts">
import { computed, inject, onMounted, ref } from "vue";
import { api } from "../../lib/api/index.ts";
import type { LocalLibrary, LocalTrack } from "../../lib/api/types.ts";
import { formatBytes, formatLongDuration } from "../../lib/format.ts";
import { playerKey } from "../../lib/player.ts";
import { songToQueueItem } from "../../lib/browserPlayerBridge.ts";
import AppIcon from "../../components/AppIcon.vue";
import Button from "../../components/Button.vue";
import HoverGroup from "../../components/HoverGroup.vue";
import HoverItem from "../../components/HoverItem.vue";
import PageHeader from "../../components/PageHeader.vue";
import SectionHeader from "../../components/SectionHeader.vue";
import TrackTable from "../../components/TrackTable.vue";
import folderIcon from "../../assets/icons/folder-rounded.svg?raw";
import playIcon from "../../assets/icons/play_arrow-rounded.svg?raw";

// player 可能拿不到：播放入口与播放全部按钮兜底隐藏，页面照常渲染
const player = inject(playerKey);

const library = ref<LocalLibrary | null>(null);
const loading = ref(true);
const failed = ref(false);

onMounted(async () => {
  try {
    library.value = await api.library.local();
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
});

// —— 监视文件夹点击过滤：纯客户端过滤 mock 数据，再点一次或"显示全部"取消 ——
const selectedFolder = ref<string | null>(null);

const tracks = computed(() => library.value?.tracks ?? []);
const filteredTracks = computed<LocalTrack[]>(() => {
  if (!selectedFolder.value) return tracks.value;
  return tracks.value.filter((track) => track.folder === selectedFolder.value);
});

const shownFolder = computed(
  () => library.value?.folders.find((folder) => folder.path === selectedFolder.value) ?? null,
);

const tableTitle = computed(() => {
  if (!shownFolder.value) return "全部音乐";
  const basename = shownFolder.value.path.split("/").pop() ?? shownFolder.value.path;
  return `${basename} · ${shownFolder.value.trackCount} 首`;
});

function toggleFolder(path: string) {
  selectedFolder.value = selectedFolder.value === path ? null : path;
}

// —— 汇总统计（页头元信息行）——
const totalSizeBytes = computed(() => tracks.value.reduce((sum, track) => sum + track.sizeBytes, 0));
const totalDurationMs = computed(() => tracks.value.reduce((sum, track) => sum + track.durationMs, 0));

/** "2026-09-05 21:30" → "09-05 21:30"：行内只留短时间，年份归页头层级 */
function shortScanTime(iso: string): string {
  return iso.slice(5);
}

function playAll() {
  if (!player || filteredTracks.value.length === 0) return;
  player.playTracks(
    filteredTracks.value.map(songToQueueItem),
    0,
  );
}
</script>

<template>
  <div class="music-library-view">
    <div v-if="loading" class="state-hint">加载中…</div>
    <div v-else-if="failed" class="state-hint">加载失败</div>

    <template v-else>
      <PageHeader title="音乐库" back-to="/library">
        <template #meta>
          <span>{{ tracks.length }} 首</span>
          <span aria-hidden="true">·</span>
          <span>{{ formatLongDuration(totalDurationMs) }}</span>
          <span aria-hidden="true">·</span>
          <span>占用 {{ formatBytes(totalSizeBytes) }}</span>
        </template>
        <div class="header-actions">
          <Button
            v-if="player && filteredTracks.length > 0"
            variant="default"
            size="sm"
            @click="playAll"
          >
            <AppIcon :src="playIcon" class="action-icon" />
            播放全部
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled
            title="桌面端接入本地索引后可用"
          >
            扫描本地音乐
          </Button>
        </div>
      </PageHeader>

      <!-- 监视文件夹：点击行过滤下方曲目表，激活行常亮（与侧栏激活态同语言） -->
      <section class="section" aria-label="监视文件夹">
        <SectionHeader title="监视文件夹">
          <template #actions>
            <Button variant="ghost" size="sm" disabled title="桌面端接入本地索引后可用">
              添加文件夹
            </Button>
          </template>
        </SectionHeader>

        <HoverGroup class="folder-list" highlight-color="var(--neutral-200)" :highlight-opacity="0.6">
          <HoverItem v-for="folder in library?.folders" :key="folder.path">
            <button
              class="folder-row"
              :class="{ 'is-active': folder.path === selectedFolder }"
              :aria-pressed="folder.path === selectedFolder"
              :title="folder.path === selectedFolder ? '取消过滤' : '只看这个文件夹'"
              @click="toggleFolder(folder.path)"
            >
              <AppIcon :src="folderIcon" class="folder-icon" />
              <span class="folder-path">{{ folder.path }}</span>
              <span class="folder-meta">
                {{ folder.trackCount }} 首
                <span aria-hidden="true">·</span>
                {{ formatBytes(folder.sizeBytes) }}
                <span aria-hidden="true">·</span>
                {{ shortScanTime(folder.lastScanAt) }} 扫描
              </span>
            </button>
          </HoverItem>
        </HoverGroup>
      </section>

      <!-- 曲目表：随文件夹过滤切换标题与内容 -->
      <section class="section" aria-label="本地曲目">
        <SectionHeader :title="tableTitle">
          <template v-if="selectedFolder" #actions>
            <button class="show-all text-link" @click="selectedFolder = null">显示全部</button>
          </template>
        </SectionHeader>
        <TrackTable :tracks="filteredTracks" :player="player" show-header />
      </section>
    </template>
  </div>
</template>

<style scoped>
.music-library-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
}

.state-hint {
  padding: var(--space-12) 0;
  text-align: center;
  color: var(--muted-foreground);
}

.header-actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-left: auto;
}

.action-icon {
  width: 1.1rem;
  height: 1.1rem;
}

.section {
  margin-top: var(--space-8);
}

.section > .section-header {
  margin-bottom: var(--space-3);
}

/* —— 监视文件夹行：HoverGroup 滑动高亮，与曲目表同款中性底 —— */
.folder-list {
  display: grid;
  gap: 2px;
  margin-inline: -0.75rem;
}

.folder-list :deep(.hover-item) {
  display: flex;
  flex-direction: column;
  border-radius: var(--radius-md);
}

.folder-row {
  display: grid;
  grid-template-columns: 2.5rem minmax(0, 1fr) auto;
  grid-template-areas: "icon path meta";
  align-items: center;
  gap: var(--space-4);
  width: 100%;
  padding: 0.55rem 0.75rem;
  text-align: left;
  color: var(--foreground);
  border-radius: var(--radius-md);
}

/* 激活（过滤中）行常亮：中性底 + 前景色，与侧栏激活项同语言；
   主题色不进列表 */
.folder-row.is-active {
  background: var(--muted);
  font-weight: 600;
}

.folder-icon {
  grid-area: icon;
  width: 1.25rem;
  height: 1.25rem;
  justify-self: center;
  color: var(--muted-foreground);
}

.folder-row.is-active .folder-icon {
  color: var(--foreground);
}

.folder-path {
  grid-area: path;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 0.92rem;
}

.folder-meta {
  grid-area: meta;
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  color: var(--muted-foreground);
  font-size: 0.82rem;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.folder-row.is-active .folder-meta {
  font-weight: 400;
}

.show-all {
  font-size: 0.85rem;
  background: none;
  cursor: pointer;
}

/* 窄窗：文件夹元信息换行到路径下，保住路径完整可读 */
@media (max-width: 42rem) {
  .folder-row {
    grid-template-columns: 2.5rem minmax(0, 1fr);
    grid-template-areas:
      "icon path"
      "icon meta";
    row-gap: 0.15rem;
  }
}
</style>
