<script setup lang="ts">
defineProps<{
  title: string;
  /** 传入则渲染"更多"路由链接 */
  moreTo?: string;
}>();
</script>

<template>
  <header class="section-header">
    <h2 class="section-title">{{ title }}</h2>
    <div class="section-actions">
      <slot name="actions"></slot>
      <RouterLink v-if="moreTo" :to="moreTo" class="section-more text-link">
        更多
        <span class="more-arrow" aria-hidden="true"></span>
      </RouterLink>
    </div>
  </header>
</template>

<style scoped>
.section-header {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--space-4);
}

.section-title {
  font-size: 1.15rem;
  font-weight: 650;
  letter-spacing: 0.01em;
}

.section-actions {
  display: flex;
  align-items: center;
  gap: var(--space-3);
}

.section-more {
  display: inline-flex;
  align-items: center;
  gap: 0.15rem;
  font-size: 0.85rem;
}

/* expand-more 旋转 -90° 指向右侧 */
.more-arrow {
  width: 0.9em;
  height: 0.9em;
  background: currentColor;
  -webkit-mask: url("../assets/icons/expand-more-rounded.svg") no-repeat center / contain;
  mask: url("../assets/icons/expand-more-rounded.svg") no-repeat center / contain;
  transform: rotate(-90deg);
}
</style>
