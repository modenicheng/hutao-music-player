// mock API 统一入口。页面组件只依赖这里的 `api` 对象，接口形状即未来
// 接线的契约（DESIGN.md §4）：
// - 搜索 / 榜单 / 歌手 / 专辑 / 歌单 / 推荐 / 歌词 → 将来经轻量 HTTP 服务
//   进程内直调 hmp-qqmusic-api（TopApi / SingerApi / AlbumApi / RecommendApi / LyricApi）；
// - 切换真实实现时：把每个方法体替换为对应调用，删除 simulateLatency 即可，
//   调用方（组件）不需要任何改动。
import type {
  AlbumDetail,
  ArtistInfo,
  ArtistRef,
  CommentSection,
  DownloadLibrary,
  LocalLibrary,
  Lyrics,
  PlaylistDetail,
  PlaylistRef,
  PurchasedMusic,
  RecommendFeed,
  SearchResults,
  SongRef,
  TopCategory,
  TopDetail,
} from "./types";
import { hashSeed } from "./covers";
import {
  allArtistRefs,
  artistAlbums,
  artistInfo,
  artistSongs,
  commentSection,
  curatedAlbums,
  createdPlaylistRefs,
  downloadLibrary,
  favoritedPlaylistRefs,
  findAlbum,
  findSong,
  likedSongs,
  localLibrary,
  lyricsOf,
  playlistDetail,
  purchasedMusic,
  recommendFeed,
  songPool,
  topCategories,
  topDetail,
} from "./mock-data";

/**
 * 模拟网络延迟（120–400ms）：让 mock 阶段的 loading 态可见、可测。
 * 延迟值本身也由 hashSeed 确定——同一方法同一参数延迟一致，测试可复现。
 * 接真实 IPC / HTTP 时删掉本函数与所有 await 即可。
 */
async function simulateLatency(method: string, ...args: string[]): Promise<void> {
  const ms = 120 + (hashSeed(`${method}:${args.join(",")}`) % 280);
  await new Promise<void>((resolve) => setTimeout(resolve, ms));
}

/** 未命中 id 统一以 reject 报错，与"未来真实接口 404 语义"对齐 */
function requireValue<T>(value: T | undefined, message: string): T {
  if (value === undefined) throw new Error(message);
  return value;
}

/** quick 搜索：对歌名 / 歌手名 / 专辑名做不区分大小写的包含匹配 */
function searchQuick(keyword: string): SearchResults {
  const keyword0 = keyword.trim().toLowerCase();
  if (!keyword0) return { songs: [], albums: [], artists: [] };
  const contains = (text: string) => text.toLowerCase().includes(keyword0);
  return {
    songs: songPool
      .filter(
        (song) =>
          contains(song.title) ||
          song.artists.some((artist) => contains(artist.name)) ||
          contains(song.album.name),
      )
      .slice(0, 12),
    albums: curatedAlbums
      .filter((album) => contains(album.name) || contains(album.artist.name))
      .slice(0, 6),
    artists: allArtistRefs.filter((artist) => contains(artist.name)).slice(0, 8),
  };
}

export const api = {
  search: {
    async quick(keyword: string): Promise<SearchResults> {
      await simulateLatency("search.quick", keyword);
      return searchQuick(keyword);
    },
  },
  top: {
    async categories(): Promise<TopCategory[]> {
      await simulateLatency("top.categories");
      return topCategories();
    },
    async detail(id: string): Promise<TopDetail> {
      await simulateLatency("top.detail", id);
      return requireValue(topDetail(id), `排行榜 ${id} not found`);
    },
  },
  artist: {
    async info(mid: string): Promise<ArtistInfo> {
      await simulateLatency("artist.info", mid);
      return requireValue(artistInfo(mid), `歌手 ${mid} not found`);
    },
    /** 热门歌曲：总池曲目 + 数字单曲，按稳定热度排序，12–20 首 */
    async songs(mid: string): Promise<SongRef[]> {
      await simulateLatency("artist.songs", mid);
      requireValue(artistInfo(mid), `歌手 ${mid} not found`);
      return artistSongs(mid);
    },
    async albums(mid: string): Promise<AlbumDetail[]> {
      await simulateLatency("artist.albums", mid);
      requireValue(artistInfo(mid), `歌手 ${mid} not found`);
      return artistAlbums(mid);
    },
    async similar(mid: string): Promise<ArtistRef[]> {
      await simulateLatency("artist.similar", mid);
      return requireValue(artistInfo(mid), `歌手 ${mid} not found`).similar;
    },
  },
  album: {
    async detail(mid: string): Promise<AlbumDetail> {
      await simulateLatency("album.detail", mid);
      return requireValue(findAlbum(mid), `专辑 ${mid} not found`);
    },
  },
  playlist: {
    async detail(id: string): Promise<PlaylistDetail> {
      await simulateLatency("playlist.detail", id);
      return requireValue(playlistDetail(id), `歌单 ${id} not found`);
    },
  },
  recommend: {
    async feed(): Promise<RecommendFeed> {
      await simulateLatency("recommend.feed");
      return recommendFeed();
    },
  },
  comment: {
    /**
     * 评论分区。sort（"hot" | "new"）当前不影响返回结构：mock 一次性返回
     * 热评 + 最新两个分区，UI 切 tab 无需重新请求；接真实后端时把 sort
     * 映射为排序参数即可。
     */
    async section(mid: string, sort?: "hot" | "new"): Promise<CommentSection> {
      await simulateLatency("comment.section", mid, sort ?? "");
      requireValue(findSong(mid), `歌曲 ${mid} not found`);
      return commentSection(mid, sort);
    },
  },
  lyrics: {
    async get(mid: string): Promise<Lyrics> {
      await simulateLatency("lyrics.get", mid);
      return requireValue(lyricsOf(mid), `歌词 ${mid} not found`);
    },
  },
  library: {
    // 账号体系未接线：各方法均由本地 mock 派生，未来换成 daemon
    // Favorite / 歌单收藏 / 本地扫描 / 下载管理 / 订单接口，调用方不需要改动。
    async liked(): Promise<SongRef[]> {
      await simulateLatency("library.liked");
      return likedSongs();
    },
    async created(): Promise<PlaylistRef[]> {
      await simulateLatency("library.created");
      return createdPlaylistRefs();
    },
    async favorited(): Promise<PlaylistRef[]> {
      await simulateLatency("library.favorited");
      return favoritedPlaylistRefs();
    },
    /** 本地音乐库（桌面端 daemon 索引） */
    async local(): Promise<LocalLibrary> {
      await simulateLatency("library.local");
      return localLibrary();
    },
    /** 下载内容 */
    async downloads(): Promise<DownloadLibrary> {
      await simulateLatency("library.downloads");
      return downloadLibrary();
    },
    /** 已购音乐 */
    async purchased(): Promise<PurchasedMusic> {
      await simulateLatency("library.purchased");
      return purchasedMusic();
    },
  },
};
