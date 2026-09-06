<script setup lang="ts">
import AppIcon from "../components/AppIcon.vue";
import Button from "../components/Button.vue";
import HoverGroup from "../components/HoverGroup.vue";
import HoverItem from "../components/HoverItem.vue";
import Scroll from "../components/Scroll.vue";
import { computed, ref } from "vue";
import { RouterLink, useRoute } from "vue-router";
import { cycleTheme, themeState } from "../lib/themeStore.ts";
import accountIcon from "../assets/icons/account-circle-rounded.svg?raw";
import addIcon from "../assets/icons/add-rounded.svg?raw";
import backIcon from "../assets/icons/arrow-back-rounded.svg?raw";
import bookmarkIcon from "../assets/icons/bookmark-rounded.svg?raw";
import downloadIcon from "../assets/icons/download-rounded.svg?raw";
import expandIcon from "../assets/icons/expand-more-rounded.svg?raw";
import exploreIcon from "../assets/icons/explore-rounded.svg?raw";
import favoriteIcon from "../assets/icons/favorite-outline-rounded.svg?raw";
import historyIcon from "../assets/icons/history-rounded.svg?raw";
import homeIcon from "../assets/icons/home-rounded.svg?raw";
import leaderboardIcon from "../assets/icons/leaderboard-rounded.svg?raw";
import leftPanelCloseIcon from "../assets/icons/left-panel-close-rounded.svg?raw";
import libraryIcon from "../assets/icons/library-music-rounded.svg?raw";
import logoutIcon from "../assets/icons/logout-rounded.svg?raw";
import playlistIcon from "../assets/icons/playlist-play-rounded.svg?raw";
import settingsIcon from "../assets/icons/settings-rounded.svg?raw";
import autoThemeIcon from "../assets/icons/brightness-auto-rounded.svg?raw";
import lightThemeIcon from "../assets/icons/light-mode-rounded.svg?raw";
import darkThemeIcon from "../assets/icons/dark-mode-rounded.svg?raw";

const props = defineProps<{ collapsed?: boolean }>();
const emit = defineEmits<{ "update:collapsed": [value: boolean] }>();

/** 图标栏（niri 窄窗）与完整侧栏的切换 */
const toggleCollapsed = () => emit("update:collapsed", !props.collapsed);

// —— 我的歌单：宽窄两套 UI ——
// 宽侧栏 = 旧分组折叠（分组标题行 + chevron + grid-rows 高度过渡）；
// 窄边栏 = 两级滑动导航（分组入口行 → 左滑进二级：返回行 + 歌单列表）。
// 宽窄切换（collapsed prop）时两套 UI 交叉淡入淡出。
type PlaylistGroupKey = "created" | "favorited";

// 宽侧栏：分组折叠状态（点分组名或箭头切换，带高度过渡）
const createdOpen = ref(true);
const favoritedOpen = ref(true);

// 窄边栏：两级滑动状态
const activeGroup = ref<PlaylistGroupKey | null>(null);
/** 退出动画期间仍需展示原分组的标题/列表，记住最后进入的分组 */
const lastGroup = ref<PlaylistGroupKey>("created");

const createdPlaylists = [
  { name: "军训结营仪式 备选", coverClass: "ceremony-cover" },
  { name: "深夜循环", coverClass: "night-cover" },
];
const favoritedPlaylists = [
  { name: "喜欢的收藏", coverClass: "favorites-cover" },
];

const PLAYLIST_GROUPS: Record<PlaylistGroupKey, { title: string; playlists: typeof createdPlaylists }> = {
  created: { title: "自建歌单", playlists: createdPlaylists },
  favorited: { title: "收藏歌单", playlists: favoritedPlaylists },
};

const shownGroup = computed(() => activeGroup.value ?? lastGroup.value);
const shownPlaylists = computed(() => PLAYLIST_GROUPS[shownGroup.value].playlists);

function enterGroup(key: PlaylistGroupKey) {
  lastGroup.value = key;
  activeGroup.value = key;
}

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

// 主导航统一接线：有路由的走 RouterLink，未开通的置灰
const route = useRoute();

interface NavItem {
  label: string;
  icon: string;
  to?: string;
  /** 点击动作（无路由的项） */
  action?: () => void;
  disabled?: boolean;
}

const navItems: NavItem[] = [
  { label: "首页", icon: homeIcon, to: "/home" },
  { label: "发现", icon: exploreIcon, to: "/discover" },
  { label: "排行榜", icon: leaderboardIcon, to: "/top" },
  { label: "我喜欢", icon: favoriteIcon, to: "/library" },
  { label: "最近播放", icon: historyIcon, to: "/library/recent" },
  { label: "本地和下载", icon: downloadIcon, disabled: true },
  { label: "已购音乐", icon: libraryIcon, disabled: true },
];

function isActive(item: NavItem) {
  if (!item.to) return false;
  if (item.to === "/home") return route.path === "/home";
  return route.path === item.to || route.path.startsWith(`${item.to}/`);
}

// emit.call(true, "update:collapsed", true);
</script>

<template>
  <aside class="sidebar" :class="{ 'is-collapsed': collapsed }">
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
      <!-- 整条侧栏滚动区共用一个滑动高亮块：主导航、歌单分组标题行、歌单项都是同一个组的 hover-item，
           滑块跨区连续滑动；"我的歌单"是纯标题（非 hover-item），滑块从上方掠过不停留 -->
      <HoverGroup
        class="sidebar-nav"
        role="navigation"
        aria-label="侧栏导航"
        highlight-color="var(--neutral-300)"
        :highlight-opacity="0.6"
      >
        <HoverItem v-for="item in navItems" :key="item.label">
          <!-- 有路由的渲染成 RouterLink；无路由的纯按钮（禁用/动作） -->
          <Button
            :as="item.to ? RouterLink : 'button'"
            :to="item.to"
            variant="ghost"
            class="sidebar-button nav-item"
            :class="{ 'is-active': isActive(item) }"
            :disabled="item.disabled"
            :title="item.disabled ? '即将上线' : collapsed ? item.label : undefined"
            :aria-current="isActive(item) ? 'page' : undefined"
            @click="item.action?.()"
          >
            <AppIcon :src="item.icon" />
            <span class="nav-label">{{ item.label }}</span>
          </Button>
        </HoverItem>

        <section class="playlist-area">
          <div class="playlist-section-title">我的歌单</div>

          <!-- 宽窄两套歌单 UI：宽 = 分组折叠（旧样式），窄 = 两级滑动；切换交叉淡入 -->
          <Transition name="playlist-swap">
            <!-- 宽侧栏：分组折叠 -->
            <div v-if="!collapsed" key="groups" class="playlist-groups">
              <section class="playlist-group">
                <HoverItem>
                  <div class="playlist-group-heading">
                    <Button
                      variant="ghost"
                      class="sidebar-button playlist-group-button"
                      :aria-expanded="createdOpen"
                      @click="createdOpen = !createdOpen"
                    >
                      <AppIcon :src="playlistIcon" />
                      <span>自建歌单</span>
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="icon-button"
                      title="新建歌单"
                      @click.stop
                    >
                      <AppIcon :src="addIcon" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="icon-button"
                      :title="createdOpen ? '收起歌单' : '展开歌单'"
                      :aria-expanded="createdOpen"
                      @click="createdOpen = !createdOpen"
                    >
                      <AppIcon
                        :src="expandIcon"
                        class="group-chevron"
                        :class="{ 'is-collapsed': !createdOpen }"
                      />
                    </Button>
                  </div>
                </HoverItem>
                <div class="playlist-collapse" :class="{ 'is-collapsed': !createdOpen }">
                  <div class="playlist-items">
                    <HoverItem v-for="playlist in createdPlaylists" :key="playlist.name">
                      <Button variant="ghost" class="playlist-item">
                        <span class="playlist-cover" :class="playlist.coverClass"></span>
                        <span>{{ playlist.name }}</span>
                      </Button>
                    </HoverItem>
                  </div>
                </div>
              </section>

              <section class="playlist-group">
                <HoverItem>
                  <div class="playlist-group-heading">
                    <Button
                      variant="ghost"
                      class="sidebar-button playlist-group-button"
                      :aria-expanded="favoritedOpen"
                      @click="favoritedOpen = !favoritedOpen"
                    >
                      <AppIcon :src="bookmarkIcon" />
                      <span>收藏歌单</span>
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="icon-button"
                      :title="favoritedOpen ? '收起歌单' : '展开歌单'"
                      :aria-expanded="favoritedOpen"
                      @click="favoritedOpen = !favoritedOpen"
                    >
                      <AppIcon
                        :src="expandIcon"
                        class="group-chevron"
                        :class="{ 'is-collapsed': !favoritedOpen }"
                      />
                    </Button>
                  </div>
                </HoverItem>
                <div class="playlist-collapse" :class="{ 'is-collapsed': !favoritedOpen }">
                  <div class="playlist-items">
                    <HoverItem v-for="playlist in favoritedPlaylists" :key="playlist.name">
                      <Button variant="ghost" class="playlist-item">
                        <span class="playlist-cover" :class="playlist.coverClass"></span>
                        <span>{{ playlist.name }}</span>
                      </Button>
                    </HoverItem>
                  </div>
                </div>
              </section>
            </div>

            <!-- 窄边栏：两级滑动导航 -->
            <div v-else key="slider" class="playlist-slider" :class="{ 'is-in': activeGroup !== null }">
              <!-- 一级：分组入口行 -->
              <div class="playlist-pane pane-entries" :inert="activeGroup !== null">
                <HoverItem>
                  <Button
                    variant="ghost"
                    class="sidebar-button playlist-entry"
                    :aria-expanded="activeGroup === 'created'"
                    @click="enterGroup('created')"
                  >
                    <AppIcon :src="playlistIcon" />
                    <span>自建歌单</span>
                  </Button>
                </HoverItem>
                <HoverItem>
                  <Button
                    variant="ghost"
                    class="sidebar-button playlist-entry"
                    :aria-expanded="activeGroup === 'favorited'"
                    @click="enterGroup('favorited')"
                  >
                    <AppIcon :src="bookmarkIcon" />
                    <span>收藏歌单</span>
                  </Button>
                </HoverItem>
              </div>

              <!-- 二级：与主导航同规格的行（返回 + 封面列表），滑块几何一致 -->
              <div class="playlist-pane pane-list" :inert="activeGroup === null">
                <HoverItem>
                  <Button
                    variant="ghost"
                    class="sidebar-button playlist-item playlist-back"
                    aria-label="返回歌单分组"
                    @click="activeGroup = null"
                  >
                    <AppIcon :src="backIcon" />
                  </Button>
                </HoverItem>
                <HoverItem v-for="playlist in shownPlaylists" :key="playlist.name">
                  <Button variant="ghost" class="sidebar-button playlist-item">
                    <span class="playlist-cover" :class="playlist.coverClass"></span>
                    <span>{{ playlist.name }}</span>
                  </Button>
                </HoverItem>
              </div>
            </div>
          </Transition>
        </section>
      </HoverGroup>
    </Scroll>

    <footer class="sidebar-footer">
      <Button
        variant="ghost"
        size="icon"
        class="collapse-toggle"
        :title="collapsed ? '展开侧栏' : '收起侧栏'"
        :aria-expanded="!collapsed"
        @click="toggleCollapsed"
      >
        <AppIcon :src="leftPanelCloseIcon" class="collapse-icon" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        :as="RouterLink"
        to="/settings"
        title="设置"
      >
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
.nav-label,
.playlist-entry span:last-child,
.playlist-group-button span:last-child,
.playlist-item > span:last-child {
  /* 宽窄过渡期间侧栏宽度剧烈变化：文字一律单行截断，防折行导致行高跳动 */
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
.playlist-entry,
.playlist-item,
.sidebar-footer {
  color: var(--muted-foreground);
}

.account-copy small {
  font-size: 0.75rem;
}

.sidebar-nav,
.playlist-items {
  display: grid;
  gap: 0.25rem;
}

/* 整条侧栏共用滑动高亮块：去掉组内所有按钮自身的 hover 背景——
   尤其是激活项上 Button 默认的白色 hover 底（surface-3）会漏出来。
   .hover-item 前缀 + .button 叠类是为了在特异性上稳定压过
   Button.vue 自己的 .button:hover 规则（scoped 编译后 (0,7,0)） */
.sidebar-nav
  :deep(.hover-item .button.button:hover:not(:disabled):not([aria-disabled="true"])) {
  background: transparent;
}

/* 让高亮块复制到与行内容一致的圆角；wrapper 上 flex 消除行内按钮的
   基线缝隙（块级 div 包 inline-flex 按钮会多出零点几到 1px 的行盒下沉，
   滑块高度随之参差） */
.sidebar-nav :deep(.hover-item) {
  display: flex;
  flex-direction: column;
  border-radius: var(--radius-md);
}

.playlist-pane {
  width: 100%;
  display: grid;
  gap: 0.25rem;
  transition: transform var(--duration-normal) var(--ease-standard);
}

.pane-entries {
  position: relative;
  transform: translateX(0);
}

.pane-list {
  position: absolute;
  top: 0;
  left: 0;
  transform: translateX(100%);
}

.playlist-slider.is-in .pane-entries {
  position: absolute;
  transform: translateX(-100%);
}

.playlist-slider.is-in .pane-list {
  position: relative;
  transform: translateX(0);
}

@media (prefers-reduced-motion: reduce) {
  .playlist-pane {
    transition: none;
  }
}

.sidebar-button {
  justify-content: flex-start;
  width: 100%;
  text-align: left;
}

/* 当前页常亮：中性底 + 前景色，HoverGroup 滑动高亮叠加其上；主题色不进导航 */
.nav-item.is-active {
  color: var(--foreground);
  background: var(--muted);
  font-weight: 600;
}

.nav-item .app-icon,
.playlist-entry .app-icon,
.playlist-back .app-icon,
.sidebar-footer .app-icon {
  width: 1.35rem;
  height: 1.35rem;
}

.playlist-area {
  position: relative;
  display: grid;
  gap: 0.5rem;
  margin-top: 1rem;
}

.playlist-section-title {
  /* 提到与 hover-item 同层：滑块跨区滑过时从标题背后穿过，不遮挡文字 */
  position: relative;
  z-index: 1;
  padding: 0 0.75rem 0.25rem;
}

/* —— 宽侧栏：分组折叠（grid-rows 0fr/1fr 高度过渡，箭头随态旋转）—— */
.playlist-groups {
  display: grid;
  gap: 0.5rem;
}

.playlist-collapse {
  display: grid;
  grid-template-rows: 1fr;
  transition: grid-template-rows var(--duration-normal) var(--ease-standard);
}

.playlist-collapse.is-collapsed {
  grid-template-rows: 0fr;
}

.playlist-collapse > .playlist-items {
  min-height: 0;
  overflow: hidden;
}

.group-chevron {
  transition: transform var(--duration-normal) var(--ease-standard);
}

.group-chevron.is-collapsed {
  transform: rotate(-90deg);
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

/* —— 宽窄切换：两套歌单 UI 交叉淡入淡出 ——
   离开侧绝对定位脱流，避免两套节点在 grid 里叠排挤高容器 */
.playlist-swap-enter-active,
.playlist-swap-leave-active {
  transition: opacity var(--duration-fast) var(--ease-standard);
}

.playlist-swap-leave-active {
  position: absolute;
  left: 0;
  right: 0;
}

.playlist-swap-enter-from,
.playlist-swap-leave-to {
  opacity: 0;
}

/* —— 窄边栏：两级滑动 ——
   高度不参与过渡：激活 pane 回到文档流瞬时撑起容器，非激活 pane 绝对定位
   挂在视口外；两侧只做 translateX 滑动，容器 overflow hidden 裁切，
   动画全程无高度重排 */
.playlist-slider {
  position: relative;
  overflow: hidden;
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

/* —— 图标栏（niri 窄窗自动收起）—— */
.collapse-toggle {
  flex: 0 0 auto;
}

.collapse-icon {
  transition: transform var(--duration-fast) var(--ease-standard);
}

.sidebar.is-collapsed .collapse-icon {
  transform: rotate(180deg);
}

.sidebar.is-collapsed {
  padding-inline: 0.25rem;
}

.sidebar.is-collapsed .account-panel,
.sidebar.is-collapsed .account-avatar,
.sidebar.is-collapsed .account-copy,
.sidebar.is-collapsed .account-logout-button,
.sidebar.is-collapsed .nav-label {
  display: none;
}

/* 窄边栏：两级滑动的纯图标形态——隐藏节标题与各处文字，只留图标/封面
   （排除 .app-icon：返回按钮等单图标行的唯一 span 也是 last-child，不能误杀） */
.sidebar.is-collapsed .playlist-section-title,
.sidebar.is-collapsed .playlist-entry span:last-child:not(.app-icon),
.sidebar.is-collapsed .playlist-item > span:last-child:not(.app-icon) {
  display: none;
}

/* 二级行与主导航行同规格：等高、去内距、居中——滑块几何与对齐和上方导航一致 */
.sidebar.is-collapsed .pane-list .playlist-items {
  padding: 0;
}

.sidebar.is-collapsed .pane-list .playlist-item {
  height: 2.5rem;
  padding: 0;
}

.sidebar.is-collapsed .sidebar-button {
  justify-content: center;
  padding-inline: 0;
}

.sidebar.is-collapsed .sidebar-footer {
  flex-direction: column;
  align-items: center;
  justify-content: flex-start;
  gap: 0.25rem;
}
</style>
