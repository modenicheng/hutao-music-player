import { describe, expect, it, vi } from "vitest";
import { PlayerController } from "../player";

const createAudio = () => ({
  currentTime: 0,
  duration: 100,
  volume: 1,
  pause: vi.fn(),
  play: vi.fn(),
});

const createStorage = (volume?: string) => {
  const values = new Map<string, string>();
  if (volume !== undefined) values.set("hmp.player.volume", volume);
  return {
    getItem: vi.fn((key: string) => values.get(key) ?? null),
    setItem: vi.fn((key: string, value: string) => values.set(key, value)),
  };
};

describe("PlayerController volume", () => {
  it("uses full volume when no persisted value exists", () => {
    const audio = createAudio();

    const player = new PlayerController("test.flac", {
      audio,
      storage: createStorage(),
    });

    expect(player.state.volume).toBe(1);
    expect(audio.volume).toBe(1);
  });

  it("restores persisted volume when created", () => {
    const audio = createAudio();
    const storage = createStorage("0.35");

    const player = new PlayerController("test.flac", { audio, storage });

    expect(player.state.volume).toBe(0.35);
    expect(audio.volume).toBe(0.35);
  });

  it("persists and forwards a locally selected volume", () => {
    const audio = createAudio();
    const storage = createStorage();
    const sendVolume = vi.fn();
    const player = new PlayerController("test.flac", {
      audio,
      storage,
      sendVolume,
    });

    player.setVolume(1.5);

    expect(player.state.volume).toBe(1);
    expect(audio.volume).toBe(1);
    expect(storage.setItem).toHaveBeenLastCalledWith("hmp.player.volume", "1");
    expect(sendVolume).toHaveBeenLastCalledWith(1);
  });

  it("applies backend volume without sending it back", () => {
    const audio = createAudio();
    const storage = createStorage();
    const sendVolume = vi.fn();
    const player = new PlayerController("test.flac", {
      audio,
      storage,
      sendVolume,
    });

    player.syncVolume(0.6);

    expect(player.state.volume).toBe(0.6);
    expect(audio.volume).toBe(0.6);
    expect(storage.setItem).toHaveBeenLastCalledWith("hmp.player.volume", "0.6");
    expect(sendVolume).not.toHaveBeenCalled();
  });

  it("ignores invalid backend volume", () => {
    const audio = createAudio();
    const player = new PlayerController("test.flac", {
      audio,
      storage: createStorage("0.4"),
    });

    player.syncVolume(Number.NaN);

    expect(player.state.volume).toBe(0.4);
    expect(audio.volume).toBe(0.4);
  });
});
