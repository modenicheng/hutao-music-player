//! 确定性 mock 数据（移植自 apps/hmp-tauri/src/lib/api/mock-data.ts 的种子表与工厂）。
//!
//! 全部内容为虚构：艺人 / 歌曲 / 厂牌 / 用户均为演示创作。组织方式与 TS 版一致：
//! 先声明"种子表"（原始事实），再由工厂函数编译成领域对象——专辑曲目反查歌手、
//! 库内容从总池派生，引用关系天然一致。确定性：伪随机一律走
//! [`crate::covers::hash_seed`]（FNV-1a，与 TS 版逐字节一致，见 covers 测试）。
//!
//! M4 将补齐：数字单曲池、榜单、推荐、评论、逐字歌词（对应 TS 版同段逻辑）。

use crate::covers::{cover_image, hash_seed};

const QUALITY_FLAC: &str = "FLAC · 44.1kHz";
const QUALITY_HI_RES: &str = "Hi-Res · 96kHz/24bit";
const QUALITY_MP3: &str = "320kbps MP3";

// ————————————————————————————————————————————————————————————
// 种子表
// ————————————————————————————————————————————————————————————

pub struct ArtistSeed {
    pub mid: &'static str,
    pub name: &'static str,
    pub desc: &'static str,
    pub mv_count: u32,
    pub hot_song_target: u32,
    pub similar: &'static [&'static str],
}

pub const ARTIST_SEEDS: &[ArtistSeed] = &[
    ArtistSeed {
        mid: "ar01",
        name: "白栖遥",
        desc: "白栖遥，流行歌手兼词曲作者，声线干净克制，擅长把日常细节写进歌里。2024 年发行创作专辑《潮汐来信》，并为剧集《拾光纪》献唱主题曲，被乐迷称为\"写信人\"。",
        mv_count: 3,
        hot_song_target: 16,
        similar: &["ar12", "ar13", "ar05", "ar07"],
    },
    ArtistSeed {
        mid: "ar02",
        name: "陈屿帆",
        desc: "陈屿帆，来自南方小城的民谣唱作人。歌里总有火车、站台和夏天的雨，被乐迷称作\"站台诗人\"。专辑《南方站台》在旧文化馆同期录制，保留了大量现场呼吸声。",
        mv_count: 2,
        hot_song_target: 15,
        similar: &["ar07", "ar14", "ar05"],
    },
    ArtistSeed {
        mid: "ar03",
        name: "雾岛电台",
        desc: "雾岛电台，三人电子乐队，成立于海雾常年不散的雾岛。他们把海雾、轮渡汽笛与灯塔白噪音采样进合成器，做出\"会呼吸\"的电子乐。",
        mv_count: 2,
        hot_song_target: 14,
        similar: &["ar08", "ar11", "ar06"],
    },
    ArtistSeed {
        mid: "ar04",
        name: "破晓列车",
        desc: "破晓列车，五人摇滚乐队，歌词关注都市夜归人。首专《午夜快线》把失真吉他与合成器并到同一条轨道上，速度拉满。",
        mv_count: 3,
        hot_song_target: 12,
        similar: &["ar09", "ar03", "ar13"],
    },
    ArtistSeed {
        mid: "ar05",
        name: "林晚风",
        desc: "林晚风，流行男歌手，声音像夏天傍晚的风。首张个人专辑《橘子汽水与晚风》写尽了夏天的小事。",
        mv_count: 2,
        hot_song_target: 12,
        similar: &["ar01", "ar13", "ar14"],
    },
    ArtistSeed {
        mid: "ar06",
        name: "顾清商",
        desc: "顾清商，钢琴演奏者，专攻印象派与东方意象的融合。独奏专辑《月白》采用单点收音，连踏板与琴体的共鸣都一并收录。",
        mv_count: 1,
        hot_song_target: 12,
        similar: &["ar10", "ar14", "ar03"],
    },
    ArtistSeed {
        mid: "ar07",
        name: "苏折枝",
        desc: "苏折枝，民谣女声，唱词古雅，善用意象。编曲崇尚克制，常宣称\"一把吉他就够\"。",
        mv_count: 1,
        hot_song_target: 12,
        similar: &["ar02", "ar14", "ar01"],
    },
    ArtistSeed {
        mid: "ar08",
        name: "星尘信号",
        desc: "星尘信号，电子音乐制作人，创作主题多为深空、失眠与便利店。建议把他的专辑当作一次不需要返程票的漫游。",
        mv_count: 2,
        hot_song_target: 12,
        similar: &["ar03", "ar11", "ar06"],
    },
    ArtistSeed {
        mid: "ar09",
        name: "未眠者乐队",
        desc: "未眠者乐队，独立摇滚乐队，主唱的假声是他们的标志。首张全长《仲夏航行》献给所有睡不着的年轻人。",
        mv_count: 2,
        hot_song_target: 12,
        similar: &["ar04", "ar03", "ar13"],
    },
    ArtistSeed {
        mid: "ar10",
        name: "温叙",
        desc: "温叙，影视配乐作者，习惯用一台旧钢琴采样讲故事。剧集《拾光纪》原声带是他目前体量最大的作品。",
        mv_count: 1,
        hot_song_target: 12,
        similar: &["ar06", "ar11", "ar03"],
    },
    ArtistSeed {
        mid: "ar11",
        name: "洛书",
        desc: "洛书，国风电子制作人，把志怪小说与合成器塞进同一首歌。常与 ACG 歌手合作，同名主打《山海入梦》由岸芷汀兰献声，动画 PV 播放量破千万。",
        mv_count: 3,
        hot_song_target: 16,
        similar: &["ar03", "ar08", "ar12"],
    },
    ArtistSeed {
        mid: "ar12",
        name: "岸芷汀兰",
        desc: "岸芷汀兰，ACG 歌手，为多部动画演唱主题曲，音色甜而不腻。代表作《汀兰谣》为动画《兰汀物语》片头曲。",
        mv_count: 4,
        hot_song_target: 12,
        similar: &["ar01", "ar11", "ar07"],
    },
    ArtistSeed {
        mid: "ar13",
        name: "姜聿",
        desc: "姜聿，R&B 唱作人，节奏松弛，旋律黏人。专辑《夜航星》适合城市晚高峰与深夜环路。",
        mv_count: 2,
        hot_song_target: 12,
        similar: &["ar01", "ar05", "ar09"],
    },
    ArtistSeed {
        mid: "ar14",
        name: "南栀",
        desc: "南栀，轻音乐唱作人，适合睡前与雨天循环。她说自己的歌\"写给所有慢半拍的人\"。",
        mv_count: 1,
        hot_song_target: 12,
        similar: &["ar07", "ar02", "ar06"],
    },
];

pub struct SongSeed {
    pub mid: &'static str,
    pub title: &'static str,
    pub duration_sec: u32,
    pub quality: Option<&'static str>,
    pub featuring: &'static [&'static str],
}

pub struct AlbumSeed {
    pub mid: &'static str,
    pub name: &'static str,
    pub artist_mid: &'static str,
    pub genre: &'static str,
    pub company: &'static str,
    pub release_date: &'static str,
    pub desc: &'static str,
    pub fav_count: u32,
    pub songs: &'static [SongSeed],
}

macro_rules! song {
    ($mid:literal, $title:literal, $dur:literal) => {
        SongSeed { mid: $mid, title: $title, duration_sec: $dur, quality: None, featuring: &[] }
    };
    ($mid:literal, $title:literal, $dur:literal, $q:expr) => {
        SongSeed { mid: $mid, title: $title, duration_sec: $dur, quality: Some($q), featuring: &[] }
    };
    ($mid:literal, $title:literal, $dur:literal, $q:expr, [$($f:literal),+]) => {
        SongSeed { mid: $mid, title: $title, duration_sec: $dur, quality: Some($q), featuring: &[$($f),+] }
    };
}

pub const ALBUM_SEEDS: &[AlbumSeed] = &[
    AlbumSeed {
        mid: "al01",
        name: "潮汐来信",
        artist_mid: "ar01",
        genre: "流行",
        company: "白日梦研究所",
        release_date: "2024-05-20",
        fav_count: 128_400,
        desc: "白栖遥的第二张创作专辑。四首歌像四封未寄出的信，写海、写晚风、写站台，也写每个欲言又止的瞬间。整张专辑在海岸边的录音棚完成，你能在曲目间隙里听到真实的潮声。",
        songs: &[
            song!("so001", "潮汐来信", 252, QUALITY_FLAC),
            song!("so002", "玻璃海", 238),
            song!("so003", "借一场晚风", 271, QUALITY_HI_RES),
            song!("so004", "无人站台", 227, QUALITY_FLAC),
        ],
    },
    AlbumSeed {
        mid: "al02",
        name: "雾中灯塔",
        artist_mid: "ar03",
        genre: "电子",
        company: "雾岛独立厂牌",
        release_date: "2023-11-02",
        fav_count: 86_200,
        desc: "雾岛电台首张录音室专辑。乐队把海雾、轮渡汽笛与灯塔白噪音采样进合成器，录成了一张\"会在夜里发光\"的电子专辑。",
        songs: &[
            song!("so005", "雾中灯塔", 302, QUALITY_FLAC),
            song!("so006", "频率 8.3", 255, QUALITY_MP3, ["ar11"]),
            song!("so007", "夜间飞行", 284),
        ],
    },
    AlbumSeed {
        mid: "al03",
        name: "南方站台",
        artist_mid: "ar02",
        genre: "民谣",
        company: "南方铁路唱片",
        release_date: "2022-09-14",
        fav_count: 64_800,
        desc: "陈屿帆的民谣专辑，写小城、铁路与回不去的夏天。全专在南方小城的旧文化馆里同期录制，保留了大量现场呼吸声。",
        songs: &[
            song!("so008", "南方站台", 266, QUALITY_FLAC),
            song!("so009", "绿皮火车", 243),
            song!("so010", "巷口的猫", 221, QUALITY_FLAC),
            song!("so011", "一封家书", 292, QUALITY_MP3),
        ],
    },
    AlbumSeed {
        mid: "al04",
        name: "午夜快线",
        artist_mid: "ar04",
        genre: "摇滚",
        company: "破晓文化",
        release_date: "2023-06-18",
        fav_count: 45_600,
        desc: "破晓列车的摇滚专辑，献给每一个末班车上的夜归人。失真吉他与合成器在午夜并线，速度拉满。",
        songs: &[
            song!("so012", "午夜快线", 232, QUALITY_FLAC),
            song!("so013", "霓虹休克", 248),
            song!("so014", "逆风奔跑", 216, QUALITY_MP3),
        ],
    },
    AlbumSeed {
        mid: "al05",
        name: "月白",
        artist_mid: "ar06",
        genre: "古典",
        company: "月白古典社",
        release_date: "2021-03-26",
        fav_count: 32_100,
        desc: "顾清商的钢琴独奏专辑，取\"月白\"为色，收录三段关于夜色的即兴与练习。录音采用单点收音，保留踏板与琴体共鸣。",
        songs: &[
            song!("so015", "月白 · 前奏曲", 213, QUALITY_FLAC),
            song!("so016", "雨打芭蕉即兴曲", 310, QUALITY_HI_RES),
            song!("so017", "晨光练习曲", 245),
        ],
    },
    AlbumSeed {
        mid: "al06",
        name: "拾光纪",
        artist_mid: "ar10",
        genre: "影视原声",
        company: "拾光影业",
        release_date: "2024-01-08",
        fav_count: 96_700,
        desc: "同名剧集原声带，由温叙操刀。以一台一九八零年代的旧钢琴为主轴，串联起剧中三代人的午后与车站。",
        songs: &[
            song!("so018", "拾光", 258, QUALITY_FLAC, ["ar01"]),
            song!("so019", "老照片", 209, QUALITY_HI_RES),
            song!("so020", "车站别离", 280, QUALITY_MP3),
            song!("so021", "尾声 · 致每个午后", 195),
        ],
    },
    AlbumSeed {
        mid: "al07",
        name: "星尘漫游指南",
        artist_mid: "ar08",
        genre: "电子",
        company: "星尘电子",
        release_date: "2024-08-30",
        fav_count: 51_300,
        desc: "星尘信号的电子专辑，主题是深空、失眠与便利店。请把它当作一次不需要返程票的漫游。",
        songs: &[
            song!("so022", "星尘漫游指南", 295, QUALITY_FLAC),
            song!("so023", "引力失效", 252, QUALITY_HI_RES),
            song!("so024", "深空便利店", 238),
        ],
    },
    AlbumSeed {
        mid: "al08",
        name: "折枝辞",
        artist_mid: "ar07",
        genre: "民谣",
        company: "折枝民谣社",
        release_date: "2023-03-21",
        fav_count: 28_900,
        desc: "苏折枝的民谣专辑，词曲古雅，写渡口、春信与折枝。编曲克制，一把吉他就够。",
        songs: &[
            song!("so025", "折枝辞", 261, QUALITY_FLAC),
            song!("so026", "春分信", 234),
            song!("so027", "渡口", 276, QUALITY_MP3),
        ],
    },
    AlbumSeed {
        mid: "al09",
        name: "仲夏航行",
        artist_mid: "ar09",
        genre: "摇滚",
        company: "未眠者自制",
        release_date: "2022-12-09",
        fav_count: 38_400,
        desc: "未眠者乐队首张全长专辑。仲夏夜的水面、失眠电台与蝉鸣鼓点，献给所有睡不着的年轻人。",
        songs: &[
            song!("so028", "仲夏航行", 269, QUALITY_FLAC),
            song!("so029", "失眠电台", 227, QUALITY_HI_RES),
            song!("so030", "鼓点与蝉鸣", 238),
        ],
    },
    AlbumSeed {
        mid: "al10",
        name: "山海入梦",
        artist_mid: "ar11",
        genre: "国风电子",
        company: "山海国风",
        release_date: "2024-10-17",
        fav_count: 105_600,
        desc: "洛书的国风电子专辑，把志怪故事搬进合成器：山神的雾、灯下的辞、观潮的人。岸芷汀兰献声同名主打。",
        songs: &[
            song!("so031", "山海入梦", 273, QUALITY_FLAC, ["ar12"]),
            song!("so032", "灯下辞", 229, QUALITY_HI_RES),
            song!("so033", "观潮", 259),
        ],
    },
    AlbumSeed {
        mid: "al11",
        name: "橘子汽水与晚风",
        artist_mid: "ar05",
        genre: "流行",
        company: "橘子汽水工作室",
        release_date: "2023-07-22",
        fav_count: 72_500,
        desc: "林晚风的首张个人专辑，写夏天的一切小事：汽水、晚风、信箱和一场没赶上的雨。",
        songs: &[
            song!("so034", "橘子汽水", 211, QUALITY_MP3, ["ar14"]),
            song!("so035", "晚风信箱", 242),
            song!("so036", "夏至未至的雨", 224),
        ],
    },
    AlbumSeed {
        mid: "al12",
        name: "汀兰谣",
        artist_mid: "ar12",
        genre: "ACG",
        company: "兰汀文化",
        release_date: "2024-04-04",
        fav_count: 88_900,
        desc: "动画《兰汀物语》原声专辑，岸芷汀兰演唱。收录片头曲《汀兰谣》及剧中插曲。",
        songs: &[
            song!("so037", "汀兰谣", 254, QUALITY_FLAC, ["ar07"]),
            song!("so038", "星海航路", 236),
            song!("so039", "萤火之径", 247, QUALITY_MP3),
        ],
    },
    AlbumSeed {
        mid: "al13",
        name: "夜航星",
        artist_mid: "ar13",
        genre: "R&B",
        company: "夜航音乐",
        release_date: "2024-09-12",
        fav_count: 42_700,
        desc: "姜聿的 R&B 专辑，节奏像夜航一样松弛。适合城市晚高峰与深夜环路。",
        songs: &[
            song!("so040", "夜航星", 262, QUALITY_FLAC),
            song!("so041", "三点水的甜", 218, QUALITY_HI_RES),
            song!("so042", "慢速心动", 251),
        ],
    },
    AlbumSeed {
        mid: "al14",
        name: "慢半拍",
        artist_mid: "ar14",
        genre: "轻音",
        company: "慢半拍工作室",
        release_date: "2023-10-26",
        fav_count: 30_200,
        desc: "南栀的轻音专辑，写给所有慢半拍的人。不着急，好的都值得等。",
        songs: &[
            song!("so043", "慢半拍", 245, QUALITY_FLAC),
            song!("so044", "云朵商店", 222, QUALITY_HI_RES),
            song!("so045", "十点半的月光", 266, QUALITY_MP3),
        ],
    },
];

pub struct PlaylistSeed {
    pub id: &'static str,
    pub name: &'static str,
    pub creator: &'static str,
    pub tags: &'static [&'static str],
    pub play_count: u64,
    pub song_mids: &'static [&'static str],
    pub desc: &'static str,
}

pub const PLAYLIST_SEEDS: &[PlaylistSeed] = &[
    PlaylistSeed {
        id: "pl01",
        name: "深夜写代码 BGM 指南",
        creator: "代码写不完",
        tags: &["学习", "电子", "专注"],
        play_count: 1_284_567,
        song_mids: &["so005", "so006", "so007", "so022", "so023", "so024", "so031", "so032", "so033", "so043", "so044", "so040", "so015"],
        desc: "写给所有凌晨还亮着的屏幕。电子、国风电子和轻音混着来，人声不多，够安静也够带感。改完最后一个 bug 之前，别关掉它。",
    },
    PlaylistSeed {
        id: "pl02",
        name: "南方、站台与绿皮火车",
        creator: "拾荒的诗人",
        tags: &["民谣", "旅行", "治愈"],
        play_count: 862_104,
        song_mids: &["so008", "so009", "so010", "so011", "so025", "so026", "so027", "so035", "so036", "so043", "so045", "so001", "so018", "so004"],
        desc: "从绿皮火车到高铁，站台一直是民谣的故乡。这些歌唱的都是离开与抵达——愿你也有一个值得回去的小城。",
    },
    PlaylistSeed {
        id: "pl03",
        name: "兰汀物语 · 二次元浓度超标",
        creator: "Momo不在家",
        tags: &["ACG", "动漫", "电子"],
        play_count: 2_140_882,
        song_mids: &["so037", "so038", "so039", "so031", "so033", "so005", "so007", "so022", "so024", "so001", "so040", "so042", "so044"],
        desc: "为动画《兰汀物语》整季整理的原声向歌单，从片头曲一路听到插曲。二次元浓度超标预警，次元壁脆弱者请系好安全带。",
    },
    PlaylistSeed {
        id: "pl04",
        name: "雨夜轻钢琴与慢歌",
        creator: "风住过的街道",
        tags: &["轻音", "钢琴", "睡前"],
        play_count: 654_310,
        song_mids: &["so015", "so016", "so017", "so019", "so020", "so021", "so043", "so044", "so045", "so026", "so011", "so042"],
        desc: "雨声是最好的编曲，钢琴是最慢的述说。睡前音量食用更佳，愿你好梦。",
    },
];

// ————————————————————————————————————————————————————————————
// 领域对象
// ————————————————————————————————————————————————————————————

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistRef {
    pub mid: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlbumRef {
    pub mid: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct SongRef {
    pub mid: String,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album: AlbumRef,
    pub duration_ms: u64,
    pub quality: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AlbumDetail {
    pub mid: String,
    pub name: String,
    pub artist: ArtistRef,
    pub company: String,
    pub release_date: String,
    pub desc: String,
    pub fav_count: u64,
    pub songs: Vec<SongRef>,
}

#[derive(Clone, Debug)]
pub struct PlaylistRef {
    pub id: String,
    pub name: String,
    pub play_count: u64,
}

fn artist_ref_of(mid: &str) -> ArtistRef {
    let seed = ARTIST_SEEDS
        .iter()
        .find(|seed| seed.mid == mid)
        .unwrap_or_else(|| panic!("unknown artist seed: {mid}"));
    ArtistRef {
        mid: seed.mid.into(),
        name: seed.name.into(),
    }
}

/// 完整专辑（14 张，每张 3–4 首）编译为领域对象
pub fn curated_albums() -> Vec<AlbumDetail> {
    ALBUM_SEEDS
        .iter()
        .map(|seed| {
            let artist = artist_ref_of(seed.artist_mid);
            AlbumDetail {
                mid: seed.mid.into(),
                name: seed.name.into(),
                artist: artist.clone(),
                company: seed.company.into(),
                release_date: seed.release_date.into(),
                desc: seed.desc.into(),
                fav_count: u64::from(seed.fav_count),
                songs: seed
                    .songs
                    .iter()
                    .map(|song| SongRef {
                        mid: song.mid.into(),
                        title: song.title.into(),
                        artists: std::iter::once(artist.clone())
                            .chain(song.featuring.iter().map(|f| artist_ref_of(f)))
                            .collect(),
                        album: AlbumRef {
                            mid: seed.mid.into(),
                            name: seed.name.into(),
                        },
                        duration_ms: u64::from(song.duration_sec) * 1000,
                        quality: song.quality.map(Into::into),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// 总歌曲池：榜单 / 推荐 / 搜索 / 库都从这里取，保证引用一致
pub fn song_pool() -> Vec<SongRef> {
    curated_albums().into_iter().flat_map(|album| album.songs).collect()
}

pub fn find_song<'a>(pool: &'a [SongRef], song_mid: &str) -> Option<&'a SongRef> {
    pool.iter().find(|song| song.mid == song_mid)
}

fn artists_text(song: &SongRef) -> String {
    song.artists
        .iter()
        .map(|artist| artist.name.as_str())
        .collect::<Vec<_>>()
        .join(" / ")
}

// ————————————————————————————————————————————————————————————
// 音乐库（本地视角）：我喜欢 / 创建的歌单 / 收藏的歌单
// 后端账号体系未接线，全部由总池与歌单种子确定性派生。
// ————————————————————————————————————————————————————————————

/// 我喜欢的歌：按 mid 哈希从总池挑出约三分之一，上限 18 首
pub fn liked_songs(pool: &[SongRef]) -> Vec<SongRef> {
    let liked: Vec<SongRef> = pool
        .iter()
        .filter(|song| hash_seed(&format!("liked:{}", song.mid)) % 3 == 0)
        .cloned()
        .collect();
    let source = if liked.is_empty() { &pool[..pool.len().min(6)] } else { &liked[..] };
    source.iter().take(18).cloned().collect()
}

/// 创建的歌单：种子表前两单视为"我"创建
pub fn created_playlist_refs() -> Vec<PlaylistRef> {
    PLAYLIST_SEEDS
        .iter()
        .filter(|seed| seed.id == "pl01" || seed.id == "pl02")
        .map(|seed| PlaylistRef {
            id: seed.id.into(),
            name: seed.name.into(),
            play_count: seed.play_count,
        })
        .collect()
}

/// 收藏的歌单：其余歌单视为收藏
pub fn favorited_playlist_refs() -> Vec<PlaylistRef> {
    PLAYLIST_SEEDS
        .iter()
        .filter(|seed| seed.id != "pl01" && seed.id != "pl02")
        .map(|seed| PlaylistRef {
            id: seed.id.into(),
            name: seed.name.into(),
            play_count: seed.play_count,
        })
        .collect()
}

// ————————————————————————————————————————————————————————————
// 落盘音乐（本地音乐库 / 下载）与已购音乐
// 桌面端 daemon 未接线，文件维度字段由曲目确定性派生。
// ————————————————————————————————————————————————————————————

/// 本地监视文件夹（路径为虚构演示数据）
pub const WATCH_FOLDER_PATHS: &[&str] = &["~/Music/无损收藏", "~/Music/Live 现场", "~/Music/早期 Demo"];
/// 各文件夹的最近扫描时间（固定"今天"之前，mock 期写死保证确定性）
const FOLDER_SCAN_TIMES: &[&str] = &["2026-09-05 21:30", "2026-08-30 14:12", "2026-08-11 09:45"];
/// 全库最近一次扫描 = 各文件夹中最新的一次
const LIBRARY_LAST_SCAN: &str = "2026-09-05 21:30";
/// 下载文件的统一存储目录
pub const DOWNLOAD_STORAGE_PATH: &str = "~/Music/胡桃音乐";

#[derive(Clone, Debug)]
pub struct LocalTrack {
    pub song: SongRef,
    pub size_bytes: u64,
    pub format: &'static str,
    pub folder: String,
}

#[derive(Clone, Debug)]
pub struct WatchedFolder {
    pub path: String,
    pub track_count: usize,
    pub size_bytes: u64,
    pub last_scan_at: String,
}

#[derive(Clone, Debug)]
pub struct LocalLibrary {
    pub tracks: Vec<LocalTrack>,
    pub folders: Vec<WatchedFolder>,
    pub last_scan_at: String,
}

#[derive(Clone, Debug)]
pub struct DownloadLibrary {
    pub tracks: Vec<LocalTrack>,
    pub storage_path: String,
}

/// 由音质文案推落盘格式与码率（kbps）：Hi-Res/FLAC → FLAC，其余按 320kbps MP3
fn bitrate_of(song: &SongRef) -> (&'static str, u32) {
    match song.quality.as_deref() {
        Some(QUALITY_HI_RES) => ("FLAC", 2100 + (hash_seed(&format!("bitrate:{}", song.mid)) % 300)),
        Some(QUALITY_FLAC) => ("FLAC", 850 + (hash_seed(&format!("bitrate:{}", song.mid)) % 150)),
        _ => ("MP3", 320),
    }
}

/// 文件大小 = 码率 × 时长，字节。
/// 注意：TS 版 sizeBytesOf 写作 `(kbps * 1000 * durationMs) / 8`，量纲少除 1000
/// （单曲显示成 ~26 GB）；这里按正确物理量纲 `kbps × durationMs / 8`（kbit/8 = KB）。
fn size_bytes_of(song: &SongRef) -> u64 {
    let (_, kbps) = bitrate_of(song);
    (u64::from(kbps) * song.duration_ms / 8).max(1)
}

fn to_local_track(song: &SongRef, folder: &str) -> LocalTrack {
    let (format, _) = bitrate_of(song);
    LocalTrack {
        song: song.clone(),
        size_bytes: size_bytes_of(song),
        format,
        folder: folder.into(),
    }
}

/// 本地音乐库：总池按 mid 哈希取三分之二（余数 0/1），指派到三个监视文件夹。
/// 与下载内容按同一哈希的余数互斥（下载取余数 2），同一首不会两边重复。
pub fn local_library(pool: &[SongRef]) -> LocalLibrary {
    let tracks: Vec<LocalTrack> = pool
        .iter()
        .filter(|song| hash_seed(&format!("local:{}", song.mid)) % 3 != 2)
        .map(|song| {
            let folder = WATCH_FOLDER_PATHS[(hash_seed(&format!("localfolder:{}", song.mid)) % 3) as usize];
            to_local_track(song, folder)
        })
        .collect();

    let folders = WATCH_FOLDER_PATHS
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let in_folder: Vec<&LocalTrack> = tracks.iter().filter(|track| &track.folder == path).collect();
            WatchedFolder {
                path: (*path).into(),
                track_count: in_folder.len(),
                size_bytes: in_folder.iter().map(|track| track.size_bytes).sum(),
                last_scan_at: FOLDER_SCAN_TIMES[i].into(),
            }
        })
        .filter(|folder| folder.track_count > 0)
        .collect();

    LocalLibrary {
        tracks,
        folders,
        last_scan_at: LIBRARY_LAST_SCAN.into(),
    }
}

/// 下载内容：与本地库共用同一哈希、取余数 2（互斥），共用存储目录
pub fn download_library(pool: &[SongRef]) -> DownloadLibrary {
    let tracks = pool
        .iter()
        .filter(|song| hash_seed(&format!("local:{}", song.mid)) % 3 == 2)
        .map(|song| to_local_track(song, DOWNLOAD_STORAGE_PATH))
        .collect();
    DownloadLibrary {
        tracks,
        storage_path: DOWNLOAD_STORAGE_PATH.into(),
    }
}

#[derive(Clone, Debug)]
pub struct PurchasedSingle {
    pub song: SongRef,
    pub purchased_at: String,
    pub price_fen: u64,
}

#[derive(Clone, Debug)]
pub struct PurchasedAlbum {
    pub album: AlbumDetail,
    pub purchased_at: String,
    pub price_fen: u64,
}

#[derive(Clone, Debug)]
pub struct PurchasedMusic {
    pub singles: Vec<PurchasedSingle>,
    pub albums: Vec<PurchasedAlbum>,
}

/// 购买日期：2026 年 1–8 月内按哈希取日（确定性，无时间源）
fn purchased_date_of(seed: &str) -> String {
    let hash = hash_seed(&format!("bought:{seed}"));
    let month = 1 + (hash % 8);
    let day = 1 + (hash_seed(&format!("boughtday:{seed}")) % 28);
    format!("2026-{month:02}-{day:02}")
}

/// 单曲实付：¥2 / ¥3 两档（哈希取舍）
fn single_price_fen(song_mid: &str) -> u64 {
    if hash_seed(&format!("price:{song_mid}")) % 3 == 0 {
        300
    } else {
        200
    }
}

/// 已购单曲：总池按 mid 哈希挑选、上限 9 首，按购买日期倒序
pub fn purchased_music(pool: &[SongRef], albums: &[AlbumDetail]) -> PurchasedMusic {
    let mut singles: Vec<PurchasedSingle> = pool
        .iter()
        .filter(|song| hash_seed(&format!("bought:{}", song.mid)) % 7 == 3)
        .take(9)
        .map(|song| PurchasedSingle {
            song: song.clone(),
            purchased_at: purchased_date_of(&song.mid),
            price_fen: single_price_fen(&song.mid),
        })
        .collect();
    singles.sort_by(|a, b| b.purchased_at.cmp(&a.purchased_at));

    let mut purchased_albums: Vec<PurchasedAlbum> = albums
        .iter()
        .filter(|album| hash_seed(&format!("bought:{}", album.mid)) % 5 == 1)
        .take(4)
        .map(|album| PurchasedAlbum {
            album: album.clone(),
            purchased_at: purchased_date_of(&format!("album:{}", album.mid)),
            // 专辑按曲目数计价：每首 ¥2，符合单曲定价的倍数直觉
            price_fen: u64::try_from(album.songs.len()).unwrap_or(0) * 200,
        })
        .collect();
    purchased_albums.sort_by(|a, b| b.purchased_at.cmp(&a.purchased_at));

    PurchasedMusic {
        singles,
        albums: purchased_albums,
    }
}

// ————————————————————————————————————————————————————————————
// 最近播放（RecentView.vue 同源逻辑：总池前 12 首 + 哈希派生距今天数）
// ————————————————————————————————————————————————————————————

pub struct RecentRecord {
    pub song: SongRef,
    /// 相对时间文案："今天" / "昨天" / "N 天前" / "N 周前"
    pub label: String,
}

fn relative_label(days_ago: u32) -> String {
    match days_ago {
        0 => "今天".into(),
        1 => "昨天".into(),
        d if d < 14 => format!("{d} 天前"),
        d => format!("{} 周前", (d as f64 / 7.0).round() as u32),
    }
}

pub fn recent_records(pool: &[SongRef], size: usize) -> Vec<RecentRecord> {
    let mut acc: u32 = 0;
    pool.iter()
        .take(size)
        .enumerate()
        .map(|(index, song)| {
            if index > 0 {
                acc += 1 + (hash_seed(&format!("recent:{}", song.mid)) % 2);
            }
            RecentRecord {
                song: song.clone(),
                label: relative_label(acc),
            }
        })
        .collect()
}

// ————————————————————————————————————————————————————————————
// UI 模型构建（slint struct 转换）
// ————————————————————————————————————————————————————————————

use crate::TrackRow;

/// SongRef → TrackRow（封面由专辑 seed 确定性生成）
pub fn to_track_row(song: &SongRef) -> TrackRow {
    let main_artist = song.artists.first();
    TrackRow {
        mid: song.mid.clone().into(),
        title: song.title.clone().into(),
        artists: artists_text(song).into(),
        artist_mid: main_artist.map(|a| a.mid.clone()).unwrap_or_default().into(),
        album: song.album.name.clone().into(),
        album_mid: song.album.mid.clone().into(),
        duration_ms: song.duration_ms as i32,
        quality: song.quality.clone().unwrap_or_default().into(),
        cover: cover_image(&format!("album:{}", song.album.mid)),
    }
}

pub fn track_rows(songs: &[SongRef]) -> Vec<TrackRow> {
    songs.iter().map(to_track_row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::covers::hash_seed;

    #[test]
    fn pool_is_complete_and_referential() {
        let pool = song_pool();
        assert_eq!(pool.len(), 45);
        // 所有专辑曲目的歌手都指向存在的种子
        assert_eq!(artist_ref_of("ar01").name, "白栖遥");
        assert_eq!(find_song(&pool, "so045").unwrap().title, "十点半的月光");
        assert!(find_song(&pool, "so999").is_none());
    }

    #[test]
    fn liked_is_deterministic_third() {
        let pool = song_pool();
        let liked = liked_songs(&pool);
        let expected: Vec<&SongRef> = pool
            .iter()
            .filter(|s| hash_seed(&format!("liked:{}", s.mid)) % 3 == 0)
            .take(18)
            .collect();
        assert_eq!(liked.len(), expected.len());
        assert_eq!(liked[0].mid, expected[0].mid);
        // 全部条目都满足 liked 谓词（哈希余数 0）
        assert!(liked
            .iter()
            .all(|song| hash_seed(&format!("liked:{}", song.mid)) % 3 == 0));
    }

    #[test]
    fn local_and_download_are_disjoint() {
        let pool = song_pool();
        let local = local_library(&pool);
        let downloads = download_library(&pool);
        for track in &downloads.tracks {
            assert!(
                !local.tracks.iter().any(|t| t.song.mid == track.song.mid),
                "同一首不应同时在本地库与下载里: {}",
                track.song.mid
            );
        }
        assert_eq!(
            local.tracks.len() + downloads.tracks.len(),
            pool.len(),
            "本地库 + 下载应覆盖总池"
        );
        // 文件夹统计与曲目一致
        let folder_total: usize = local.folders.iter().map(|f| f.track_count).sum();
        assert_eq!(folder_total, local.tracks.len());
    }

    #[test]
    fn purchased_sorted_desc() {
        let pool = song_pool();
        let albums = curated_albums();
        let purchased = purchased_music(&pool, &albums);
        for pair in purchased.singles.windows(2) {
            assert!(pair[0].purchased_at >= pair[1].purchased_at);
        }
        assert!(purchased.singles.len() <= 9);
    }

    #[test]
    fn recent_labels_monotonic() {
        let pool = song_pool();
        let records = recent_records(&pool, 12);
        assert_eq!(records[0].label, "今天");
        assert_eq!(records.len(), 12);
    }
}
