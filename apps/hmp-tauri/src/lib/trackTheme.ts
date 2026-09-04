import { watch, type WatchStopHandle } from "vue";
import { extractPaletteFromUrl } from "./color/adapter.ts";
import { themeState } from "./themeStore.ts";
import type { PlayerController } from "./player.ts";

/**
 * 曲目层调色全局应用（DESIGN.md §1.2）：
 * 当前曲目封面 / 明暗主题任一变化时重取调色板并整族覆写 --track-* 变量。
 * 必须挂在 App 级——TrackTable、队列抽屉、播放条在任何页面都要跟随曲目换妆，
 * 而不是只在播放页打开时才生效。
 */
export function applyTrackTheme(player: PlayerController): WatchStopHandle {
  const theme = themeState();
  return watch(
    () => [player.state.currentTrack?.coverUrl ?? null, theme.resolved] as const,
    async ([coverUrl, resolved]) => {
      const palette = await extractPaletteFromUrl(coverUrl ?? "", {
        mode: resolved,
      });
      const rootStyle = document.documentElement.style;
      rootStyle.setProperty("--track-accent", palette.accent);
      rootStyle.setProperty("--track-on-accent", palette.onAccent);
      rootStyle.setProperty("--track-accent-soft", palette.accentSoft);
      rootStyle.setProperty("--track-deep", palette.deep);
      rootStyle.setProperty("--track-deep-fg", palette.deepFg);
      rootStyle.setProperty("--track-grad-from", palette.gradFrom);
      rootStyle.setProperty("--track-grad-to", palette.gradTo);
      rootStyle.setProperty("--track-equalizer", palette.accent);
    },
    { immediate: true },
  );
}
