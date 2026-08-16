<script setup lang="ts">
import { onMounted, ref } from "vue";

const audioStatus = ref({
  playing: false,
  progress: 0,
});

const test_uri = "Hope_is_the_thing_with_featers.flac";

const audio = new Audio(test_uri);

const togglePlay = () => {
  if (audioStatus.value.playing) {
    audio.pause();
    audioStatus.value.playing = false;
    return;
  }
  audio.play();
  audioStatus.value.playing = true;
};

const progressbarRef = ref<HTMLElement>();

const setProgress = () => {
  currentControlStatus.value = controlStatus.idle;
  let progress = Math.min(1, Math.max(0, dragPercent));
  audio.currentTime = progress * audio.duration;
};

let dragPercent = 0;

onMounted(() => {
  window.addEventListener("mouseup", () => {
    if (currentControlStatus.value === controlStatus.dragging) {
      setProgress();
    }
  });
  window.addEventListener("mousemove", (ev) => {
    // console.log("bar", ev);
    dragPercent =
      (ev.clientX -
        (progressbarRef.value?.offsetLeft
          ? progressbarRef.value?.offsetLeft
          : 0)) /
      (progressbarRef.value?.clientWidth
        ? progressbarRef.value?.clientWidth
        : 0);

    dragPercent = Math.min(1, Math.max(0, dragPercent));
  });
});

const progressRender = () => {
  if (currentControlStatus.value === controlStatus.idle) {
    audioStatus.value.progress = audio.currentTime / audio.duration;
  } else {
    audioStatus.value.progress = dragPercent;
  }
  requestAnimationFrame(progressRender);
};

requestAnimationFrame(progressRender);

enum controlStatus {
  idle,
  mousedown,
  dragging,
  mouseup,
}

const currentControlStatus = ref<controlStatus>(controlStatus.idle);
</script>

<template>
  <footer class="player-bar">
    <div
      ref="progressbarRef"
      :class="[
        'progress-bar',
        currentControlStatus === controlStatus.dragging
          ? 'progress-bar-hover'
          : '',
      ]"
      @mousedown="currentControlStatus = controlStatus.dragging"
      @mouseup="setProgress"
    >
      <div
        class="progress"
        ref="progressRef"
        :style="{
          transform: `scaleX(${audioStatus.progress})`,
          transformOrigin: `left`,
        }"
      ></div>
    </div>
    <div class="status-card">
      <div class="btn" @click="togglePlay">
        {{ audioStatus.playing ? `pause` : `play` }}
      </div>
    </div>
  </footer>
</template>

<style lang="css" scoped>
.player-bar {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  min-height: var(--player-bar-height);
  color: var(--foreground);
}

.progress-bar {
  width: 100%;
  height: var(--control-height-xs);
  background: var(--track);
  border-radius: var(--radius-full);
  transform: scaleY(1);
  transition: transform var(--duration-fast);
}

.progress {
  width: 100%;
  height: 100%;
  background: var(--primary);
  border-radius: var(--radius-full);
}
.progress-bar-hover {
  transform: scaleY(1.5);
  transition: transform var(--duration-fast);
}

.progress-bar:hover {
  transform: scaleY(1.5);
  transition: transform var(--duration-fast);
}
.cursor {
  height: 0.8rem;
  width: 0.3rem;
  background-color: var(--primary);
}

.status-card {
  width: 100%;
  flex: 1;
  min-height: 0;
  background: var(--surface-2);
  border-radius: var(--radius-lg);
}
</style>
