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
        // 昵称：主页头部（encrypt_uin 入参）。失败保持回退值。
        if !credential.encrypt_uin.is_empty() {
            if let Ok(data) = UserApi::new(&self.client)
                .get_homepage(&credential.encrypt_uin, Some(&credential))
                .await
            {
                if let Some(nick) = find_str(&data, "nick")
                    .or_else(|| find_str(&data, "nickname"))
                    .or_else(|| find_str(&data, "name"))
                {
                    if !nick.is_empty() {
                        info.nickname = nick.to_string();
                    }
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
}
