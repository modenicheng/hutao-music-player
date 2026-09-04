<script setup lang="ts">
import { onMounted, ref } from "vue";
import { api } from "../lib/api/index.ts";
import type { TopCategory } from "../lib/api/types.ts";

const categories = ref<TopCategory[]>([]);
const loading = ref(true);
const failed = ref(false);

onMounted(async () => {
  try {
    categories.value = await api.top.categories();
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
});
</script>

<template>
  <div class="top-view">
    <div v-if="loading" class="state-hint">加载中…</div>
    <div v-else-if="failed" class="state-hint">加载失败</div>

    <template v-else>
      <header class="top-page-header">
        <h1 class="top-page-title">排行榜</h1>
        <p class="top-page-subtitle">每周三更新 · 共 {{ categories.length }} 个榜单</p>
      </header>

      <!-- 榜单卡允许矩形封面（DESIGN.md §3.2），这里用 4:3 大图突出封面氛围 -->
      <div class="top-grid">
        <RouterLink
          v-for="chart in categories"
          :key="chart.id"
          :to="`/top/${chart.id}`"
          class="top-card"
        >
          <span class="top-cover">
            <img :src="chart.picUrl" :alt="chart.name" loading="lazy" />
            <span class="top-flag">{{ chart.trackCount }}首</span>
          </span>
          <span class="top-name">{{ chart.name }}</span>
          <span class="top-meta">{{ chart.updateTime }} 更新</span>
        </RouterLink>
      </div>
    </template>
  </div>
</template>

<style scoped>
.top-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
}

.state-hint {
  padding: var(--space-12) 0;
  text-align: center;
  color: var(--muted-foreground);
}

.top-page-header {
  padding: var(--space-2) 0 var(--space-6);
}

.top-page-title {
  font-size: clamp(1.6rem, 3vw, 2.2rem);
  font-weight: 650;
  line-height: 1.2;
}

.top-page-subtitle {
  margin-top: 0.35rem;
  font-size: 0.88rem;
  color: var(--muted-foreground);
}

.top-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(12rem, 1fr));
  gap: var(--space-5);
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
  position: relative;
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

/* 曲数角标：压在封面上，强调"这是能点进去听的榜单" */
.top-flag {
  position: absolute;
  left: 0.6rem;
  bottom: 0.6rem;
  padding: 0.1rem 0.5rem;
  font-size: 0.72rem;
  color: var(--foreground);
  background: var(--track-accent-soft);
  backdrop-filter: blur(4px);
  border-radius: var(--radius-full);
  font-variant-numeric: tabular-nums;
}

.top-name {
  display: block;
  margin-top: 0.55rem;
  font-size: 1rem;
  font-weight: 600;
}

.top-meta {
  display: block;
  margin-top: 0.15rem;
  font-size: 0.8rem;
  color: var(--muted-foreground);
  font-variant-numeric: tabular-nums;
}
</style>
