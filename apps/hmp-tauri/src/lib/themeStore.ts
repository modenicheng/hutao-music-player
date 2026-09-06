import { reactive } from "vue";
import {
  THEME_STORAGE_KEY,
  isThemeMode,
  nextThemeMode,
  resolveTheme,
  type ThemeMode,
  type ResolvedTheme,
} from "./theme.ts";

/**
 * 主题状态单例：theme.ts 里的纯逻辑 + Vue 响应式壳 + localStorage 持久化。
 * 侧栏主题按钮等 UI 只跟这个 store 打交道。
 */
interface ThemeStore {
  mode: ThemeMode;
  resolved: ResolvedTheme;
}

const state = reactive<ThemeStore>({
  mode: "auto",
  resolved: "light",
});

let mediaQuery: MediaQueryList | null = null;
let onMediaChange: (() => void) | null = null;

function readStoredMode(): ThemeMode {
  if (typeof localStorage === "undefined") return "auto";
  const stored = localStorage.getItem(THEME_STORAGE_KEY);
  return isThemeMode(stored) ? stored : "auto";
}

function apply() {
  const systemPrefersDark =
    mediaQuery?.matches ??
    (typeof window !== "undefined"
      ? window.matchMedia("(prefers-color-scheme: dark)").matches
      : false);
  state.resolved = resolveTheme(state.mode, systemPrefersDark);
  document.documentElement.classList.toggle("dark", state.resolved === "dark");
}

/** 在应用启动时调用一次；重复调用无副作用 */
export function initTheme() {
  state.mode = readStoredMode();
  mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
  onMediaChange = () => apply();
  mediaQuery.addEventListener("change", onMediaChange);
  apply();
}

export function cycleTheme() {
  state.mode = nextThemeMode(state.mode);
  localStorage.setItem(THEME_STORAGE_KEY, state.mode);
  apply();
}

/** 直接指定主题模式（设置页用）；与循环切换共用同一份持久化 */
export function setThemeMode(mode: ThemeMode) {
  state.mode = mode;
  localStorage.setItem(THEME_STORAGE_KEY, mode);
  apply();
}

export function themeState() {
  return state;
}
