<script setup lang="ts">
defineProps<{
  src: string;
}>();
</script>

<template>
  <span
    class="app-icon"
    aria-hidden="true"
    v-html="src"
  ></span>
</template>

<style scoped>
.app-icon {
  display: block;
  flex: 0 0 auto;
  width: 1em;
  height: 1em;
  color: inherit;
}

.app-icon :deep(svg) {
  display: block;
  width: 100%;
  height: 100%;
}
</style>
