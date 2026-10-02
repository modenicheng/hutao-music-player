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
//! 进程内同图只发一次，失败不重试——下次进页面由 daemon 盘缓存毫秒回。

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;

/// 并发上限：daemon 命中盘索引时极快，未命中时打满 CDN 的在途请求数也
/// 不至于触发限流；大歌单（2000 首）分钟级渐进填满，全程不阻塞 UI。
const MAX_INFLIGHT: usize = 8;

thread_local! {
    /// 已发起过的 mid|url（完成或失败均不重发，进程级）。
    static REQUESTED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// 列表落地后的预取入口（纯 IPC：出网与落库全在 daemon 侧，UI 线程只派发）。
///
/// `targets`：(mid, 远程封面 URL) 对——QQ 行的 `remote_cover` 投影，或在线页
/// 行的 picurl。
pub fn prefetch_tracks(
    runtime: &Arc<crate::backend::BackendRuntime>,
    targets: Vec<(String, String)>,
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
        // 进程内去重：同图只发一次（完成或失败均不重发）
        let key = format!("{mid}|{url}");
        if REQUESTED.with(|set| !set.borrow_mut().insert(key)) {
            continue;
        }
        let permits = semaphore.clone();
        let runtime = runtime.clone();
        runtime.spawn(async move {
            let _permit = permits.acquire_owned().await.expect("semaphore open");
            // 响应不回 UI（无图可回填）；daemon 侧完成盘缓存登记与 rebind
            let _ = crate::backend::request(hmp_core::Request::CoverGet { url }).await;
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
}
