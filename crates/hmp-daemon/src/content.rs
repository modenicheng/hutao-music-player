//! 内容读服务（AUDIT §8.2/§8.3/§8.6/§8.4）：搜索 / 歌词 / 账号状态 /
//! QQ 封面取本地产物。daemon 持凭证统一出网，客户端（桌面/CLI）不直连 QQ。
//!
//! 读走内存 TTL cache（评论服务同类先例）；封面经 `covers/<hash>.jpg`
//! 目录契约落盘（与本地扫描共用 persist_cover，天然去重）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hmp_core::{AccountInfo, LyricPage, SearchPage};
use hmp_qqmusic_api::credential::Credential;
use hmp_qqmusic_api::{LyricApi, QqMusicClient, UserApi, song::SongApi};
use hmp_storage::credential::CredentialStore;

/// 歌词缓存 TTL（同评论服务量级）。
const LYRIC_TTL: Duration = Duration::from_secs(300);
/// 账号状态缓存 TTL（昵称/VIP 变化低频，设置页打开无需每次出网）。
const ACCOUNT_TTL: Duration = Duration::from_secs(600);
/// 缓存条目上限（防 (mid) 键无限增长；超限清空，同评论服务策略）。
const CACHE_CAP: usize = 128;

/// QQ 封面 CDN 前缀（CoverGet 只接受该来源，收敛 SSRF 面）。
const COVER_HOST_PREFIX: &str = "https://y.gtimg.cn/";

/// 缓存条目是否新鲜（纯函数便于测试）。
fn fresh(at: Instant, now: Instant, ttl: Duration) -> bool {
    now.duration_since(at) < ttl
}

/// 内容服务（server 持有；daemon 注入）。
pub struct ContentService {
    client: QqMusicClient,
    store: Box<dyn CredentialStore>,
    lyric_cache: Arc<Mutex<HashMap<String, (Instant, LyricPage)>>>,
    account_cache: Arc<Mutex<Option<(Instant, AccountInfo)>>>,
}

impl Clone for ContentService {
    fn clone(&self) -> Self {
        Self {
            client: QqMusicClient::new(),
            store: hmp_storage::credential::store_from_env(),
            lyric_cache: Arc::clone(&self.lyric_cache),
            account_cache: Arc::clone(&self.account_cache),
        }
    }
}

impl ContentService {
    /// 新建。
    pub fn new(store: Box<dyn CredentialStore>) -> Self {
        Self {
            client: QqMusicClient::new(),
            store,
            lyric_cache: Arc::new(Mutex::new(HashMap::new())),
            account_cache: Arc::new(Mutex::new(None)),
        }
    }

    /// 读有效凭证（Ok(None) = 未登录；Err 仅存储读取失败）。
    fn credential(&self) -> Result<Option<Credential>, String> {
        Ok(self
            .store
            .load()
            .map_err(|e| format!("failed to read credentials: {e}"))?
            .filter(|c| c.is_logged_in()))
    }

    /// 快速搜索（免登录 smartbox；AUDIT §8.2）。
    pub async fn search(&self, keyword: &str) -> Result<SearchPage, String> {
        if keyword.trim().is_empty() {
            return Ok(SearchPage::default());
        }
        let result = self
            .client
            .quick_search(keyword)
            .await
            .map_err(|e| e.to_string())?;
        Ok(SearchPage {
            songs: result
                .songs
                .into_iter()
                .map(|s| hmp_core::SearchSong {
                    mid: s.mid,
                    name: s.name,
                    singer: s.singer,
                })
                .collect(),
            albums: result
                .albums
                .into_iter()
                .map(|a| hmp_core::SearchAlbum {
                    mid: a.mid,
                    name: a.name,
                    singer: a.singer,
                })
                .collect(),
            singers: result
                .singers
                .into_iter()
                .map(|s| hmp_core::SearchSinger {
                    mid: s.mid,
                    name: s.name,
                })
                .collect(),
        })
    }

    /// 歌词（AUDIT §8.3）：song_type 由 daemon 内部经详情补齐（老 AppCore
    /// 从播放列表项取该字段；daemon 解析路径未透出 → 此处自查）。
    pub async fn lyric(&self, mid: &str) -> Result<LyricPage, String> {
        if let Some((at, page)) = self.lyric_cache.lock().unwrap().get(mid) {
            if fresh(*at, Instant::now(), LYRIC_TTL) {
                return Ok(page.clone());
            }
        }
        // song_type：详情接口的 track.type（1=普通歌曲 2=长音频 6=视频）。
        let detail = SongApi::new(&self.client)
            .get_detail(mid)
            .await
            .map_err(|e| e.to_string())?;
        let song_type = detail.track.type_;
        let resp = LyricApi::new(&self.client)
            .get_lyric(mid, song_type, false, true, false, false)
            .await
            .map_err(|e| e.to_string())?;
        let page = LyricPage {
            lyric: resp.lyric,
            translation: resp.trans,
        };
        let mut cache = self.lyric_cache.lock().unwrap();
        if cache.len() >= CACHE_CAP {
            cache.clear();
        }
        cache.insert(mid.to_string(), (Instant::now(), page.clone()));
        Ok(page)
    }

    /// 账号状态（AUDIT §8.6）：未登录返回 `logged_in=false`（Ok，非错误）；
    /// 已登录时昵称/VIP 拉取失败各自优雅降级（昵称回退 "QQ {uin}"）。
    pub async fn account_status(&self) -> Result<AccountInfo, String> {
        let Some(credential) = self.credential()? else {
            return Ok(AccountInfo::default());
        };
        if let Some((at, info)) = self.account_cache.lock().unwrap().as_ref() {
            if fresh(*at, Instant::now(), ACCOUNT_TTL) {
                return Ok(info.clone());
            }
        }
        let mut info = AccountInfo {
            logged_in: true,
            uin: credential.uin.clone(),
            nickname: format!("QQ {}", credential.uin),
            vip_summary: String::new(),
        };
        // 昵称：音乐基因用户名片（GetProfileReport，2026-09-29 实测可用）；
        // 主页 GetHomepageHeader 当前服务端返回 10000 空壳，仅作兜底。
        // 失败保持回退值。
        if !credential.encrypt_uin.is_empty() {
            let api = UserApi::new(&self.client);
            let nick = match api
                .get_music_gene(&credential.encrypt_uin, Some(&credential))
                .await
            {
                Ok(gene) if !gene.userinfo_card.nick_name.is_empty() => {
                    Some(gene.userinfo_card.nick_name)
                }
                _ => {
                    let mut nick = None;
                    if let Ok(data) = api
                        .get_homepage(&credential.encrypt_uin, Some(&credential))
                        .await
                    {
                        nick = find_str(&data, "nick")
                            .or_else(|| find_str(&data, "nickname"))
                            .or_else(|| find_str(&data, "name"))
                            .map(String::from);
                    }
                    nick
                }
            };
            if let Some(nick) = nick {
                if !nick.is_empty() {
                    info.nickname = nick;
                }
            }
        }
        // VIP 摘要：vip_login_base 的 isVip 标志。失败 → 空串（不猜测）。
        if let Ok(data) = UserApi::new(&self.client).get_vip_info(&credential).await {
            if let Some(is_vip) = find_flag(&data, "isVip") {
                info.vip_summary = if is_vip {
                    "VIP 会员".into()
                } else {
                    "非会员".into()
                };
            }
        }
        *self.account_cache.lock().unwrap() = Some((Instant::now(), info.clone()));
        Ok(info)
    }

    /// QQ 封面取本地产物（AUDIT §8.4）：下载进 `<data_dir>/covers/<hash>.jpg`
    /// （persist_cover 内容哈希去重，二次请求零网络），返回 `file://` URI。
    pub async fn cover(&self, url: &str) -> Result<String, String> {
        if !url.starts_with(COVER_HOST_PREFIX) {
            return Err(format!("cover url must be {COVER_HOST_PREFIX}…"));
        }
        let bytes = hmp_media::cdn_client()
            .get(url)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| format!("cover download failed: {e}"))?
            .bytes()
            .await
            .map_err(|e| e.to_string())?;
        if bytes.is_empty() {
            return Err("cover download returned empty body".into());
        }
        hmp_storage::scan::persist_cover(&bytes).map_err(|e| e.to_string())
    }

    /// 发现页聚合（AUDIT §8.2 同源）：推荐歌单广场 + 新歌，免登录。
    pub async fn discover(
        &self,
        songlist_page: u32,
        new_song_type: u32,
    ) -> Result<hmp_core::DiscoverPage, String> {
        let api = hmp_qqmusic_api::recommend::RecommendApi::new(&self.client);
        let playlists = api
            .get_recommend_songlist(hmp_qqmusic_api::pagination::Page::new(songlist_page, 30))
            .await
            .map_err(|e| e.to_string())?;
        let newsong = api
            .get_recommend_newsong(new_song_type as i64)
            .await
            .map_err(|e| e.to_string())?;
        Ok(hmp_core::DiscoverPage {
            playlists: playlists
                .songlists
                .into_iter()
                .map(|p| hmp_core::DiscoverPlaylist {
                    id: p.id,
                    title: p.title,
                    picurl: p.picurl,
                    creator: p.creator_nick,
                    songnum: p.songnum,
                    listennum: p.listennum,
                })
                .collect(),
            has_more_playlists: playlists.has_more,
            new_songs: newsong.songs.iter().map(project_song).collect(),
        })
    }

    /// 排行榜分类（免登录）：分组 + 各榜预览前 3 首。
    pub async fn top_category(&self) -> Result<hmp_core::TopCategoryPage, String> {
        let resp = hmp_qqmusic_api::top::TopApi::new(&self.client)
            .get_category()
            .await
            .map_err(|e| e.to_string())?;
        Ok(hmp_core::TopCategoryPage {
            groups: resp
                .group
                .into_iter()
                .map(|g| hmp_core::TopGroupDto {
                    id: g.id,
                    name: g.name,
                    tops: g
                        .toplist
                        .into_iter()
                        .map(|t| hmp_core::TopSummaryDto {
                            id: t.id,
                            name: t.name,
                            title_sub: t.title_sub,
                            update_time: t.update_time,
                            listen_num: t.listen_num,
                            picurl: t.front_pic_url,
                            preview: t
                                .songs
                                .iter()
                                .take(3)
                                .map(|s| format!("{}. {} - {}", s.rank, s.name, s.singer_name))
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        })
    }

    /// 排行榜详情（免登录）：完整曲目列表分页。
    pub async fn top_detail(
        &self,
        top_id: i64,
        num: i64,
        page: i64,
    ) -> Result<hmp_core::TopDetailPage, String> {
        let resp = hmp_qqmusic_api::top::TopApi::new(&self.client)
            .get_detail(
                top_id,
                hmp_qqmusic_api::pagination::Page::new(page.max(1) as u32, num.max(1) as u32),
                false,
            )
            .await
            .map_err(|e| e.to_string())?;
        let total = resp.info.total_num;
        let fetched = (num * page.max(1)).max(0);
        Ok(hmp_core::TopDetailPage {
            name: resp.info.name,
            title_sub: resp.info.title_sub,
            update_time: resp.info.update_time,
            songs: resp
                .songs
                .iter()
                .map(|s| {
                    let p = project_song(s);
                    hmp_core::TopSongDto {
                        mid: p.mid,
                        name: p.name,
                        singer: p.singer,
                        album: p.album,
                        interval: p.interval,
                        picurl: p.picurl,
                    }
                })
                .collect(),
            total,
            has_more: fetched < total,
        })
    }

    /// 猜你喜欢（需登录；非安卓匿名返回 1000，由上层转 NotLoggedIn；
    /// 上游接口当前无分页参数，页号仅预留）。
    pub async fn guess(&self, _page: u32) -> Result<hmp_core::GuessPage, String> {
        let Some(cred) = self.credential().map_err(|e| e.to_string())? else {
            return Err("not logged in".into());
        };
        let resp = hmp_qqmusic_api::recommend::RecommendApi::new(&self.client)
            .get_guess_recommend(&cred)
            .await
            .map_err(|e| e.to_string())?;
        Ok(hmp_core::GuessPage {
            songs: resp.songs.iter().map(project_song).collect(),
        })
    }
}

/// 展示型字段提取：从 JSON 任意层级找第一个指定 key 的字符串值
/// （CLI `hmp account profile` 同逻辑下沉）。
fn find_str<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(s)) = map.get(key) {
                return Some(s);
            }
            map.values().find_map(|sub| find_str(sub, key))
        }
        serde_json::Value::Array(items) => items.iter().find_map(|sub| find_str(sub, key)),
        _ => None,
    }
}

/// 布尔标志提取：任意层级找 key 的 bool/数值形态（isVip 等字段形态不定）。
fn find_flag(v: &serde_json::Value, key: &str) -> Option<bool> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(val) = map.get(key) {
                match val {
                    serde_json::Value::Bool(b) => return Some(*b),
                    serde_json::Value::Number(n) => return Some(n.as_i64() != Some(0)),
                    _ => {}
                }
            }
            map.values().find_map(|sub| find_flag(sub, key))
        }
        serde_json::Value::Array(items) => items.iter().find_map(|sub| find_flag(sub, key)),
        _ => None,
    }
}

/// `Song` → 发现页/榜单/猜你喜欢共用窄投影。
fn project_song(s: &hmp_qqmusic_api::models::Song) -> hmp_core::DiscoverNewSong {
    let singer = if s.singer.is_empty() {
        String::new()
    } else {
        s.singer
            .iter()
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>()
            .join(" / ")
    };
    hmp_core::DiscoverNewSong {
        mid: s.mid.clone(),
        name: s.name.clone(),
        singer,
        album: s.album.name.clone(),
        interval: s.interval,
        picurl: s.album.pmid.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TTL 判定边界（新鲜 → 命中；到期 → miss）。
    #[test]
    fn freshness_respects_ttl() {
        let now = Instant::now();
        assert!(fresh(now, now, LYRIC_TTL));
        assert!(!fresh(now, now + LYRIC_TTL, LYRIC_TTL));
        assert!(!fresh(
            now,
            now + LYRIC_TTL + Duration::from_secs(1),
            LYRIC_TTL
        ));
    }

    #[test]
    fn find_str_searches_nested() {
        let v = serde_json::json!({ "data": { "userinfo": { "nick": "胡桃" } } });
        assert_eq!(find_str(&v, "nick"), Some("胡桃"));
        assert_eq!(find_str(&v, "missing"), None);
    }

    #[test]
    fn find_flag_matches_bool_and_number_forms() {
        let v = serde_json::json!({ "vip": { "isVip": true } });
        assert_eq!(find_flag(&v, "isVip"), Some(true));
        let v = serde_json::json!({ "isVip": 0 });
        assert_eq!(find_flag(&v, "isVip"), Some(false));
        let v = serde_json::json!({ "isVip": 1 });
        assert_eq!(find_flag(&v, "isVip"), Some(true));
        assert_eq!(find_flag(&serde_json::json!({}), "isVip"), None);
    }

    /// 封面前缀守卫：非 y.gtimg.cn 拒绝（不出网）。
    #[tokio::test]
    async fn cover_rejects_non_gtimg_urls() {
        let svc = ContentService::new(hmp_storage::credential::store_from_env());
        for url in [
            "http://y.gtimg.cn/a.jpg",
            "https://evil.example.com/a.jpg",
            "file:///etc/passwd",
            "",
        ] {
            assert!(svc.cover(url).await.is_err(), "应拒绝: {url}");
        }
    }

    fn song(mid: &str, name: &str, singer: &str) -> QuickSong {
        QuickSong {
            mid: mid.to_string(),
            name: name.to_string(),
            singer: singer.to_string(),
        }
    }

    /// 匹配评分：标题全等 + 歌手命中 > 标题含 + 歌手命中 > 仅标题含（拒绝）。
    #[test]
    fn pick_best_song_scores_and_thresholds() {
        let songs = vec![
            song("a", "夜曲 (Live)", "别的歌手"),
            song("b", "夜曲", "周杰伦 /费玉清"),
            song("c", "夜曲钢琴版", "周杰伦"),
        ];
        assert_eq!(
            pick_best_song(&songs, "夜 曲", "周杰伦").map(|s| s.mid.as_str()),
            Some("b")
        );
        // 互为包含 + 歌手命中 = 2 分：采纳
        let partial = vec![song("p", "夜曲钢琴版", "周杰伦")];
        assert_eq!(
            pick_best_song(&partial, "夜曲", "周杰伦").map(|s| s.mid.as_str()),
            Some("p")
        );
        // 仅标题含、无歌手命中 = 1 分：拒绝
        let weak = vec![song("w", "夜曲 (Live)", "别人")];
        assert_eq!(pick_best_song(&weak, "夜曲", "周杰伦"), None);
        // 空标题 / 空结果：None
        assert_eq!(pick_best_song(&songs, "  ", "周杰伦"), None);
        assert_eq!(pick_best_song(&[], "夜曲", "周杰伦"), None);
    }

    /// L2 磁盘命中：缓存文件存在时 `lyric` 直接返回、不出网
    /// （无 hit 会走 detail API 对非法 mid 报错，断言即可甄别路径）。
    #[allow(clippy::await_holding_lock)] // 测试串行锁，有意跨 await
    #[tokio::test]
    async fn lyric_disk_cache_hits_before_network() {
        // 隔离 XDG_CACHE_HOME（daemon 测试套件无其他读者；改 env 串行由本锁保证）
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _env = ENV_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("hmp-content-lyric-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", &root);
        }
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe { std::env::remove_var("XDG_CACHE_HOME") };
            }
        }
        let _restore = Restore;

        hmp_storage::lyric_cache::write_cached_lyric("cached01", "[00:01.00]盘内\n", "").unwrap();
        let svc = ContentService::new(hmp_storage::credential::store_from_env());
        let page = svc.lyric("cached01").await.unwrap();
        assert_eq!(page.lyric, "[00:01.00]盘内\n");
        assert_eq!(page.source, "qq");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 匹配评分：多歌手任一命中即可；平分先到先得。
    #[test]
    fn pick_best_song_artist_tokens_and_ties() {
        let songs = vec![
            song("first", "晴天", "周杰伦"),
            song("second", "晴天", "周杰伦 / 杨瑞代"),
        ];
        assert_eq!(
            pick_best_song(&songs, "晴天", "杨瑞代 / 周杰伦").map(|s| s.mid.as_str()),
            Some("first")
        );
    }

    /// 本地优先端到端（零出网）：同目录 `.lrc` 命中 → source=local；
    /// 无 sidecar/无标签 → 走 QQ 检索（离线报 Err 而非静默空页）。
    #[tokio::test]
    async fn track_lyric_local_sidecar_wins() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("song.mp3");
        std::fs::write(&audio, b"not really audio").unwrap();
        std::fs::write(dir.path().join("song.lrc"), "[00:01.00]本地歌词\n").unwrap();
        let svc = ContentService::new(hmp_storage::credential::store_from_env());
        let page = svc
            .track_lyric(&format!("local:{}", audio.display()), "song", "artist")
            .await
            .unwrap();
        assert_eq!(page.source, "local");
        assert!(page.lyric.contains("本地歌词"));
    }

    #[tokio::test]
    async fn track_lyric_empty_title_skips_search() {
        let svc = ContentService::new(hmp_storage::credential::store_from_env());
        let page = svc
            .track_lyric("local:/nonexistent/x.mp3", "  ", "")
            .await
            .unwrap();
        assert_eq!(page, LyricPage::default());
    }
}
