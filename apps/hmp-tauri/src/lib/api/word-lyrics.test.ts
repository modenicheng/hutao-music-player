import { describe, expect, it } from "vitest";
import { lyricsOf } from "./mock-data.ts";
import {
  QUALITY_TIERS,
  effectiveTierId,
  trackMaxTierId,
} from "../qualityStore.ts";

describe("逐字歌词时间轴", () => {
  it("主打歌每行都有逐字 token，时间单调且落在行区间内", () => {
    const lyrics = lyricsOf("so001");
    expect(lyrics).toBeDefined();
    for (const line of lyrics!.lines) {
      expect(line.words).toBeDefined();
      expect(line.words!.length).toBeGreaterThan(0);
      expect(line.words![0]!.startMs).toBeGreaterThanOrEqual(line.timeMs);
      for (const word of line.words!) {
        expect(word.endMs).toBeGreaterThan(word.startMs);
      }
      for (let i = 1; i < line.words!.length; i += 1) {
        expect(line.words![i]!.startMs).toBeGreaterThanOrEqual(line.words![i - 1]!.endMs - 1);
      }
    }
  });

  it("分词保真：token 拼回去等于原文（不含空白变化）", () => {
    const lyrics = lyricsOf("so008");
    for (const line of lyrics!.lines) {
      const joined = line.words!.map((word) => word.text).join("");
      expect(joined.replace(/\s+/g, "")).toBe(line.text.replace(/\s+/g, ""));
    }
  });

  it("汉字逐字切分：纯中文行 token 数等于字符数", () => {
    const lyrics = lyricsOf("so001");
    const line = lyrics!.lines[0]!; // "信纸上有潮汐的痕迹"
    expect(line.text.replace(/\s/g, "").length).toBe(9);
    expect(line.words!.length).toBe(9);
  });

  it("占位歌词同样带逐字时间轴；未知 mid 拿不到歌词", () => {
    const lyrics = lyricsOf("so002");
    expect(lyrics).toBeDefined();
    expect(lyrics!.lines[0]!.words!.length).toBeGreaterThan(0);
    expect(lyricsOf("so999")).toBeUndefined();
  });
});

describe("音质档位", () => {
  it("四档顺序固定", () => {
    expect(QUALITY_TIERS.map((tier) => tier.id)).toEqual([
      "standard",
      "high",
      "lossless",
      "hires",
    ]);
  });

  it("从曲目音质文案推最高档位", () => {
    expect(trackMaxTierId("Hi-Res · 96kHz/24bit")).toBe("hires");
    expect(trackMaxTierId("FLAC · 44.1kHz")).toBe("lossless");
    expect(trackMaxTierId("320kbps MP3")).toBe("high");
    expect(trackMaxTierId()).toBe("standard");
  });

  it("生效档位不超过曲目最高档", () => {
    expect(effectiveTierId("hires", "lossless")).toBe("lossless");
    expect(effectiveTierId("standard", "hires")).toBe("standard");
    expect(effectiveTierId("lossless", "lossless")).toBe("lossless");
  });
});
