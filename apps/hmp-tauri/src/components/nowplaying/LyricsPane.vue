<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { Lyrics } from "../../lib/api/types.ts";

/**
 * 歌词区（DESIGN.md §3.1.5）：当前行高亮 + 自动跟随滚动；
 * 用户手动滚动后暂停跟随 3 秒；点击任意行 seek 至该行。
 */
const props = defineProps<{
  lyrics: Lyrics | null;
  /** 播放位置（ms），由播放页透传 */
  positionMs: number;
  disabled?: boolean;
}>();

const emit = defineEmits<{ seek: [timeMs: number] }>();

const container = ref<HTMLElement | null>(null);
const lastUserScrollAt = ref(0);
const FOLLOW_PAUSE_MS = 3000;

const currentIndex = computed(() => {
  const lines = props.lyrics?.lines ?? [];
  let index = -1;
  for (let i = 0; i < lines.length; i += 1) {
    if (lines[i].timeMs <= props.positionMs) index = i;
    else break;
  }
  return index;
});

function isCurrent(index: number) {
  return index === currentIndex.value;
}

/** 亮度随距离当前行衰减：1 / 0.55 / 0.32 */
function lineOpacity(index: number) {
  if (currentIndex.value < 0) return 0.55;
  const distance = Math.abs(index - currentIndex.value);
  if (distance === 0) return 1;
  if (distance === 1) return 0.55;
  return Math.max(0.32, 0.55 - (distance - 1) * 0.12);
}

function onUserScroll() {
  lastUserScrollAt.value = Date.now();
}

function seekTo(timeMs: number) {
  if (!props.disabled) emit("seek", timeMs);
}

watch(currentIndex, async (index) => {
  if (index < 0 || !container.value) return;
  if (Date.now() - lastUserScrollAt.value < FOLLOW_PAUSE_MS) return;
  const line = container.value.querySelector<HTMLElement>(
    `[data-line-index="${index}"]`,
  );
  if (!line) return;
  container.value.scrollTo({
    top: line.offsetTop - container.value.clientHeight / 2 + line.clientHeight / 2,
    behavior: "smooth",
  });
});
</script>

<template>
  <div
    ref="container"
    class="lyrics-pane"
    :class="{ 'is-disabled': disabled }"
    @wheel="onUserScroll"
    @touchmove="onUserScroll"
  >
    <div v-if="!lyrics || lyrics.lines.length === 0" class="lyrics-empty">
      暂无歌词
    </div>
    <template v-else>
      <div
        v-for="(line, index) in lyrics.lines"
        :key="`${line.timeMs}-${index}`"
        class="lyric-line"
        :class="{ 'is-current': isCurrent(index) }"
        :style="{ opacity: lineOpacity(index) }"
        :data-line-index="index"
        @click="seekTo(line.timeMs)"
      >
        <p class="lyric-text">{{ line.text }}</p>
        <p v-if="line.trans" class="lyric-trans">{{ line.trans }}</p>
      </div>
      <div class="lyrics-tail" aria-hidden="true"></div>
    </template>
  </div>
</template>

<style scoped>
.lyrics-pane {
  height: 100%;
  overflow-y: auto;
  padding: var(--space-8) 0;
  scroll-behavior: smooth;
  scrollbar-width: none;
}

.lyrics-pane::-webkit-scrollbar {
  display: none;
}

.lyrics-empty {
  padding: var(--space-12) 0;
  text-align: center;
  color: var(--muted-foreground);
}

.lyric-line {
  padding: 0.55rem var(--space-4);
  text-align: center;
  cursor: pointer;
  transition: opacity var(--duration-normal) var(--ease-standard);
}

.is-disabled .lyric-line {
  cursor: default;
}

.lyric-text {
  font-size: 1.15rem;
  line-height: 1.6;
  transition:
    color var(--duration-normal) var(--ease-standard),
    transform var(--duration-normal) var(--ease-standard);
}

.lyric-line.is-current .lyric-text {
  color: var(--track-accent);
  font-weight: 700;
  transform: scale(1.05);
}

.lyric-trans {
  margin-top: 0.2rem;
  font-size: 0.8em;
  color: var(--muted-foreground);
}

.lyrics-tail {
  height: 30vh;
}
</style>
