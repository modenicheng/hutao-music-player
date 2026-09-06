<script setup lang="ts">
/**
 * 三根跳动条（正在播放指示）：TrackTable 序号列与播放队列行共用。
 * 负 animation-delay 让三根条挂载即处于错开的稳态相位，
 * 不再有"延迟期显示满高、启动骤缩"的初始跳变；刚挂载即暂停时相位也是确定的。
 * 容器统一裁剪：三根条是独立的缩放动画层，个别条的抗锯齿可能多画出 1px，
 * 共用一条裁剪线保证绘制底边（而非仅几何底边）齐平。
 */
withDefaults(defineProps<{ paused?: boolean }>(), { paused: false });
</script>

<template>
  <span class="equalizer" :class="{ 'is-paused': paused }" aria-hidden="true">
    <i></i><i></i><i></i>
  </span>
</template>

<style scoped>
.equalizer {
  display: flex;
  align-items: flex-end;
  gap: 2px;
  height: 0.85rem;
  overflow: hidden;
}

.equalizer i {
  width: 3px;
  height: var(--h);
  background: var(--track-equalizer);
  border-radius: 1px;
  /* 缩放锚定底边：默认绕中心缩放会让高度不同的条底边错位 */
  transform-origin: bottom;
  animation: eq-bounce 0.9s ease-in-out infinite;
  animation-delay: calc(var(--i) * -0.3s);
}

.equalizer i:nth-child(1) { --i: 0; --h: 60%; }
.equalizer i:nth-child(2) { --i: 1; --h: 100%; }
.equalizer i:nth-child(3) { --i: 2; --h: 45%; }

.equalizer.is-paused i {
  animation-play-state: paused;
}

@keyframes eq-bounce {
  0%,
  100% {
    transform: scaleY(0.55);
  }
  50% {
    transform: scaleY(1);
  }
}

@media (prefers-reduced-motion: reduce) {
  .equalizer i {
    animation: none;
  }
}
</style>
