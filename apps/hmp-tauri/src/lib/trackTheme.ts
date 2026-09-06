import { ref, watch, type WatchStopHandle } from "vue";
import { extractPaletteFromUrl } from "./color/adapter.ts";
import { themeState } from "./themeStore.ts";
import type { PlayerController } from "./player.ts";

/**
 * 曲目层调色（DESIGN.md §1.2，v0.3 收窄作用域）：
 * 取色仍在 App 级随曲目/明暗预热，但动态 --track-* 变量只经
 * trackPaletteVars 注入播放页 overlay 子树；其余界面一律消费
 * :root 的品牌胡桃木回退值，保持全局观感统一。
 */
export const trackPaletteVars = ref<Record<string, string> | null>(null);

export function applyTrackTheme(player: PlayerController): WatchStopHandle {
  const theme = themeState();
  return watch(
    () => [player.state.currentTrack?.coverUrl ?? null, theme.resolved] as const,
    async ([coverUrl, resolved]) => {
      const palette = await extractPaletteFromUrl(coverUrl ?? "", {
        mode: resolved,
      });
      trackPaletteVars.value = {
        "--track-accent": palette.accent,
        "--track-on-accent": palette.onAccent,
        "--track-accent-soft": palette.accentSoft,
        "--track-deep": palette.deep,
        "--track-deep-fg": palette.deepFg,
        "--track-grad-from": palette.gradFrom,
        "--track-grad-to": palette.gradTo,
        "--track-on-ambient": palette.onAmbient,
        "--track-equalizer": palette.accent,
      };
    },
    { immediate: true },
  );
}
