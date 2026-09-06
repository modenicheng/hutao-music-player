<script setup lang="ts">
import { computed, ref, watch } from "vue";
import AppIcon from "../AppIcon.vue";
import RulerProgress from "../RulerProgress.vue";
import LyricsPane from "./LyricsPane.vue";
import CommentsSection from "./CommentsSection.vue";
import VolumeControl from "../VolumeControl.vue";
import QualityBadge from "../QualityBadge.vue";
import Scroll from "../Scroll.vue";
import {
  PlayerControlStatus,
  type PlayerController,
} from "../../lib/player";
import { api, type Lyrics } from "../../lib/api/index.ts";
import { findSong } from "../../lib/api/mock-data.ts";
import playIcon from "../../assets/icons/play_arrow-rounded.svg?raw";
import pauseIcon from "../../assets/icons/pause-rounded.svg?raw";
import skipNextIcon from "../../assets/icons/skip-next-rounded.svg?raw";
import skipPreviousIcon from "../../assets/icons/skip-previous-rounded.svg?raw";
import shuffleIcon from "../../assets/icons/shuffle-rounded.svg?raw";
import repeatIcon from "../../assets/icons/repeat-rounded.svg?raw";
import queueIcon from "../../assets/icons/queue-music-outline-rounded.svg?raw";
import favoriteIcon from "../../assets/icons/favorite-outline-rounded.svg?raw";
import favoriteFilledIcon from "../../assets/icons/favorite-filled-rounded.svg?raw";
import commentIcon from "../../assets/icons/comment-outline-rounded.svg?raw";

/**
 * 播放页主体：环境层 + 首屏舞台（封面，信息随其下 | 右侧整列歌词），
 * 上滑进入网易云式评论区；底部控制台（刻度进度条 + 控制排）常驻不随内容滚走。
 * 由全屏播放层（PlayerOverlay）独占使用。
 */
const props = defineProps<{ player: PlayerController }>();

const shuffleOn = ref(false);
const repeatOn = ref(false);
const lyrics = ref<Lyrics | null>(null);
const npRoot = ref<HTMLElement | null>(null);

const track = computed(() => props.player.state.currentTrack);
const songDetail = computed(() =>
  track.value ? findSong(track.value.mid) ?? null : null,
);

// 喜欢态：mock（数据层未就绪），仅视觉演示——换曲即回落未点亮
const liked = ref(false);
watch(
  () => track.value?.mid ?? null,
  () => (liked.value = false),
);

// —— 歌词随曲目加载（调色由 App 级 trackTheme 全局负责）——
watch(
  () => track.value?.mid ?? null,
  async (mid) => {
    lyrics.value = null;
    if (!mid) return;
    try {
      lyrics.value = await api.lyrics.get(mid);
    } catch {
      lyrics.value = null;
    }
  },
  { immediate: true },
);

function scrollToComments() {
  npRoot.value
    ?.querySelector<HTMLElement>("#comments-anchor")
    ?.scrollIntoView({ behavior: "smooth", block: "start" });
}
</script>

<template>
  <div ref="npRoot" class="now-playing">
    <!-- 环境层：渐变 + 封面放大模糊，光从音乐里透出来 -->
    <div class="ambient" aria-hidden="true">
      <img
        v-if="track?.coverUrl"
        class="ambient-cover"
        :src="track.coverUrl"
        alt=""
      />
      <div class="ambient-tint"></div>
    </div>

    <div class="np-shell">
      <Scroll direction="vertical" class="np-scroll" fill>
        <template v-if="track">
          <!-- 首屏舞台：封面（信息随其下）| 右侧整列歌词；恰好一屏，上滑进评论 -->
          <section class="np-stage">
            <div class="np-stage-left">
              <img
                v-if="track.coverUrl"
                class="np-cover"
                :src="track.coverUrl"
                :alt="`《${track.title}》专辑封面`"
              />
              <div class="np-meta">
                <div class="np-title-row">
                  <h1 class="np-title">{{ track.title }}</h1>
                  <button
                    class="np-like"
                    :class="{ 'is-liked': liked }"
                    :title="liked ? '取消喜欢' : '喜欢'"
                    :aria-label="liked ? '取消喜欢' : '喜欢'"
                    :aria-pressed="liked"
                    @click="liked = !liked"
                  >
                    <AppIcon
                      class="np-like-icon"
                      :src="liked ? favoriteFilledIcon : favoriteIcon"
                    />
                  </button>
                </div>
                <p class="np-artists">
                  <!-- 歌手链接需要 mid：songDetail 未就绪时退化为纯文本 -->
                  <template v-if="songDetail">
                    <template v-for="(artist, i) in songDetail.artists" :key="artist.mid">
                      <span v-if="i > 0" class="artist-sep">/</span>
                      <RouterLink :to="`/artist/${artist.mid}`" class="np-artist text-link">{{ artist.name }}</RouterLink>
                    </template>
                  </template>
                  <template v-else>
                    <template v-for="(artist, i) in track.artists" :key="artist">
                      <span v-if="i > 0" class="artist-sep">/</span>
                      <span>{{ artist }}</span>
                    </template>
                  </template>
                </p>
                <p v-if="songDetail" class="np-album">
                  <RouterLink :to="`/album/${songDetail.album.mid}`" class="np-album-link text-link">
                    {{ songDetail.album.name }}
                  </RouterLink>
                </p>
              </div>
            </div>

            <div class="np-stage-right">
              <div id="lyrics-anchor" class="np-lyrics">
                <LyricsPane
                  class="np-lyrics-pane"
                  :lyrics="lyrics"
                  :position-ms="player.state.positionMs"
                  :playing="player.state.playing"
                  @seek="(timeMs) => player.seek(timeMs)"
                />
              </div>
            </div>
          </section>

          <!-- 评论区（继续上滑，网易云式） -->
          <section id="comments-anchor" class="np-comments">
            <CommentsSection :mid="track.mid" />
          </section>
        </template>

        <div v-else class="np-empty">
          <p class="np-empty-title">还没有播放中的歌曲</p>
          <p class="np-empty-hint">从发现、搜索或歌单里挑一首开始吧</p>
        </div>
      </Scroll>

      <!-- 底部控制台：常驻，不随内容滚动 -->
      <footer class="np-console">
        <div class="np-ruler">
          <RulerProgress
            :progress="player.state.progress"
            :duration-ms="player.state.durationMs"
            :disabled="player.state.controlStatus === PlayerControlStatus.dragging"
            @seek="(percent) => player.seekToPercent(percent)"
          />
        </div>

        <div class="np-console-row">
          <div class="np-console-side np-console-left">
            <QualityBadge :track-quality="songDetail?.quality" align="up" />
            <VolumeControl :player="player" />
          </div>

          <div class="np-main-controls">
            <button
              class="mode-button"
              :class="{ 'is-on': shuffleOn }"
              :title="shuffleOn ? '随机播放：开' : '随机播放：关'"
              @click="shuffleOn = !shuffleOn"
            >
              <AppIcon :src="shuffleIcon" />
            </button>
            <button class="control-button" title="上一曲" @click="player.previous">
              <AppIcon :src="skipPreviousIcon" />
            </button>
            <button
              class="play-button"
              :title="player.state.playing ? '暂停' : '播放'"
              @click="player.togglePlay"
            >
              <AppIcon :src="player.state.playing ? pauseIcon : playIcon" />
            </button>
            <button class="control-button" title="下一曲" @click="player.next">
              <AppIcon :src="skipNextIcon" />
            </button>
            <button
              class="mode-button"
              :class="{ 'is-on': repeatOn }"
              :title="repeatOn ? '循环播放：开' : '循环播放：关'"
              @click="repeatOn = !repeatOn"
            >
              <AppIcon :src="repeatIcon" />
            </button>
          </div>

          <div class="np-console-side np-console-right">
            <!-- 纯图标跳转键：outlined 轻量风格，语义由 title/aria-label 承载 -->
            <button
              class="jump-button"
              title="跳到评论"
              aria-label="跳到评论"
              @click="scrollToComments"
            >
              <AppIcon :src="commentIcon" class="jump-icon" />
            </button>
            <button
              class="jump-button"
              title="播放列表"
              aria-label="播放列表"
              @click="player.toggleQueue"
            >
              <AppIcon :src="queueIcon" class="jump-icon" />
            </button>
          </div>
        </div>
      </footer>
    </div>
  </div>
</template>

<style scoped>
.now-playing {
  position: relative;
  height: 100%;
  min-height: 0;
  color: var(--foreground);
}

/* —— 环境层 —— */
.ambient {
  position: absolute;
  inset: 0;
  overflow: hidden;
  background: linear-gradient(180deg, var(--track-grad-from), var(--track-grad-to));
  transition: background var(--duration-slow) var(--ease-standard);
}

.ambient-cover {
  position: absolute;
  inset: -12%;
  width: 124%;
  height: 124%;
  object-fit: cover;
  filter: blur(80px) saturate(1.2);
  opacity: 0.25;
}

.ambient-tint {
  position: absolute;
  inset: 0;
  background: linear-gradient(180deg, transparent 40%, var(--track-grad-to) 100%);
}

.np-shell {
  position: relative;
  z-index: 1;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}

.np-scroll {
  flex: 1;
  min-height: 0;
}

/* —— 首屏舞台：封面（信息随其下）| 右侧整列歌词；100cqh 取 Scroll 视口高度 ——
   必须定高：歌词列内容很长，flex 行高会取其内容高度把舞台撑爆；
   定高后右列在确定高度内自滚，min-height 兜底极矮窗口 */
.np-stage {
  display: flex;
  align-items: stretch;
  gap: var(--space-8);
  height: 100cqh;
  min-height: 30rem;
  max-width: 68rem;
  margin: 0 auto;
  /* 侧距加大：左列离窗口边框更远，窄窗下不贴边 */
  padding: var(--space-8) var(--space-10) var(--space-6);
}

.np-stage-left {
  display: flex;
  flex-direction: column;
  justify-content: center;
  flex: 0 0 auto;
  width: min(36vh, 40%, 21rem);
}

.np-cover {
  width: 100%;
  aspect-ratio: 1 / 1;
  object-fit: cover;
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
}

.np-meta {
  min-width: 0;
  margin-top: var(--space-5);
}

.np-title-row {
  /* 标题字号上提为变量：喜欢图标按比例放大，与标题字形视觉对齐 */
  --title-size: clamp(1.8rem, 4vw, 2.6rem);
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.np-title {
  flex: 1 1 0;
  min-width: 0;
  font-size: var(--title-size);
  font-weight: 700;
  line-height: 1.25;
  overflow: hidden;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
}

/* 裸图标：无底无框，hover 只变色；负右距让图形贴齐列右缘 */
.np-like {
  --like-red: #e5484d;
  display: grid;
  place-items: center;
  flex: 0 0 auto;
  width: 2.5rem;
  height: 2.5rem;
  margin-right: -0.5rem;
  color: var(--foreground);
  transition: color var(--duration-fast) var(--ease-standard);
}

.np-like-icon {
  width: calc(var(--title-size) * 0.8);
  height: calc(var(--title-size) * 0.8);
}

.np-like:hover {
  color: var(--track-accent);
}

/* 点亮：红色实心，hover 加深 */
.np-like.is-liked,
.np-like.is-liked:hover {
  color: var(--like-red);
}

.np-like.is-liked:hover {
  color: #d13a3f;
}

.np-artists {
  margin-top: 0.5rem;
  font-size: 1.05rem;
  color: var(--muted-foreground);
}

.artist-sep {
  margin: 0 0.4em;
  opacity: 0.5;
}

.np-album {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--space-2);
  margin-top: 0.35rem;
  font-size: 0.92rem;
  color: var(--muted-foreground);
}

/* —— 歌词列：随舞台拉满一屏高度，内部自滚 —— */
.np-stage-right {
  display: flex;
  flex-direction: column;
  flex: 1 1 0;
  min-width: 0;
}

.np-lyrics {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
}

.np-lyrics-pane {
  flex: 1;
  min-height: 0;
}

.np-comments {
  /* 评论区接在歌词之后，继续上滑自然抵达；落在渐变端色上，可读性优先。
     环境层固定在视口而本节随内容滚动，实心底会与之撞出移动的硬接缝，
     顶端用透明→端色过渡带让阅读面从舞台氛围里浮现 */
  background: linear-gradient(180deg, transparent, var(--track-grad-to) 7rem);
}

/* —— 底部控制台 —— */
.np-console {
  flex: 0 0 auto;
  padding: var(--space-2) var(--space-6) var(--space-3);
  background: color-mix(in srgb, var(--track-grad-to) 72%, transparent);
  backdrop-filter: blur(18px) saturate(1.1);
  border-top: 1px solid color-mix(in srgb, var(--foreground) 8%, transparent);
}

.np-ruler {
  max-width: 72rem;
  margin: 0 auto;
}

.np-console-row {
  display: flex;
  align-items: center;
  gap: var(--space-4);
  max-width: 72rem;
  margin: 0 auto;
  padding-top: var(--space-2);
}

.np-console-side {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex: 1;
  min-width: 0;
}

.np-console-right {
  justify-content: flex-end;
}

.np-main-controls {
  display: flex;
  align-items: center;
  gap: var(--space-4);
}

.control-button {
  display: grid;
  place-items: center;
  width: 2.5rem;
  height: 2.5rem;
  color: var(--foreground);
  border-radius: var(--radius-full);
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.control-button .app-icon {
  width: 1.4rem;
  height: 1.4rem;
}

.control-button:hover {
  background: var(--track-accent-soft);
}

/* 主播放键：48px 圆，曲目层强调色——仍最大但不过分高出邻键 */
.play-button {
  display: grid;
  place-items: center;
  width: 3rem;
  height: 3rem;
  color: var(--track-on-accent);
  background: var(--track-accent);
  border-radius: var(--radius-full);
  box-shadow: var(--shadow-md);
  transition:
    transform var(--duration-fast) var(--ease-standard),
    box-shadow var(--duration-fast) var(--ease-standard),
    background-color var(--duration-normal) var(--ease-standard);
}

.play-button .app-icon {
  width: 1.5rem;
  height: 1.5rem;
}

.play-button:hover {
  transform: scale(1.04);
  box-shadow: var(--shadow-lg);
}

.play-button:active {
  transform: scale(0.98);
}

.mode-button {
  display: grid;
  place-items: center;
  width: 2.25rem;
  height: 2.25rem;
  color: var(--muted-foreground);
  border-radius: var(--radius-full);
  transition:
    color var(--duration-fast) var(--ease-standard),
    background-color var(--duration-fast) var(--ease-standard);
}

.mode-button .app-icon {
  width: 1.15rem;
  height: 1.15rem;
}

.mode-button:hover {
  color: var(--foreground);
  background: var(--track-accent-soft);
}

.mode-button.is-on {
  color: var(--track-accent);
}

/* 纯图标跳转键：与控制键同规格的方形命中区 */
.jump-button {
  display: grid;
  place-items: center;
  flex: 0 0 auto;
  width: 2.25rem;
  height: 2.25rem;
  color: var(--foreground);
  border-radius: var(--radius-full);
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.jump-icon {
  width: 1.3rem;
  height: 1.3rem;
}

.jump-button:hover {
  background: var(--track-accent-soft);
}

.np-empty {
  padding: var(--space-12) var(--space-6);
  text-align: center;
}

.np-empty-title {
  font-size: 1.3rem;
  font-weight: 650;
}

.np-empty-hint {
  margin-top: 0.5rem;
  color: var(--muted-foreground);
}

/* 窄窗兜底：舞台竖排（封面+信息居中、歌词接其下），控制排允许换行 */
@media (max-width: 48rem) {
  .np-stage {
    flex-direction: column;
    align-items: center;
  }

  .np-stage-left {
    width: min(34vh, 72vw, 20rem);
  }

  .np-stage-right {
    width: 100%;
    /* 弹性填满舞台剩余高度：定高会把居中基准沉到控制台底下 */
    flex: 1 1 0;
    min-height: 12rem;
  }

  .np-console-row {
    flex-wrap: wrap;
  }

  .np-console-side {
    flex-basis: 100%;
    justify-content: center;
    order: 2;
  }

  .np-main-controls {
    order: 1;
    margin: 0 auto;
  }
}
</style>
