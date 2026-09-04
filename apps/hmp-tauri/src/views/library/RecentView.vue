<script setup lang="ts">
import { inject } from "vue";
import { songPool } from "../../lib/api/mock-data.ts";
import { hashSeed } from "../../lib/api/covers.ts";
import type { SongRef } from "../../lib/api/types.ts";
import { playerKey } from "../../lib/player.ts";
import PageHeader from "../../components/PageHeader.vue";
import TrackTable from "../../components/TrackTable.vue";

// player 可能拿不到：TrackTable 内部已兜底，页面照常渲染
const player = inject(playerKey);

// 最近播放本期不调 api（后端 recent_plays 未接线）：本地取总池前 12 首
const RECENT_SIZE = 12;

/** "距今天数" → 相对时间文案；纯函数保证同一首歌永远显示同一文案 */
function relativeLabel(daysAgo: number): string {
  if (daysAgo <= 0) return "今天";
  if (daysAgo === 1) return "昨天";
  if (daysAgo < 14) return `${daysAgo} 天前`;
  const weeks = Math.round(daysAgo / 7);
  return `${weeks} 周前`;
}

// 距今天数由 mid 哈希派生，且随列表下标单调不减——最近播放本身就是按时间倒序排的。
// 不用 Date.now：与 mock 层同一套确定性纪律，每次渲染结果一致。
const records: Array<{ song: SongRef; label: string }> = (() => {
  let acc = 0;
  return songPool.slice(0, RECENT_SIZE).map((song, index) => {
    acc += index === 0 ? 0 : 1 + (hashSeed(`recent:${song.mid}`) % 2);
    return { song, label: relativeLabel(acc) };
  });
})();

const tracks = records.map((record) => record.song);
// TrackTable 暂不支持附加列（不可改动共享组件），相对时间汇总进页头元信息
const latestLabel = records[0]?.label ?? "今天";
const earliestLabel = records[records.length - 1]?.label ?? "今天";
</script>

<template>
  <div class="recent-view">
    <PageHeader title="最近播放" back-to="/library">
      <template #meta>
        <span>共 {{ tracks.length }} 首</span>
        <span aria-hidden="true">·</span>
        <span>最近一次播放于{{ latestLabel }}</span>
        <span aria-hidden="true">·</span>
        <span>最早记录{{ earliestLabel }}</span>
      </template>
    </PageHeader>

    <TrackTable :tracks="tracks" :player="player" show-header />
  </div>
</template>

<style scoped>
.recent-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
}
</style>
