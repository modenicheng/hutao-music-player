import { reactive } from "vue";

/** 音质档位（mock 期固定四档，接真实后端后由曲目可用音质驱动） */
export type QualityTierId = "standard" | "high" | "lossless" | "hires";

export interface QualityTier {
  id: QualityTierId;
  /** 徽章短文案 */
  label: string;
  /** 弹层里的完整说明 */
  detail: string;
}

const QUALITY_TIERS: QualityTier[] = [
  { id: "standard", label: "标准", detail: "128kbps MP3" },
  { id: "high", label: "高清", detail: "320kbps MP3" },
  { id: "lossless", label: "无损", detail: "FLAC · 44.1kHz" },
  { id: "hires", label: "Hi-Res", detail: "FLAC · 96kHz/24bit" },
];

const TIER_RANK: Record<QualityTierId, number> = {
  standard: 0,
  high: 1,
  lossless: 2,
  hires: 3,
};

const QUALITY_STORAGE_KEY = "hmp.player.quality";

interface QualityStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function tierById(id: string): QualityTier | undefined {
  return QUALITY_TIERS.find((tier) => tier.id === id);
}

function loadSelected(storage?: QualityStorage): QualityTierId {
  const stored = storage?.getItem(QUALITY_STORAGE_KEY) ?? null;
  return stored && tierById(stored) ? (stored as QualityTierId) : "lossless";
}

/**
 * 音质偏好（全局唯一）：徽章与弹层共享一份状态，localStorage 持久化。
 * 实际生效档位 = min(用户偏好, 当前曲目最高音质)。
 */
function createStore(storage?: QualityStorage) {
  const state = reactive({
    selected: loadSelected(storage),
  });

  function select(id: QualityTierId) {
    if (!tierById(id)) return;
    state.selected = id;
    storage?.setItem(QUALITY_STORAGE_KEY, id);
  }

  return { state, select };
}

const hasLocalStorage = typeof localStorage !== "undefined";
const defaultStore = createStore(hasLocalStorage ? localStorage : undefined);

export const qualityState = defaultStore.state;
export const selectQuality = defaultStore.select;
export { QUALITY_TIERS, TIER_RANK, tierById };

/** 从 SongRef.quality 文案（"FLAC · 44.1kHz" 等）推曲目最高档位 */
export function trackMaxTierId(quality?: string): QualityTierId {
  if (!quality) return "standard";
  if (quality.includes("Hi-Res")) return "hires";
  if (quality.includes("FLAC")) return "lossless";
  if (quality.includes("320")) return "high";
  return "standard";
}

/** 实际生效档位：不超过曲目最高档 */
export function effectiveTierId(selected: QualityTierId, max: QualityTierId): QualityTierId {
  return TIER_RANK[selected] <= TIER_RANK[max] ? selected : max;
}
