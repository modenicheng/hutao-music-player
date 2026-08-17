import { describe, expect, it, vi } from "vitest";
import {
  PlayerController,
  type PlayerBridge,
  type PlayerStateSnapshot,
} from "./player";

const snapshot = (
  overrides: Partial<PlayerStateSnapshot> = {},
): PlayerStateSnapshot => ({
  status: "paused",
  positionMs: 25_000,
  durationMs: 100_000,
  volume: 0.6,
  canSeek: true,
  canGoNext: false,
  canGoPrevious: false,
  title: null,
  artists: [],
  error: null,
  ...overrides,
});

const createBridge = () => {
  let listener: ((state: PlayerStateSnapshot) => void) | undefined;
  let errorListener: ((message: string) => void) | undefined;
  const bridge: PlayerBridge = {
    getState: vi.fn(async () => snapshot()),
    onStateChanged: vi.fn(async (next) => {
      listener = next;
      return () => {
        listener = undefined;
      };
    }),
    onError: vi.fn(async (next) => {
      errorListener = next;
      return () => {
        errorListener = undefined;
      };
    }),
    togglePlay: vi.fn(async () => undefined),
    seek: vi.fn(async () => undefined),
    setVolume: vi.fn(async () => undefined),
    previous: vi.fn(async () => undefined),
    next: vi.fn(async () => undefined),
    stop: vi.fn(async () => undefined),
  };
  return {
    bridge,
    emit: (state: PlayerStateSnapshot) => listener?.(state),
    emitError: (message: string) => errorListener?.(message),
  };
};

const createStorage = (volume?: string) => {
  const values = new Map<string, string>();
  if (volume !== undefined) values.set("hmp.player.volume", volume);
  return {
    getItem: vi.fn((key: string) => values.get(key) ?? null),
    setItem: vi.fn((key: string, value: string) => values.set(key, value)),
  };
};

describe("PlayerController daemon bridge", () => {
  it("hydrates from the daemon and follows state events", async () => {
    const { bridge, emit } = createBridge();
    const player = new PlayerController(bridge, { storage: createStorage() });

    await player.mount();
    expect(player.state.progress).toBe(0.25);
    expect(player.state.volume).toBe(0.6);
    expect(player.state.playing).toBe(false);

    emit(snapshot({ status: "playing", positionMs: 50_000 }));
    expect(player.state.playing).toBe(true);
    expect(player.state.progress).toBe(0.5);
  });

  it("routes playback controls to the daemon", async () => {
    const { bridge } = createBridge();
    const player = new PlayerController(bridge, { storage: createStorage() });

    player.togglePlay();
    player.previous();
    player.next();
    player.stop();
    await vi.waitFor(() => expect(bridge.stop).toHaveBeenCalledOnce());

    expect(bridge.togglePlay).toHaveBeenCalledOnce();
    expect(bridge.previous).toHaveBeenCalledOnce();
    expect(bridge.next).toHaveBeenCalledOnce();
  });

  it("persists local volume and does not echo daemon volume events", async () => {
    const { bridge, emit } = createBridge();
    const storage = createStorage("0.35");
    const player = new PlayerController(bridge, { storage });

    expect(player.state.volume).toBe(0.35);
    await player.mount();
    player.setVolume(1.5);
    await vi.waitFor(() => expect(bridge.setVolume).toHaveBeenCalledWith(1));

    emit(snapshot({ volume: 0.4 }));
    expect(player.state.volume).toBe(0.4);
    expect(bridge.setVolume).toHaveBeenCalledTimes(1);
    expect(storage.setItem).toHaveBeenLastCalledWith("hmp.player.volume", "0.4");
  });

  it("seeks by daemon duration after a drag", async () => {
    const { bridge } = createBridge();
    const player = new PlayerController(bridge, { storage: createStorage() });
    await player.mount();
    player.startDragging();
    player.updateDragPercent(0.75);
    player.setProgress();

    await vi.waitFor(() => expect(bridge.seek).toHaveBeenCalledWith(75_000));
  });

  it("surfaces bridge failures without pretending playback changed", async () => {
    const { bridge } = createBridge();
    vi.mocked(bridge.togglePlay).mockRejectedValueOnce(new Error("offline"));
    const player = new PlayerController(bridge, { storage: createStorage() });

    player.togglePlay();
    await vi.waitFor(() => expect(player.state.error).toBe("offline"));
    expect(player.state.playing).toBe(false);
  });

  it("surfaces a daemon disconnect event", async () => {
    const { bridge, emitError } = createBridge();
    const player = new PlayerController(bridge, { storage: createStorage() });
    await player.mount();

    emitError("daemon disconnected");

    expect(player.state.error).toBe("daemon disconnected");
  });
});
