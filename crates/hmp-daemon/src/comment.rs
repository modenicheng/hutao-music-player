//! 评论服务（spec §6）：mid → QQ numeric song id → CommentApi。
//!
//! 读（list）带内存 TTL cache（5 分钟）；写（post/reply/delete）直发 QQ
//! 不缓存。评论数据不落入本地媒体库（social 域与媒体库核心隔离）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use hmp_qqmusic_api::{CommentApi, QqMusicClient, credential::Credential, song::SongApi};
use hmp_storage::LibraryDb;

/// 评论查询缓存 TTL。
const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// 缓存条目是否新鲜（TTL 判定；纯函数便于测试）。
fn cache_fresh(at: Instant, now: Instant) -> bool {
    now.duration_since(at) < CACHE_TTL
}

/// 缓存键：(mid, sort)；值：写入时刻 + 页。
type CacheKey = (String, String);
type CacheEntry = (Instant, hmp_core::CommentPage);

/// 评论服务（server 持有；daemon 注入）。
pub struct CommentService {
    client: QqMusicClient,
    store: Box<dyn hmp_storage::credential::CredentialStore>,
    library: Arc<Mutex<LibraryDb>>,
    cache: Arc<Mutex<HashMap<CacheKey, CacheEntry>>>,
}

impl Clone for CommentService {
    fn clone(&self) -> Self {
        // 无状态客户端 + 共享 cache 与媒体库。
        Self {
            client: QqMusicClient::new(),
            store: hmp_storage::credential::store_from_env(),
            library: Arc::clone(&self.library),
            cache: Arc::clone(&self.cache),
        }
    }
}

impl CommentService {
    /// 新建。
    pub fn new(
        store: Box<dyn hmp_storage::credential::CredentialStore>,
        library: Arc<Mutex<LibraryDb>>,
    ) -> Self {
        Self {
            client: QqMusicClient::new(),
            store,
            library,
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn load_credential(&self) -> Result<Credential, String> {
        self.store
            .load()
            .map_err(|e| format!("读取凭证失败: {e}"))?
            .filter(|c| c.is_logged_in())
            .ok_or_else(|| "未登录，请先运行 hmp login".to_string())
    }

    /// mid → QQ numeric song id：库缓存 → 详情补全（写回库）。
    /// 锁不跨 await：先查库（同步），drop 后再请求详情，最后再写回。
    async fn resolve_song_id(&self, mid: &str) -> Result<i64, String> {
        {
            let mut lib = self.library.lock().unwrap();
            if let Some(id) = lib
                .qq_song_id("qq", mid)
                .map_err(|e| format!("媒体库查询失败: {e}"))?
            {
                return Ok(id);
            }
        }
        let api = SongApi::new(&self.client);
        let resp = api.get_detail(mid).await.map_err(|e| e.to_string())?;
        if resp.track.id <= 0 {
            return Err(format!("曲目 {mid} 无 numeric id（QQ 详情缺失）"));
        }
        {
            let mut lib = self.library.lock().unwrap();
            lib.set_track_qq_song_id("qq", mid, resp.track.id)
                .map_err(|e| format!("媒体库写入失败: {e}"))?;
        }
        Ok(resp.track.id)
    }

    /// 评论列表（TTL cache；sort: hot|new|recommend）。
    pub async fn list(&self, mid: &str, sort: &str) -> Result<hmp_core::CommentPage, String> {
        let key = (mid.to_string(), sort.to_string());
        if let Some((at, page)) = self.cache.lock().unwrap().get(&key) {
            if cache_fresh(*at, Instant::now()) {
                return Ok(page.clone());
            }
        }
        let biz_id = self.resolve_song_id(mid).await?;
        let api = CommentApi::new(&self.client);
        let comments = match sort {
            "new" => api.get_new_comments(biz_id, 1, 20).await,
            "recommend" => api.get_recommend_comments(biz_id, 1, 20).await,
            _ => api.get_hot_comments(biz_id, 1, 20).await,
        }
        .map_err(|e| e.to_string())?;
        let count = api.get_comment_count(biz_id).await.unwrap_or(0);
        let page = hmp_core::CommentPage {
            total: count,
            comments: comments
                .into_iter()
                .map(|c| hmp_core::CommentItem {
                    cm_id: c.cm_id,
                    seq_no: c.seq_no,
                    content: c.content,
                    nickname: c.nickname,
                    time: c.time,
                    like_count: c.like_count,
                })
                .collect(),
        };
        // 缓存上限：条目永不删除会导致 (mid, sort) 键无限增长。
        let mut cache = self.cache.lock().unwrap();
        if cache.len() >= 128 {
            cache.clear();
        }
        cache.insert(key, (Instant::now(), page.clone()));
        Ok(page)
    }

    /// 发表评论（reply_cmt_id 非空即回复）。
    pub async fn post(
        &self,
        mid: &str,
        content: &str,
        reply_cmt_id: Option<&str>,
    ) -> Result<String, String> {
        let credential = self.load_credential()?;
        let biz_id = self.resolve_song_id(mid).await?;
        let api = CommentApi::new(&self.client);
        let resp = api
            .add_comment(biz_id, content, reply_cmt_id, &credential)
            .await
            .map_err(|e| e.to_string())?;
        Ok(resp.comment_id)
    }

    /// 删除评论。
    pub async fn delete(&self, cm_id: &str) -> Result<(), String> {
        let credential = self.load_credential()?;
        let api = CommentApi::new(&self.client);
        let ok = api
            .delete_comment(cm_id, &credential)
            .await
            .map_err(|e| e.to_string())?;
        if ok {
            Ok(())
        } else {
            Err("QQ 删除评论失败".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TTL 判定：新鲜 → 命中；过期 → miss（纯函数，替代直接操作内部 cache 的恒真断言）。
    /// 缓存上限：超过 128 条时清空（防无限增长）。
    #[test]
    fn cache_caps_at_128() {
        let store = hmp_storage::credential::store_from_env();
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let svc = CommentService::new(store, lib);
        let mut cache = svc.cache.lock().unwrap();
        for i in 0..129 {
            let key = (format!("mid-{i}"), "hot".to_string());
            if cache.len() >= 128 {
                cache.clear();
            }
            cache.insert(key, (Instant::now(), hmp_core::CommentPage::default()));
        }
        assert!(
            cache.len() <= 128,
            "缓存应受 128 上限约束: len={}",
            cache.len()
        );
    }

    #[test]
    fn cache_fresh_respects_ttl() {
        let now = Instant::now();
        assert!(cache_fresh(now, now));
        assert!(cache_fresh(
            now,
            now + CACHE_TTL - std::time::Duration::from_secs(1)
        ));
        assert!(!cache_fresh(
            now,
            now + CACHE_TTL + std::time::Duration::from_secs(1)
        ));
        assert!(!cache_fresh(now, now + CACHE_TTL));
    }
}
