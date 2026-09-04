<script setup lang="ts">
import { inject, onMounted, ref, watch } from "vue";
import { api, coverUrl } from "../lib/api/index.ts";
import type { AlbumDetail, ArtistInfo, SongRef } from "../lib/api/types.ts";
import { playerKey } from "../lib/player.ts";
import CoverCard from "../components/CoverCard.vue";
import SectionHeader from "../components/SectionHeader.vue";
import TrackTable from "../components/TrackTable.vue";

// 路由以 props: true 传入 :id；点相似歌手时同组件复用，靠 watch 重新拉数据
const props = defineProps<{ id: string }>();

const player = inject(playerKey);

const info = ref<ArtistInfo | null>(null);
const songs = ref<SongRef[]>([]);
const albums = ref<AlbumDetail[]>([]);
const loading = ref(true);
const failed = ref(false);

async function load() {
  loading.value = true;
  failed.value = false;
  try {
    // 三个请求互不依赖，并发取齐后再整体出画，避免三段 loading 闪烁
    const [infoData, songData, albumData] = await Promise.all([
      api.artist.info(props.id),
      api.artist.songs(props.id),
      api.artist.albums(props.id),
    ]);
    info.value = infoData;
    songs.value = songData;
    albums.value = albumData;
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
}

onMounted(load);
watch(() => props.id, load);
</script>

<template>
  <div class="artist-view">
    <div v-if="loading" class="state-hint">加载中…</div>
    <div v-else-if="failed || !info" class="state-hint">加载失败</div>

    <template v-else>
      <!-- hero：左图右信息，比背景大图稳（不依赖封面明度） -->
      <section class="artist-hero">
        <img class="artist-photo" :src="info.picUrl" :alt="info.name" />
        <div class="artist-copy">
          <h1 class="artist-name">{{ info.name }}</h1>
          <div class="artist-stats">
            <span class="stat"><b>{{ info.songCount }}</b>歌曲</span>
            <span class="stat"><b>{{ info.albumCount }}</b>专辑</span>
            <span class="stat"><b>{{ info.mvCount }}</b>MV</span>
          </div>
          <p class="artist-desc">{{ info.desc }}</p>
        </div>
      </section>

      <section class="section">
        <SectionHeader title="热门歌曲" />
        <TrackTable :tracks="songs" :player="player" show-header />
      </section>

      <section class="section">
        <SectionHeader title="专辑" />
        <div class="album-grid">
          <CoverCard
            v-for="album in albums"
            :key="album.mid"
            :cover-url="album.picUrl"
            :title="album.name"
            :subtitle="album.releaseDate"
            :to="`/album/${album.mid}`"
          />
        </div>
      </section>

      <section class="section">
        <SectionHeader title="相似歌手" />
        <div class="similar-grid">
          <!-- 相似歌手只有 mid/name：封面用与歌手页同一 seed 生成，图片天然一致 -->
          <RouterLink
            v-for="artist in info.similar"
            :key="artist.mid"
            class="similar-card"
            :to="`/artist/${artist.mid}`"
          >
            <img class="similar-avatar" :src="coverUrl(`artist:${artist.mid}`)" :alt="artist.name" loading="lazy" />
            <span class="similar-name">{{ artist.name }}</span>
          </RouterLink>
        </div>
      </section>
    </template>
  </div>
</template>

<style scoped>
.artist-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
}

.state-hint {
  padding: var(--space-12) 0;
  text-align: center;
  color: var(--muted-foreground);
}

/* —— hero —— */
.artist-hero {
  display: flex;
  align-items: center;
  gap: var(--space-8);
  padding: var(--space-2) 0 var(--space-8);
}

.artist-photo {
  width: clamp(10rem, 18vw, 14rem);
  aspect-ratio: 1 / 1;
  flex: 0 0 auto;
  object-fit: cover;
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
}

.artist-copy {
  min-width: 0;
}

.artist-name {
  font-size: clamp(1.8rem, 3.4vw, 2.6rem);
  font-weight: 700;
  line-height: 1.2;
}

.artist-stats {
  display: flex;
  gap: var(--space-6);
  margin-top: var(--space-4);
  color: var(--muted-foreground);
  font-size: 0.85rem;
}

.stat {
  display: inline-flex;
  align-items: baseline;
  gap: 0.35em;
}

/* 数字大一号做"统计带"的刻度感 */
.stat b {
  font-size: 1.25rem;
  font-weight: 650;
  color: var(--foreground);
  font-variant-numeric: tabular-nums;
}

.artist-desc {
  margin-top: var(--space-4);
  max-width: 44rem;
  color: var(--muted-foreground);
  font-size: 0.9rem;
  line-height: 1.7;
  /* 简介两行截断：完整版留给专辑/歌曲页的用户主动探索 */
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

/* —— 区块节奏 —— */
.section {
  margin-top: var(--space-8);
}

.section > .section-header {
  margin-bottom: var(--space-4);
}

.album-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(9rem, 1fr));
  gap: var(--space-4);
}

/* —— 相似歌手：圆形头像卡 —— */
.similar-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(7.5rem, 1fr));
  gap: var(--space-5);
}

.similar-card {
  display: grid;
  justify-items: center;
  gap: var(--space-2);
  padding: var(--space-3);
  border-radius: var(--radius-lg);
  text-align: center;
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.similar-card:hover {
  background: var(--track-accent-soft);
}

.similar-avatar {
  width: 6.5rem;
  aspect-ratio: 1 / 1;
  object-fit: cover;
  border-radius: var(--radius-full);
  box-shadow: var(--shadow-md);
}

.similar-name {
  font-size: 0.9rem;
  font-weight: 550;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 100%;
}
</style>
