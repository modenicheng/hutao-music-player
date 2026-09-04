<script setup lang="ts">
import { inject, onMounted, ref, watch } from "vue";
import { api } from "../lib/api/index.ts";
import type { TopDetail } from "../lib/api/types.ts";
import { playerKey } from "../lib/player.ts";
import PageHeader from "../components/PageHeader.vue";
import TrackTable from "../components/TrackTable.vue";

// 路由以 props: true 传入 :id；同组件在不同榜单间复用时靠 watch 重新拉数据
const props = defineProps<{ id: string }>();

const player = inject(playerKey);

const detail = ref<TopDetail | null>(null);
const loading = ref(true);
const failed = ref(false);

async function load() {
  loading.value = true;
  failed.value = false;
  try {
    detail.value = await api.top.detail(props.id);
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
  <div class="top-detail-view">
    <div v-if="loading" class="state-hint">加载中…</div>
    <div v-else-if="failed || !detail" class="state-hint">加载失败</div>

    <template v-else>
      <PageHeader :title="detail.category.name" back-to="/top">
        <template #meta>
          <span>{{ detail.category.updateTime }} 更新</span>
          <span aria-hidden="true">·</span>
          <span>{{ detail.entries.length }}首</span>
        </template>
      </PageHeader>

      <!-- 榜单就是一份曲目表：名次直接复用 TrackTable 的序号列 -->
      <TrackTable :tracks="detail.entries.map((entry) => entry.song)" :player="player" show-header />
    </template>
  </div>
</template>

<style scoped>
.top-detail-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
}

.state-hint {
  padding: var(--space-12) 0;
  text-align: center;
  color: var(--muted-foreground);
}
</style>
