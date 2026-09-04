<script setup lang="ts">
import AppIcon from "../components/AppIcon.vue";
import Button from "../components/Button.vue";
import HoverGroup from "../components/HoverGroup.vue";
import HoverItem from "../components/HoverItem.vue";
import Scroll from "../components/Scroll.vue";
import { computed } from "vue";
import { cycleTheme, themeState } from "../lib/themeStore.ts";
import accountIcon from "../assets/icons/account-circle-rounded.svg?raw";
import addIcon from "../assets/icons/add-rounded.svg?raw";
import downloadIcon from "../assets/icons/download-rounded.svg?raw";
import expandIcon from "../assets/icons/expand-more-rounded.svg?raw";
import favoriteIcon from "../assets/icons/favorite-outline-rounded.svg?raw";
import historyIcon from "../assets/icons/history-rounded.svg?raw";
import homeIcon from "../assets/icons/home-rounded.svg?raw";
import libraryIcon from "../assets/icons/library-music-rounded.svg?raw";
import logoutIcon from "../assets/icons/logout-rounded.svg?raw";
import playlistIcon from "../assets/icons/playlist-play-rounded.svg?raw";
import queueIcon from "../assets/icons/queue-music-rounded.svg?raw";
import settingsIcon from "../assets/icons/settings-rounded.svg?raw";
import autoThemeIcon from "../assets/icons/brightness-auto-rounded.svg?raw";
import lightThemeIcon from "../assets/icons/light-mode-rounded.svg?raw";
import darkThemeIcon from "../assets/icons/dark-mode-rounded.svg?raw";

defineProps<{ collapsed?: boolean }>();
defineEmits<{ "update:collapsed": [value: boolean] }>();

const theme = themeState();
const themeIcon = computed(() =>
  theme.mode === "dark"
    ? darkThemeIcon
    : theme.mode === "light"
      ? lightThemeIcon
      : autoThemeIcon,
);
const themeTitle = computed(
  () =>
    `主题（当前：${theme.mode === "auto" ? "跟随系统" : theme.mode === "light" ? "浅色" : "深色"}，点击切换）`,
);

// emit.call(true, "update:collapsed", true);
</script>

<template>
  <aside class="sidebar">
    <section>
      <div class="account-panel">
        <span class="account-avatar">
          <AppIcon :src="accountIcon" />
        </span>
        <span class="account-copy">
          <strong>Username</strong>
          <small>普通会员</small>
        </span>
        <Button
          variant="ghost"
          size="icon"
          class="account-logout-button"
          title="登出"
        >
          <AppIcon :src="logoutIcon" />
        </Button>
      </div>
    </section>
    <Scroll direction="vertical">
      <HoverGroup
        class="primary-nav"
        role="navigation"
        aria-label="主导航"
        highlight-color="var(--neutral-200)"
        :highlight-opacity="0.6"
      >
        <HoverItem>
          <Button variant="ghost" class="sidebar-button nav-item">
            <AppIcon :src="homeIcon" />
            <span>首页</span>
          </Button>
        </HoverItem>
        <HoverItem>
          <Button variant="ghost" class="sidebar-button nav-item">
            <AppIcon :src="favoriteIcon" />
            <span>我喜欢</span>
          </Button>
        </HoverItem>
        <HoverItem>
          <Button variant="ghost" class="sidebar-button nav-item">
            <AppIcon :src="historyIcon" />
            <span>最近播放</span>
          </Button>
        </HoverItem>
        <HoverItem>
          <Button variant="ghost" class="sidebar-button nav-item">
            <AppIcon :src="downloadIcon" />
            <span>本地和下载</span>
          </Button>
        </HoverItem>
        <HoverItem>
          <Button variant="ghost" class="sidebar-button nav-item">
            <AppIcon :src="libraryIcon" />
            <span>已购音乐</span>
          </Button>
        </HoverItem>
        <HoverItem>
          <Button variant="ghost" class="sidebar-button nav-item">
            <AppIcon :src="queueIcon" />
            <span>试听列表</span>
          </Button>
        </HoverItem>
      </HoverGroup>

      <section class="playlist-area">
        <div class="playlist-section-title">我的歌单</div>

        <section class="playlist-group">
          <div class="playlist-group-heading">
            <Button
              variant="ghost"
              class="sidebar-button playlist-group-button"
            >
              <AppIcon :src="playlistIcon" />
              <span>自建歌单</span>
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="icon-button"
              title="新建歌单"
            >
              <AppIcon :src="addIcon" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="icon-button"
              title="展开歌单"
            >
              <AppIcon :src="expandIcon" />
            </Button>
          </div>
          <div class="playlist-items">
            <Button variant="ghost" class="playlist-item">
              <span class="playlist-cover ceremony-cover"></span>
              <span>军训结营仪式 备选</span>
            </Button>
            <Button variant="ghost" class="playlist-item">
              <span class="playlist-cover night-cover"></span>
              <span>深夜循环</span>
            </Button>
          </div>
        </section>

        <section class="playlist-group">
          <div class="playlist-group-heading">
            <Button
              variant="ghost"
              class="sidebar-button playlist-group-button"
            >
              <AppIcon :src="playlistIcon" />
              <span>收藏歌单</span>
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="icon-button"
              title="展开歌单"
            >
              <AppIcon :src="expandIcon" />
            </Button>
          </div>
          <div class="playlist-items">
            <Button variant="ghost" class="playlist-item">
              <span class="playlist-cover favorites-cover"></span>
              <span>喜欢的收藏</span>
            </Button>
          </div>
        </section>
      </section>
    </Scroll>

    <footer class="sidebar-footer">
      <Button variant="ghost" size="icon" title="设置">
        <AppIcon :src="settingsIcon" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        :title="themeTitle"
        @click="cycleTheme"
      >
        <AppIcon :src="themeIcon" />
      </Button>
    </footer>
  </aside>
</template>

<style scoped>
.sidebar {
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  min-width: 0;
  overflow: hidden;
}

.sidebar-scroll {
  min-height: 0;
  overflow: auto;
}

.account-panel {
  display: flex;
  align-items: center;
  gap: 1rem;
  padding: 0.5rem;
}

.account-avatar {
  display: grid;
  place-items: center;
  width: 2.4rem;
  height: 2.4rem;
  flex: 0 0 auto;
  color: var(--primary);
  background: var(--muted);
  border-radius: var(--radius-sm);
}

.account-avatar .app-icon {
  width: 1.35rem;
  height: 1.35rem;
}

.account-copy {
  display: grid;
  min-width: 0;
  flex: 1;
  line-height: 1.25;
}

.account-copy strong,
.account-copy small,
.playlist-item > span:last-child {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.account-copy strong {
  font-size: 0.95rem;
  font-weight: 600;
}

.account-copy small,
.playlist-section-title,
.nav-item,
.playlist-group-heading,
.playlist-item,
.sidebar-footer {
  color: var(--muted-foreground);
}

.account-copy small {
  font-size: 0.75rem;
}

.primary-nav,
.playlist-items {
  display: grid;
  gap: 0.25rem;
}

/* 主导航改用统一高亮块：去掉逐项按钮自身的 hover 背景，让滑动高亮成为唯一效果 */
.primary-nav :deep(.button:hover:not(:disabled):not([aria-disabled="true"])) {
  background: transparent;
}

/* 让高亮块复制到与按钮一致的圆角 */
.primary-nav :deep(.hover-item) {
  border-radius: var(--radius-md);
}

.sidebar-button {
  justify-content: flex-start;
  width: 100%;
  text-align: left;
}

.nav-item .app-icon,
.playlist-group-button .app-icon,
.sidebar-footer .app-icon {
  width: 1.35rem;
  height: 1.35rem;
}

.playlist-area {
  display: grid;
  gap: 0.5rem;
  margin-top: 1rem;
}

.playlist-section-title {
  padding: 0 0.75rem 0.25rem;
  /* font-size: 0.75rem; */
}

.playlist-group-heading {
  display: flex;
  align-items: center;
  gap: 0.25rem;
}

.playlist-group-button {
  min-width: 0;
  flex: 1;
}

.icon-button {
  flex: 0 0 auto;
}

.playlist-items {
  padding: 0.25rem 0 0.25rem 1.5rem;
}

.playlist-item {
  justify-content: flex-start;
  min-width: 0;
  padding: 0.25rem 0.5rem;
  font-size: 0.8rem;
}

.playlist-cover {
  display: block;
  width: 1.75rem;
  height: 1.75rem;
  flex: 0 0 auto;
  border-radius: var(--radius-sm);
}

.ceremony-cover {
  background: linear-gradient(135deg, #e44b32, #f3b32f);
}

.night-cover {
  background: linear-gradient(135deg, #1e385f, #d49b60);
}

.favorites-cover {
  background: linear-gradient(135deg, #7660a4, #ef9d9d);
}

.sidebar-footer {
  display: flex;
  justify-content: flex-end;
  gap: 0.5rem;
  margin-top: auto;
  padding-top: 1rem;
}
</style>
