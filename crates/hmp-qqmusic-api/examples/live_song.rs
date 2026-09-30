//! song/lyric/singer 域 live 探针（真机核验全部 pub API）。
//!
//! 运行：`cargo run -p hmp-qqmusic-api --example live_song`（全量）
//! 可选模式（互斥，控制请求量）：
//! - `stream`：仅取流三连（TRY 免登录 / MP3_128 免登录 / FLAC 登录 EVkey）+ Range 64KB；
//! - `dump`：仅 song 详情原始键结构 dump + 类型化提取结果。
//!
//! 样本：孙燕姿《开始懂了》（id=186016，media_mid=003BEgWZ2eI1Qo）、
//! 周杰伦（mid=0025NhlN2yWrP4）、周杰伦《晴天》（id=97773，QRC 逐字歌词）。
//! 取流核验只 Range GET 前 64KB；请求间隔 ≥1.1s；只读，无写操作，
//! 绝不调用 logout/refresh_credential。

use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::credential::Credential;
use hmp_qqmusic_api::lyric::LyricApi;
use hmp_qqmusic_api::pagination::Page;
use hmp_qqmusic_api::protocol::cgi::CgiRequest;
use hmp_qqmusic_api::singer::{AreaType, GenreType, IndexType, SexType, SingerApi, TabType};
use hmp_qqmusic_api::song::{SongApi, SongFileInfo, SongFileType, SongQueryInfo};
use serde_json::json;

/// 样本歌曲 MID（开始懂了）。
const SONG_MID: &str = "001qHjVZ4SfmWQ";
/// 样本歌曲数字 ID。
const SONG_ID: i64 = 186016;
/// 样本歌曲媒体 MID（CDN 文件名使用，实测 doubled song-mid 文件名 404）。
const MEDIA_MID: &str = "003BEgWZ2eI1Qo";
/// QRC 样本歌曲 ID（晴天）。
const QRC_SONG_ID: i64 = 97773;
/// 样本歌手 MID（周杰伦）。
const SINGER_MID: &str = "0025NhlN2yWrP4";
/// 请求间隔。
const GAP: std::time::Duration = std::time::Duration::from_millis(1100);

fn load_credential() -> Option<Credential> {
    hmp_storage::credential::store_from_env()
        .load()
        .ok()
        .flatten()
        .filter(|c| c.is_logged_in())
}

async fn gap() {
    tokio::time::sleep(GAP).await;
}

/// Range GET 前 64KB，返回 (HTTP 状态, 实读字节数, 前 8 字节 hex)。
async fn range_get_64k(url: &str) -> (u16, usize, String) {
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header("Range", "bytes=0-65535")
        .header("Referer", "https://y.qq.com/")
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await;
    let mut resp = match resp {
        Ok(r) => r,
        Err(e) => return (0, 0, format!("ERR {e}")),
    };
    let status = resp.status().as_u16();
    let mut head = Vec::new();
    while head.len() < 65536 {
        match resp.chunk().await {
            Ok(Some(chunk)) => head.extend_from_slice(&chunk),
            _ => break,
        }
    }
    let head_hex: String = head.iter().take(8).map(|b| format!("{b:02X}")).collect();
    (status, head.len(), head_hex)
}

#[tokio::main]
async fn main() {
    let dump = std::env::args().any(|a| a == "dump");
    let stream = std::env::args().any(|a| a == "stream");
    let cred = load_credential();
    let client = QqMusicClient::new();

    match &cred {
        Some(c) => println!("credential: uin={} euin_len={}", c.uin, c.encrypt_uin.len()),
        None => println!("credential: none (login-required APIs will SKIP)"),
    }

    if stream {
        run_stream(&client, cred.as_ref()).await;
        return;
    }
    if dump {
        run_detail_dump(&client).await;
        return;
    }

    let song = SongApi::new(&client);
    let lyric = LyricApi::new(&client);
    let singer = SingerApi::new(&client);

    // ---------- song ----------
    match song.get_detail(&SONG_ID.to_string()).await {
        Ok(r) => println!(
            "PASS song.get_detail(by id): track={}-{} singers={} company={} genre={} intro={} lan={} pub_time={}",
            r.track.id,
            r.track.name,
            r.track.singer.len(),
            r.company.len(),
            r.genre.len(),
            r.intro.len(),
            r.lan.len(),
            r.pub_time.len()
        ),
        Err(e) => println!("FAIL song.get_detail(by id): {e}"),
    }
    gap().await;

    match song.get_detail(SONG_MID).await {
        Ok(r) => println!(
            "PASS song.get_detail(by mid): track={}-{} media_mid={}",
            r.track.id, r.track.name, r.track.file.media_mid
        ),
        Err(e) => println!("FAIL song.get_detail(by mid): {e}"),
    }
    gap().await;

    match song
        .query_song(&[SongQueryInfo {
            id: None,
            mid: Some(SONG_MID.into()),
            song_type: 0,
        }])
        .await
    {
        Ok(t) => println!(
            "PASS song.query_song(by mid): tracks={} first={:?}/{:?}",
            t.len(),
            t.first().map(|s| s.id),
            t.first().map(|s| s.name.clone())
        ),
        Err(e) => println!("FAIL song.query_song(by mid): {e}"),
    }
    gap().await;

    match song
        .query_song(&[SongQueryInfo {
            id: Some(SONG_ID),
            mid: None,
            song_type: 0,
        }])
        .await
    {
        Ok(t) => println!(
            "PASS song.query_song(by id): tracks={} first_mid={:?}",
            t.len(),
            t.first().map(|s| s.mid.clone())
        ),
        Err(e) => println!("FAIL song.query_song(by id): {e}"),
    }
    gap().await;

    // ---------- 取流三连（shared：stream 模式与全量模式共用） ----------
    run_stream(&client, cred.as_ref()).await;

    // ---------- lyric ----------
    match lyric
        .get_lyric(&SONG_ID.to_string(), 0, true, false, false, false)
        .await
    {
        Ok(r) => println!(
            "PASS lyric.get_lyric(186016 qrc): songid={} lrc_t={} qrc_t={} lyric_prefix={:?} trans_len={} roma_len={}",
            r.songid,
            r.lrc_t,
            r.qrc_t,
            prefix(&r.lyric, 40),
            r.trans.len(),
            r.roma.len()
        ),
        Err(e) => println!("FAIL lyric.get_lyric(186016 qrc): {e}"),
    }
    gap().await;

    match lyric
        .get_lyric(&QRC_SONG_ID.to_string(), 0, true, false, false, false)
        .await
    {
        Ok(r) => println!(
            "PASS lyric.get_lyric(97773 晴天 qrc): songid={} lyric_prefix={:?} contains_xml={}",
            r.songid,
            prefix(&r.lyric, 40),
            r.lyric.contains("[ti:")
        ),
        Err(e) => println!("FAIL lyric.get_lyric(97773 qrc): {e}"),
    }
    gap().await;

    // ---------- singer ----------
    match singer
        .get_singer_list(AreaType::All, SexType::All, GenreType::All)
        .await
    {
        Ok(r) => println!(
            "PASS singer.get_singer_list: code={} list={} hotlist={}",
            r.code,
            r.singerlist.len(),
            r.hotlist.len()
        ),
        Err(e) => println!("FAIL singer.get_singer_list: {e}"),
    }
    gap().await;

    match singer
        .get_singer_list_index(
            AreaType::All,
            SexType::All,
            GenreType::All,
            IndexType::All,
            Page::new(1, 80),
        )
        .await
    {
        Ok(r) => println!(
            "PASS singer.get_singer_list_index: base.code={} list={} total={}",
            r.base.code,
            r.base.singerlist.len(),
            r.total
        ),
        Err(e) => println!("FAIL singer.get_singer_list_index: {e}"),
    }
    gap().await;

    match singer.get_info(SINGER_MID).await {
        Ok(r) => println!(
            "PASS singer.get_info: status={} singer={}-{} base_name={}",
            r.status, r.singer.id, r.singer.name, r.base_info.name
        ),
        Err(e) => println!("FAIL singer.get_info: {e}"),
    }
    gap().await;

    match singer
        .get_tab_detail(SINGER_MID, TabType::Song, Page::new(1, 5))
        .await
    {
        Ok(r) => println!(
            "PASS singer.get_tab_detail(song): tab_id={} songs={} has_more={}",
            r.tab_id,
            r.song_tab.len(),
            r.has_more
        ),
        Err(e) => println!("FAIL singer.get_tab_detail(song): {e}"),
    }
    gap().await;

    match singer
        .get_desc(&[SINGER_MID.to_string()], false, false, false, true, false)
        .await
    {
        Ok(r) => println!(
            "PASS singer.get_desc(min param): list={} name={:?} pic={}",
            r.singer_list.len(),
            r.singer_list.first().map(|d| d.basic_info.name.clone()),
            r.singer_list
                .first()
                .map(|d| d.pic.pic.is_empty())
                .unwrap_or(true)
        ),
        Err(e) => println!("FAIL singer.get_desc(min param): {e}"),
    }
    gap().await;

    match singer
        .get_desc(&[SINGER_MID.to_string()], true, true, true, true, true)
        .await
    {
        Ok(r) => println!(
            "PASS singer.get_desc(all true): list={} desc_len={}",
            r.singer_list.len(),
            r.singer_list
                .first()
                .map(|d| d.ex_info.desc.len())
                .unwrap_or(0)
        ),
        Err(e) => {
            println!("FAIL singer.get_desc(all true)（历史记录 ex/group 扩展参数可能 10006）: {e}")
        }
    }
    gap().await;

    match singer.get_similar(SINGER_MID, 5).await {
        Ok(r) => println!(
            "PASS singer.get_similar: code={} list={} err={:?}",
            r.code,
            r.singerlist.len(),
            r.err_msg
        ),
        Err(e) => println!("FAIL singer.get_similar: {e}"),
    }
    gap().await;

    match singer.get_songs_list(SINGER_MID, Page::new(1, 5)).await {
        Ok(r) => println!(
            "PASS singer.get_songs_list: singer_mid={} total={} songs={} first={:?}",
            r.singer_mid,
            r.total_num,
            r.song_list.len(),
            r.song_list.first().map(|s| s.name.clone())
        ),
        Err(e) => println!("FAIL singer.get_songs_list: {e}"),
    }
    gap().await;

    match singer.get_album_list(SINGER_MID, Page::new(1, 5)).await {
        Ok(r) => println!(
            "PASS singer.get_album_list: total={} albums={} first={:?}",
            r.total,
            r.album_list.len(),
            r.album_list.first().map(|a| a.album.name.clone())
        ),
        Err(e) => println!("FAIL singer.get_album_list: {e}"),
    }
    gap().await;

    match singer.get_mv_list(SINGER_MID, Page::new(1, 5)).await {
        Ok(r) => println!(
            "PASS singer.get_mv_list: total={} mvs={} first_vid={:?}",
            r.total,
            r.mv_list.len(),
            r.mv_list.first().map(|v| v.vid.clone())
        ),
        Err(e) => println!("FAIL singer.get_mv_list: {e}"),
    }

    println!("done.");
}

/// 取流三连：TRY 免登录 / MP3_128 免登录 / FLAC 登录 EVkey，各接一次 Range 64KB。
/// 文件名一律带 media_mid（实测与官方客户端一致；doubled song-mid 文件名 CDN 404）。
async fn run_stream(client: &QqMusicClient, cred: Option<&Credential>) {
    let song = SongApi::new(client);

    // 取流 1：RS02 试听（免登录）
    let try_url = match song
        .get_song_urls(
            &[SongFileInfo {
                mid: SONG_MID.into(),
                file_type: None,
                song_type: 0,
                media_mid: Some(MEDIA_MID.into()),
            }],
            SongFileType::TRY,
            None,
        )
        .await
    {
        Ok(r) => {
            let item = &r.data[0];
            println!(
                "PASS song.get_song_urls(TRY anon): result={} filename={} purl_prefix={} ekey_len={}",
                item.result,
                item.filename,
                &item.purl[..item.purl.len().min(40)],
                item.ekey.len()
            );
            r.build_urls().into_iter().flatten().next()
        }
        Err(e) => {
            println!("FAIL song.get_song_urls(TRY anon): {e}");
            None
        }
    };
    gap().await;
    if let Some(u) = &try_url {
        let (status, n, head) = range_get_64k(u).await;
        println!("RANGE try url: status={status} bytes={n} head8={head} (期望 206/200 且 >0)");
        gap().await;
    }

    // 取流 2：MP3_128 完整音质（免登录，预期 result=104003 无权限、purl 空）
    match song
        .get_song_urls(
            &[SongFileInfo {
                mid: SONG_MID.into(),
                file_type: None,
                song_type: 0,
                media_mid: Some(MEDIA_MID.into()),
            }],
            SongFileType::MP3_128,
            None,
        )
        .await
    {
        Ok(r) => println!(
            "PASS song.get_song_urls(MP3_128 anon): result={} purl_empty={}",
            r.data[0].result,
            r.data[0].purl.is_empty()
        ),
        Err(e) => println!("FAIL song.get_song_urls(MP3_128 anon): {e}"),
    }
    gap().await;

    // 取流 3：加密 FLAC（需登录，验证 ekey 与 Range 可读）
    let flac_url = if let Some(cred) = cred {
        match song
            .get_song_urls(
                &[SongFileInfo {
                    mid: SONG_MID.into(),
                    file_type: None,
                    song_type: 0,
                    media_mid: Some(MEDIA_MID.into()),
                }],
                SongFileType::FLAC,
                Some(cred),
            )
            .await
        {
            Ok(r) => {
                let item = &r.data[0];
                println!(
                    "PASS song.get_song_urls(FLAC login EVkey): result={} filename={} ekey_len={} expiration={}",
                    item.result,
                    item.filename,
                    item.ekey.len(),
                    r.expiration
                );
                if item.ekey.is_empty() {
                    println!("WARN ekey 为空（VIP 权限或 CgiGetEVkey 返回需核对）");
                }
                r.build_urls().into_iter().flatten().next()
            }
            Err(e) => {
                println!("FAIL song.get_song_urls(FLAC login EVkey): {e}");
                None
            }
        }
    } else {
        println!("SKIP song.get_song_urls(FLAC login EVkey): no credential");
        None
    };
    gap().await;
    if let Some(u) = &flac_url {
        let (status, n, head) = range_get_64k(u).await;
        println!("RANGE flac url: status={status} bytes={n} head8={head}");
    }
}

/// song 详情原始键结构 dump + 类型化提取结果（核对 `info.*.content`）。
async fn run_detail_dump(client: &QqMusicClient) {
    let req = CgiRequest::new(
        "music.pf_song_detail_svr",
        "get_song_detail_yqq",
        json!({"song_id": SONG_ID}),
    );
    match client.musicu_request(&req, None).await {
        Ok(sub) => {
            let data = &sub["data"];
            println!("DUMP song.detail data keys: {:?}", keys_of(data));
            println!("DUMP song.detail info keys: {:?}", keys_of(&data["info"]));
            println!(
                "DUMP song.detail info.company keys: {:?}",
                keys_of(&data["info"]["company"])
            );
            println!(
                "DUMP song.detail info.company.content[0]: {}",
                data["info"]["company"]["content"][0]
            );
            println!(
                "DUMP song.detail extras keys: {:?}",
                keys_of(&data["extras"])
            );
        }
        Err(e) => println!("DUMP song.detail raw FAIL: {e}"),
    }
    gap().await;
    let song = SongApi::new(client);
    match song.get_detail(&SONG_ID.to_string()).await {
        Ok(r) => println!(
            "TYPED song.get_detail: company={} genre={} intro={} lan={} pub_time={} first_intro={:?}",
            r.company.len(),
            r.genre.len(),
            r.intro.len(),
            r.lan.len(),
            r.pub_time.len(),
            r.intro.first().map(|c| c.value.clone())
        ),
        Err(e) => println!("TYPED song.get_detail FAIL: {e}"),
    }
}

fn keys_of(v: &serde_json::Value) -> Vec<String> {
    v.as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn prefix(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
