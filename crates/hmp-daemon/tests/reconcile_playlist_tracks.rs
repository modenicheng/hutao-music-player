//! 歌单曲目缓存 reconcile 集成测试（wiremock QQ API + 内存库，零真实出网）。
//!
//! 根因回归锚定（2026-10-02「所有歌单无法获取歌曲」）：listing reconcile 只建
//! `playlists` 行不建曲目行，本地库全部消费方（桌面歌单页/详情页、CLI
//! `playlist list/show`、`playlist:local:<id>` 播放）读 sqlite → 曲目数恒 0。
//! 本测试锚定补上的第四条腿：`reconcile_user_library` 逐歌单拉 `CgiGetDiss`
//! 快照差集合入 `playlist_tracks`，并覆盖：
//! 1. 首轮全量入列（owned 行 remote_id=dirid 经 listing 换算 tid 请求）；
//! 2. 重放幂等（同快照零重复）；
//! 3. 远端已删曲目移除 + 远端取消收藏歌单整行删除（AUDIT §4.4 FK 地雷：
//!    有曲目缓存的 subscribed 行删除前须先清子行）；
//! 4. 本地意图胜出：pending op 行在场 → 跳过出网抓快照。
//!
//! 全程 file 凭证后端 + 隔离 XDG 目录（同 e2e.rs EnvGuard 范式）。

use std::sync::{Arc, Mutex};

use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::config::ClientConfig;
use hmp_qqmusic_api::credential::Credential;
use hmp_storage::LibraryDb;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// 测试 owned 歌单：listing tid=9001 / dirid=7（remote_id 存 dirid）。
const OWNED_TID: i64 = 9001;
const OWNED_DIRID: &str = "7";
/// 测试 subscribed 歌单：disstid=8888。
const SUB_TID: i64 = 8888;

/// 解析请求体的 `req_0` 字段（模块/方法/参数匹配用）。
fn req0(req: &Request) -> serde_json::Value {
    serde_json::from_slice(&req.body).unwrap_or(serde_json::json!({}))["req_0"].clone()
}

/// 歌单快照响应（CgiGetDiss data）：给定的歌曲键值对按序入 songlist。
fn diss_data(songs: &[(&str, &str)]) -> serde_json::Value {
    diss_data_with_pmid(
        &songs
            .iter()
            .map(|(mid, name)| (*mid, *name, ""))
            .collect::<Vec<_>>(),
    )
}

/// 同 [`diss_data`]，歌曲另带专辑封面 pmid（上游 `album.logo`）——封面
/// 入库回归（reconcile 带封面落地）的输入形态。
fn diss_data_with_pmid(songs: &[(&str, &str, &str)]) -> serde_json::Value {
    let songlist: Vec<serde_json::Value> = songs
        .iter()
        .enumerate()
        .map(|(i, (mid, name, pmid))| {
            serde_json::json!({
                "id": 1000 + i,
                "mid": mid,
                "name": name,
                "type": 1,
                "singer": [{"singerName": format!("歌手{i}")}],
                "album": {"albumName": format!("专辑{i}"), "logo": pmid},
                "interval": 180 + i as i64,
            })
        })
        .collect();
    serde_json::json!({
        "code": 0,
        "dirinfo": {"dissid": 0, "dissname": "快照歌单", "songnum": songs.len()},
        "songlist": songlist,
        "songlist_size": songs.len(),
        "total_song_num": songs.len(),
        "hasmore": 0,
    })
}

/// musicu 信封：`req_0.data` 载荷。
fn envelope(data: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "code": 0, "req_0": { "code": 0, "data": data } })
}

/// 计数响应器：第 1 次（或前 `first_n` 次）返回 first，其后返回 rest。
/// 依赖 wiremock 按挂载序匹配的确定性会引入脆弱性，改由响应器内建状态。
fn counting_responder(
    first: serde_json::Value,
    rest: serde_json::Value,
) -> impl Fn(&Request) -> ResponseTemplate {
    let calls = Arc::new(Mutex::new(0u32));
    move |_req| {
        let mut n = calls.lock().unwrap();
        *n += 1;
        let body = if *n == 1 { &first } else { &rest };
        ResponseTemplate::new(200).set_body_json(body.clone())
    }
}

/// CgiGetDiss 匹配：param.disstid == tid。
fn diss_by_disstid(tid: i64) -> impl Fn(&Request) -> bool {
    move |req: &Request| req0(req)["param"]["disstid"] == serde_json::json!(tid)
}

/// CgiGetDiss 匹配：「我喜欢」形态（param.dirid == 201，get_fav_song）。
fn diss_fav_song() -> impl Fn(&Request) -> bool {
    |req: &Request| req0(req)["param"]["dirid"] == serde_json::json!(201)
}

/// listing 响应（GetPlaylistByUin data）：owned 测试歌单（tid/dirId 双 ID）。
fn created_listing() -> serde_json::Value {
    serde_json::json!({
        "songlist": [{
            "tid": OWNED_TID,
            "dirId": 7,
            "dissname": "测试歌单",
            "songnum": 2,
            "logo": "",
        }],
        "bFinish": true,
    })
}

/// 收藏歌单 listing（PlaylistFavRead data）：`subscribed` 参数控制 8888 在场。
fn fav_listing(subscribed: bool) -> serde_json::Value {
    let v_list = if subscribed {
        serde_json::json!([{
            "tid": SUB_TID,
            "dissname": "收藏单",
            "songnum": 1,
            "logo": "",
        }])
    } else {
        serde_json::json!([])
    };
    serde_json::json!({ "number": v_list.as_array().map_or(0, |l| l.len()), "hasmore": 0, "v_list": v_list })
}

/// 挂载四个 listing/空壳 mock（不变部分）；歌单快照由各测试按场景自挂。
async fn mount_listings(server: &MockServer) {
    // 自建歌单 listing（owned：tid=9001 / dirid=7）。
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &Request| {
            req0(req)["module"] == serde_json::json!("music.musicasset.PlaylistBaseRead")
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(created_listing())))
        .mount(server)
        .await;
    // 收藏歌单 listing（subscribed：8888 首轮在场）。
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &Request| {
            req0(req)["module"] == serde_json::json!("music.musicasset.PlaylistFavRead")
        })
        .respond_with(counting_responder(
            envelope(fav_listing(true)),
            envelope(fav_listing(false)),
        ))
        .mount(server)
        .await;
    // 收藏专辑 listing：空。
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &Request| {
            req0(req)["module"] == serde_json::json!("music.musicasset.AlbumFavRead")
        })
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(envelope(serde_json::json!({ "hasmore": 0, "v_list": [] }))),
        )
        .mount(server)
        .await;
    // 「我喜欢」（dirid=201 CgiGetDiss）：空快照（hasmore=0 即止）。
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(diss_fav_song())
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(diss_data(&[]))))
        .mount(server)
        .await;
}

/// 隔离环境（file 凭证 + 临时 XDG 目录），进程级还原。
struct EnvGuard {
    backend: Option<std::ffi::OsString>,
    xdg_config: Option<std::ffi::OsString>,
    xdg_cache: Option<std::ffi::OsString>,
}

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

impl EnvGuard {
    fn install(dir: &std::path::Path) -> Self {
        let backend = std::env::var_os("HMP_CREDENTIAL_BACKEND");
        let xdg_config = std::env::var_os("XDG_CONFIG_HOME");
        let xdg_cache = std::env::var_os("XDG_CACHE_HOME");
        unsafe {
            std::env::set_var("HMP_CREDENTIAL_BACKEND", "file");
            std::env::set_var("XDG_CONFIG_HOME", dir);
            std::env::set_var("XDG_CACHE_HOME", dir.join("cache"));
        }
        Self {
            backend,
            xdg_config,
            xdg_cache,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.backend {
                Some(v) => std::env::set_var("HMP_CREDENTIAL_BACKEND", v),
                None => std::env::remove_var("HMP_CREDENTIAL_BACKEND"),
            }
            match &self.xdg_config {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match &self.xdg_cache {
                Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
                None => std::env::remove_var("XDG_CACHE_HOME"),
            }
        }
    }
}

fn save_credential(dir: &std::path::Path) {
    let _ = dir;
    let store: Box<dyn hmp_storage::credential::CredentialStore> =
        hmp_storage::credential::store_from_env();
    store
        .save(&Credential {
            uin: "10001".into(),
            music_id: "10001".into(),
            music_key: "secret-key".into(),
            str_musicid: "10001".into(),
            encrypt_uin: "e-10001".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(
        hmp_storage::config_dir().join("credential.json").exists(),
        "file 凭证应落在 $XDG_CONFIG_HOME/hmp/credential.json"
    );
}

fn client_for(base_url: &str) -> QqMusicClient {
    QqMusicClient::with_config(ClientConfig {
        base_url: base_url.to_owned(),
        ..Default::default()
    })
}

/// 本轮收到的 CgiGetDiss 快照请求的 disstid 集合（dirid=201 形态除外）。
async fn requested_disstids(server: &MockServer) -> Vec<i64> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|req| {
            let body = req0(req);
            (body["module"] == serde_json::json!("music.srfDissInfo.DissInfo"))
                .then(|| body["param"]["disstid"].as_i64())
                .flatten()
                .filter(|tid| *tid > 0)
        })
        .collect()
}

fn owned_row(db: &mut LibraryDb) -> hmp_storage::PlaylistRow {
    db.list_playlists()
        .unwrap()
        .into_iter()
        .find(|p| p.relation == "owned")
        .expect("owned 歌单行应存在")
}

/// 封面入库回归（2026-10-03「大部分歌曲封面显示不出来」根因之一）：
/// 快照歌曲带专辑 pmid → `tracks.cover_uri` 落 T002 模板 URL（此前恒 NULL，
/// 库内曲目无任何封面线索，桌面列表只能程序化占位）；空 pmid → NULL。
/// rebind 成 file:// 的行再次 reconcile 不得被远程 URL 降级（upsert CASE 保护）。
#[tokio::test]
async fn reconcile_tracks_carry_cover_url_and_rebind_survives_resync() {
    let _lock = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvGuard::install(dir.path());
    save_credential(dir.path());

    let server = MockServer::start().await;
    mount_listings(&server).await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(diss_by_disstid(OWNED_TID))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(diss_data_with_pmid(&[
                ("m-1", "歌一", "alPmid1"),
                ("m-2", "歌二", ""),
            ]))),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
    let credential = {
        let store: Box<dyn hmp_storage::credential::CredentialStore> =
            hmp_storage::credential::store_from_env();
        store.load().unwrap().expect("凭证已落盘")
    };

    hmp_daemon::reconcile::reconcile_user_library(&client, &credential, &lib).await;

    let url_m1 = "https://y.gtimg.cn/music/photo_new/T002R300x300M000alPmid1.jpg";
    let local = "file://C:/covers/aa.jpg";
    {
        let mut db = lib.lock().unwrap();
        let meta = db.track_meta_batch("qq", &["m-1".to_owned(), "m-2".to_owned()]).unwrap();
        assert_eq!(
            meta.iter().find(|m| m.source_key == "m-1").unwrap().cover_uri.as_deref(),
            Some(url_m1),
            "pmid 应套 T002 模板入库"
        );
        assert!(
            meta.iter()
                .find(|m| m.source_key == "m-2")
                .unwrap()
                .cover_uri
                .is_none(),
            "空 pmid 不落封面"
        );
        // 模拟 UI 经 CoverGet 取图后的 rebind 回写
        db.rebind_cover_url(url_m1, local).unwrap();
    }

    // 二轮 reconcile（同快照重放）→ file:// 不得被远程 URL 打回
    hmp_daemon::reconcile::reconcile_user_library(&client, &credential, &lib).await;
    let mut db = lib.lock().unwrap();
    let meta = db.track_meta_batch("qq", &["m-1".to_owned()]).unwrap();
    assert_eq!(
        meta[0].cover_uri.as_deref(),
        Some(local),
        "rebind 后的本地产物在重放 reconcile 后仍保留"
    );
}

#[tokio::test]
async fn reconcile_caches_playlist_tracks_and_replays_idempotently() {
    let _lock = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvGuard::install(dir.path());
    save_credential(dir.path());

    let server = MockServer::start().await;
    mount_listings(&server).await;
    // owned 歌单快照（disstid=9001）：2 首。
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(diss_by_disstid(OWNED_TID))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(envelope(diss_data(&[("m-1", "歌一"), ("m-2", "歌二")]))),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
    let credential = {
        let store: Box<dyn hmp_storage::credential::CredentialStore> =
            hmp_storage::credential::store_from_env();
        store.load().unwrap().expect("凭证已落盘")
    };

    hmp_daemon::reconcile::reconcile_user_library(&client, &credential, &lib).await;

    {
        let mut db = lib.lock().unwrap();
        let row = owned_row(&mut db);
        assert_eq!(
            row.remote_id.as_deref(),
            Some(OWNED_DIRID),
            "remote_id 存 dirid"
        );
        assert_eq!(row.track_count, 2, "首轮全量入列");
        let keys: Vec<_> = db
            .playlist_tracks(row.id)
            .unwrap()
            .into_iter()
            .map(|r| r.source_key)
            .collect();
        assert_eq!(keys, ["m-1", "m-2"], "远端顺序");
        let meta = db.track_meta_batch("qq", &["m-1".to_owned()]).unwrap();
        assert_eq!(meta[0].title, "歌一");
        assert_eq!(meta[0].artist.as_deref(), Some("歌手0"));
        assert_eq!(meta[0].album.as_deref(), Some("专辑0"));
        assert_eq!(meta[0].duration_ms, Some(180_000));
    }

    // 快照请求确实以 listing 换算的 tid 发起（disstid=9001，而非 dirid=7）。
    assert!(
        requested_disstids(&server).await.contains(&OWNED_TID),
        "曲目快照应按 tid 请求 CgiGetDiss"
    );

    // 重放：同快照零重复、零误删。
    hmp_daemon::reconcile::reconcile_user_library(&client, &credential, &lib).await;
    let mut db = lib.lock().unwrap();
    let row = owned_row(&mut db);
    assert_eq!(row.track_count, 2, "重放幂等");
}

#[tokio::test]
async fn reconcile_diffs_remote_removal_and_purges_unsubscribed_playlists() {
    let _lock = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvGuard::install(dir.path());
    save_credential(dir.path());

    let server = MockServer::start().await;
    mount_listings(&server).await;
    // owned 快照：首轮 2 首，其后 1 首（远端删了 m-2）。
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(diss_by_disstid(OWNED_TID))
        .respond_with(counting_responder(
            envelope(diss_data(&[("m-1", "歌一"), ("m-2", "歌二")])),
            envelope(diss_data(&[("m-1", "歌一")])),
        ))
        .mount(&server)
        .await;
    // subscribed 快照（disstid=8888）：1 首（仅首轮消费）。
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(diss_by_disstid(SUB_TID))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(diss_data(&[("m-8", "歌八")]))),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
    let credential = {
        let store: Box<dyn hmp_storage::credential::CredentialStore> =
            hmp_storage::credential::store_from_env();
        store.load().unwrap().expect("凭证已落盘")
    };

    // 第一轮：owned 2 首；subscribed 8888 入列并缓存 1 首。
    hmp_daemon::reconcile::reconcile_user_library(&client, &credential, &lib).await;
    {
        let mut db = lib.lock().unwrap();
        let sub = db
            .list_playlists()
            .unwrap()
            .into_iter()
            .find(|p| p.relation == "subscribed")
            .expect("subscribed 行应存在");
        assert_eq!(sub.remote_id.as_deref(), Some("8888"));
        assert_eq!(sub.track_count, 1);
    }

    // 第二轮：listing 不再含 8888（远端取消收藏）+ owned 快照收窄。
    // FK 地雷：8888 行有曲目缓存，delete_playlists_absent 删父行前必须清子行。
    hmp_daemon::reconcile::reconcile_user_library(&client, &credential, &lib).await;
    {
        let mut db = lib.lock().unwrap();
        let rows = db.list_playlists().unwrap();
        assert!(
            !rows.iter().any(|p| p.relation == "subscribed"),
            "远端取消收藏的 subscribed 行应被删除（子行已前置清理，不 FK 失败）"
        );
        let row = owned_row(&mut db);
        assert_eq!(row.track_count, 1, "远端已删曲目应移除");
        let keys: Vec<_> = db
            .playlist_tracks(row.id)
            .unwrap()
            .into_iter()
            .map(|r| r.source_key)
            .collect();
        assert_eq!(keys, ["m-1"]);
    }
}

#[tokio::test]
async fn reconcile_skips_playlists_with_pending_local_intent() {
    let _lock = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvGuard::install(dir.path());
    save_credential(dir.path());

    let server = MockServer::start().await;
    mount_listings(&server).await;
    // 若守卫失效会请求 disstid=9001；故意不挂 9001 快照 mock，
    // 断言只看 received_requests（守卫应让请求根本不发生）。

    let client = client_for(&server.uri());
    let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
    let credential = {
        let store: Box<dyn hmp_storage::credential::CredentialStore> =
            hmp_storage::credential::store_from_env();
        store.load().unwrap().expect("凭证已落盘")
    };
    // 本地意图：owned 行（dirid=7）+ 未消费的曲目追加 op。
    let row_id = {
        let mut db = lib.lock().unwrap();
        let row_id = db
            .reconcile_playlist(OWNED_DIRID, "本地优先", "owned")
            .unwrap();
        db.enqueue_playlist_op(row_id, "add", Some("mid-x"), None)
            .unwrap();
        row_id
    };

    hmp_daemon::reconcile::reconcile_user_library(&client, &credential, &lib).await;

    assert!(
        !requested_disstids(&server).await.contains(&OWNED_TID),
        "pending 意图在场：不应出网拉该歌单快照"
    );
    let mut db = lib.lock().unwrap();
    assert!(
        db.playlist_tracks(row_id).unwrap().is_empty(),
        "本地胜出：远端快照不得改写本地曲目"
    );
    // 正面路径（无本地意图的歌单正常补抓）由前两个测试覆盖，此处不再重复。
}
