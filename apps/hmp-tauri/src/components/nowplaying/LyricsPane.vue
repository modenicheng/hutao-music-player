<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from "vue";
import Scroll from "../Scroll.vue";
import type { Lyrics } from "../../lib/api/types.ts";

/**
 * 歌词区（DESIGN.md §3.1.5 / v0.9）：Apple Music 式大字排版 + 弹簧跟随滚动。
 * 跟随不用原生 smooth——rAF 循环里跑临界阻尼弹簧，连续换行、大跨度 seek
 * 都平滑衔接且可随时打断。
 * 景深取景：各行亮度/模糊/缩放按「行中心 ↔ 视口中心」距离逐帧计算——
 * 跟随播放时焦点即播放行，用户手动滚动时焦点随视口走；缩放走 transform 不动布局。
 * 逐字歌词（QRC）当前行按字/词扫色，扫色进度直写 CSS 变量不进渲染管线；
 * 渐变软边端点与 done/todo 颜色全等，交棒前钉 --p=1，起唱收字均无跳变。
 * 桥每 250ms 推一次播放位置，rAF 在两次快照之间插值，外推上限 500ms
 * 防止后台标签页 rAF 冻结导致位置飞走。
 * 用户手动滚动后暂停跟随 3 秒；点击任意行/词 seek 至对应时间。
 */
const props = withDefaults(
  defineProps<{
    lyrics: Lyrics | null;
    /** 播放位置（ms），由播放页透传（桥的快照值） */
    positionMs: number;
    /** 是否播放中：暂停时冻结在快照位置 */
    playing?: boolean;
    disabled?: boolean;
  }>(),
  { playing: false, disabled: false },
);

const emit = defineEmits<{ seek: [timeMs: number] }>();

const container = ref<HTMLElement | null>(null);
const scroller = ref<InstanceType<typeof Scroll> | null>(null);
const FOLLOW_PAUSE_MS = 3000;
let lastUserScrollAt = 0;

// —— 逐帧状态：行/词索引进 Vue 响应式（低频变更），扫色进度走 CSS 变量 ——
const frameLine = ref(-1);
const frameWord = ref(-1);

// —— 弹簧跟随：纯 rAF 域，不进响应式 ——
let followX = 0;
let followV = 0;
/** 换词表/换曲后免动画直达当前行，不从顶部滚一遍 */
let snapPending = true;
/** ω rad/s、ζ=1 临界阻尼：快起缓收无过冲，约 0.35s 收敛 */
const SPRING_OMEGA = 12;
const SETTLE_PX = 0.5;
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

let posBase = props.positionMs;
let posSyncedAt = performance.now();
let lastSnapshotMs = props.positionMs;
let lastFrameAt = performance.now();
let rafId = 0;

function activeLineElement() {
  return container.value?.querySelector<HTMLElement>(
    `[data-line-index="${frameLine.value}"]`,
  );
}

function activeWordElement() {
  return container.value?.querySelector<HTMLElement>(
    `[data-line-index="${frameLine.value}"] .word[data-word-index="${frameWord.value}"]`,
  );
}

/** rAF 可能被后台标签页冻结数分钟：外推最多 500ms（≈2 个桥快照间隔），防止歌词被墙钟带飞 */
const MAX_EXTRAPOLATION_MS = 500;

const tick = (now: number) => {
  const dt = Math.min((now - lastFrameAt) / 1000, 0.064);
  lastFrameAt = now;

  // 直接读响应式 prop 对齐最新快照：快照一变就重置插值基点
  if (props.positionMs !== lastSnapshotMs) {
    lastSnapshotMs = props.positionMs;
    posBase = props.positionMs;
    posSyncedAt = performance.now();
  }
  const position =
    posBase +
    (props.playing
      ? Math.min(performance.now() - posSyncedAt, MAX_EXTRAPOLATION_MS)
      : 0);
  const lines = props.lyrics?.lines ?? [];

  const prevLine = frameLine.value;
  const prevWord = frameWord.value;

  let lineIndex = -1;
  for (let i = 0; i < lines.length; i += 1) {
    if (lines[i]!.timeMs <= position) lineIndex = i;
    else break;
  }
  if (lineIndex !== frameLine.value) {
    frameLine.value = lineIndex;
    // 离开旧行：清掉行内所有扫色残留，避免 seek 后词上挂着陈旧 --p
    if (prevLine >= 0) {
      container.value
        ?.querySelectorAll<HTMLElement>(`[data-line-index="${prevLine}"] .word`)
        .forEach((w) => w.style.removeProperty("--p"));
    }
  }

  const line = lineIndex >= 0 ? lines[lineIndex] : undefined;
  const words = line?.words;
  let wordIndex = -1;
  if (words && words.length > 0) {
    for (let i = 0; i < words.length; i += 1) {
      if (words[i]!.startMs <= position) wordIndex = i;
      else break;
    }
  }
  if (wordIndex !== frameWord.value) {
    frameWord.value = wordIndex;
    // 自然推进：交棒前把上个词钉到满进度——渐变端点与 done 全等，收字瞬间无跳变
    if (lineIndex === prevLine && prevWord >= 0 && wordIndex > prevWord) {
      container.value
        ?.querySelector<HTMLElement>(
          `[data-line-index="${lineIndex}"] .word[data-word-index="${prevWord}"]`,
        )
        ?.style.setProperty("--p", "1");
    }
  }
  if (words && wordIndex >= 0) {
    const word = words[wordIndex]!;
    const progress =
      word.endMs > word.startMs
        ? Math.min(1, (position - word.startMs) / (word.endMs - word.startMs))
        : 1;
    activeWordElement()?.style.setProperty("--p", progress.toFixed(3));
  }

  followTick(dt);
  depthTick();
  rafId = requestAnimationFrame(tick);
};
rafId = requestAnimationFrame(tick);
onBeforeUnmount(() => cancelAnimationFrame(rafId));

const clamp = (value: number, min: number, max: number) =>
  Math.min(max, Math.max(min, value));

/** 弹簧跟随：当前行推向视口垂直居中 */
function followTick(dt: number) {
  const viewport = scroller.value?.viewport;
  if (!viewport) return;
  const lineEl = frameLine.value >= 0 ? activeLineElement() : null;
  if (!lineEl) {
    if (snapPending) {
      snapPending = false;
      followX = 0;
      followV = 0;
      viewport.scrollTop = 0;
    }
    return;
  }
  // 目标 scrollTop = 当前行中心落到视口中心；rect 差值法，与当前滚动位置无关
  const vpRect = viewport.getBoundingClientRect();
  const rect = lineEl.getBoundingClientRect();
  const target = clamp(
    viewport.scrollTop + (rect.top - vpRect.top) + rect.height / 2 - vpRect.height / 2,
    0,
    viewport.scrollHeight - viewport.clientHeight,
  );

  if (Date.now() - lastUserScrollAt < FOLLOW_PAUSE_MS) {
    // 用户接管期间弹簧静默并贴住实际位置，恢复跟随时不跳变
    followX = viewport.scrollTop;
    followV = 0;
    return;
  }
  if (snapPending || reducedMotion) {
    snapPending = false;
    followX = target;
    followV = 0;
    viewport.scrollTop = target;
    return;
  }
  // 半隐式欧拉积分
  followV += (SPRING_OMEGA * SPRING_OMEGA * (target - followX) - 2 * SPRING_OMEGA * followV) * dt;
  followX += followV * dt;
  if (Math.abs(target - followX) < SETTLE_PX && Math.abs(followV) < 8) {
    followX = target;
    followV = 0;
  }
  viewport.scrollTop = followX;
}

// —— 景深取景：亮度/模糊/缩放随「行中心 ↔ 视口中心」距离衰减 ——
// 修复：旧实现按「索引距播放行」阶梯取值，手动滚动后景深仍锁死在播放行；
// 现按像素距离逐帧计算，焦点跟随视口（跟随播放时=播放行，滚动时=用户所视行）。
let lineEls: HTMLElement[] = [];
let lineElsDirty = true;
/** 每行上次写入的 o/blur/scale，变动小于取整精度不重写，静置零样式抖动 */
let depthCache = new Float64Array(0);

function depthTick() {
  if (lineElsDirty && container.value) {
    lineEls = Array.from(
      container.value.querySelectorAll<HTMLElement>(".lyric-line"),
    );
    depthCache = new Float64Array(lineEls.length * 3).fill(Number.NaN);
    lineElsDirty = false;
  }
  const viewport = scroller.value?.viewport;
  if (!viewport || lineEls.length === 0) return;

  // 先批量读 rect 再批量写样式，避免逐行读写交叉触发强制布局
  const vpRect = viewport.getBoundingClientRect();
  const mid = vpRect.top + vpRect.height / 2;
  const rects = lineEls.map((el) => el.getBoundingClientRect());
  // 衰减长度随排版密度伸缩：行距取前两行中心距，单行兜底 88px
  const pitch =
    rects.length > 1 && rects[1]!.top !== rects[0]!.top
      ? Math.abs(rects[1]!.top - rects[0]!.top)
      : 88;
  for (let i = 0; i < rects.length; i += 1) {
    const rect = rects[i]!;
    const x = Math.abs(rect.top + rect.height / 2 - mid) / pitch;
    // 透明度指数衰减 ≈ 旧阶梯 1/.62/.48/.40/.34；blur 半行起 2.7 行满；缩放 2.2 行内收满
    const o = Number((0.34 + 0.66 * Math.exp(-x / 1.15)).toFixed(3));
    const blur = Number((1.6 * clamp((x - 0.5) / 2.2, 0, 1)).toFixed(2));
    const scale = Number((1 - 0.08 * clamp(x / 2.2, 0, 1)).toFixed(3));
    const el = lineEls[i]!;
    const b = i * 3;
    if (depthCache[b] !== o) {
      depthCache[b] = o;
      el.style.setProperty("--line-o", String(o));
    }
    if (depthCache[b + 1] !== blur) {
      depthCache[b + 1] = blur;
      el.style.setProperty("--line-blur", `${blur}px`);
    }
    if (depthCache[b + 2] !== scale) {
      depthCache[b + 2] = scale;
      el.style.setProperty("--line-scale", String(scale));
    }
  }
}

function wordClass(lineIndex: number, wordIndex: number) {
  if (lineIndex !== frameLine.value) return "";
  if (wordIndex < frameWord.value) return "is-done";
  if (wordIndex === frameWord.value) return "is-active";
  return "is-todo";
}

function onUserScroll() {
  lastUserScrollAt = Date.now();
}

function seekTo(timeMs: number) {
  if (!props.disabled) emit("seek", timeMs);
}

watch(
  () => props.lyrics,
  () => {
    // 新词表：行/词立即失效，下一帧 snap 直达当前行，不从顶部滚一遍
    snapPending = true;
    frameLine.value = -1;
    frameWord.value = -1;
    lineElsDirty = true;
  },
);
</script>

<template>
  <div
    ref="container"
    class="lyrics-pane"
    :class="{ 'is-disabled': disabled }"
    @wheel="onUserScroll"
    @touchmove="onUserScroll"
  >
    <!-- 沉浸式阅读区：不渲染任何滚动条 -->
    <Scroll
      ref="scroller"
      direction="vertical"
      fill
      hide-scrollbar
      @user-scroll="onUserScroll"
    >
      <div v-if="!lyrics || lyrics.lines.length === 0" class="lyrics-empty">暂无歌词</div>
      <template v-else>
        <div class="lyrics-list">
          <div
            v-for="(line, index) in lyrics.lines"
            :key="`${line.timeMs}-${index}`"
            class="lyric-line"
            :class="{ 'is-current': index === frameLine }"
            :data-line-index="index"
            @click="seekTo(line.timeMs)"
          >
            <p class="lyric-text">
              <template v-if="line.words && line.words.length > 0">
                <span
                  v-for="(word, wordIndex) in line.words"
                  :key="wordIndex"
                  class="word"
                  :class="wordClass(index, wordIndex)"
                  :data-word-index="wordIndex"
                  @click.stop="seekTo(word.startMs)"
                >{{ word.text }}</span>
              </template>
              <template v-else>{{ line.text }}</template>
            </p>
            <p v-if="line.trans" class="lyric-trans">{{ line.trans }}</p>
          </div>
        </div>
      </template>
    </Scroll>
  </div>
</template>

<style scoped>
.lyrics-pane {
  height: 100%;
  /* 上下渐隐：边缘行淡出视口而非硬裁切（Apple Music 式） */
  -webkit-mask-image: linear-gradient(180deg, transparent 0, #000 7%, #000 92%, transparent 100%);
  mask-image: linear-gradient(180deg, transparent 0, #000 7%, #000 92%, transparent 100%);
}

.lyrics-list {
  /* 首尾各留约半屏：第一行/最后一行也能滚到居中位（fill 后 cqh = 歌词视口高） */
  padding: 45cqh 0 52cqh;
}

.lyrics-empty {
  padding: var(--space-12) 0;
  text-align: center;
  color: var(--muted-foreground);
}

.lyric-line {
  padding: 0.6rem var(--space-4);
  text-align: center;
  cursor: pointer;
  opacity: var(--line-o, 1);
  filter: blur(var(--line-blur, 0px));
  /* 焦点行放大走 transform：不触发重排，随滚动逐帧取景 */
  transform: scale(var(--line-scale, 1));
  /* 变量由 rAF 逐帧直写，短线性过渡仅作低通平滑 */
  transition:
    opacity 150ms linear,
    filter 150ms linear;
}

/* hover 快速提亮找行，移出慢速沉回景深 */
.lyric-line:hover {
  opacity: max(var(--line-o, 1), 0.85);
  transition-duration: var(--duration-fast);
}

.is-disabled .lyric-line {
  cursor: default;
}

.is-disabled .lyric-line:hover {
  opacity: var(--line-o, 1);
  transition-duration: var(--duration-slow);
}

.lyric-text {
  font-size: clamp(1.3rem, 0.95rem + 0.85vw, 1.6rem);
  line-height: 1.5;
  font-weight: 600;
  transition: color var(--duration-normal) var(--ease-standard);
}

.lyric-line.is-current .lyric-text {
  color: var(--track-accent);
  font-weight: 700;
}

/* —— 逐字扫色：done 全色（accent），active 用 --p 软边扇形推进，todo 未唱（中性） ——
   软边端点设计成 p=0/p=1 时与 todo/done 颜色像素全等（边带完全推出文字外），
   且 .word 不做 color 过渡——起唱/收字的类切换瞬间不产生任何闪变 */
.word {
  border-radius: 0.1em;
}

.word.is-done {
  color: var(--track-accent);
}

.word.is-todo {
  color: color-mix(in srgb, var(--foreground) 58%, transparent);
}

.word.is-active {
  color: transparent;
  background: linear-gradient(
    90deg,
    var(--track-accent) calc(var(--p, 0) * (100% + 0.24em) - 0.12em),
    color-mix(in srgb, var(--foreground) 58%, transparent)
      calc(var(--p, 0) * (100% + 0.24em))
  );
  -webkit-background-clip: text;
  background-clip: text;
}

.lyric-trans {
  margin-top: 0.3em;
  font-size: 0.78em;
  font-weight: 500;
  color: var(--muted-foreground);
}
</style>
