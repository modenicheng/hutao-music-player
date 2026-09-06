import { describe, expect, it } from "vitest";
import { formatBytes, formatCny, formatLongDuration } from "./format";

describe("formatBytes", () => {
  it("MB 区间一位小数", () => {
    expect(formatBytes(680 * 1024 ** 2)).toBe("680 MB");
    expect(formatBytes(1.5 * 1024 ** 2)).toBe("1.5 MB");
  });

  it("GB 区间一位小数", () => {
    expect(formatBytes(1.4 * 1024 ** 3)).toBe("1.4 GB");
  });

  it("整值去尾 .0", () => {
    expect(formatBytes(2 * 1024 ** 3)).toBe("2 GB");
    expect(formatBytes(512 * 1024 ** 2)).toBe("512 MB");
  });
});

describe("formatLongDuration", () => {
  it("一小时内只报分钟", () => {
    expect(formatLongDuration(46 * 60_000)).toBe("46 分钟");
  });

  it("跨小时报小时 + 分钟", () => {
    expect(formatLongDuration((3 * 60 + 42) * 60_000)).toBe("3 小时 42 分钟");
  });

  it("整小时去尾零分钟段", () => {
    expect(formatLongDuration(2 * 3_600_000)).toBe("2 小时 0 分钟");
  });
});

describe("formatCny", () => {
  it("分转元恒两位小数", () => {
    expect(formatCny(3600)).toBe("¥36.00");
    expect(formatCny(200)).toBe("¥2.00");
  });
});
