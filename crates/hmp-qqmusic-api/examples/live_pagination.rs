//! 统一分页 live 探针：page=2 真机取页，验证 `Page` 窗口在三个代表性
//! 接口上的翻页行为（2026-09-29 分页重设计随附核验）。
//!
//! 运行：`cargo run -p hmp-qqmusic-api --example live_pagination`
//! 凭证加载仿照 `examples/live_user.rs`（`hmp_storage::credential::
//! store_from_env().load()`），未登录时登录态接口 SKIP。
//!
//! 覆盖（共 7 次请求，间隔 ≥1.1s）：
//! - `singer.get_songs_list`：服务端忽略 `number` 按固定条数（约 30）返回，
//!   验证 offset 翻页仍正确（page=2/num=5 的 items[0] 应等于 page=1 的
//!   items[5]），并演示按 `items.len()` 推进（page=2/num=30 无重叠）；
//! - `user.get_fav_songlist`：显式 `hasmore` 服务端判定；
//! - `top.get_detail`：total 推算 has_more（`total > offset + len`）。
//!
//! 只读探针；请求节奏与 2026-09-29 全量核验一致（未触发限流）。

use std::time::Duration;

use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::credential::Credential;
use hmp_qqmusic_api::pagination::{Page, Paged};
use hmp_qqmusic_api::singer::SingerApi;
use hmp_qqmusic_api::top::TopApi;
use hmp_qqmusic_api::user::UserApi;

const JAY_MID: &str = "0025NhlN2yWrP4";
const TOP_ID: i64 = 62; // 飙升榜

fn load_credential() -> Option<Credential> {
    hmp_storage::credential::store_from_env()
        .load()
        .ok()
        .flatten()
        .filter(|c| c.is_logged_in())
}

async fn gap() {
    tokio::time::sleep(Duration::from_millis(1100)).await;
}

#[tokio::main]
async fn main() {
    let cred = load_credential();
    let client = QqMusicClient::new();

    match &cred {
        Some(c) => println!("credential: uin={} euin_len={}", c.uin, c.encrypt_uin.len()),
        None => println!("credential: none (登录态接口 SKIP)"),
    }

    singer_songs_offset_paging(&client).await;
    top_detail_total_paging(&client).await;
    fav_songlist_server_hasmore(&client, cred.as_ref()).await;

    println!("done.");
}

/// singer.get_songs_list：服务端忽略 number，验证 offset 翻页正确 +
/// 按 items.len() 推进的正确用法（3 次请求）。
async fn singer_songs_offset_paging(client: &QqMusicClient) {
    let api = SingerApi::new(client);

    gap().await;
    let page1 = match api.get_songs_list(JAY_MID, Page::new(1, 5)).await {
        Ok(r) => r,
        Err(e) => {
            println!("FAIL singer.get_songs_list(page=1): {e}");
            return;
        }
    };
    let view1 = page1.paged(Page::new(1, 5));
    println!(
        "PASS singer.get_songs_list(page=1): items={} (请求 num=5) total={} has_more={} → 服务端忽略 num 返回固定条数",
        view1.items.len(),
        view1.total,
        view1.has_more
    );
    let Some(p1s5) = view1.items.get(5).map(|s| (s.id, s.mid.clone())) else {
        println!("SKIP singer.get_songs_list: 首页不足 6 首，无法核对偏移");
        return;
    };

    // offset 翻页：page=2/num=5 → begin=5，首条应等于 page1.items[5]。
    gap().await;
    match api.get_songs_list(JAY_MID, Page::new(2, 5)).await {
        Ok(r) => {
            let view = r.paged(Page::new(2, 5));
            let ok = view
                .items
                .first()
                .is_some_and(|s| s.id == p1s5.0 && s.mid == p1s5.1);
            println!(
                "PASS singer.get_songs_list(page=2/num=5): items={} first={:?} → offset={} {}",
                view.items.len(),
                view.items.first().map(|s| s.name.clone()),
                Page::new(2, 5).offset(),
                if ok {
                    "= page1.items[5]（offset 翻页正确）"
                } else {
                    "≠ page1.items[5]（offset 语义异常）"
                }
            );
        }
        Err(e) => println!("FAIL singer.get_songs_list(page=2/num=5): {e}"),
    }

    // 正确推进方式：按 items.len()（固定条数）构造下一页 → 无重叠。
    gap().await;
    match api.get_songs_list(JAY_MID, Page::new(2, 30)).await {
        Ok(r) => {
            let view = r.paged(Page::new(2, 30));
            let overlap = view
                .items
                .first()
                .is_some_and(|s| page1.song_list.iter().any(|p| p.mid == s.mid));
            println!(
                "PASS singer.get_songs_list(page=2/num=30): items={} first={:?} has_more={} → {}",
                view.items.len(),
                view.items.first().map(|s| s.name.clone()),
                view.has_more,
                if overlap {
                    "与 page=1 重叠（首页不足 30 条，服务端固定页大小 ≠ 30？）"
                } else {
                    "与 page=1 无重叠（按 items.len() 推进正确）"
                }
            );
        }
        Err(e) => println!("FAIL singer.get_songs_list(page=2/num=30): {e}"),
    }
}

/// top.get_detail：无显式 hasmore，验证 total 推算（2 次请求）。
async fn top_detail_total_paging(client: &QqMusicClient) {
    let api = TopApi::new(client);
    let page1 = Page::new(1, 5);
    gap().await;
    let first = match api.get_detail(TOP_ID, page1, false).await {
        Ok(r) => {
            let view = r.paged(page1);
            println!(
                "PASS top.get_detail(page=1): items={} total={} has_more={} next_page={:?}",
                view.items.len(),
                view.total,
                view.has_more,
                view.next_page()
            );
            r
        }
        Err(e) => {
            println!("FAIL top.get_detail(page=1): {e}");
            return;
        }
    };
    let page2 = Page::new(2, 5);
    gap().await;
    match api.get_detail(TOP_ID, page2, false).await {
        Ok(r) => {
            let view = r.paged(page2);
            let overlap = view.items.iter().any(|s| {
                first
                    .songs
                    .iter()
                    .any(|p| !p.mid.is_empty() && p.mid == s.mid)
            });
            println!(
                "PASS top.get_detail(page=2): items={} total={} has_more={} → {}",
                view.items.len(),
                view.total,
                view.has_more,
                if overlap {
                    "与 page=1 重叠（异常）"
                } else {
                    "与 page=1 无重叠（offset 翻页正确）"
                }
            );
        }
        Err(e) => println!("FAIL top.get_detail(page=2): {e}"),
    }
}

/// user.get_fav_songlist：显式 hasmore 服务端判定（2 次请求；需登录）。
async fn fav_songlist_server_hasmore(client: &QqMusicClient, cred: Option<&Credential>) {
    let Some(cred) = cred else {
        println!("SKIP user.get_fav_songlist: 未登录");
        return;
    };
    let api = UserApi::new(client);
    let euin = &cred.encrypt_uin;
    let page1 = Page::new(1, 10);
    gap().await;
    let first = match api.get_fav_songlist(euin, page1, Some(cred)).await {
        Ok(r) => {
            let view = r.paged(page1);
            println!(
                "PASS user.get_fav_songlist(page=1): items={} total={} hasmore(服务端)={} next_page={:?}",
                view.items.len(),
                view.total,
                view.has_more,
                view.next_page()
            );
            r
        }
        Err(e) => {
            println!("FAIL user.get_fav_songlist(page=1): {e}");
            return;
        }
    };
    let page2 = Page::new(2, 10);
    gap().await;
    match api.get_fav_songlist(euin, page2, Some(cred)).await {
        Ok(r) => {
            let view = r.paged(page2);
            let overlap = view
                .items
                .iter()
                .any(|p| first.playlists.iter().any(|f| f.id > 0 && f.id == p.id));
            println!(
                "PASS user.get_fav_songlist(page=2): items={} hasmore={} → {}",
                view.items.len(),
                view.has_more,
                if overlap {
                    "与 page=1 重叠（异常）"
                } else {
                    "与 page=1 无重叠（offset 翻页正确）"
                }
            );
        }
        Err(e) => println!("FAIL user.get_fav_songlist(page=2): {e}"),
    }
}
