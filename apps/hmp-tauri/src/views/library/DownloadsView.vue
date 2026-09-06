<script setup lang="ts">
import { computed, inject, onMounted, ref } from "vue";
import { api } from "../../lib/api/index.ts";
import type { DownloadLibrary } from "../../lib/api/types.ts";
import { formatBytes } from "../../lib/format.ts";
import { playerKey } from "../../lib/player.ts";
import { songToQueueItem } from "../../lib/browserPlayerBridge.ts";
import { QUALITY_TIERS, qualityState } from "../../lib/qualityStore.ts";
import AppIcon from "../../components/AppIcon.vue";
import Button from "../../components/Button.vue";
import PageHeader from "../../components/PageHeader.vue";
import SectionHeader from "../../components/SectionHeader.vue";
import TrackTable from "../../components/TrackTable.vue";
import playIcon from "../../assets/icons/play_arrow-rounded.svg?raw";

// player 可能拿不到：播放入口与播放全部按钮兜底隐藏，页面照常渲染
const player = inject(playerKey);

const library = ref<DownloadLibrary | null>(null);
const loading = ref(true);
const failed = ref(false);

onMounted(async () => {
  try {
    library.value = await api.library.downloads();
  } catch {
    failed.value = true;
  } finally {
    loading.value = false;
  }
});

const tracks = computed(() => library.value?.tracks ?? []);
const totalSizeBytes = computed(() => tracks.value.reduce((sum, track) => sum + track.sizeBytes, 0));
const losslessCount = computed(() => tracks.value.filter((track) => track.format === "FLAC").length);

// 下载音质偏好与播放共用一份 qualityStore（真实生效、localStorage 持久化）
const preferredTier = computed(
  () => QUALITY_TIERS.find((tier) => tier.id === qualityState.selected) ?? QUALITY_TIERS[0],
);

function playAll() {
  if (!player || tracks.value.length === 0) return;
  player.playTracks(
    tracks.value.map(songToQueueItem),
    0,
  );
}
</script>

<template>
  <div class="downloads-view">
    <div v-if="loading" class="state-hint">加载中…</div>
    <div v-else-if="failed" class="state-hint">加载失败</div>

    <template v-else>
      <PageHeader title="下载音乐" back-to="/library">
        <template #meta>
          <span>{{ tracks.length }} 首</span>
          <span aria-hidden="true">·</span>
          <span>占用 {{ formatBytes(totalSizeBytes) }}</span>
          <span aria-hidden="true">·</span>
          <span>含无损 {{ losslessCount }} 首</span>
        </template>
        <div class="header-actions">
          <Button
            v-if="player && tracks.length > 0"
            variant="default"
            size="sm"
            @click="playAll"
          >
            <AppIcon :src="playIcon" class="action-icon" />
            播放全部
          </Button>
        </div>
      </PageHeader>

      <section class="section" aria-label="已下载">
        <SectionHeader title="已下载" />
        <TrackTable :tracks="tracks" :player="player" show-header />
      </section>

      <!-- 落盘明细：一行一事的发丝线清单（与评论区同一套编辑部式语言） -->
      <section class="section" aria-label="落盘明细">
        <SectionHeader title="落盘明细" />
        <dl class="fact-list">
          <div class="fact-row">
            <dt>存储位置</dt>
            <dd>{{ library?.storagePath }}</dd>
          </div>
          <div class="fact-row">
            <dt>下载音质</dt>
            <dd>
              {{ preferredTier.label }}
              <RouterLink class="text-link adjust-link" to="/settings/playback">在设置中调整</RouterLink>
            </dd>
          </div>
        </dl>
      </section>
    </template>
  </div>
</template>

<style scoped>
.downloads-view {
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

/* —— 落盘明细：发丝线分隔的定义行，数字 tabular —— */
.fact-list {
  display: grid;
}

.fact-row {
  display: grid;
  grid-template-columns: 6rem minmax(0, 1fr);
  align-items: baseline;
  gap: var(--space-4);
  padding: var(--space-3) 0;
}

.fact-row + .fact-row {
  border-top: 1px solid color-mix(in srgb, var(--foreground) 7%, transparent);
}

.fact-row dt {
  color: var(--muted-foreground);
  font-size: 0.85rem;
}

.fact-row dd {
  font-size: 0.88rem;
  font-variant-numeric: tabular-nums;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.adjust-link {
  margin-left: var(--space-2);
  font-size: 0.85rem;
}
</style>
