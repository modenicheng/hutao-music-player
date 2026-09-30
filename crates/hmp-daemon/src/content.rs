//! 内容读服务（AUDIT §8.2/§8.3/§8.6/§8.4）：搜索 / 歌词 / 账号状态 /
//! QQ 封面取本地产物。daemon 持凭证统一出网，客户端（桌面/CLI）不直连 QQ。
//!
//! 读走内存 TTL cache（评论服务同类先例）；封面经 `covers/<hash>.jpg`
//! 目录契约落盘（与本地扫描共用 persist_cover，天然去重）。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hmp_core::{AccountInfo, LyricPage, SearchPage};
use hmp_qqmusic_api::credential::Credential;
use hmp_qqmusic_api::protocol::search::QuickSong;
use hmp_qqmusic_api::{LoginApi, LyricApi, QqMusicClient, UserApi, song::SongApi};
use hmp_storage::credential::CredentialStore;

/// 歌词缓存 TTL（同评论服务量级）。
const LYRIC_TTL: Duration = Duration::from_secs(300);
/// 账号状态缓存 TTL（昵称/VIP 变化低频，设置页打开无需每次出网）。
const ACCOUNT_TTL: Duration = Duration::from_secs(600);
/// 缓存条目上限（防 (mid) 键无限增长；超限清空，同评论服务策略）。
const CACHE_CAP: usize = 128;

/// QQ 封面 CDN 允许域（CoverGet 只接受这些来源，收敛 SSRF 面）。
/// 推荐歌单广场/新歌实测（2026-09-30 探针）返回 `qpic.y.qq.com` 与
/// `music-file.y.qq.com`，专辑 pmid 模板（player.rs 同款）在 `y.gtimg.cn`。
const COVER_HOSTS: [&str; 3] = ["y.gtimg.cn", "qpic.y.qq.com", "music-file.y.qq.com"];

/// 校验并归一封面 URL：host 必须在允许域内；scheme 一律升级 https
/// （三个域均实测支持；上游常给 http 形式的 qpic 链接）。
fn normalize_cover_url(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let host = rest.split(['/', '?', '#']).next()?;
    if !COVER_HOSTS.contains(&host) {
        return None;
    }
    Some(format!(
        "https://{host}/{}",
        rest[host.len()..].trim_start_matches('/')
    ))
}

/// 专辑 pmid → 封面 URL（T002R300x300M000 模板，与 daemon player.rs /
/// 桌面 app.rs 同款；空 pmid → 空串，UI 保持程序化占位）。
fn cover_url_from_pmid(pmid: &str) -> String {
    if pmid.is_empty() {
        return String::new();
    }
    format!("https://y.gtimg.cn/music/photo_new/T002R300x300M000{pmid}.jpg")
}

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

    /// 账号缓存失效（登录/登出后由 server 调用）：下次 `AccountStatus`
    /// 绕过 TTL 重新出网，桌面账号页立即可见新登录态。
    pub fn invalidate_account_cache(&self) {
        *self.account_cache.lock().unwrap() = None;
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
    /// 三级读取：内存 TTL（L1）→ 磁盘缓存（L2，`cache_dir()/lyrics/`，
    /// 跨重启免重复出网）→ QQ 网络；网络成功后双写回填。
    pub async fn lyric(&self, mid: &str) -> Result<LyricPage, String> {
        if let Some((at, page)) = self.lyric_cache.lock().unwrap().get(mid) {
            if fresh(*at, Instant::now(), LYRIC_TTL) {
                return Ok(page.clone());
            }
        }
        if let Some((lyric, translation)) = hmp_storage::lyric_cache::read_cached_lyric(mid) {
            let page = LyricPage {
                lyric,
                translation,
                source: "qq".into(),
            };
            self.lyric_insert(mid, page.clone());
            return Ok(page);
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
        // L2 落盘（空正文静默跳过；失败仅丢缓存不阻断）。落盘前校验内容
        // 像 LRC（含 `[` 标签）：上游偶发返回未解密的 QRC hex，落盘会把
        // 解密失败的脏数据永久化，存疑内容不缓存、下次请求重试网络。
        if resp.lyric.contains('[') {
            if let Err(e) =
                hmp_storage::lyric_cache::write_cached_lyric(mid, &resp.lyric, &resp.trans)
            {
                tracing::warn!(mid, %e, "lyric disk cache write failed");
            }
        }
        let page = LyricPage {
            lyric: resp.lyric,
            translation: resp.trans,
            source: "qq".into(),
        };
        self.lyric_insert(mid, page.clone());
        Ok(page)
    }

    /// L1 内存缓存写入（超限清空，同评论服务策略）。
    fn lyric_insert(&self, key: &str, page: LyricPage) {
        let mut cache = self.lyric_cache.lock().unwrap();
        if cache.len() >= CACHE_CAP {
            cache.clear();
        }
        cache.insert(key.to_string(), (Instant::now(), page));
    }

    /// 歌词（统一入口，本地优先）：`local:` 曲先读同目录 sidecar `.lrc`、
    /// 再试内嵌歌词标签；两者皆无 → 按标题+歌手检索 QQ 最佳匹配曲目取词
    /// （免登录 smartbox）；QQ 曲按 mid 直取。结果（含空页负缓存，防
    /// 换曲风暴重复出网）按键 `id` 入缓存。
    pub async fn track_lyric(
        &self,
        id: &str,
        title: &str,
        artist: &str,
    ) -> Result<LyricPage, String> {
        if let Some((at, page)) = self.lyric_cache.lock().unwrap().get(id) {
            if fresh(*at, Instant::now(), LYRIC_TTL) {
                return Ok(page.clone());
            }
        }
        let page = match id.strip_prefix("local:") {
            Some(path) => {
                self.local_lyric_with_fallback(Path::new(path), title, artist)
                    .await?
            }
            None => self.lyric(id).await?,
        };
        self.lyric_insert(id, page.clone());
        Ok(page)
    }

    /// 本地曲歌词：sidecar `.lrc` → 内嵌歌词标签 → QQ 检索兜底。
    /// 本地命中标注 `source: "local"`；兜底无达标匹配 → 空页（非错误，
    /// UI 呈现"暂无歌词"）。
    async fn local_lyric_with_fallback(
        &self,
        path: &Path,
        title: &str,
        artist: &str,
    ) -> Result<LyricPage, String> {
        if let Some(lyric) =
            hmp_storage::read_sidecar_lrc(path).or_else(|| hmp_storage::read_embedded_lyrics(path))
        {
            return Ok(LyricPage {
                lyric,
                translation: String::new(),
                source: "local".into(),
            });
        }
        let Some(mid) = self.search_lyric_mid(title, artist).await? else {
            return Ok(LyricPage::default());
        };
        self.lyric(&mid).await
    }

    /// 本地曲 QQ 检索兜底：免登录 smartbox，先 `"标题 歌手"` 再退 `"标题"`；
    /// 命中 [`pick_best_song`] 评分门槛 → mid。检索全部失败 → Err（网络
    /// 问题向上浮）；全部成功但无达标匹配 → Ok(None)。
    async fn search_lyric_mid(&self, title: &str, artist: &str) -> Result<Option<String>, String> {
        let trimmed_title = title.trim();
        if trimmed_title.is_empty() {
            return Ok(None);
        }
        let trimmed_artist = artist.trim();
        let mut keywords = Vec::with_capacity(2);
        if !trimmed_artist.is_empty() {
            keywords.push(format!("{trimmed_title} {trimmed_artist}"));
        }
        keywords.push(trimmed_title.to_owned());
        let mut last_err = None;
        for keyword in keywords {
            match self.client.quick_search(&keyword).await {
                Ok(result) => {
                    if let Some(song) = pick_best_song(&result.songs, trimmed_title, trimmed_artist)
                    {
                        return Ok(Some(song.mid.clone()));
                    }
                }
                Err(e) => last_err = Some(e.to_string()),
            }
        }
        match last_err {
            Some(message) => Err(message),
            None => Ok(None),
        }
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
        // 昵称多通道（2026-09-30 重排）：QQ 扫码走三方接入（QQConnectLogin），
        // 登录响应不含 encryptUin → 旧逻辑整段昵称拉取被 `encrypt_uin` 非空门禁
        // 跳过，永远显示回退 "QQ {uin}"。通道按「不依赖加密 uin → 依赖」排序：
        //   1) fcg_get_profile_homepage.fcg（数字 uin 即可，主通道）
        //   2) 音乐基因 GetProfileReport（加密 uin；2026-09-29 实测可用）
        //   3) 主页 GetHomepageHeader（加密 uin；当前服务端 10000 空壳，兜底）
        //   4) vip_login_base 的 isVip（顺带兜底昵称/加密 uin）
        // 全失败保持回退值；期间回捞到的加密 uin 回写凭证库（下次直接可用）。
        let api = UserApi::new(&self.client);
        let mut nick: Option<String> = None;
        let mut euin = credential.encrypt_uin.clone();
        if let Ok(data) = LoginApi::new(&self.client)
            .get_profile_homepage(&credential)
            .await
        {
            nick = find_any_str(&data, &["nick", "NickName", "nickName", "nick_name"]);
            if euin.is_empty() {
                euin = find_any_str(&data, &["encryptUin", "encrypt_uin", "ecuin"])
                    .unwrap_or_default();
            }
        }
        if nick.is_none() && !euin.is_empty() {
            if let Ok(gene) = api.get_music_gene(&euin, Some(&credential)).await {
                let card = gene.userinfo_card.nick_name;
                if !card.is_empty() {
                    nick = Some(card);
                }
            }
        }
        if nick.is_none() && !euin.is_empty() {
            if let Ok(data) = api.get_homepage(&euin, Some(&credential)).await {
                nick = find_any_str(&data, &["nick", "nickname", "name"]);
            }
        }
        // VIP 摘要：vip_login_base 的 isVip 标志。失败 → 空串（不猜测）。
        if let Ok(data) = api.get_vip_info(&credential).await {
            if let Some(is_vip) = find_flag(&data, "isVip") {
                info.vip_summary = if is_vip {
                    "VIP 会员".into()
                } else {
                    "非会员".into()
                };
            }
            if nick.is_none() {
                nick = find_any_str(&data, &["nick", "NickName", "nickName", "nick_name"]);
            }
            if euin.is_empty() {
                euin = find_any_str(&data, &["encryptUin", "encrypt_uin", "ecuin"])
                    .unwrap_or_default();
            }
        }
        if let Some(patched) = patch_encrypt_uin(&credential, &euin) {
            // 一次性补救，失败不影响本次展示（下次 AccountStatus 再试）
            let _ = self.store.save(&patched);
        }
        if let Some(nick) = nick {
            if !nick.is_empty() {
                info.nickname = nick;
            }
        }
        *self.account_cache.lock().unwrap() = Some((Instant::now(), info.clone()));
        Ok(info)
    }

    /// QQ 封面取本地产物（AUDIT §8.4）：下载进 `<data_dir>/covers/<hash>.jpg`
    /// （persist_cover 内容哈希去重，二次请求零网络），返回 `file://` URI。
    pub async fn cover(&self, url: &str) -> Result<String, String> {
        let url = normalize_cover_url(url).ok_or_else(|| {
            format!("cover url host not allowed (expect one of {COVER_HOSTS:?})")
        })?;
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

/// 查询归一化：小写 + 去全部空白，用于标题/歌手比对（全半角混排容错）。
fn normalize_text(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// QQ 检索结果最佳匹配（保守门槛，宁缺毋错词）：归一化标题全等 +2，
/// 互为包含 +1；歌手令牌（`"A / B"` 按 `/` 拆）命中 +1。总分 ≥ 2 才采纳，
/// 取最高分（先到先得平分）。无达标 → None（上层空页负缓存）。
fn pick_best_song<'a>(songs: &'a [QuickSong], title: &str, artist: &str) -> Option<&'a QuickSong> {
    let title_norm = normalize_text(title);
    if title_norm.is_empty() {
        return None;
    }
    let artist_tokens: Vec<String> = artist
        .split('/')
        .map(normalize_text)
        .filter(|t| !t.is_empty())
        .collect();
    let mut best: Option<(i32, &QuickSong)> = None;
    for song in songs {
        let name_norm = normalize_text(&song.name);
        let mut score = 0;
        if name_norm == title_norm {
            score += 2;
        } else if !name_norm.is_empty()
            && (name_norm.contains(&title_norm) || title_norm.contains(&name_norm))
        {
            score += 1;
        }
        if !artist_tokens.is_empty() {
            let singer_norm = normalize_text(&song.singer);
            if artist_tokens
                .iter()
                .any(|token| singer_norm.contains(token.as_str()))
            {
                score += 1;
            }
        }
        if score >= 2
            && best
                .as_ref()
                .is_none_or(|(best_score, _)| score > *best_score)
        {
            best = Some((score, song));
        }
    }
    best.map(|(_, song)| song)
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

/// 多键名依序提取：按 `keys` 顺序首个**非空**命中（服务端同一字段有
/// camelCase/legacy 多种拼写，泛搜 `name` 之类宽键时放最后）。
/// 注意空值豁免只在键之间——`find_str` 对单个键返回首个命中（含空），
/// 不对同键的更深嵌套实例重试。
fn find_any_str(v: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| find_str(v, k).filter(|s| !s.is_empty()).map(String::from))
}

/// 用响应里回捞的加密 uin 补全凭证（无变化/空值返回 None）。
///
/// QQ 三方接入登录响应不含 `encryptUin`，首次成功后回写凭证库，
/// 之后依赖加密 uin 的接口（音乐基因名片等）即可直接工作。
fn patch_encrypt_uin(credential: &Credential, euin: &str) -> Option<Credential> {
    if !euin.is_empty() && euin != credential.encrypt_uin {
        let mut patched = credential.clone();
        patched.encrypt_uin = euin.to_owned();
        Some(patched)
    } else {
        None
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
        // 裸 pmid 不是 URL（2026-09-30 修复：此前直接透传，UI 经 CoverGet
        // 换图时被域守卫拒绝）；套 T002 模板成完整封面地址
        picurl: cover_url_from_pmid(&s.album.pmid),
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
    fn find_any_str_skips_empty_and_respects_order() {
        // 首键命中为空 → 视为未命中，落到次键（键之间豁免空值）
        let v = serde_json::json!({ "nick": "", "nickname": "胡桃" });
        assert_eq!(find_any_str(&v, &["nick", "nickname"]), Some("胡桃".into()));
        // 键名按序优先（camelCase 变体在前的先命中）
        let v = serde_json::json!({ "NickName": "甲", "nick": "乙" });
        assert_eq!(find_any_str(&v, &["nick", "NickName"]), Some("乙".into()));
        assert_eq!(find_any_str(&v, &["NickName", "nick"]), Some("甲".into()));
        // 全 miss → None
        assert_eq!(find_any_str(&v, &["missing"]), None);
    }

    #[test]
    fn patch_encrypt_uin_only_on_change() {
        let cred = Credential {
            uin: "42".into(),
            encrypt_uin: String::new(),
            ..Default::default()
        };
        // 回捞到新值 → 补全
        let patched = patch_encrypt_uin(&cred, "e-abc").expect("should patch");
        assert_eq!(patched.encrypt_uin, "e-abc");
        assert_eq!(patched.uin, "42");
        // 同值/空值 → 不动
        assert!(patch_encrypt_uin(&patched, "e-abc").is_none());
        assert!(patch_encrypt_uin(&cred, "").is_none());
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

    /// URL 归一（纯函数）：允许域 http 升 https；域外/非 http scheme 拒绝。
    #[test]
    fn normalize_cover_url_allowlist_and_upgrade() {
        // 三个允许域全放行，http 一律升 https
        assert_eq!(
            normalize_cover_url("http://y.gtimg.cn/a.jpg").as_deref(),
            Some("https://y.gtimg.cn/a.jpg")
        );
        assert_eq!(
            normalize_cover_url("https://y.gtimg.cn/a.jpg").as_deref(),
            Some("https://y.gtimg.cn/a.jpg")
        );
        assert_eq!(
            normalize_cover_url("http://qpic.y.qq.com/music_cover/x/300?n=1").as_deref(),
            Some("https://qpic.y.qq.com/music_cover/x/300?n=1")
        );
        assert_eq!(
            normalize_cover_url(
                "https://music-file.y.qq.com/songlist/u/a/b.jpg?imageView2/4/w/300/h/300"
            )
            .as_deref(),
            Some("https://music-file.y.qq.com/songlist/u/a/b.jpg?imageView2/4/w/300/h/300")
        );
        // 域外 / 伪装 host / 非 http scheme / 空串：拒绝
        assert_eq!(normalize_cover_url("https://evil.example.com/a.jpg"), None);
        assert_eq!(normalize_cover_url("https://y.gtimg.cn.evil.com/a.jpg"), None);
        assert_eq!(normalize_cover_url("file:///etc/passwd"), None);
        assert_eq!(normalize_cover_url(""), None);
    }

    /// pmid → T002 模板（空 pmid → 空串）。
    #[test]
    fn cover_url_from_pmid_template() {
        assert_eq!(
            cover_url_from_pmid("002RDtQX06q5Is_1"),
            "https://y.gtimg.cn/music/photo_new/T002R300x300M000002RDtQX06q5Is_1.jpg"
        );
        assert_eq!(cover_url_from_pmid(""), "");
    }

    /// 封面域守卫：非允许域拒绝（不出网）；http 形式的允许域升级后放行。
    #[tokio::test]
    async fn cover_rejects_non_gtimg_urls() {
        let svc = ContentService::new(hmp_storage::credential::store_from_env());
        for url in [
            "https://evil.example.com/a.jpg",
            "https://y.gtimg.cn.evil.com/a.jpg",
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
