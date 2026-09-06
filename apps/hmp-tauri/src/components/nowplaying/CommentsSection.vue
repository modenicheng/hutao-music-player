<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from "vue";
import AppIcon from "../AppIcon.vue";
import favoriteOutlineIcon from "../../assets/icons/favorite-outline-rounded.svg?raw";
import favoriteFilledIcon from "../../assets/icons/favorite-filled-rounded.svg?raw";
import { api, type Comment, type CommentSection } from "../../lib/api/index.ts";

/**
 * 评论区（DESIGN.md §3.1.6）：吸顶节头 + 下划线排序 tab +
 * 发丝线分隔的编辑部式列表；引用块为半透明板（随主题/专辑底自适应）；
 * 输入框为 mock（不发送）。
 * 节头无面板底（流内完全透明）；仅吸顶后淡入全宽同色渐隐幕防正文穿越。
 */
const props = defineProps<{ mid: string }>();

const section = ref<CommentSection | null>(null);
const loading = ref(false);
const sort = ref<"hot" | "new">("hot");
const likedIds = ref(new Set<string>());
const headSentinel = ref<HTMLElement | null>(null);
const headStuck = ref(false);

let headObserver: IntersectionObserver | null = null;
onMounted(() => {
  headObserver = new IntersectionObserver(
    ([entry]) => {
      headStuck.value = !entry.isIntersecting;
    },
    { threshold: 0 },
  );
  if (headSentinel.value) headObserver.observe(headSentinel.value);
});
onUnmounted(() => headObserver?.disconnect());

async function load(mid: string) {
  loading.value = true;
  try {
    section.value = await api.comment.section(mid);
  } catch {
    section.value = null;
  } finally {
    loading.value = false;
  }
}

onMounted(() => void load(props.mid));
watch(
  () => props.mid,
  (mid) => void load(mid),
);

const visibleComments = ref<Comment[]>([]);
watch(
  [section, sort],
  () => {
    if (!section.value) {
      visibleComments.value = [];
      return;
    }
    visibleComments.value = sort.value === "hot"
      ? section.value.hot
      : section.value.latest;
  },
  { immediate: true },
);

function formatCount(count: number) {
  if (count >= 10000) return `${(count / 10000).toFixed(1)}万`;
  return String(count);
}

function toggleLike(comment: Comment) {
  const next = new Set(likedIds.value);
  if (next.has(comment.id)) next.delete(comment.id);
  else next.add(comment.id);
  likedIds.value = next;
}

function isLiked(comment: Comment) {
  return likedIds.value.has(comment.id);
}
</script>

<template>
  <section class="comments-section">
    <div ref="headSentinel" class="head-sentinel" aria-hidden="true"></div>
    <header class="comments-head" :class="{ 'is-stuck': headStuck }">
      <h2 class="comments-title">
        评论
        <span v-if="section" class="comments-total">{{ formatCount(section.total) }}</span>
      </h2>
      <div class="sort-tabs" role="tablist" aria-label="评论排序">
        <button
          class="sort-tab"
          :class="{ 'is-active': sort === 'hot' }"
          role="tab"
          :aria-selected="sort === 'hot'"
          @click="sort = 'hot'"
        >最热</button>
        <button
          class="sort-tab"
          :class="{ 'is-active': sort === 'new' }"
          role="tab"
          :aria-selected="sort === 'new'"
          @click="sort = 'new'"
        >最新</button>
      </div>
    </header>

    <div class="comment-composer">
      <input
        class="composer-input"
        type="text"
        placeholder="随乐一想，发表你的评论…"
        aria-label="评论输入框（演示）"
      />
      <button class="composer-send" disabled>发送</button>
    </div>

    <p v-if="loading && !section" class="comments-loading">评论加载中…</p>

    <ul v-else class="comment-list">
      <li
        v-for="comment in visibleComments"
        :key="comment.id"
        class="comment"
      >
        <img class="comment-avatar" :src="comment.user.avatarUrl" :alt="comment.user.name" loading="lazy" />
        <div class="comment-body">
          <div class="comment-meta">
            <span class="comment-name">{{ comment.user.name }}</span>
            <span v-if="comment.location" class="comment-location">{{ comment.location }}</span>
            <span v-if="comment.isPinned" class="comment-pin-badge">置顶</span>
          </div>
          <p class="comment-content">{{ comment.content }}</p>

          <div v-if="comment.replies.length > 0" class="comment-replies">
            <div v-for="reply in comment.replies" :key="reply.id" class="reply">
              <span class="reply-name">{{ reply.user.name }}：</span>{{ reply.content }}
            </div>
            <button v-if="(comment.replyCount ?? 0) > comment.replies.length" class="reply-more">
              共 {{ formatCount(comment.replyCount ?? 0) }} 条回复
              <span class="reply-more-arrow" aria-hidden="true"></span>
            </button>
          </div>

          <div class="comment-foot">
            <span class="comment-time">{{ comment.time }}</span>
            <div class="comment-actions">
              <button
                class="comment-like"
                :class="{ 'is-liked': isLiked(comment) }"
                :title="isLiked(comment) ? '取消点赞' : '点赞'"
                @click="toggleLike(comment)"
              >
                <AppIcon :src="isLiked(comment) ? favoriteFilledIcon : favoriteOutlineIcon" class="like-icon" />
                <span class="like-count">{{ formatCount(comment.likes + (isLiked(comment) ? 1 : 0)) }}</span>
              </button>
              <button class="comment-reply-btn" title="回复">回复</button>
            </div>
          </div>
        </div>
      </li>
    </ul>
  </section>
</template>

<style scoped>
.comments-section {
  max-width: 46rem;
  margin: 0 auto;
  padding: 0 var(--space-6) var(--space-12);
}

/* —— 吸顶节头：标题 + 下划线 tab（呼应刻度进度条的器具感）——
   无面板底：流内完全透明（白条浮在氛围上即"框"）；吸顶后才淡入
   全宽同色渐隐幕（纵向渐透明、无 blur 无边缘），防正文从标题下穿越 */
.head-sentinel {
  height: 1px;
  margin-bottom: -1px;
}

.comments-head {
  position: sticky;
  top: 0;
  z-index: var(--z-sticky);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  padding: 0.8rem var(--space-2) 0.6rem;
}

.comments-head::before {
  content: "";
  position: absolute;
  top: 0;
  /* 负向水平外扩铺满视口宽，容器裁剪兜底；高度延伸至头下 ~4.5rem 渐透明 */
  left: -50vw;
  right: -50vw;
  height: calc(100% + 4.5rem);
  background: linear-gradient(180deg, var(--track-grad-to) 38%, transparent 100%);
  opacity: 0;
  pointer-events: none;
  transition: opacity var(--duration-normal) var(--ease-standard);
}

.comments-head.is-stuck::before {
  opacity: 1;
}

.comments-title {
  /* 提到渐隐幕（::before，绝对定位）之上，否则吸顶时标题被幕布盖住 */
  position: relative;
  display: flex;
  align-items: baseline;
  gap: 0.5rem;
  font-size: 1.08rem;
  font-weight: 700;
  letter-spacing: 0.01em;
}

.comments-total {
  color: var(--muted-foreground);
  font-weight: 400;
  font-size: 0.85rem;
  font-variant-numeric: tabular-nums;
}

.sort-tabs {
  display: flex;
  gap: var(--space-5);
}

.sort-tab {
  position: relative;
  padding: 0.3rem 0.1rem;
  font-size: 0.88rem;
  color: var(--muted-foreground);
  transition: color var(--duration-fast) var(--ease-standard);
}

.sort-tab::after {
  content: "";
  position: absolute;
  left: 0;
  right: 0;
  bottom: -0.4rem;
  height: 2px;
  border-radius: 1px;
  background: var(--track-accent);
  transform: scaleX(0);
  transform-origin: center;
  transition: transform var(--duration-normal) var(--ease-standard);
}

.sort-tab:hover {
  color: var(--foreground);
}

.sort-tab.is-active {
  color: var(--foreground);
  font-weight: 600;
}

.sort-tab.is-active::after {
  transform: scaleX(1);
}

.sort-tab:focus-visible,
.composer-input:focus-visible,
.composer-send:focus-visible,
.comment-like:focus-visible,
.comment-reply-btn:focus-visible,
.reply-more:focus-visible {
  outline: 2px solid var(--ring);
  outline-offset: 2px;
  border-radius: var(--radius-sm);
}

/* —— 输入行：胶囊输入 + 文字按钮，弱于内容 —— */
.comment-composer {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-4) var(--space-2) var(--space-2);
}

.composer-input {
  flex: 1;
  min-width: 0;
  padding: 0.5rem 1rem;
  /* 半透明前景混色：随明暗主题与专辑底色自适应，不用实心灰板 */
  background: color-mix(in srgb, var(--foreground) 5%, transparent);
  border: 1px solid color-mix(in srgb, var(--foreground) 9%, transparent);
  border-radius: var(--radius-full);
  color: var(--foreground);
  transition: border-color var(--duration-fast) var(--ease-standard);
}

.composer-input::placeholder {
  color: var(--muted-foreground);
}

.composer-input:focus {
  border-color: color-mix(in srgb, var(--track-accent) 55%, transparent);
}

.composer-send {
  flex: 0 0 auto;
  padding: 0.5rem 0.4rem;
  /* 播放页内动作色一律跟专辑层，品牌红不得进入 overlay（DESIGN.md §1.2 v0.3） */
  color: var(--track-accent);
  font-size: 0.88rem;
  opacity: 0.45;
  cursor: not-allowed;
}

.comments-loading {
  padding: var(--space-8) 0;
  text-align: center;
  color: var(--muted-foreground);
}

/* —— 列表：发丝线分隔的编辑部式排版，一条评论一行一事 —— */
.comment-list {
  display: grid;
}

.comment {
  display: flex;
  gap: var(--space-3);
  padding: var(--space-5) var(--space-2);
}

.comment + .comment {
  border-top: 1px solid color-mix(in srgb, var(--foreground) 7%, transparent);
}

.comment-avatar {
  width: 2.25rem;
  height: 2.25rem;
  flex: 0 0 auto;
  border-radius: 50%;
  object-fit: cover;
}

.comment-body {
  min-width: 0;
  flex: 1;
}

/* 身份行：昵称 + 属地 + 置顶徽标 */
.comment-meta {
  display: flex;
  align-items: baseline;
  gap: 0.55rem;
  line-height: 1.4;
}

.comment-name {
  font-size: 0.9rem;
  font-weight: 600;
  color: var(--foreground);
}

.comment-location {
  font-size: 0.78rem;
  color: var(--muted-foreground);
}

.comment-pin-badge {
  padding: 0.05rem 0.45rem;
  font-size: 0.68rem;
  line-height: 1.5;
  color: var(--track-accent);
  background: var(--track-accent-soft);
  border-radius: var(--radius-full);
  align-self: center;
}

.comment-content {
  margin-top: 0.45rem;
  font-size: 0.95rem;
  line-height: 1.75;
  white-space: pre-wrap;
}

/* 回复引用：半透明板，层级低于正文 */
.comment-replies {
  margin-top: 0.65rem;
  padding: 0.6rem 0.85rem;
  background: color-mix(in srgb, var(--foreground) 4.5%, transparent);
  border-radius: var(--radius-md);
  font-size: 0.86rem;
  line-height: 1.7;
}

.reply + .reply {
  margin-top: 0.35rem;
}

.reply-name {
  font-weight: 600;
}

.reply-more {
  display: inline-flex;
  align-items: center;
  gap: 0.2rem;
  margin-top: 0.45rem;
  color: var(--track-accent);
  font-size: 0.8rem;
  /* 唯一保留下划线的例外：自绘 1px 底线，hover 从左向右展开、移出原路收回 */
  background-image: linear-gradient(currentColor, currentColor);
  background-repeat: no-repeat;
  background-size: 0% 1px;
  background-position: left calc(100% - 0.05em);
  padding-bottom: 1px;
  transition: background-size var(--duration-fast) var(--ease-standard);
}

.reply-more:hover {
  background-size: 100% 1px;
}

.reply-more-arrow {
  width: 0.85em;
  height: 0.85em;
  background: currentColor;
  -webkit-mask: url("../../assets/icons/expand-more-rounded.svg") no-repeat center / contain;
  mask: url("../../assets/icons/expand-more-rounded.svg") no-repeat center / contain;
}

/* 证据行：时间居左，点赞/回复居右——数字右对齐成可扫列 */
.comment-foot {
  display: flex;
  align-items: center;
  gap: var(--space-4);
  margin-top: 0.55rem;
}

.comment-time {
  color: var(--muted-foreground);
  font-size: 0.78rem;
  font-variant-numeric: tabular-nums;
}

.comment-actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-left: auto;
}

.comment-like {
  display: inline-flex;
  align-items: center;
  gap: 0.3rem;
  min-height: 24px;
  margin-right: -0.35rem;
  padding: 0 0.35rem;
  color: var(--muted-foreground);
  font-size: 0.8rem;
  font-variant-numeric: tabular-nums;
  transition: color var(--duration-fast) var(--ease-standard);
}

.comment-like .like-icon {
  width: 0.95rem;
  height: 0.95rem;
}

.comment-like:hover {
  color: var(--track-accent);
}

.comment-like.is-liked {
  color: var(--track-accent);
}

.comment-reply-btn {
  min-height: 24px;
  padding: 0 0.35rem;
  color: var(--muted-foreground);
  font-size: 0.8rem;
  transition: color var(--duration-fast) var(--ease-standard);
}

.comment-reply-btn:hover {
  color: var(--foreground);
}
</style>
