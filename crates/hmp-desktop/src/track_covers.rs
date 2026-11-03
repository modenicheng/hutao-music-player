//! 列表曲目封面预取（2026-10-03 封面审计第四断点）：列表落地即批量取图。
//!
//! 此前只有 discover 歌单卡（online_covers）与当前播放曲（player_bridge）有
//! 异步补图；歌曲行（歌单详情/我喜欢/榜单……）永远程序化占位。预取链路：
//!
//! UI 列表落地 → (mid, url) 清单 → daemon `CoverGet`（v8 `cover_cache` 盘级
//! 索引优先，同 URL 全生命周期只下载一次）→ 本地产物落盘 + `rebind_cover_url`
//! 把库内远程 URL 升级为 file:// ——下一轮投影直读盘、零 IPC。
//!
//! **UI 侧不回填模型**（2026-10-03 内存治理 §21 语义合并）：表格行已不携带
//! cover（TrackRow 剥离，12+ 表格模型每行背全尺寸位图是 RSS 2.4G 根因之一），
//! 预取的价值在 daemon 盘缓存预热——播放/队列抽屉/详情页/侧栏随后的封面读取
//! 经 `cover_cache`（字节加权 LRU）直读盘产物。原 `COVER_IMAGES`/`IMAGE_CACHE`
//! 两个无界线程本地位图缓存随回填一并移除（正是被治理的病灶形态）。
//!
//! 去重纪律（online_covers / player_bridge 同款）：`REQUESTED`（mid|url 全局）
//! 进程内在途/已成只发一次；**失败出账**（2026-10-04 封面审计⑤）——冷启动
//! 窗口（daemon 未就绪/StaleBackend 重启中）一次失败不该让该图整个会话
//! 永远占位。重试节奏由触发源决定：本模块的触发源是进页面/队列重建
//! （低频），失败出账即可自然重试；10Hz 高频触发源（当前曲）另带退避
//! （player_bridge::spawn_cover_fetch）。

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;

/// 并发上限：daemon 命中盘索引时极快，未命中时打满 CDN 的在途请求数也
/// 不至于触发限流；大歌单（2000 首）分钟级渐进填满，全程不阻塞 UI。
const MAX_INFLIGHT: usize = 8;

thread_local! {
    /// 在账的 mid|url（在途或已成功；失败即出账允许重试，进程级）。
    static REQUESTED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// 记账：key 未在账则入账并返回 true（允许发起）。
fn account(key: &str) -> bool {
    REQUESTED.with(|set| set.borrow_mut().insert(key.to_owned()))
}

/// 出账：请求失败后调用，下一次触发源（进页面/队列重建）可重试。
fn release(key: &str) {
    REQUESTED.with(|set| {
        set.borrow_mut().remove(key);
    });
}

/// 每张图回包后的 UI 线程回调（(mid, 降采样图)）。slint::Image 非 Send，
/// 转交发生在 invoke_from_event_loop 闭包内，回调本体只见到主线程产物。
pub type OnCover = Arc<dyn Fn(String, slint::Image) + Send + Sync>;

/// 列表落地后的预取入口（纯 IPC：出网与落库全在 daemon 侧，UI 线程只派发）。
///
/// `targets`：(mid, 远程封面 URL) 对——QQ 行的 `remote_cover` 投影，或在线页
/// 行的 picurl。
pub fn prefetch_tracks(
    runtime: &Arc<crate::backend::BackendRuntime>,
    targets: Vec<(String, String)>,
) {
    prefetch_tracks_with(runtime, targets, None);
}

/// 带回包回填的预取：每张图落到本地产物后（UI 线程）调 `on_cover(mid, image)`
/// ——队列抽屉行原地换图（2026-10-04 封面审计①：队列行封面在投影时固化，
/// daemon rebind 不触发队列重建，不回填则非当前曲整场会话占位）。
pub fn prefetch_tracks_with(
    runtime: &Arc<crate::backend::BackendRuntime>,
    targets: Vec<(String, String)>,
    on_cover: Option<OnCover>,
) {
    if targets.is_empty() {
        return;
    }
    let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_INFLIGHT));
    let mut dispatched = 0usize;
    for (mid, url) in targets {
        if mid.is_empty() || url.is_empty() {
            continue;
        }
        // 进程内去重：在途/已成功只发一次（失败出账可重试）
        let key = format!("{mid}|{url}");
        if !account(&key) {
            continue;
        }
        let permits = semaphore.clone();
        let runtime = runtime.clone();
        let on_cover = on_cover.clone();
        runtime.spawn(async move {
            let _permit = permits.acquire_owned().await.expect("semaphore open");
            match crate::backend::request(hmp_core::Request::CoverGet { url }).await {
                Ok(hmp_core::Response::Cover(uri)) => {
                    if let Some(on_cover) = &on_cover {
                        let on_cover = Arc::clone(on_cover);
                        let mid = mid.clone();
                        // 降采样必须发生在 UI 线程（cover_cache 是 thread_local）
                        let _ = slint::invoke_from_event_loop(move || {
                            let path = crate::covers::file_uri_to_path(&uri).unwrap_or(uri);
                            if let Some(image) = crate::cover_cache::get_or_load(&path, 256) {
                                on_cover(mid, image);
                            }
                        });
                    }
                    // 无回调时响应到此为止：daemon 侧已完成盘缓存登记与 rebind
                }
                _ => release(&key),
            }
        });
        dispatched += 1;
    }
    if dispatched > 0 {
        tracing::debug!(dispatched, "track covers prefetch dispatched");
    }
}

/// SongRow 列表 → 预取清单（QQ 行的 remote_cover 投影）。
pub fn targets_from_rows(rows: &[crate::library_view::SongRow]) -> Vec<(String, String)> {
    rows.iter()
        .filter_map(|row| row.remote_cover.clone().map(|url| (row.mid.clone(), url)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_view::SongRow;

    #[test]
    fn targets_from_rows_keeps_qq_remote_only() {
        let row = |mid: &str, remote: Option<&str>| SongRow {
            mid: mid.into(),
            source: 0,
            title: "t".into(),
            artists: String::new(),
            album: String::new(),
            duration_ms: 0,
            quality: String::new(),
            cover_uri: None,
            remote_cover: remote.map(String::from),
            folder: None,
        };
        let rows = vec![
            row("m1", Some("https://y.gtimg.cn/a.jpg")),
            row("m2", None),
            row("local:C:/x.mp3", None),
        ];
        let targets = targets_from_rows(&rows);
        assert_eq!(
            targets,
            vec![("m1".into(), "https://y.gtimg.cn/a.jpg".into())]
        );
    }

    /// 失败出账语义（2026-10-04 封面审计⑤）：在账拦重复；release 后可再入账
    /// ——冷启动窗口的一次失败不再让该图整个会话占位。
    #[test]
    fn failed_request_releases_dedup_key() {
        let key = "test-mid|test-url";
        assert!(account(key), "首次入账应放行");
        assert!(!account(key), "在账应拦截重复请求");
        release(key);
        assert!(account(key), "出账后应允许重试");
        release(key);
    }
}
