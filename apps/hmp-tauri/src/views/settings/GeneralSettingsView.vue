<script setup lang="ts">
import { computed } from "vue";
import AppIcon from "../../components/AppIcon.vue";
import PageHeader from "../../components/PageHeader.vue";
import SettingsNav from "./SettingsNav.vue";
import { setThemeMode, themeState } from "../../lib/themeStore.ts";
import type { ThemeMode } from "../../lib/theme.ts";
import autoThemeIcon from "../../assets/icons/brightness-auto-rounded.svg?raw";
import lightThemeIcon from "../../assets/icons/light-mode-rounded.svg?raw";
import darkThemeIcon from "../../assets/icons/dark-mode-rounded.svg?raw";

/** 常规设置：主题模式真实生效；启动页与语言未接线的档位一律禁用展示 */
const theme = themeState();

const themeOptions: Array<{ mode: ThemeMode; label: string; icon: string }> = [
  { mode: "auto", label: "跟随系统", icon: autoThemeIcon },
  { mode: "light", label: "浅色", icon: lightThemeIcon },
  { mode: "dark", label: "深色", icon: darkThemeIcon },
];

const resolvedLabel = computed(() =>
  theme.mode === "auto" ? `跟随系统（当前 ${theme.resolved === "light" ? "浅色" : "深色"}）` : theme.mode === "light" ? "浅色" : "深色",
);
</script>

<template>
  <div class="general-settings-view">
    <PageHeader title="常规设置" back-to="/settings" />
    <SettingsNav />

    <section class="settings-group" aria-labelledby="appearance-title">
      <h2 id="appearance-title" class="group-title">外观</h2>

      <div class="setting-row">
        <div class="setting-copy">
          <span class="setting-label">主题模式</span>
          <span class="setting-desc">品牌胡桃木色阶不变，切换的只是明暗两态</span>
        </div>
        <div class="theme-options" role="radiogroup" aria-label="主题模式">
          <button
            v-for="option in themeOptions"
            :key="option.mode"
            type="button"
            role="radio"
            class="theme-option"
            :class="{ 'is-active': theme.mode === option.mode }"
            :aria-checked="theme.mode === option.mode"
            @click="setThemeMode(option.mode)"
          >
            <AppIcon :src="option.icon" />
            <span>{{ option.label }}</span>
          </button>
        </div>
      </div>

      <p class="group-caption">当前生效：{{ resolvedLabel }}</p>
    </section>

    <section class="settings-group" aria-labelledby="general-title">
      <h2 id="general-title" class="group-title">通用</h2>

      <div class="setting-row">
        <div class="setting-copy">
          <span class="setting-label">启动页</span>
          <span class="setting-desc">应用启动时默认打开的页面</span>
        </div>
        <div class="pill-options" role="radiogroup" aria-label="启动页">
          <button type="button" role="radio" class="pill-option is-active" :aria-checked="true">
            首页
          </button>
          <button
            type="button"
            role="radio"
            class="pill-option"
            :aria-checked="false"
            disabled
            title="即将上线"
          >
            上次页面
          </button>
        </div>
      </div>

      <div class="setting-row">
        <div class="setting-copy">
          <span class="setting-label">语言</span>
          <span class="setting-desc">界面显示语言</span>
        </div>
        <div class="pill-options" role="radiogroup" aria-label="语言">
          <button type="button" role="radio" class="pill-option is-active" :aria-checked="true">
            简体中文
          </button>
          <button
            type="button"
            role="radio"
            class="pill-option"
            :aria-checked="false"
            disabled
            title="即将上线"
          >
            English
          </button>
        </div>
      </div>
    </section>
  </div>
</template>

<style scoped>
.general-settings-view {
  padding: var(--space-6) var(--space-8) var(--space-10);
  max-width: 56rem;
}

/* —— 设置组卡片：中性表面 + 行间发丝线 —— */
.settings-group {
  padding: var(--space-4) var(--space-5);
  background: var(--surface-3);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-sm);
}

.settings-group + .settings-group {
  margin-top: var(--space-4);
}

.group-title {
  padding: var(--space-1) var(--space-2) var(--space-3);
  font-size: 1rem;
  font-weight: 650;
}

.setting-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-6);
  padding: var(--space-3) var(--space-2);
}

.setting-row + .setting-row {
  border-top: 1px solid var(--border);
}

.setting-copy {
  display: grid;
  min-width: 0;
}

.setting-label {
  font-size: 0.92rem;
  font-weight: 550;
}

.setting-desc {
  margin-top: 0.15rem;
  font-size: 0.8rem;
  color: var(--muted-foreground);
}

.group-caption {
  padding: var(--space-2) var(--space-2) var(--space-1);
  font-size: 0.8rem;
  color: var(--muted-foreground);
}

/* —— 主题三选：中性胶囊，选中 = muted 底（主题色不进设置控件） —— */
.theme-options {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex: 0 0 auto;
}

.theme-option {
  display: inline-flex;
  align-items: center;
  gap: 0.4em;
  padding: 0.35rem 0.8rem;
  font-size: 0.85rem;
  color: var(--muted-foreground);
  background: transparent;
  border: 1px solid var(--border);
  border-radius: var(--radius-full);
  transition:
    background-color var(--duration-fast) var(--ease-standard),
    color var(--duration-fast) var(--ease-standard),
    border-color var(--duration-fast) var(--ease-standard);
}

.theme-option:hover:not(.is-active) {
  color: var(--foreground);
  background: var(--muted);
}

.theme-option.is-active {
  color: var(--foreground);
  background: var(--muted);
  border-color: var(--border-strong);
  font-weight: 550;
}

.theme-option .app-icon {
  width: 1.05rem;
  height: 1.05rem;
}

/* —— 通用档位胶囊（启动页 / 语言）—— */
.pill-options {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex: 0 0 auto;
}

.pill-option {
  padding: 0.35rem 0.8rem;
  font-size: 0.85rem;
  color: var(--muted-foreground);
  background: transparent;
  border: 1px solid var(--border);
  border-radius: var(--radius-full);
  transition:
    background-color var(--duration-fast) var(--ease-standard),
    color var(--duration-fast) var(--ease-standard);
}

.pill-option:hover:not(.is-active):not(:disabled) {
  color: var(--foreground);
  background: var(--muted);
}

.pill-option.is-active {
  color: var(--foreground);
  background: var(--muted);
  border-color: var(--border-strong);
  font-weight: 550;
}

.pill-option:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

/* 窄窗：控制组换行到标签下方 */
@media (max-width: 42rem) {
  .setting-row {
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-3);
  }
}
</style>
