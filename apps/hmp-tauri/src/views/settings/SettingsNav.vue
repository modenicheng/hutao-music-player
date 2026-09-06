<script setup lang="ts">
import { useRoute } from "vue-router";

/**
 * 设置子页导航（总览页不显示）：路由驱动的中性胶囊 tab，
 * active 样式与搜索页 tab / 侧栏激活项同款（--muted 底）。
 */
const route = useRoute();

const tabs = [
  { label: "常规", to: "/settings/general" },
  { label: "播放", to: "/settings/playback" },
  { label: "账号", to: "/settings/account" },
];
</script>

<template>
  <nav class="settings-nav" aria-label="设置分类">
    <RouterLink
      v-for="tab in tabs"
      :key="tab.to"
      :to="tab.to"
      class="settings-tab"
      :class="{ 'is-active': route.path === tab.to }"
      :aria-current="route.path === tab.to ? 'page' : undefined"
    >
      {{ tab.label }}
    </RouterLink>
  </nav>
</template>

<style scoped>
.settings-nav {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding-bottom: var(--space-4);
}

.settings-tab {
  padding: 0.3rem 0.85rem;
  font-size: 0.9rem;
  color: var(--muted-foreground);
  border-radius: var(--radius-full);
  transition:
    background-color var(--duration-fast) var(--ease-standard),
    color var(--duration-fast) var(--ease-standard);
}

.settings-tab:hover {
  color: var(--foreground);
}

.settings-tab.is-active {
  color: var(--foreground);
  background: var(--muted);
  font-weight: 550;
}
</style>
