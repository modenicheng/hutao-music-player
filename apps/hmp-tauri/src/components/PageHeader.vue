<script setup lang="ts">
import { useRouter } from "vue-router";

/**
 * 内容页页头（DESIGN.md §3.0）：返回 + 大标题 + 元信息插槽。
 * 吸顶收缩留给后续按需加。
 */
const props = defineProps<{
  title: string;
  /** 不传则不显示返回按钮 */
  backTo?: string;
}>();

const router = useRouter();

function goBack() {
  if (props.backTo) {
    void router.push(props.backTo);
  } else {
    router.back();
  }
}
</script>

<template>
  <header class="page-header">
    <button v-if="backTo !== undefined" class="page-back" title="返回" @click="goBack">
      <span class="back-icon" aria-hidden="true"></span>
    </button>
    <div class="page-copy">
      <h1 class="page-title">{{ title }}</h1>
      <div class="page-meta">
        <slot name="meta"></slot>
      </div>
    </div>
    <slot></slot>
  </header>
</template>

<style scoped>
.page-header {
  display: flex;
  align-items: center;
  gap: var(--space-4);
  padding: var(--space-2) 0 var(--space-6);
}

.page-back {
  display: grid;
  place-items: center;
  width: 2.25rem;
  height: 2.25rem;
  flex: 0 0 auto;
  color: var(--foreground);
  border-radius: var(--radius-full);
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.page-back:hover {
  background: var(--muted);
}

.back-icon {
  width: 1.25rem;
  height: 1.25rem;
  background: currentColor;
  -webkit-mask: url("../assets/icons/arrow-back-rounded.svg") no-repeat center / contain;
  mask: url("../assets/icons/arrow-back-rounded.svg") no-repeat center / contain;
}

.page-copy {
  min-width: 0;
}

.page-title {
  font-size: clamp(1.6rem, 3vw, 2.2rem);
  font-weight: 650;
  line-height: 1.2;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.page-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
  margin-top: 0.35rem;
  color: var(--muted-foreground);
  font-size: 0.88rem;
}
</style>
