import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { PlayerBridge, PlayerStateSnapshot } from "./player";

const PLAYER_STATE_EVENT = "hmp://player-state";
const CONTROL_ERROR_EVENT = "hmp://control-error";

export const tauriPlayerBridge: PlayerBridge = {
  getState: () => invoke<PlayerStateSnapshot>("get_player_state"),
  onStateChanged: async (listener) =>
    listen<PlayerStateSnapshot>(PLAYER_STATE_EVENT, (event) =>
      listener(event.payload),
    ),
  onError: async (listener) =>
    listen<string>(CONTROL_ERROR_EVENT, (event) => listener(event.payload)),
  togglePlay: () => invoke("toggle_play"),
  seek: (positionMs) => invoke("seek", { positionMs }),
  setVolume: (volume) => invoke("set_volume", { volume }),
  previous: () => invoke("previous"),
  next: () => invoke("next"),
  stop: () => invoke("stop"),
};
