//! 诊断探针（2026-10-02「远程曲目无法播放」排查）：
//! 1. dump 线上 `track_info.file` 原始 JSON（核对 models::File alias）；
//! 2. 以 keyring 凭证逐档取流（回退链全档位），打印每档 result/ekey
//!    ——定位「音质不存在」是哪一档、什么码（全档 104003 = 凭证失效）。
//!
//! 只读，无写操作，不调用 logout/refresh_credential。
//! 运行：`cargo run -p hmp-qqmusic-api --example quality_dump -- [urls]`。

use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::protocol::cgi::CgiRequest;
use hmp_qqmusic_api::song::{SongApi, SongFileInfo, SongFileType};
use serde_json::json;

/// 样本：孙燕姿《开始懂了》（song_mid=001qHjVZ4SfmWQ，media_mid=003BEgWZ2eI1Qo）。
const SONG_MID: &str = "001qHjVZ4SfmWQ";
const MEDIA_MID: &str = "003BEgWZ2eI1Qo";

fn load_credential() -> Option<hmp_storage::credential::Credential> {
    hmp_storage::credential::store_from_env()
        .load()
        .ok()
        .flatten()
        .filter(|c| c.is_logged_in())
}

async fn dump_file(client: &QqMusicClient) {
    for (label, param) in [
        ("开始懂了", json!({"song_id": 186016})),
        ("晴天", json!({"song_id": 97773})),
    ] {
        let req = CgiRequest::new("music.pf_song_detail_svr", "get_song_detail_yqq", param);
        match client.musicu_request(&req, None).await {
            Ok(sub) => {
                let file = &sub["data"]["track_info"]["file"];
                println!("== {label} file keys ==");
                if let Some(obj) = file.as_object() {
                    let mut keys: Vec<_> = obj.iter().collect();
                    keys.sort_by(|a, b| a.0.cmp(b.0));
                    for (k, v) in keys {
                        println!("  {k} = {v}");
                    }
                } else {
                    println!("  (no file object) {file}");
                }
            }
            Err(e) => println!("FAIL {label}: {e}"),
        }
    }
}

/// 逐档取流（带凭证）：回退链映射的全档位 + 免登录对照。
async fn probe_rungs(client: &QqMusicClient, cred: Option<&hmp_storage::credential::Credential>) {
    let song = SongApi::new(client);
    let file_info = SongFileInfo {
        mid: SONG_MID.into(),
        file_type: None,
        song_type: 0,
        media_mid: Some(MEDIA_MID.into()),
    };
    // 与 player.rs quality_to_file_type 同表：回退链档位 → SongFileType。
    // （Master/HiRes 同映射 AIM0；Aac 不在文档化链内，附带核对。）
    let rungs: [(&str, SongFileType); 7] = [
        ("master/hires(AIM0)", SongFileType::MASTER),
        ("atmos(Q0M0)", SongFileType::ATMOS_2),
        ("flac(F0M0)", SongFileType::FLAC),
        ("aac(C600)", SongFileType::AAC_192),
        ("320(M800)", SongFileType::MP3_320),
        ("128(M500)", SongFileType::MP3_128),
        ("try(RS02,免登录对照)", SongFileType::TRY),
    ];
    println!(
        "== 逐档取流 credential={:?} ==",
        cred.map(|c| c.uin.clone())
    );
    for (label, ft) in rungs {
        match song
            .get_song_urls(std::slice::from_ref(&file_info), ft, cred)
            .await
        {
            Ok(resp) => {
                let item = &resp.data[0];
                println!(
                    "  {label}: result={} purl_empty={} ekey_len={} filename={}",
                    item.result,
                    item.purl.is_empty(),
                    item.ekey.len(),
                    item.filename
                );
            }
            Err(e) => println!("  {label}: REQUEST FAIL {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    }
}

#[tokio::main]
async fn main() {
    let client = QqMusicClient::new();
    let urls = std::env::args().any(|a| a == "urls");
    if urls {
        let cred = load_credential();
        probe_rungs(&client, cred.as_ref()).await;
        return;
    }
    dump_file(&client).await;
}
