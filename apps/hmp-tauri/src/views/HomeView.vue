<script lang="ts" setup>
import HoverGroup from "../components/HoverGroup.vue";
import HoverItem from "../components/HoverItem.vue";

const items = [
  { label: "播放", radius: "var(--radius-full)" },
  { label: "暂停", radius: "var(--radius-md)" },
  { label: "下一首", radius: "var(--radius-full)" },
  { label: "收藏", radius: "var(--radius-md)" },
  { label: "设置", radius: "var(--radius-sm)" },
  { label: "歌单", radius: "var(--radius-full)" },
];
</script>

<template>
  <div class="home-view">
    <h1>首页</h1>
    <!-- 同一容器内任意布局（网格/混合尺寸）都能平滑滑动 -->
    <HoverGroup
      highlight-color="var(--walnut-300)"
      :highlight-opacity="0.35"
      style="
        display: flex;
        flex-wrap: wrap;
        gap: 0.75rem;
        margin-top: 1rem;
        width: 100%;
      "
    >
      <HoverItem
        v-for="i in items"
        :key="i.label"
        :style="{
          padding: '0.4rem 0.9rem',
          borderRadius: i.radius,
          fontSize: '0.9rem',
        }"
      >
        {{ i.label }}
      </HoverItem>
    </HoverGroup>
  </div>
</template>
