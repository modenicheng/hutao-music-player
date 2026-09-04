<script setup lang="ts">
import { computed, inject, onMounted, ref, watch } from "vue";
import { api, coverUrl } from "../lib/api/index.ts";
import type { SearchResults } from "../lib/api/types.ts";
import { playerKey } from "../lib/player.ts";
import AppIcon from "../components/AppIcon.vue";
import CoverCard from "../components/CoverCard.vue";
import TrackTable from "../components/TrackTable.vue";
import searchIcon from "../assets/icons/search-rounded.svg?raw";

const player = inject(playerKey);

const keyword = ref("");
const inputEl = ref<HTMLInputElement | null>(null);

const results = ref<SearchResults | null>(null);
const searching = ref(false);
const failed = ref(false);

const trimmedKeyword = computed(() => keyword.value.trim());

onMounted(() => {
  // 大搜索框自动聚焦（DESIGN.md §3.2）：进页即打字
  inputEl.value?.focus();
});

// 输入防抖 300ms 再查；mock 延迟虽短，仍按真实异步对待（seq 只采纳最后一次结果）
const DEBOUNCE_MS = 300;
let debounceTimer: ReturnType<typeof setTimeout> | null = null;
let requestSeq = 0;

watch(keyword, (value) => {
  if (debounceTimer !== null) clearTimeout(debounceTimer);
  if (!value.trim()) {
    // 空关键词回到提示态，不保留旧结果
    results.value = null;
    searching.value = false;
    failed.value = false;
    return;
  }
  searching.value = true;
  debounceTimer = setTimeout(() => void search(value.trim()), DEBOUNCE_MS);
});

async function search(text: string) {
  const seq = ++requestSeq;
  try {
    const data = await api.search.quick(text);
    if (seq !== requestSeq) return;
    results.value = data;
    failed.value = false;
  } catch {
    if (seq !== requestSeq) return;
    failed.value = true;
  } finally {
    if (seq === requestSeq) searching.value = false;
  }
}

// —— 三个结果 tab ——
const tabs = [
  { key: "songs", label: "歌曲" },
  { key: "albums", label: "专辑" },
  { key: "artists", label: "歌手" },
] as const;
type TabKey = (typeof tabs)[number]["key"];
const activeTab = ref<TabKey>("songs");

function countOf(key: TabKey): number {
  if (!results.value) return 0;
  if (key === "songs") return results.value.songs.length;
  if (key === "albums") return results.value.albums.length;
  return results.value.artists.length;
}

const totalCount = computed(() =>
  results.value === null
    ? 0
    : results.value.songs.length + results.value.albums.length + results.value.artists.length,
);

// 当前 tab 的结果数：0 时给"该分类下暂无结果"的兜底，避免切 tab 出现空白区
const activeCount = computed(() => (results.value ? countOf(activeTab.value) : 0));
</script>

<template>
  <div class="search-view">
    <div class="search-box">
      <AppIcon :src="searchIcon" class="search-icon" />
      <input
        ref="inputEl"
        v-model="keyword"
        class="search-input"
        type="text"
        placeholder="搜索歌曲、专辑、歌手"
      >
    </div>

    <!-- 空关键词提示态 -->
    <div v-if="!trimmedKeyword" class="state-hint">输入关键词，搜索歌曲、专辑或歌手</div>

    <template v-else>
      <div class="search-tabs" role="tablist" aria-label="搜索结果分类">
        <button
          v-for="tab in tabs"
          :key="tab.key"
          class="search-tab"
          :class="{ 'is-active': activeTab === tab.key }"
          role="tab"
          :aria-selected="activeTab === tab.key"
          @click="activeTab = tab.key"
        >
          {{ tab.label }}
          <span class="tab-badge">{{ countOf(tab.key) }}</span>
        </button>
        <span v-if="searching" class="searching-hint">搜索中…</span>
      </div>

      <div v-if="failed" class="state-hint">加载失败</div>
      <!-- 无结果空态：三个分区都为空才提示 -->
      <div v-else-if="results !== null && totalCount === 0" class="state-hint">
        未找到与“{{ trimmedKeyword }}”相关的结果，换个关键词试试
      </div>

      <template v-else-if="results">
        <!-- 歌曲结果 -->
        <TrackTable
          v-if="activeTab === 'songs' && activeCount > 0"
          :tracks="results.songs"
          :player="player"
          show-header
        />

        <!-- 专辑结果 -->
        <div
          v-if="activeTab === 'albums' && activeCount > 0"
          class="album-grid"
        >
          <CoverCard
            v-for="album in results.albums"
            :key="album.mid"
            :cover-url="album.picUrl"
            :title="album.name"
            :subtitle="album.artist.name"
            :to="`/album/${album.mid}`"
          />
        </div>

        <!-- 歌手结果：圆形卡列表，封面与歌手页同 seed，图片一致 -->
        <div
          v-if="activeTab === 'artists' && activeCount > 0"
          class="artist-grid"
        >
          <RouterLink
            v-for="artist in results.artists"
            :key="artist.mid"
            class="artist-card"
            :to="`/artist/${artist.mid}`"
          >
            <img class="artist-avatar" :src="coverUrl(`artist:${artist.mid}`)" :alt="artist.name" loading="lazy" />
            <span class="artist-name">{{ artist.name }}</span>
          </RouterLink>
        </div>

        <div v-if="activeCount === 0" class="state-hint">该分类下没有相关结果</div>
      </template>
    </template>
  </div>
</template>

<style scoped>
.search-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
}

/* —— 大搜索框 —— */
.search-box {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  max-width: 34rem;
  padding: 0 var(--space-5);
  background: var(--input);
  border: 1px solid transparent;
  border-radius: var(--radius-full);
  transition: border-color var(--duration-fast) var(--ease-standard);
}

.search-box:focus-within {
  border-color: var(--ring);
}

.search-icon {
  width: 1.2rem;
  height: 1.2rem;
  flex: 0 0 auto;
  color: var(--muted-foreground);
}

.search-input {
  flex: 1;
  min-width: 0;
  height: var(--control-height-lg);
  font-size: 1rem;
  color: var(--foreground);
}

.search-input::placeholder {
  color: var(--muted-foreground);
}

.state-hint {
  padding: var(--space-12) 0;
  text-align: center;
  color: var(--muted-foreground);
}

/* —— 结果 tab —— */
.search-tabs {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin: var(--space-6) 0 var(--space-4);
}

.search-tab {
  display: inline-flex;
  align-items: center;
  gap: 0.4em;
  padding: 0.3rem 0.85rem;
  font-size: 0.9rem;
  color: var(--muted-foreground);
  border-radius: var(--radius-full);
  transition:
    background-color var(--duration-fast) var(--ease-standard),
    color var(--duration-fast) var(--ease-standard);
}

.search-tab:hover {
  color: var(--foreground);
}

.search-tab.is-active {
  color: var(--foreground);
  background: var(--track-accent-soft);
}

.tab-badge {
  font-size: 0.72rem;
  font-variant-numeric: tabular-nums;
  color: var(--muted-foreground);
}

.search-tab.is-active .tab-badge {
  color: var(--track-accent);
}

.searching-hint {
  margin-left: var(--space-2);
  font-size: 0.82rem;
  color: var(--muted-foreground);
}

/* —— 结果区 —— */
.album-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(9rem, 1fr));
  gap: var(--space-4);
}

.artist-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(7.5rem, 1fr));
  gap: var(--space-5);
}

.artist-card {
  display: grid;
  justify-items: center;
  gap: var(--space-2);
  padding: var(--space-3);
  border-radius: var(--radius-lg);
  text-align: center;
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.artist-card:hover {
  background: var(--track-accent-soft);
}

.artist-avatar {
  width: 6.5rem;
  aspect-ratio: 1 / 1;
  object-fit: cover;
  border-radius: var(--radius-full);
  box-shadow: var(--shadow-md);
}

.artist-name {
  max-width: 100%;
  font-size: 0.9rem;
  font-weight: 550;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
