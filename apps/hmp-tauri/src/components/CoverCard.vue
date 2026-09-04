<script setup lang="ts">
import AppIcon from "./AppIcon.vue";
import playIcon from "../assets/icons/play_arrow-rounded.svg?raw";

/**
 * 封面卡（DESIGN.md §3.0）：hover 上浮 + 播放浮层按钮。
 * to 存在时整卡可点进入详情；播放按钮独立 emit。
 */
defineProps<{
  coverUrl: string;
  title: string;
  subtitle?: string;
  /** 目标路由（如 /album/:mid） */
  to?: string;
  /** 传入才显示播放浮层按钮 */
  playable?: boolean;
}>();

const emit = defineEmits<{ play: [] }>();
</script>

<template>
  <component
    :is="to ? 'router-link' : 'div'"
    :to="to"
    class="cover-card"
  >
    <div class="cover-frame">
      <img class="cover-img" :src="coverUrl" :alt="title" loading="lazy" />
      <button
        v-if="playable"
        class="cover-play"
        :title="`播放《${title}》`"
        @click.stop="emit('play')"
      >
        <AppIcon :src="playIcon" class="cover-play-icon" />
      </button>
    </div>
    <div class="cover-copy">
      <div class="cover-title" :title="title">{{ title }}</div>
      <div v-if="subtitle" class="cover-subtitle">{{ subtitle }}</div>
    </div>
  </component>
</template>

<style scoped>
.cover-card {
  display: block;
  min-width: 0;
  border-radius: var(--radius-lg);
  transition: transform var(--duration-normal) var(--ease-standard);
}

.cover-card:hover {
  transform: translateY(-2px);
}

.cover-frame {
  position: relative;
  aspect-ratio: 1 / 1;
  border-radius: var(--radius-lg);
  overflow: hidden;
  box-shadow: var(--shadow-md);
  transition: box-shadow var(--duration-normal) var(--ease-standard);
}

.cover-card:hover .cover-frame {
  box-shadow: var(--shadow-lg);
}

.cover-img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

/* 播放浮层：右下角圆形按钮，hover 出现 */
.cover-play {
  position: absolute;
  right: 0.6rem;
  bottom: 0.6rem;
  display: grid;
  place-items: center;
  width: 2.5rem;
  height: 2.5rem;
  color: var(--primary-foreground);
  background: var(--primary);
  border-radius: var(--radius-full);
  box-shadow: var(--shadow-md);
  opacity: 0;
  transform: translateY(0.35rem);
  transition:
    opacity var(--duration-fast) var(--ease-standard),
    transform var(--duration-fast) var(--ease-standard);
}

.cover-card:hover .cover-play,
.cover-play:focus-visible {
  opacity: 1;
  transform: translateY(0);
}

.cover-play-icon {
  width: 1.4rem;
  height: 1.4rem;
}

.cover-copy {
  margin-top: 0.55rem;
}

.cover-title {
  font-size: 0.92rem;
  font-weight: 550;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.cover-subtitle {
  margin-top: 0.15rem;
  font-size: 0.8rem;
  color: var(--muted-foreground);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
