import { describe, expect, it } from "vitest";
import { api } from "./client";
import { avatarUrl, coverUrl } from "./covers";
import { songPool } from "./mock-data";

// 轻量冒烟测试：只锁住 mock 层最容易退化的两个性质——
// Promise 契约（resolves / rejects）与确定性输出。
describe("mock api 冒烟测试", () => {
  it("api.top.categories() resolves 且给出 6 个榜单分类", async () => {
    const categories = await api.top.categories();
    expect(categories).toHaveLength(6);
    for (const category of categories) {
      expect(category.name.length).toBeGreaterThan(0);
      expect(category.trackCount).toBe(20);
      expect(category.picUrl.startsWith("data:image/svg+xml,")).toBe(true);
    }
  });

  it("榜单详情条目来自总歌曲池，且带名次与播放量", async () => {
    const poolMids = new Set(songPool.map((song) => song.mid));
    const detail = await api.top.detail("hot");
    expect(detail.entries).toHaveLength(20);
    expect(detail.entries[0]?.rank).toBe(1);
    for (const entry of detail.entries) {
      expect(poolMids.has(entry.song.mid)).toBe(true);
      expect(entry.playCount).toBeGreaterThan(0);
    }
    // 三种名次状态（升/降/新上榜）都应出现，供榜单页的箭头 UI 使用
    const states = new Set(
      detail.entries.map((entry) =>
        entry.prevRank === undefined
          ? "new"
          : entry.prevRank > entry.rank
            ? "up"
            : entry.prevRank < entry.rank
              ? "down"
              : "flat",
      ),
    );
    expect(states.size).toBeGreaterThan(1);
  });

  it("歌手页数据完整且热门歌曲落在 12–20 首", async () => {
    const info = await api.artist.info("ar01");
    expect(info.name.length).toBeGreaterThan(0);
    expect(info.similar.length).toBeGreaterThan(0);
    const songs = await api.artist.songs("ar01");
    expect(songs.length).toBeGreaterThanOrEqual(12);
    expect(songs.length).toBeLessThanOrEqual(20);
  });

  it("数据引用一致：专辑曲目反查歌手，单曲专辑可再查详情", async () => {
    const album = await api.album.detail("al01");
    expect(album.songs.length).toBeGreaterThan(0);
    for (const song of album.songs) {
      expect(song.album.mid).toBe(album.mid);
      expect(song.artists.some((artist) => artist.mid === album.artist.mid)).toBe(true);
    }
    // 歌手热门歌曲里的单曲，其数字单曲专辑也应能反查
    const hot = await api.artist.songs("ar01");
    const single = hot.find((song) => song.mid.startsWith("so1x"));
    expect(single).toBeDefined();
    const singleAlbum = await api.album.detail(single!.album.mid);
    expect(singleAlbum.songs).toHaveLength(1);
  });

  it("封面 / 头像生成是确定性的：同 seed 一致、不同 seed 不同", () => {
    expect(coverUrl("al01")).toBe(coverUrl("al01"));
    expect(coverUrl("al01")).not.toBe(coverUrl("al02"));
    expect(coverUrl("al01", 300)).toBe(coverUrl("al01", 300));
    expect(avatarUrl("山间邮筒")).toBe(avatarUrl("山间邮筒"));
    expect(avatarUrl("山间邮筒")).not.toBe(avatarUrl("橘白猫店长"));
  });

  it("未知 id 返回 reject，搜索空关键词返回空数组", async () => {
    await expect(api.album.detail("al999")).rejects.toThrow("not found");
    await expect(api.artist.info("ar999")).rejects.toThrow("not found");
    await expect(api.top.detail("nope")).rejects.toThrow("not found");
    await expect(api.lyrics.get("so999")).rejects.toThrow("not found");
    const empty = await api.search.quick("");
    expect(empty.songs).toHaveLength(0);
    expect(empty.albums).toHaveLength(0);
    expect(empty.artists).toHaveLength(0);
  });
});
