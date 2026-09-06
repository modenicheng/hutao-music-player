// 领域类型契约（DESIGN.md §4）：这里的形状 = 未来接真实后端时的接口形状。
// 命名贴 QQ 音乐域模型（mid / 歌手 / 专辑 / 榜单 / 歌单），接 hmp-qqmusic-api 时
// 只需要做一层薄适配，页面代码不需要改动。

export interface ArtistRef {
  /** 歌手唯一 id（QQ 音乐域为 hexish mid，mock 用可读短 id） */
  mid: string;
  name: string;
}

export interface AlbumRef {
  mid: string;
  name: string;
  picUrl: string;
}

export interface SongRef {
  mid: string;
  title: string;
  /** 多人合唱时按 主歌手 → feat 顺序排列 */
  artists: ArtistRef[];
  album: AlbumRef;
  durationMs: number;
  /** 音质徽章文案，如 "FLAC · 44.1kHz"；无损以下可不返回 */
  quality?: string;
}

export interface TopCategory {
  id: string;
  name: string;
  picUrl: string;
  /** 最近一次更新时间，如 "2026-09-02" */
  updateTime: string;
  /** 榜单曲目数，与 detail().entries 长度一致 */
  trackCount: number;
}

export interface ChartEntry {
  rank: number;
  /** 上一期名次；缺省表示新上榜（UI 画"新"而非箭头） */
  prevRank?: number;
  song: SongRef;
  playCount?: number;
}

export interface TopDetail {
  category: TopCategory;
  entries: ChartEntry[];
}

export interface ArtistInfo {
  mid: string;
  name: string;
  picUrl: string;
  songCount: number;
  albumCount: number;
  mvCount: number;
  desc: string;
  similar: ArtistRef[];
}

export interface AlbumDetail {
  mid: string;
  name: string;
  artist: ArtistRef;
  picUrl: string;
  company: string;
  /** 发行日期，如 "2024-05-20" */
  releaseDate: string;
  desc: string;
  songs: SongRef[];
  favCount: number;
}

export interface PlaylistRef {
  id: string;
  name: string;
  coverUrl: string;
  playCount: number;
}

export interface PlaylistDetail {
  id: string;
  name: string;
  coverUrl: string;
  creator: { name: string; avatarUrl: string };
  tags: string[];
  trackCount: number;
  playCount: number;
  desc: string;
  songs: SongRef[];
}

export interface RecommendFeed {
  daily: { title: string; date: string; coverUrl: string };
  guessYouLike: SongRef[];
  newSongs: SongRef[];
  topCharts: TopCategory[];
  playlists: PlaylistRef[];
}

export interface CommentReply {
  id: string;
  user: { name: string; avatarUrl: string };
  content: string;
  likes: number;
}

export interface Comment {
  id: string;
  user: { name: string; avatarUrl: string };
  /** 展示用时间文案，如 "09月05日 08:24" */
  time: string;
  location?: string;
  content: string;
  likes: number;
  isPinned?: boolean;
  replies: CommentReply[];
  /** 回复总数（可能大于 replies 内嵌条数，其余折叠为"共 N 条回复"） */
  replyCount?: number;
}

export interface CommentSection {
  /** 评论总数（mock 为数万级，供 "评论 · 12.4万" 展示） */
  total: number;
  /** 热评：置顶优先，其余按点赞数降序 */
  hot: Comment[];
  /** 最新：按时间倒序 */
  latest: Comment[];
}

/** 逐字歌词的一个字/词：QQ 音乐 QRC 式时间戳 */
export interface LyricWord {
  text: string;
  startMs: number;
  endMs: number;
}

export interface LyricLine {
  timeMs: number;
  text: string;
  /** 翻译行，随原文行下挂展示 */
  trans?: string;
  /** 逐字时间轴；缺省时该行退回逐行高亮 */
  words?: LyricWord[];
}

export interface Lyrics {
  mid: string;
  title: string;
  lines: LyricLine[];
}

export interface SearchResults {
  songs: SongRef[];
  albums: AlbumDetail[];
  artists: ArtistRef[];
}

// ——— 本地音乐 / 下载（落盘文件，桌面端 daemon 索引结果的契约形状）———

/** 落盘音频文件：本地索引或下载得到的曲目，在 SongRef 之上附文件维度字段 */
export interface LocalTrack extends SongRef {
  /** 文件大小（字节） */
  sizeBytes: number;
  /** 编码格式，如 "FLAC" / "MP3" */
  format: string;
  /** 文件所在文件夹（本地索引为监视文件夹，下载为统一存储目录） */
  folder: string;
}

/** 本地监视文件夹（桌面端扫描范围） */
export interface WatchedFolder {
  /** 文件夹路径，如 "~/Music/无损收藏" */
  path: string;
  trackCount: number;
  sizeBytes: number;
  /** 最近一次扫描时间，如 "2026-09-05 21:30" */
  lastScanAt: string;
}

/** 本地音乐库 */
export interface LocalLibrary {
  tracks: LocalTrack[];
  folders: WatchedFolder[];
  /** 全库最近一次扫描时间 */
  lastScanAt: string;
}

/** 下载内容（统一存储目录下的落盘文件） */
export interface DownloadLibrary {
  tracks: LocalTrack[];
  /** 下载文件统一存储目录 */
  storagePath: string;
}

// ——— 已购音乐（订单维度字段；金额一律分为单位）———

export interface PurchasedSingle {
  song: SongRef;
  /** 购买日期，如 "2026-08-30" */
  purchasedAt: string;
  /** 实付金额（分） */
  priceFen: number;
}

export interface PurchasedAlbum {
  album: AlbumDetail;
  purchasedAt: string;
  priceFen: number;
}

export interface PurchasedMusic {
  /** 已购单曲，按购买日期倒序 */
  singles: PurchasedSingle[];
  /** 已购专辑，按购买日期倒序 */
  albums: PurchasedAlbum[];
}
