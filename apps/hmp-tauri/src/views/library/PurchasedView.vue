<script setup lang="ts">
import { computed, inject, onMounted, ref } from "vue";
import { api } from "../../lib/api/index.ts";
import type { PurchasedMusic } from "../../lib/api/types.ts";
import { formatCny } from "../../lib/format.ts";
import { playerKey } from "../../lib/player.ts";
import CoverCard from "../../components/CoverCard.vue";
import PageHeader from "../../components/PageHeader.vue";
import SectionHeader from "../../components/SectionHeader.vue";
import TrackTable from "../../components/TrackTable.vue";

// player 可能拿不到：TrackTable 内部已兜底，页面照常渲染
const player = inject(playerKey);

const purchased = ref<PurchasedMusic | null>(null);
const loading = ref(true);
const failed = ref(false);

onMounted(async () => {
  try {
    purchased.value = await api.library.purchased();
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
});

const singles = computed(() => purchased.value?.singles ?? []);
const albums = computed(() => purchased.value?.albums ?? []);
const singleTracks = computed(() => singles.value.map((entry) => entry.song));
const totalSpentFen = computed(
  () =>
    singles.value.reduce((sum, entry) => sum + entry.priceFen, 0) +
    albums.value.reduce((sum, entry) => sum + entry.priceFen, 0),
);
</script>

<template>
  <div class="purchased-view">
    <div v-if="loading" class="state-hint">加载中…</div>
    <div v-else-if="failed" class="state-hint">加载失败</div>

    <template v-else>
      <PageHeader title="已购音乐" back-to="/library">
        <template #meta>
          <span>单曲 {{ singles.length }}</span>
          <span aria-hidden="true">·</span>
          <span>专辑 {{ albums.length }}</span>
          <span aria-hidden="true">·</span>
          <span>合计 {{ formatCny(totalSpentFen) }}</span>
        </template>
      </PageHeader>

      <!-- 已购单曲：整表可播，购买日期归页头聚合、不进行内列 -->
      <section class="section" aria-label="已购单曲">
        <SectionHeader title="已购单曲" />
        <TrackTable :tracks="singleTracks" :player="player" show-header />
      </section>

      <!-- 已购专辑：封面卡进入专辑详情 -->
      <section class="section" aria-label="已购专辑">
        <SectionHeader title="已购专辑" />
        <div class="album-grid">
          <CoverCard
            v-for="entry in albums"
            :key="entry.album.mid"
            :cover-url="entry.album.picUrl"
            :title="entry.album.name"
            :subtitle="`${entry.album.songs.length} 首 · ${entry.purchasedAt} 购买`"
            :to="`/album/${entry.album.mid}`"
          />
        </div>
      </section>
    </template>
  </div>
</template>

<style scoped>
.purchased-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
}

.state-hint {
  padding: var(--space-12) 0;
  text-align: center;
  color: var(--muted-foreground);
}

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
</style>
