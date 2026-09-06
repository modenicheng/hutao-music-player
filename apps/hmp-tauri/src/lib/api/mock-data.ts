// 确定性 mock 数据。全部内容为虚构：艺人 / 歌曲 / 厂牌 / 用户均为演示创作，
// 不指向任何真实实体（名字是编的，事是编的，数据也是编的）。
//
// 组织方式：先声明"种子表"（原始事实），再由工厂函数编译成领域对象。
// 好处是引用关系天然一致——专辑曲目反查得到歌手、榜单条目来自总歌曲池、
// 歌单曲目全部能在池中命中——不会出现手抄漂移。
// 确定性：不用 Math.random / Date.now，伪随机一律走 hashSeed（FNV-1a）。

import { avatarUrl, coverUrl, hashSeed } from "./covers";
import type {
  AlbumDetail,
  ArtistInfo,
  ArtistRef,
  ChartEntry,
  Comment,
  CommentReply,
  CommentSection,
  DownloadLibrary,
  LocalLibrary,
  LocalTrack,
  LyricLine,
  LyricWord,
  Lyrics,
  PlaylistDetail,
  PlaylistRef,
  PurchasedMusic,
  RecommendFeed,
  SongRef,
  TopCategory,
  TopDetail,
} from "./types";

const QUALITY_FLAC = "FLAC · 44.1kHz";
const QUALITY_HI_RES = "Hi-Res · 96kHz/24bit";
const QUALITY_MP3 = "320kbps MP3";

const CHART_SIZE = 20;
/** mock 数据的固定"今天"，保证每次运行输出一致 */
const FEED_DATE = "2026-09-05";
/** 榜单统一按周更新（上周三） */
const CHART_UPDATE_TIME = "2026-09-02";

// ————————————————————————————————————————————————————————————
// 种子表
// ————————————————————————————————————————————————————————————

interface ArtistSeed {
  mid: string;
  name: string;
  desc: string;
  mvCount: number;
  /** 歌手页"热门歌曲"数量，落在 12–20 */
  hotSongTarget: number;
  /** 相似歌手（mid 列表，手工指定保证可信） */
  similar: string[];
}

const ARTIST_SEEDS: ArtistSeed[] = [
  {
    mid: "ar01",
    name: "白栖遥",
    desc: "白栖遥，流行歌手兼词曲作者，声线干净克制，擅长把日常细节写进歌里。2024 年发行创作专辑《潮汐来信》，并为剧集《拾光纪》献唱主题曲，被乐迷称为\"写信人\"。",
    mvCount: 3,
    hotSongTarget: 16,
    similar: ["ar12", "ar13", "ar05", "ar07"],
  },
  {
    mid: "ar02",
    name: "陈屿帆",
    desc: "陈屿帆，来自南方小城的民谣唱作人。歌里总有火车、站台和夏天的雨，被乐迷称作\"站台诗人\"。专辑《南方站台》在旧文化馆同期录制，保留了大量现场呼吸声。",
    mvCount: 2,
    hotSongTarget: 15,
    similar: ["ar07", "ar14", "ar05"],
  },
  {
    mid: "ar03",
    name: "雾岛电台",
    desc: "雾岛电台，三人电子乐队，成立于海雾常年不散的雾岛。他们把海雾、轮渡汽笛与灯塔白噪音采样进合成器，做出\"会呼吸\"的电子乐。",
    mvCount: 2,
    hotSongTarget: 14,
    similar: ["ar08", "ar11", "ar06"],
  },
  {
    mid: "ar04",
    name: "破晓列车",
    desc: "破晓列车，五人摇滚乐队，歌词关注都市夜归人。首专《午夜快线》把失真吉他与合成器并到同一条轨道上，速度拉满。",
    mvCount: 3,
    hotSongTarget: 12,
    similar: ["ar09", "ar03", "ar13"],
  },
  {
    mid: "ar05",
    name: "林晚风",
    desc: "林晚风，流行男歌手，声音像夏天傍晚的风。首张个人专辑《橘子汽水与晚风》写尽了夏天的小事。",
    mvCount: 2,
    hotSongTarget: 12,
    similar: ["ar01", "ar13", "ar14"],
  },
  {
    mid: "ar06",
    name: "顾清商",
    desc: "顾清商，钢琴演奏者，专攻印象派与东方意象的融合。独奏专辑《月白》采用单点收音，连踏板与琴体的共鸣都一并收录。",
    mvCount: 1,
    hotSongTarget: 12,
    similar: ["ar10", "ar14", "ar03"],
  },
  {
    mid: "ar07",
    name: "苏折枝",
    desc: "苏折枝，民谣女声，唱词古雅，善用意象。编曲崇尚克制，常宣称\"一把吉他就够\"。",
    mvCount: 1,
    hotSongTarget: 12,
    similar: ["ar02", "ar14", "ar01"],
  },
  {
    mid: "ar08",
    name: "星尘信号",
    desc: "星尘信号，电子音乐制作人，创作主题多为深空、失眠与便利店。建议把他的专辑当作一次不需要返程票的漫游。",
    mvCount: 2,
    hotSongTarget: 12,
    similar: ["ar03", "ar11", "ar06"],
  },
  {
    mid: "ar09",
    name: "未眠者乐队",
    desc: "未眠者乐队，独立摇滚乐队，主唱的假声是他们的标志。首张全长《仲夏航行》献给所有睡不着的年轻人。",
    mvCount: 2,
    hotSongTarget: 12,
    similar: ["ar04", "ar03", "ar13"],
  },
  {
    mid: "ar10",
    name: "温叙",
    desc: "温叙，影视配乐作者，习惯用一台旧钢琴采样讲故事。剧集《拾光纪》原声带是他目前体量最大的作品。",
    mvCount: 1,
    hotSongTarget: 12,
    similar: ["ar06", "ar11", "ar03"],
  },
  {
    mid: "ar11",
    name: "洛书",
    desc: "洛书，国风电子制作人，把志怪小说与合成器塞进同一首歌。常与 ACG 歌手合作，同名主打《山海入梦》由岸芷汀兰献声，动画 PV 播放量破千万。",
    mvCount: 3,
    hotSongTarget: 16,
    similar: ["ar03", "ar08", "ar12"],
  },
  {
    mid: "ar12",
    name: "岸芷汀兰",
    desc: "岸芷汀兰，ACG 歌手，为多部动画演唱主题曲，音色甜而不腻。代表作《汀兰谣》为动画《兰汀物语》片头曲。",
    mvCount: 4,
    hotSongTarget: 12,
    similar: ["ar01", "ar11", "ar07"],
  },
  {
    mid: "ar13",
    name: "姜聿",
    desc: "姜聿，R&B 唱作人，节奏松弛，旋律黏人。专辑《夜航星》适合城市晚高峰与深夜环路。",
    mvCount: 2,
    hotSongTarget: 12,
    similar: ["ar01", "ar05", "ar09"],
  },
  {
    mid: "ar14",
    name: "南栀",
    desc: "南栀，轻音乐唱作人，适合睡前与雨天循环。她说自己的歌\"写给所有慢半拍的人\"。",
    mvCount: 1,
    hotSongTarget: 12,
    similar: ["ar07", "ar02", "ar06"],
  },
];

interface SongSeed {
  mid: string;
  title: string;
  durationSec: number;
  quality?: string;
  /** 合作歌手（主歌手 = 专辑歌手） */
  featuring?: string[];
}

interface AlbumSeed {
  mid: string;
  name: string;
  artistMid: string;
  /** 曲风，供榜单加权与文案使用 */
  genre: string;
  company: string;
  releaseDate: string;
  desc: string;
  favCount: number;
  songs: SongSeed[];
}

const ALBUM_SEEDS: AlbumSeed[] = [
  {
    mid: "al01",
    name: "潮汐来信",
    artistMid: "ar01",
    genre: "流行",
    company: "白日梦研究所",
    releaseDate: "2024-05-20",
    favCount: 128400,
    desc: "白栖遥的第二张创作专辑。四首歌像四封未寄出的信，写海、写晚风、写站台，也写每个欲言又止的瞬间。整张专辑在海岸边的录音棚完成，你能在曲目间隙里听到真实的潮声。",
    songs: [
      { mid: "so001", title: "潮汐来信", durationSec: 252, quality: QUALITY_FLAC },
      { mid: "so002", title: "玻璃海", durationSec: 238 },
      { mid: "so003", title: "借一场晚风", durationSec: 271, quality: QUALITY_HI_RES },
      { mid: "so004", title: "无人站台", durationSec: 227, quality: QUALITY_FLAC },
    ],
  },
  {
    mid: "al02",
    name: "雾中灯塔",
    artistMid: "ar03",
    genre: "电子",
    company: "雾岛独立厂牌",
    releaseDate: "2023-11-02",
    favCount: 86200,
    desc: "雾岛电台首张录音室专辑。乐队把海雾、轮渡汽笛与灯塔白噪音采样进合成器，录成了一张\"会在夜里发光\"的电子专辑。",
    songs: [
      { mid: "so005", title: "雾中灯塔", durationSec: 302, quality: QUALITY_FLAC },
      { mid: "so006", title: "频率 8.3", durationSec: 255, quality: QUALITY_MP3, featuring: ["ar11"] },
      { mid: "so007", title: "夜间飞行", durationSec: 284 },
    ],
  },
  {
    mid: "al03",
    name: "南方站台",
    artistMid: "ar02",
    genre: "民谣",
    company: "南方铁路唱片",
    releaseDate: "2022-09-14",
    favCount: 64800,
    desc: "陈屿帆的民谣专辑，写小城、铁路与回不去的夏天。全专在南方小城的旧文化馆里同期录制，保留了大量现场呼吸声。",
    songs: [
      { mid: "so008", title: "南方站台", durationSec: 266, quality: QUALITY_FLAC },
      { mid: "so009", title: "绿皮火车", durationSec: 243 },
      { mid: "so010", title: "巷口的猫", durationSec: 221, quality: QUALITY_FLAC },
      { mid: "so011", title: "一封家书", durationSec: 292, quality: QUALITY_MP3 },
    ],
  },
  {
    mid: "al04",
    name: "午夜快线",
    artistMid: "ar04",
    genre: "摇滚",
    company: "破晓文化",
    releaseDate: "2023-06-18",
    favCount: 45600,
    desc: "破晓列车的摇滚专辑，献给每一个末班车上的夜归人。失真吉他与合成器在午夜并线，速度拉满。",
    songs: [
      { mid: "so012", title: "午夜快线", durationSec: 232, quality: QUALITY_FLAC },
      { mid: "so013", title: "霓虹休克", durationSec: 248 },
      { mid: "so014", title: "逆风奔跑", durationSec: 216, quality: QUALITY_MP3 },
    ],
  },
  {
    mid: "al05",
    name: "月白",
    artistMid: "ar06",
    genre: "古典",
    company: "月白古典社",
    releaseDate: "2021-03-26",
    favCount: 32100,
    desc: "顾清商的钢琴独奏专辑，取\"月白\"为色，收录三段关于夜色的即兴与练习。录音采用单点收音，保留踏板与琴体共鸣。",
    songs: [
      { mid: "so015", title: "月白 · 前奏曲", durationSec: 213, quality: QUALITY_FLAC },
      { mid: "so016", title: "雨打芭蕉即兴曲", durationSec: 310, quality: QUALITY_HI_RES },
      { mid: "so017", title: "晨光练习曲", durationSec: 245 },
    ],
  },
  {
    mid: "al06",
    name: "拾光纪",
    artistMid: "ar10",
    genre: "影视原声",
    company: "拾光影业",
    releaseDate: "2024-01-08",
    favCount: 96700,
    desc: "同名剧集原声带，由温叙操刀。以一台一九八零年代的旧钢琴为主轴，串联起剧中三代人的午后与车站。",
    songs: [
      { mid: "so018", title: "拾光", durationSec: 258, quality: QUALITY_FLAC, featuring: ["ar01"] },
      { mid: "so019", title: "老照片", durationSec: 209, quality: QUALITY_HI_RES },
      { mid: "so020", title: "车站别离", durationSec: 280, quality: QUALITY_MP3 },
      { mid: "so021", title: "尾声 · 致每个午后", durationSec: 195 },
    ],
  },
  {
    mid: "al07",
    name: "星尘漫游指南",
    artistMid: "ar08",
    genre: "电子",
    company: "星尘电子",
    releaseDate: "2024-08-30",
    favCount: 51300,
    desc: "星尘信号的电子专辑，主题是深空、失眠与便利店。请把它当作一次不需要返程票的漫游。",
    songs: [
      { mid: "so022", title: "星尘漫游指南", durationSec: 295, quality: QUALITY_FLAC },
      { mid: "so023", title: "引力失效", durationSec: 252, quality: QUALITY_HI_RES },
      { mid: "so024", title: "深空便利店", durationSec: 238 },
    ],
  },
  {
    mid: "al08",
    name: "折枝辞",
    artistMid: "ar07",
    genre: "民谣",
    company: "折枝民谣社",
    releaseDate: "2023-03-21",
    favCount: 28900,
    desc: "苏折枝的民谣专辑，词曲古雅，写渡口、春信与折枝。编曲克制，一把吉他就够。",
    songs: [
      { mid: "so025", title: "折枝辞", durationSec: 261, quality: QUALITY_FLAC },
      { mid: "so026", title: "春分信", durationSec: 234 },
      { mid: "so027", title: "渡口", durationSec: 276, quality: QUALITY_MP3 },
    ],
  },
  {
    mid: "al09",
    name: "仲夏航行",
    artistMid: "ar09",
    genre: "摇滚",
    company: "未眠者自制",
    releaseDate: "2022-12-09",
    favCount: 38400,
    desc: "未眠者乐队首张全长专辑。仲夏夜的水面、失眠电台与蝉鸣鼓点，献给所有睡不着的年轻人。",
    songs: [
      { mid: "so028", title: "仲夏航行", durationSec: 269, quality: QUALITY_FLAC },
      { mid: "so029", title: "失眠电台", durationSec: 227, quality: QUALITY_HI_RES },
      { mid: "so030", title: "鼓点与蝉鸣", durationSec: 238 },
    ],
  },
  {
    mid: "al10",
    name: "山海入梦",
    artistMid: "ar11",
    genre: "国风电子",
    company: "山海国风",
    releaseDate: "2024-10-17",
    favCount: 105600,
    desc: "洛书的国风电子专辑，把志怪故事搬进合成器：山神的雾、灯下的辞、观潮的人。岸芷汀兰献声同名主打。",
    songs: [
      { mid: "so031", title: "山海入梦", durationSec: 273, quality: QUALITY_FLAC, featuring: ["ar12"] },
      { mid: "so032", title: "灯下辞", durationSec: 229, quality: QUALITY_HI_RES },
      { mid: "so033", title: "观潮", durationSec: 259 },
    ],
  },
  {
    mid: "al11",
    name: "橘子汽水与晚风",
    artistMid: "ar05",
    genre: "流行",
    company: "橘子汽水工作室",
    releaseDate: "2023-07-22",
    favCount: 72500,
    desc: "林晚风的首张个人专辑，写夏天的一切小事：汽水、晚风、信箱和一场没赶上的雨。",
    songs: [
      { mid: "so034", title: "橘子汽水", durationSec: 211, quality: QUALITY_MP3, featuring: ["ar14"] },
      { mid: "so035", title: "晚风信箱", durationSec: 242 },
      { mid: "so036", title: "夏至未至的雨", durationSec: 224 },
    ],
  },
  {
    mid: "al12",
    name: "汀兰谣",
    artistMid: "ar12",
    genre: "ACG",
    company: "兰汀文化",
    releaseDate: "2024-04-04",
    favCount: 88900,
    desc: "动画《兰汀物语》原声专辑，岸芷汀兰演唱。收录片头曲《汀兰谣》及剧中插曲。",
    songs: [
      { mid: "so037", title: "汀兰谣", durationSec: 254, quality: QUALITY_FLAC, featuring: ["ar07"] },
      { mid: "so038", title: "星海航路", durationSec: 236 },
      { mid: "so039", title: "萤火之径", durationSec: 247, quality: QUALITY_MP3 },
    ],
  },
  {
    mid: "al13",
    name: "夜航星",
    artistMid: "ar13",
    genre: "R&B",
    company: "夜航音乐",
    releaseDate: "2024-09-12",
    favCount: 42700,
    desc: "姜聿的 R&B 专辑，节奏像夜航一样松弛。适合城市晚高峰与深夜环路。",
    songs: [
      { mid: "so040", title: "夜航星", durationSec: 262, quality: QUALITY_FLAC },
      { mid: "so041", title: "三点水的甜", durationSec: 218, quality: QUALITY_HI_RES },
      { mid: "so042", title: "慢速心动", durationSec: 251 },
    ],
  },
  {
    mid: "al14",
    name: "慢半拍",
    artistMid: "ar14",
    genre: "轻音",
    company: "慢半拍工作室",
    releaseDate: "2023-10-26",
    favCount: 30200,
    desc: "南栀的轻音专辑，写给所有慢半拍的人。不着急，好的都值得等。",
    songs: [
      { mid: "so043", title: "慢半拍", durationSec: 245, quality: QUALITY_FLAC },
      { mid: "so044", title: "云朵商店", durationSec: 222, quality: QUALITY_HI_RES },
      { mid: "so045", title: "十点半的月光", durationSec: 266, quality: QUALITY_MP3 },
    ],
  },
];

interface PlaylistSeed {
  id: string;
  name: string;
  creator: string;
  tags: string[];
  desc: string;
  playCount: number;
  songMids: string[];
}

const PLAYLIST_SEEDS: PlaylistSeed[] = [
  {
    id: "pl01",
    name: "深夜写代码 BGM 指南",
    creator: "代码写不完",
    tags: ["学习", "电子", "专注"],
    playCount: 1284567,
    songMids: ["so005", "so006", "so007", "so022", "so023", "so024", "so031", "so032", "so033", "so043", "so044", "so040", "so015"],
    desc: "写给所有凌晨还亮着的屏幕。电子、国风电子和轻音混着来，人声不多，够安静也够带感。改完最后一个 bug 之前，别关掉它。",
  },
  {
    id: "pl02",
    name: "南方、站台与绿皮火车",
    creator: "拾荒的诗人",
    tags: ["民谣", "旅行", "治愈"],
    playCount: 862104,
    songMids: ["so008", "so009", "so010", "so011", "so025", "so026", "so027", "so035", "so036", "so043", "so045", "so001", "so018", "so004"],
    desc: "从绿皮火车到高铁，站台一直是民谣的故乡。这些歌唱的都是离开与抵达——愿你也有一个值得回去的小城。",
  },
  {
    id: "pl03",
    name: "兰汀物语 · 二次元浓度超标",
    creator: "Momo不在家",
    tags: ["ACG", "动漫", "电子"],
    playCount: 2140882,
    songMids: ["so037", "so038", "so039", "so031", "so033", "so005", "so007", "so022", "so024", "so001", "so040", "so042", "so044"],
    desc: "为动画《兰汀物语》整季整理的原声向歌单，从片头曲一路听到插曲。二次元浓度超标预警，次元壁脆弱者请系好安全带。",
  },
  {
    id: "pl04",
    name: "雨夜轻钢琴与慢歌",
    creator: "风住过的街道",
    tags: ["轻音", "钢琴", "睡前"],
    playCount: 654310,
    songMids: ["so015", "so016", "so017", "so019", "so020", "so021", "so043", "so044", "so045", "so026", "so011", "so042"],
    desc: "雨声是最好的编曲，钢琴是最慢的述说。睡前音量食用更佳，愿你好梦。",
  },
];

// ————————————————————————————————————————————————————————————
// 工厂：歌手 / 专辑 / 歌曲
// ————————————————————————————————————————————————————————————

const artistSeedByMid = new Map(ARTIST_SEEDS.map((seed) => [seed.mid, seed]));
const albumSeedByMid = new Map(ALBUM_SEEDS.map((seed) => [seed.mid, seed]));

function artistRefOf(mid: string): ArtistRef {
  const seed = artistSeedByMid.get(mid);
  if (!seed) throw new Error(`unknown artist seed: ${mid}`);
  return { mid: seed.mid, name: seed.name };
}

/** 完整专辑（14 张，每张 3–4 首）编译为领域对象；搜索只在这批专辑里做 */
export const curatedAlbums: AlbumDetail[] = ALBUM_SEEDS.map((seed) => {
  const artist = artistRefOf(seed.artistMid);
  const picUrl = coverUrl(`album:${seed.mid}`);
  return {
    mid: seed.mid,
    name: seed.name,
    artist,
    picUrl,
    company: seed.company,
    releaseDate: seed.releaseDate,
    desc: seed.desc,
    songs: seed.songs.map((song) => ({
      mid: song.mid,
      title: song.title,
      artists: [artist, ...(song.featuring ?? []).map(artistRefOf)],
      album: { mid: seed.mid, name: seed.name, picUrl },
      durationMs: song.durationSec * 1000,
      quality: song.quality,
    })),
    favCount: seed.favCount,
  };
});

/** 总歌曲池：榜单 / 推荐 / 搜索都从这里取，保证引用一致 */
export const songPool: SongRef[] = curatedAlbums.flatMap((album) => album.songs);
const songByMid = new Map(songPool.map((song) => [song.mid, song]));
const albumDetailByMid = new Map(curatedAlbums.map((album) => [album.mid, album]));
export const allArtistRefs: ArtistRef[] = ARTIST_SEEDS.map((seed) => ({
  mid: seed.mid,
  name: seed.name,
}));

// 数字单曲：为每位歌手补足"热门歌曲"数量（12–20 首）。
// 每首单曲有自己的同名数字单曲专辑（QQ 音乐常见形态），并登记进专辑目录，
// 这样从歌手页点进单曲的专辑也不会 404。
const SINGLE_TITLE_BANK = [
  "未寄出的明信片",
  "候鸟第十七日",
  "枕着星光入睡",
  "半糖失眠",
  "屋顶上的电台",
  "纸飞机航线",
  "雾散之前",
  "慢速快门",
  "一个人的合唱",
  "夏天的备用钥匙",
  "候车室的除夕",
  "橙色黄昏",
  "退潮之后",
  "街角修理铺",
  "三月的第三封信",
  "萤火虫站台",
  "云层之上",
  "白日梦售票处",
  "旧磁带 B 面",
  "迟到的春天",
  "路灯下的圆舞曲",
  "无声电台",
  "十字路口的北",
  "给星期三的信",
  "银河修理工",
  "窗台植物观察日记",
  "六月的长镜头",
  "午夜便利店",
  "归航",
  "过云雨备忘",
];

function pickSingleQuality(hash: number): string | undefined {
  const bucket = hash % 10;
  if (bucket < 3) return QUALITY_FLAC; // ~30%
  if (bucket < 5) return QUALITY_HI_RES; // ~20%
  if (bucket < 7) return QUALITY_MP3; // ~20%
  return undefined; // 其余不给音质徽章
}

function singleReleaseDate(songMid: string): string {
  const hash = hashSeed(`date:${songMid}`);
  const month = String(1 + (hash % 12)).padStart(2, "0");
  const day = String(1 + ((hash >>> 8) % 28)).padStart(2, "0");
  return `${2023 + ((hash >>> 16) % 3)}-${month}-${day}`;
}

const singlesByArtist = new Map<string, SongRef[]>();
const singleAlbumByMid = new Map<string, AlbumDetail>();

ARTIST_SEEDS.forEach((seed, artistIndex) => {
  const curatedCount = songPool.filter((song) =>
    song.artists.some((artist) => artist.mid === seed.mid),
  ).length;
  const singleCount = Math.max(0, seed.hotSongTarget - curatedCount);
  const artist = { mid: seed.mid, name: seed.name };
  // (artistIndex*11 + i*7) 与 30 互质步进，保证同一歌手 12 首单曲标题互不重复
  const singles: SongRef[] = Array.from({ length: singleCount }, (_, i) => {
    const title = SINGLE_TITLE_BANK[(artistIndex * 11 + i * 7) % SINGLE_TITLE_BANK.length];
    const songMid = `so${artistIndex + 1}x${i + 1}`;
    const albumMid = `al${artistIndex + 1}x${i + 1}`;
    const hash = hashSeed(`single:${songMid}`);
    return {
      mid: songMid,
      title,
      artists: [artist],
      album: { mid: albumMid, name: title, picUrl: coverUrl(`album:${albumMid}`) },
      durationMs: (180 + (hash % 141)) * 1000, // 180s–320s
      quality: pickSingleQuality(hash >>> 4),
    };
  });
  singlesByArtist.set(seed.mid, singles);
  singles.forEach((song) => {
    singleAlbumByMid.set(song.album.mid, {
      mid: song.album.mid,
      name: song.album.name,
      artist,
      picUrl: song.album.picUrl,
      company: "独立发行",
      releaseDate: singleReleaseDate(song.mid),
      desc: `数字单曲专辑，收录同名主打曲《${song.album.name}》。`,
      songs: [song],
      favCount: 800 + (hashSeed(`fav:${song.mid}`) % 40000),
    });
  });
});

// ————————————————————————————————————————————————————————————
// 工厂：歌手页
// ————————————————————————————————————————————————————————————

function curatedSongsOf(artistMid: string): SongRef[] {
  return songPool.filter((song) =>
    song.artists.some((artist) => artist.mid === artistMid),
  );
}

/** 稳定"热度"：同一首歌在任何页面、任何时间热度一致，各处排序才对得上 */
function popularity(songMid: string): number {
  return hashSeed(`pop:${songMid}`) % 10000;
}

export function artistSongs(artistMid: string): SongRef[] {
  const seed = artistSeedByMid.get(artistMid);
  if (!seed) return [];
  return [...curatedSongsOf(artistMid), ...(singlesByArtist.get(artistMid) ?? [])]
    .sort((a, b) => popularity(b.mid) - popularity(a.mid))
    .slice(0, seed.hotSongTarget);
}

export function artistAlbums(artistMid: string): AlbumDetail[] {
  const ownAlbums = curatedAlbums.filter((album) => album.artist.mid === artistMid);
  const ownSingles = (singlesByArtist.get(artistMid) ?? []).flatMap((song) => {
    const album = albumDetailByMid.get(song.album.mid) ?? singleAlbumByMid.get(song.album.mid);
    return album ? [album] : [];
  });
  return [...ownAlbums, ...ownSingles];
}

export function artistInfo(artistMid: string): ArtistInfo | undefined {
  const seed = artistSeedByMid.get(artistMid);
  if (!seed) return undefined;
  return {
    mid: seed.mid,
    name: seed.name,
    picUrl: coverUrl(`artist:${seed.mid}`),
    songCount: artistSongs(seed.mid).length,
    albumCount: artistAlbums(seed.mid).length,
    mvCount: seed.mvCount,
    desc: seed.desc,
    similar: seed.similar.map(artistRefOf),
  };
}

// ————————————————————————————————————————————————————————————
// 工厂：排行榜
// ————————————————————————————————————————————————————————————

interface ChartSeed {
  id: string;
  name: string;
  /** 各榜以不同口径对总池排序；口径本身是纯函数，结果确定 */
  score: (song: SongRef) => number;
}

function genreBonus(song: SongRef, keywords: string[]): number {
  const seed = albumSeedByMid.get(song.album.mid);
  if (!seed) return 0;
  return keywords.some((keyword) => seed.genre.includes(keyword)) ? 5000 : 0;
}

/** "2024-05-20" → 可比较整数（新歌榜排序用） */
function dateKey(date: string): number {
  return Number(date.slice(0, 4) + date.slice(5, 7) + date.slice(8, 10));
}

function releaseScore(song: SongRef): number {
  const seed = albumSeedByMid.get(song.album.mid);
  return (seed ? dateKey(seed.releaseDate) : 0) * 10000 + popularity(song.mid);
}

const CHART_SEEDS: ChartSeed[] = [
  {
    id: "soar",
    name: "飙升榜",
    score: (song) => popularity(song.mid) + (hashSeed(`rise:${song.mid}`) % 4200),
  },
  { id: "hot", name: "热歌榜", score: (song) => popularity(song.mid) },
  { id: "new", name: "新歌榜", score: releaseScore },
  {
    id: "original",
    name: "原创榜",
    score: (song) =>
      genreBonus(song, ["民谣", "摇滚", "流行"]) + popularity(song.mid),
  },
  {
    id: "acg",
    name: "ACG 曲榜",
    score: (song) =>
      genreBonus(song, ["ACG", "电子", "国风"]) + popularity(song.mid),
  },
  {
    id: "chill",
    name: "轻音慢歌榜",
    score: (song) =>
      genreBonus(song, ["民谣", "古典", "轻音", "R&B", "影视"]) +
      popularity(song.mid),
  },
];

function categoryOf(chart: ChartSeed): TopCategory {
  return {
    id: chart.id,
    name: chart.name,
    picUrl: coverUrl(`top:${chart.id}`),
    updateTime: CHART_UPDATE_TIME,
    trackCount: CHART_SIZE,
  };
}

// prevRank 相对本期名次做 ±4 抖动，约 1/7 直接给 undefined（新上榜），
// 让榜单详情页的升降箭头三种状态都能出现。
function prevRankOf(chartId: string, rank: number, songMid: string): number | undefined {
  const hash = hashSeed(`prev:${chartId}:${songMid}`);
  if (hash % 7 === 0) return undefined;
  const prev = rank + ((hash % 9) - 4);
  return prev >= 1 && prev <= CHART_SIZE ? prev : undefined;
}

function playCountOf(rank: number, songMid: string): number {
  return 8_000_000 + (CHART_SIZE - rank) * 1_100_000 + (hashSeed(`pc:${songMid}`) % 900_000);
}

export function topCategories(): TopCategory[] {
  return CHART_SEEDS.map(categoryOf);
}

export function topDetail(id: string): TopDetail | undefined {
  const chart = CHART_SEEDS.find((seed) => seed.id === id);
  if (!chart) return undefined;
  const ranked = [...songPool]
    .sort((a, b) => chart.score(b) - chart.score(a))
    .slice(0, CHART_SIZE);
  const entries: ChartEntry[] = ranked.map((song, index) => {
    const rank = index + 1;
    return {
      rank,
      prevRank: prevRankOf(chart.id, rank, song.mid),
      song,
      playCount: playCountOf(rank, song.mid),
    };
  });
  return { category: categoryOf(chart), entries };
}

// ————————————————————————————————————————————————————————————
// 工厂：歌单 / 推荐
// ————————————————————————————————————————————————————————————

function playlistRefOf(seed: PlaylistSeed): PlaylistRef {
  return {
    id: seed.id,
    name: seed.name,
    coverUrl: coverUrl(`playlist:${seed.id}`),
    playCount: seed.playCount,
  };
}

export function playlistRefs(): PlaylistRef[] {
  return PLAYLIST_SEEDS.map(playlistRefOf);
}

export function playlistDetail(id: string): PlaylistDetail | undefined {
  const seed = PLAYLIST_SEEDS.find((item) => item.id === id);
  if (!seed) return undefined;
  const songs = seed.songMids.flatMap((mid) => {
    const song = songByMid.get(mid);
    return song ? [song] : [];
  });
  return {
    id: seed.id,
    name: seed.name,
    coverUrl: coverUrl(`playlist:${seed.id}`),
    creator: { name: seed.creator, avatarUrl: avatarUrl(`user:${seed.creator}`) },
    tags: seed.tags,
    trackCount: songs.length,
    playCount: seed.playCount,
    desc: seed.desc,
    songs,
  };
}

export function recommendFeed(): RecommendFeed {
  // 猜你喜欢：手工挑 5 首跨曲风的歌，比纯算法更能展示 TrackTable 混排
  const guessMids = ["so001", "so012", "so025", "so031", "so040"];
  const guessYouLike = guessMids.flatMap((mid) => {
    const song = songByMid.get(mid);
    return song ? [song] : [];
  });
  const newSongs = [...songPool].sort((a, b) => releaseScore(b) - releaseScore(a)).slice(0, 8);
  return {
    daily: {
      title: "每日推荐",
      date: FEED_DATE,
      coverUrl: coverUrl(`daily:${FEED_DATE}`),
    },
    guessYouLike,
    newSongs,
    topCharts: topCategories(),
    playlists: playlistRefs(),
  };
}

// ————————————————————————————————————————————————————————————
// 工厂：评论区
// ————————————————————————————————————————————————————————————

interface CommentReplySeed {
  id: string;
  user: string;
  content: string;
  likes: number;
}

interface CommentSeed {
  id: string;
  user: string;
  time: string;
  content: string;
  likes: number;
  location?: string;
  pinned?: boolean;
  replies?: CommentReplySeed[];
  replyCount?: number;
}

// 评论池：数组顺序即时间顺序（旧 → 新），供"最新"排序使用。
// 全部为虚构用户与虚构评论，语气刻意参差——有长有短、有梗有真情实感。
const COMMENT_POOL: CommentSeed[] = [
  { id: "c01", user: "山间邮筒", time: "06月02日 21:18", likes: 8321, location: "广东", content: "2026 年了，还有人在这里报到吗？——有，每天睡前一遍。" },
  {
    id: "c02", user: "橘白猫店长", time: "06月10日 08:47", likes: 12004, location: "浙江·杭州", pinned: true,
    content: "耳机分你一半，这首歌也分你一半。评论区里留下来的人，我们都是同类。",
    replies: [
      { id: "c02r1", user: "凌晨四点半", content: "同类+1，晚安。", likes: 862 },
      { id: "c02r2", user: "汽水不加冰", content: "头像好可爱，歌也好听。", likes: 431 },
    ],
    replyCount: 128,
  },
  {
    id: "c03", user: "凌晨四点半", time: "06月15日 23:55", likes: 6420, location: "四川·成都",
    content: "前奏一响，我们家音箱都自觉调大了一格。",
    replies: [{ id: "c03r1", user: "老橡树", content: "音箱：谢谢配合。", likes: 298 }],
    replyCount: 45,
  },
  { id: "c04", user: "汽水不加冰", time: "06月21日 14:02", likes: 5873, location: "上海", content: "高考完的那个夏天循环了整个七月，现在前奏一起还是会鼻子发酸。" },
  {
    id: "c05", user: "老橡树", time: "06月28日 19:36", likes: 4310, location: "北京",
    content: "说个冷知识：这首歌的鼓是现场一次录完的，后面几轨都是围着它铺的。",
    replies: [{ id: "c05r1", user: "晚风修音师", content: "冷知识+1，鼓手直播里说过。", likes: 512 }],
    replyCount: 89,
  },
  { id: "c06", user: "Momo不在家", time: "07月03日 00:12", likes: 3987, location: "江苏·南京", content: "编曲里那个若有若无的合成器音色好绝，戴耳机才能听见，建议全程耳机。" },
  { id: "c07", user: "拾荒的诗人", time: "07月08日 22:41", likes: 3562, location: "湖北·武汉", content: "在地铁上听哭了，只好假装打哈欠。旁边大哥递给我一张纸巾，谢谢你，陌生人大哥。" },
  { id: "c08", user: "风住过的街道", time: "07月12日 17:29", likes: 2984, location: "福建·厦门", content: "副歌那段和声是神来之笔，循环一百遍都不腻。" },
  {
    id: "c09", user: "蓝鲸加载中", time: "07月15日 12:08", likes: 2541,
    content: "求同款翻唱，前两天刷到一个宝藏翻唱，嗓子像是泡过海水。",
    replies: [{ id: "c09r1", user: "Momo不在家", content: "求链接！", likes: 67 }],
    replyCount: 23,
  },
  { id: "c10", user: "晚风修音师", time: "07月19日 20:33", likes: 2308, location: "广东·深圳", content: "歌词写得真克制，最喜欢的就是那句没说出口的。留白才是中文歌词的浪漫。" },
  { id: "c11", user: "一只废柴柴", time: "07月23日 09:15", likes: 1876, location: "陕西·西安", content: "深夜赶论文，靠这首歌续命。凌晨三点，它比咖啡管用。" },
  { id: "c12", user: "南巷旧人", time: "07月26日 21:57", likes: 1654, location: "湖南·长沙", content: "第一遍觉得一般，第二遍开始就出不来了。有些歌就是得给它两次机会。" },
  { id: "c13", user: "雾里看海的人", time: "07月30日 23:04", likes: 1420, location: "山东·青岛", content: "这贝斯线也太丝滑了，戴耳机听像有人贴着耳膜画画。" },
  { id: "c14", user: "代码写不完", time: "08月02日 10:26", likes: 1332, location: "广东", content: "路过一家奶茶店在放这首歌，进去买了杯其实不爱喝的珍珠奶茶。歌是好歌，奶茶是难喝的。" },
  { id: "c15", user: "山间邮筒", time: "08月05日 18:44", likes: 1102, location: "广东", content: "副歌一起来我整个人在原地打拍子，同事以为我在抖腿，其实我在打拍子。" },
  { id: "c16", user: "橘白猫店长", time: "08月08日 07:03", likes: 987, location: "浙江·杭州", content: "单曲循环第 41 遍打卡。每十遍换一次音质，FLAC 确实不一样。" },
  { id: "c17", user: "凌晨四点半", time: "08月11日 01:22", likes: 876, location: "四川·成都", content: "想把这首歌设成闹钟，又怕醒来第一件事是把闹钟卸载。" },
  { id: "c18", user: "汽水不加冰", time: "08月14日 16:40", likes: 765, location: "上海", content: "钢琴进来那一下，世界都安静了。" },
  {
    id: "c19", user: "老橡树", time: "08月17日 20:19", likes: 690, location: "北京",
    content: "上个月看了现场，比录音室版还稳，票值回十倍。蹲一个明年巡演。",
    replies: [{ id: "c19r1", user: "拾荒的诗人", content: "同蹲！来我们城市吧！", likes: 44 }],
    replyCount: 12,
  },
  { id: "c20", user: "Momo不在家", time: "08月20日 13:51", likes: 588, content: "有人知道这首歌的吉他谱吗？想学，学会就去找小时候的自己显摆。" },
  { id: "c21", user: "拾荒的诗人", time: "08月23日 22:10", likes: 512, location: "湖北·武汉", content: "距离上次听过去五年了，前奏响起还是那个夏天的味道。原来歌是有保质期的，保质期是永远。" },
  { id: "c22", user: "风住过的街道", time: "08月26日 19:08", likes: 447, location: "福建·厦门", content: "评论区怎么都在讲故事，我是来听歌的，结果看评论看哭了。" },
  { id: "c23", user: "蓝鲸加载中", time: "08月29日 08:36", likes: 401, content: "歌词翻译也加分，读完原文再读翻译，像同一首诗的两个侧面。" },
  { id: "c24", user: "晚风修音师", time: "09月01日 11:47", likes: 356, location: "广东·深圳", content: "洗车的时候随机到这首歌，雨刮器都在打拍子。师傅以为我中彩票了。" },
  { id: "c25", user: "一只废柴柴", time: "09月02日 23:33", likes: 298, location: "陕西·西安", content: "送给三年后的自己：希望你还在听歌，还热爱生活，还没秃。" },
  { id: "c26", user: "南巷旧人", time: "09月03日 07:52", likes: 240, location: "湖南·长沙", content: "好听的，就是有点费耳机。" },
  { id: "c27", user: "雾里看海的人", time: "09月03日 21:15", likes: 205, location: "山东·青岛", content: "副歌的转音学不会，唱 K 必跳过，不听又不行，很纠结。" },
  {
    id: "c28", user: "代码写不完", time: "09月04日 00:28", likes: 189, location: "广东",
    content: "刚失恋的人别听，会哭得更凶。……但我还是单曲循环了一整晚。",
    replies: [
      { id: "c28r1", user: "山间邮筒", content: "抱抱，会过去的。", likes: 156 },
      { id: "c28r2", user: "Momo不在家", content: "同款夜晚，明天会好的。", likes: 98 },
    ],
    replyCount: 77,
  },
  { id: "c29", user: "山间邮筒", time: "09月04日 12:36", likes: 154, location: "广东", content: "制作水准是真的高，每一轨都经得起单独细听。" },
  { id: "c30", user: "橘白猫店长", time: "09月04日 15:49", likes: 88, location: "浙江·杭州", content: "嗯，好听。" },
  { id: "c31", user: "凌晨四点半", time: "09月04日 21:02", likes: 76, location: "四川·成都", content: "蹲一个巡演，求求了，来我们城市吧。票钱我已经攒好了。" },
  { id: "c32", user: "汽水不加冰", time: "09月05日 00:11", likes: 61, location: "上海", content: "听完顺手把整张专辑听完了，一张没踩雷，难得。" },
  { id: "c33", user: "老橡树", time: "09月05日 08:24", likes: 52, location: "北京", content: "听到这里的你也早点睡，晚安，做个好梦。" },
];

function compileReply(seed: CommentReplySeed): CommentReply {
  return {
    id: seed.id,
    user: { name: seed.user, avatarUrl: avatarUrl(`user:${seed.user}`) },
    content: seed.content,
    likes: seed.likes,
  };
}

function compileComment(seed: CommentSeed): Comment {
  return {
    id: seed.id,
    user: { name: seed.user, avatarUrl: avatarUrl(`user:${seed.user}`) },
    time: seed.time,
    location: seed.location,
    content: seed.content,
    likes: seed.likes,
    isPinned: seed.pinned,
    replies: (seed.replies ?? []).map(compileReply),
    replyCount: seed.replyCount,
  };
}

/**
 * 按歌曲 mid 从评论池确定性取一个连续窗口（12–18 条）组装评论区。
 * 置顶评论强制包含（QQ 音乐式：置顶永远可见），窗口抽不到时补进首位。
 * sort 当前不影响返回结构：mock 一次性返回热评 + 最新两个分区，UI 端切换
 * tab 无需重新请求；接真实后端时把 sort 映射为排序参数即可。
 */
export function commentSection(songMid: string, _sort?: "hot" | "new"): CommentSection {
  const hash = hashSeed(`comment:${songMid}`);
  const count = 12 + (hash % 7); // 12–18 条
  const start = hash % COMMENT_POOL.length;
  const pickedSeeds = Array.from(
    { length: count },
    (_, i) => COMMENT_POOL[(start + i) % COMMENT_POOL.length],
  );
  const pinned = COMMENT_POOL.find((seed) => seed.pinned);
  if (pinned && !pickedSeeds.some((seed) => seed.pinned)) {
    pickedSeeds[0] = pinned; // 挤掉窗口首位，总数保持不变
  }
  const picked: Comment[] = pickedSeeds.map(compileComment);
  const hot = [...picked].sort(
    (a, b) => Number(b.isPinned ?? false) - Number(a.isPinned ?? false) || b.likes - a.likes,
  );
  // 池内顺序即时间顺序（旧 → 新），倒序即"最新"
  const latest = [...picked].reverse();
  return {
    total: 18_000 + (hash % 162_000), // 1.8 万 – 18 万，符合"数万级"
    hot,
    latest,
  };
}

// ————————————————————————————————————————————————————————————
// 工厂：歌词
// ————————————————————————————————————————————————————————————

// 三首主打歌制作完整 Lrc 风格逐行歌词（含 trans 翻译行），其余走通用占位。
const FULL_LYRICS: Record<string, LyricLine[]> = {
  so001: [
    { timeMs: 18_000, text: "信纸上有潮汐的痕迹", trans: "There are tidemarks on this letter" },
    { timeMs: 24_500, text: "那是海替我练过的笔迹", trans: "Handwriting the sea rehearsed for me" },
    { timeMs: 31_000, text: "写到你的名字就停下", trans: "I always stop at your name" },
    { timeMs: 37_500, text: "墨水在纸上退成了浪", trans: "The ink recedes into waves on the page" },
    { timeMs: 46_000, text: "我把想念折成纸船", trans: "I fold my longing into a paper boat" },
    { timeMs: 53_000, text: "放进涨潮的夜里", trans: "And set it adrift on the rising tide" },
    { timeMs: 61_000, text: "如果它搁浅在你的岸", trans: "If it runs aground on your shore" },
    { timeMs: 68_000, text: "请替我说一声晚安", trans: "Please say goodnight for me" },
    { timeMs: 80_000, text: "潮汐来信 落款是风", trans: "A letter from the tide, signed by the wind" },
    { timeMs: 87_000, text: "每一行都涨落不定", trans: "Every line rises and falls" },
    { timeMs: 95_000, text: "读到最后一句 天就亮了", trans: "By the last line the sky turns light" },
    { timeMs: 103_000, text: "潮汐来信 邮戳是月", trans: "A letter from the tide, postmarked by the moon" },
    { timeMs: 110_000, text: "盖在午夜两点的沙滩上", trans: "Stamped on the sand at two a.m." },
    { timeMs: 118_000, text: "你读或者不读", trans: "Read it or not" },
    { timeMs: 125_000, text: "海都会再来一趟", trans: "The sea will come back all the same" },
    { timeMs: 148_000, text: "后来信封换了季节", trans: "Later the envelope changed its season" },
    { timeMs: 155_000, text: "邮票上落了初雪", trans: "First snow fell upon the stamp" },
    { timeMs: 162_000, text: "我把地址划掉重写", trans: "I crossed the address out and wrote anew" },
    { timeMs: 169_000, text: "反正潮水记得回程", trans: "The tide remembers the way back anyway" },
    { timeMs: 181_000, text: "潮汐来信 落款是风", trans: "A letter from the tide, signed by the wind" },
    { timeMs: 188_000, text: "每一行都涨落不定", trans: "Every line rises and falls" },
    { timeMs: 196_000, text: "读到最后一句 天就亮了", trans: "By the last line the sky turns light" },
    { timeMs: 204_000, text: "潮汐来信 邮戳是月", trans: "A letter from the tide, postmarked by the moon" },
    { timeMs: 211_000, text: "盖在午夜两点的沙滩上", trans: "Stamped on the sand at two a.m." },
    { timeMs: 219_000, text: "你读或者不读", trans: "Read it or not" },
    { timeMs: 226_000, text: "海都会再来一趟", trans: "The sea will come back all the same" },
    { timeMs: 244_000, text: "潮声渐远", trans: "The tide fades into the distance" },
  ],
  so008: [
    { timeMs: 20_000, text: "绿皮火车摇过第三个隧道", trans: "The green train sways through the third tunnel" },
    { timeMs: 28_000, text: "车窗上你哈了一口气", trans: "You breathe on the window glass" },
    { timeMs: 36_000, text: "写下没看清的再见", trans: "And write a goodbye I never quite read" },
    { timeMs: 45_000, text: "南方的雨追了一路", trans: "The southern rain chased us all the way" },
    { timeMs: 53_000, text: "到站台才追上我", trans: "It caught me only at the platform" },
    { timeMs: 62_000, text: "你说小城的月台太短", trans: "You said the platform of this small town is too short" },
    { timeMs: 70_000, text: "装不下四年的日落", trans: "To hold four years of sunsets" },
    { timeMs: 79_000, text: "可我记得每一根枕木", trans: "Yet I remember every wooden tie" },
    { timeMs: 87_000, text: "都枕着你的名字入眠", trans: "Sleeping on the sound of your name" },
    { timeMs: 99_000, text: "南方站台 汽笛响了两次", trans: "Southern platform — the whistle sounds twice" },
    { timeMs: 107_000, text: "一次送走夏天", trans: "Once to see the summer off" },
    { timeMs: 115_000, text: "一次留下我", trans: "Once to leave me behind" },
    { timeMs: 123_000, text: "南方站台 广播念着晚点", trans: "Southern platform — the speaker announces a delay" },
    { timeMs: 131_000, text: "我却听成了别来无恙", trans: "But I hear \"hope you've been well\"" },
    { timeMs: 139_000, text: "你笑着挥手的样子", trans: "The way you waved with a smile" },
    { timeMs: 146_000, text: "我替铁轨记了一辈子", trans: "I keep for the rails a whole lifetime" },
    { timeMs: 168_000, text: "后来我也去了南方", trans: "Later I went south too" },
    { timeMs: 176_000, text: "在很多站台下过车", trans: "Got off at platform after platform" },
    { timeMs: 184_000, text: "人潮把日子冲得很快", trans: "Crowds wash the days away too fast" },
    { timeMs: 192_000, text: "只有汽笛一响", trans: "Yet one whistle is all it takes" },
    { timeMs: 200_000, text: "所有站台都变成那一个", trans: "And every platform becomes that one" },
    { timeMs: 212_000, text: "南方站台 汽笛响了两次", trans: "Southern platform — the whistle sounds twice" },
    { timeMs: 220_000, text: "一次送走夏天", trans: "Once to see the summer off" },
    { timeMs: 228_000, text: "一次留下我", trans: "Once to leave me behind" },
    { timeMs: 236_000, text: "铁轨替我去了远方", trans: "The rails travel far for me" },
    { timeMs: 248_000, text: "而我还在等下一次晚点", trans: "And I still wait for the next delay" },
  ],
  so031: [
    { timeMs: 17_000, text: "云梦泽的水汽漫过山脊", trans: "Vapor from the sea of clouds spills over the ridge" },
    { timeMs: 25_000, text: "鲸背驮着一座城的灯", trans: "A whale carries the lights of a city on its back" },
    { timeMs: 34_000, text: "你从旧志怪里醒来", trans: "You wake from an old book of legends" },
    { timeMs: 42_000, text: "衣袖上还沾着星子", trans: "Starlight still clinging to your sleeves" },
    { timeMs: 51_000, text: "我借一盏渔火问路", trans: "I borrow a fishing lamp to ask the way" },
    { timeMs: 59_000, text: "山神指了指雾的方向", trans: "The mountain god points into the fog" },
    { timeMs: 68_000, text: "雾里有人采药 酿酒 晾衣", trans: "In the fog, someone gathers herbs, brews wine, dries clothes" },
    { timeMs: 76_000, text: "独独没有回头", trans: "But never once turns around" },
    { timeMs: 88_000, text: "山海入梦 梦入山海", trans: "Mountains and seas enter the dream, the dream enters them" },
    { timeMs: 96_000, text: "你是迟迟不肯醒来的那一层", trans: "You are the layer that refuses to wake" },
    { timeMs: 104_000, text: "钟声敲了三下", trans: "The bell tolls three times" },
    { timeMs: 112_000, text: "惊起白鹭掠过千年", trans: "Startling egrets across a thousand years" },
    { timeMs: 124_000, text: "山海入梦 梦入山海", trans: "Mountains and seas enter the dream, the dream enters them" },
    { timeMs: 132_000, text: "我是把灯芯挑亮的那个人", trans: "I am the one who trims the lamp wick" },
    { timeMs: 140_000, text: "等雾散成一幅画", trans: "Waiting for the fog to scatter into a painting" },
    { timeMs: 149_000, text: "等你从画里说一声 久等了", trans: "Waiting for you to step out and say \"sorry for the wait\"" },
    { timeMs: 172_000, text: "后来志怪都写错了", trans: "The legends later got it wrong" },
    { timeMs: 180_000, text: "说你化作了山间的风", trans: "Saying you became the wind in the hills" },
    { timeMs: 188_000, text: "可我明明在雨夜看见", trans: "Yet I clearly saw on a rainy night" },
    { timeMs: 196_000, text: "你提灯走过石桥", trans: "You crossing the stone bridge with a lantern" },
    { timeMs: 204_000, text: "桥下的鱼衔着月光", trans: "Fish beneath it carrying moonlight in their mouths" },
    { timeMs: 216_000, text: "山海入梦 梦入山海", trans: "Mountains and seas enter the dream, the dream enters them" },
    { timeMs: 224_000, text: "你是迟迟不肯醒来的那一层", trans: "You are the layer that refuses to wake" },
    { timeMs: 232_000, text: "钟声敲了三下", trans: "The bell tolls three times" },
    { timeMs: 240_000, text: "惊起白鹭掠过千年", trans: "Startling egrets across a thousand years" },
    { timeMs: 248_000, text: "山海入梦 梦入山海", trans: "Mountains and seas enter the dream, the dream enters them" },
    { timeMs: 256_000, text: "我把名字写在水上", trans: "I write my name on the water" },
    { timeMs: 263_000, text: "水替我流经你的门前", trans: "And the water passes your door for me" },
  ],
};

// 其余歌曲的通用占位歌词：明确告诉观看者这是占位，接入 LyricApi 后替换
const PLACEHOLDER_LINES = [
  "（演示数据 · 歌词占位）",
  "接入 LyricApi 后将替换为逐行歌词",
  "歌词区会跟随播放进度自动滚动",
  "点击任意行可定位播放进度",
  "翻译行会以小字号挂在原文下方",
  "词：佚名 / 曲：佚名",
  "制作人：佚名",
  "晚安，做个好梦",
];

// —— 逐字时间轴工厂：mock 没有 QRC 源，按行内 token 确定性均摊行时长 ——
// 切分规则：汉字逐字成 token；拉丁字母/数字连续段成词；标点跟随前段；空白并入前段。
// 行末留 6% 呼吸间隙，让换行高亮不粘连。
function tokenizeLine(text: string): string[] {
  const tokens: string[] = [];
  const isWordChar = (char: string) => /[\p{L}\p{N}']/u.test(char);
  const isHan = (char: string) => /\p{Script=Han}/u.test(char);
  for (const char of Array.from(text)) {
    if (/\s/.test(char)) {
      const last = tokens[tokens.length - 1];
      if (last !== undefined && !last.endsWith(" ")) tokens[tokens.length - 1] = last + char;
      continue;
    }
    const last = tokens[tokens.length - 1];
    const lastChar = last === undefined ? "" : last[last.length - 1]!;
    if (last !== undefined && isHan(char)) {
      // 汉字逐字推进，扫色粒度到字
      tokens.push(char);
    } else if (last !== undefined && /[\p{P}\p{S}]/u.test(char)) {
      // 标点/符号贴住前段，不单独高亮
      tokens[tokens.length - 1] = last + char;
    } else if (last !== undefined && isWordChar(char) && isWordChar(lastChar) && !isHan(lastChar)) {
      tokens[tokens.length - 1] = last + char;
    } else {
      tokens.push(char);
    }
  }
  return tokens;
}

function buildWords(text: string, startMs: number, endMs: number): LyricWord[] {
  const tokens = tokenizeLine(text);
  if (tokens.length === 0) return [];
  const span = Math.max(0, endMs - startMs) * 0.94;
  const weights = tokens.map((token) => token.trimEnd().length);
  const total = weights.reduce((sum, weight) => sum + weight, 0);
  const words: LyricWord[] = [];
  let elapsed = 0;
  tokens.forEach((token, index) => {
    elapsed += weights[index]! / total;
    const end = Math.round(startMs + span * elapsed);
    words.push({
      text: token,
      startMs: words.length === 0 ? Math.round(startMs) : words[words.length - 1]!.endMs,
      endMs: Math.max(end, (words.length === 0 ? Math.round(startMs) : words[words.length - 1]!.endMs) + 1),
    });
  });
  return words;
}

/** 给一组歌词补逐字时间轴：行末 = 下一行起点（最后一行 +5s） */
function withWordTimings(lines: LyricLine[]): LyricLine[] {
  return lines.map((line, index) => ({
    ...line,
    words: buildWords(line.text, line.timeMs, lines[index + 1]?.timeMs ?? line.timeMs + 5_000),
  }));
}

export function lyricsOf(songMid: string): Lyrics | undefined {
  const song = songByMid.get(songMid);
  if (!song) return undefined;
  const full = FULL_LYRICS[songMid];
  if (full) return { mid: song.mid, title: song.title, lines: withWordTimings(full) };
  // 占位歌词 5–8 行，行数与起始时间都由 mid 确定
  const count = 5 + (hashSeed(`lyr:${songMid}`) % 4);
  const step = (song.durationMs - 16_000) / count;
  return {
    mid: song.mid,
    title: song.title,
    lines: withWordTimings(
      PLACEHOLDER_LINES.slice(0, count).map((text, index) => ({
        timeMs: Math.round(9_000 + step * index),
        text,
      })),
    ),
  };
}

// ————————————————————————————————————————————————————————————
// 查询辅助（供 client.ts 组装 api）
// ————————————————————————————————————————————————————————————

export function findSong(songMid: string): SongRef | undefined {
  return songByMid.get(songMid);
}

export function findAlbum(albumMid: string): AlbumDetail | undefined {
  return albumDetailByMid.get(albumMid) ?? singleAlbumByMid.get(albumMid);
}

// ————————————————————————————————————————————————————————————
// 音乐库（本地视角）：我喜欢 / 创建的歌单 / 收藏的歌单
// 后端账号体系未接线，全部由总池与歌单种子确定性派生；
// 接真实后端后这三个方法换成 daemon Favorite / 歌单收藏接口即可。
// ————————————————————————————————————————————————————————————

/** 我喜欢的歌：按 mid 哈希从总池挑出约三分之一，上限 18 首 */
export function likedSongs(): SongRef[] {
  const liked = songPool.filter((song) => hashSeed(`liked:${song.mid}`) % 3 === 0);
  return (liked.length > 0 ? liked : songPool.slice(0, 6)).slice(0, 18);
}

/** 创建的歌单：种子表前两单视为"我"创建 */
export function createdPlaylistRefs(): PlaylistRef[] {
  return PLAYLIST_SEEDS.filter((seed) => ["pl01", "pl02"].includes(seed.id)).map(playlistRefOf);
}

/** 收藏的歌单：其余歌单视为收藏 */
export function favoritedPlaylistRefs(): PlaylistRef[] {
  return PLAYLIST_SEEDS.filter((seed) => !["pl01", "pl02"].includes(seed.id)).map(playlistRefOf);
}

// ————————————————————————————————————————————————————————————
// 落盘音乐（本地音乐库 / 下载）与已购音乐
// 桌面端 daemon 未接线，文件维度字段由曲目确定性派生；
// 接真实后端后这三个方法换成 daemon 本地扫描 / 下载管理 / 订单接口即可。
// ————————————————————————————————————————————————————————————

/** 本地监视文件夹（路径为虚构演示数据） */
const WATCH_FOLDER_PATHS = ["~/Music/无损收藏", "~/Music/Live 现场", "~/Music/早期 Demo"];

/** 各文件夹的最近扫描时间（固定"今天"之前，mock 期写死保证确定性） */
const FOLDER_SCAN_TIMES = ["2026-09-05 21:30", "2026-08-30 14:12", "2026-08-11 09:45"];
/** 全库最近一次扫描 = 各文件夹中最新的一次 */
const LIBRARY_LAST_SCAN = "2026-09-05 21:30";
/** 下载文件的统一存储目录 */
const DOWNLOAD_STORAGE_PATH = "~/Music/胡桃音乐";

/** 由音质文案推落盘格式与码率（kbps）：Hi-Res/FLAC → FLAC，其余按 320kbps MP3 */
function formatAndBitrateOf(song: SongRef): { format: string; kbps: number } {
  if (song.quality === QUALITY_HI_RES) {
    // Hi-Res 96kHz/24bit 码率落 in 2100–2400 kbps
    return { format: "FLAC", kbps: 2100 + (hashSeed(`bitrate:${song.mid}`) % 300) };
  }
  if (song.quality === QUALITY_FLAC) {
    // 无损 44.1kHz/16bit 码率落 in 850–1000 kbps
    return { format: "FLAC", kbps: 850 + (hashSeed(`bitrate:${song.mid}`) % 150) };
  }
  return { format: "MP3", kbps: 320 };
}

/** 文件大小 = 码率 × 时长，字节 */
function sizeBytesOf(song: SongRef): number {
  const { kbps } = formatAndBitrateOf(song);
  return Math.round((kbps * 1000 * song.durationMs) / 8);
}

/** 把 SongRef 落成 LocalTrack：文件夹由 mid 哈希在监视文件夹中指派 */
function toLocalTrack(song: SongRef, folder: string): LocalTrack {
  return {
    ...song,
    sizeBytes: sizeBytesOf(song),
    format: formatAndBitrateOf(song).format,
    folder,
  };
}

/**
 * 本地音乐库：总池按 mid 哈希取三分之二（余数 0/1），指派到三个监视文件夹。
 * 与下载内容按同一哈希的余数互斥（下载取余数 2），同一首不会两边重复。
 */
export function localLibrary(): LocalLibrary {
  const tracks = songPool
    .filter((song) => hashSeed(`local:${song.mid}`) % 3 !== 2)
    .map((song) => {
      const folder = WATCH_FOLDER_PATHS[hashSeed(`localfolder:${song.mid}`) % WATCH_FOLDER_PATHS.length];
      return toLocalTrack(song, folder);
    });

  const folders = WATCH_FOLDER_PATHS.map((path, i) => {
    const inFolder = tracks.filter((track) => track.folder === path);
    return {
      path,
      trackCount: inFolder.length,
      sizeBytes: inFolder.reduce((sum, track) => sum + track.sizeBytes, 0),
      lastScanAt: FOLDER_SCAN_TIMES[i],
    };
  }).filter((folder) => folder.trackCount > 0);

  return { tracks, folders, lastScanAt: LIBRARY_LAST_SCAN };
}

/** 下载内容：与本地库共用同一哈希、取余数 2（互斥），共用存储目录 */
export function downloadLibrary(): DownloadLibrary {
  const tracks = songPool
    .filter((song) => hashSeed(`local:${song.mid}`) % 3 === 2)
    .map((song) => toLocalTrack(song, DOWNLOAD_STORAGE_PATH));
  return { tracks, storagePath: DOWNLOAD_STORAGE_PATH };
}

/** 购买日期：2026 年 1–8 月内按哈希取日（确定性，无 Date.now） */
function purchasedDateOf(seed: string): string {
  const hash = hashSeed(`bought:${seed}`);
  const month = String(1 + (hash % 8)).padStart(2, "0");
  const day = String(1 + (hashSeed(`boughtday:${seed}`) % 28)).padStart(2, "0");
  return `2026-${month}-${day}`;
}

/** 单曲实付：¥2 / ¥3 两档（哈希取舍） */
function singlePriceFen(songMid: string): number {
  return hashSeed(`price:${songMid}`) % 3 === 0 ? 300 : 200;
}

/** 已购单曲：总池按 mid 哈希挑选、上限 9 首，按购买日期倒序 */
export function purchasedMusic(): PurchasedMusic {
  const singles = songPool
    .filter((song) => hashSeed(`bought:${song.mid}`) % 7 === 3)
    .slice(0, 9)
    .map((song) => ({ song, purchasedAt: purchasedDateOf(song.mid), priceFen: singlePriceFen(song.mid) }))
    .sort((a, b) => b.purchasedAt.localeCompare(a.purchasedAt));

  const albums = curatedAlbums
    .filter((album) => hashSeed(`bought:${album.mid}`) % 5 === 1)
    .slice(0, 4)
    .map((album) => ({
      album,
      purchasedAt: purchasedDateOf(`album:${album.mid}`),
      // 专辑按曲目数计价：每首 ¥2，符合单曲定价的倍数直觉
      priceFen: album.songs.length * 200,
    }))
    .sort((a, b) => b.purchasedAt.localeCompare(a.purchasedAt));

  return { singles, albums };
}
