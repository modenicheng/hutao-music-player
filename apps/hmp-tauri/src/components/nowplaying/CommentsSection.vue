<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import AppIcon from "../AppIcon.vue";
import favoriteIcon from "../../assets/icons/favorite-outline-rounded.svg?raw";
import { api, type Comment, type CommentSection } from "../../lib/api/index.ts";

/**
 * 评论区（DESIGN.md §3.1.6，网易云式）：吸顶节头 + 排序 tab +
 * 置顶/热评/回复引用块；输入框为 mock（不发送）。
 */
const props = defineProps<{ mid: string }>();

const section = ref<CommentSection | null>(null);
const loading = ref(false);
const sort = ref<"hot" | "new">("hot");
const likedIds = ref(new Set<string>());

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

/** 热评前三（非置顶）的点赞数用 accent 强调 */
function isTopHot(comment: Comment) {
  if (sort.value !== "hot" || comment.isPinned) return false;
  const index = section.value?.hot.indexOf(comment) ?? -1;
  return index >= 0 && index < 3;
}
</script>

<template>
  <section class="comments-section">
    <header class="comments-head">
      <h2 class="comments-title">
        评论
        <span v-if="section" class="comments-total">· {{ formatCount(section.total) }}</span>
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
        :class="{ 'is-pinned': comment.isPinned }"
      >
        <img class="comment-avatar" :src="comment.user.avatarUrl" :alt="comment.user.name" loading="lazy" />
        <div class="comment-body">
          <div class="comment-meta">
            <span class="comment-name">{{ comment.user.name }}</span>
            <span v-if="comment.location" class="comment-location">{{ comment.location }}</span>
            <span class="comment-time">{{ comment.time }}</span>
          </div>
          <p v-if="comment.isPinned" class="comment-pin-badge">置顶</p>
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

          <div class="comment-actions">
            <button
              class="comment-like"
              :class="{ 'is-liked': isLiked(comment), 'is-top': isTopHot(comment) && !isLiked(comment) }"
              :title="isLiked(comment) ? '取消点赞' : '点赞'"
              @click="toggleLike(comment)"
            >
              <AppIcon :src="favoriteIcon" class="like-icon" />
              <span class="like-count">{{ formatCount(comment.likes + (isLiked(comment) ? 1 : 0)) }}</span>
            </button>
            <button class="comment-reply-btn" title="回复">回复</button>
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

.comments-head {
  position: sticky;
  top: 0;
  z-index: var(--z-sticky);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  padding: var(--space-3) var(--space-2);
  /* 吸底色随环境：借用 surface 并保留毛玻璃 */
  background: color-mix(in srgb, var(--surface-2) 82%, transparent);
  backdrop-filter: blur(16px);
  border-bottom: 1px solid var(--border);
}

.comments-title {
  font-size: 1.1rem;
  font-weight: 650;
}

.comments-total {
  color: var(--muted-foreground);
  font-weight: 400;
  font-size: 0.9em;
}

.sort-tabs {
  display: flex;
  gap: 0.25rem;
}

.sort-tab {
  padding: 0.25rem 0.8rem;
  font-size: 0.85rem;
  color: var(--muted-foreground);
  border-radius: var(--radius-full);
  transition:
    background-color var(--duration-fast) var(--ease-standard),
    color var(--duration-fast) var(--ease-standard);
}

.sort-tab.is-active {
  color: var(--track-accent);
  background: var(--track-accent-soft);
  font-weight: 600;
}

.comment-composer {
  display: flex;
  gap: var(--space-2);
  margin: var(--space-4) 0 var(--space-6);
}

.composer-input {
  flex: 1;
  padding: 0.55rem 0.9rem;
  background: var(--input);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  color: var(--foreground);
}

.composer-input::placeholder {
  color: var(--muted-foreground);
}

.composer-send {
  padding: 0 1.1rem;
  color: var(--primary-foreground);
  background: var(--primary);
  border-radius: var(--radius-md);
  opacity: 0.45;
  cursor: not-allowed;
}

.comments-loading {
  padding: var(--space-8) 0;
  text-align: center;
  color: var(--muted-foreground);
}

.comment-list {
  display: grid;
  gap: var(--space-5);
}

.comment {
  display: flex;
  gap: var(--space-3);
  padding: var(--space-3);
  border-radius: var(--radius-lg);
  transition: background-color var(--duration-fast) var(--ease-standard);
}

.comment:hover {
  background: var(--track-accent-soft);
}

/* 置顶评论：accent 左边条 */
.comment.is-pinned {
  box-shadow: inset 3px 0 0 var(--track-accent);
}

.comment-avatar {
  width: 2rem;
  height: 2rem;
  flex: 0 0 auto;
  border-radius: 50%;
  object-fit: cover;
}

.comment-body {
  min-width: 0;
  flex: 1;
}

.comment-meta {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  font-size: 0.8rem;
}

.comment-name {
  font-weight: 600;
  color: var(--foreground);
}

.comment-location,
.comment-time {
  color: var(--muted-foreground);
}

.comment-time {
  margin-left: auto;
  font-variant-numeric: tabular-nums;
}

.comment-pin-badge {
  display: inline-block;
  margin-top: 0.35rem;
  padding: 0 0.45rem;
  font-size: 0.68rem;
  line-height: 1.5;
  color: var(--track-accent);
  background: var(--track-accent-soft);
  border-radius: var(--radius-sm);
}

.comment-content {
  margin-top: 0.3rem;
  font-size: 0.92rem;
  line-height: 1.65;
  white-space: pre-wrap;
}

.comment-replies {
  margin-top: 0.5rem;
  padding: 0.5rem 0.75rem;
  background: var(--muted);
  border-radius: var(--radius-md);
  font-size: 0.84rem;
  line-height: 1.6;
}

.reply-name {
  font-weight: 600;
}

.reply-more {
  display: inline-flex;
  align-items: center;
  gap: 0.2rem;
  margin-top: 0.3rem;
  color: var(--track-accent);
  font-size: 0.8rem;
}

.reply-more-arrow {
  width: 0.8em;
  height: 0.8em;
  background: currentColor;
  -webkit-mask: url("../../assets/icons/expand-more-rounded.svg") no-repeat center / contain;
  mask: url("../../assets/icons/expand-more-rounded.svg") no-repeat center / contain;
}

.comment-actions {
  display: flex;
  align-items: center;
  gap: var(--space-4);
  margin-top: 0.5rem;
}

.comment-like {
  display: inline-flex;
  align-items: center;
  gap: 0.3rem;
  color: var(--muted-foreground);
  font-size: 0.8rem;
  transition: color var(--duration-fast) var(--ease-standard);
}

.comment-like .like-icon {
  width: 1rem;
  height: 1rem;
}

.comment-like.is-liked {
  color: var(--track-accent);
}

.comment-like.is-top {
  color: var(--track-accent);
}

.like-count {
  font-variant-numeric: tabular-nums;
}

.comment-reply-btn {
  color: var(--muted-foreground);
  font-size: 0.8rem;
}

.comment-reply-btn:hover {
  color: var(--foreground);
}
</style>
