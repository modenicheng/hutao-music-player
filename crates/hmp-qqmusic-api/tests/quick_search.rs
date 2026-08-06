//! `quick_search`（上游 `SearchApi.quick_search` → smartbox_new.fcg GET）测试。
//!
//! fixture `tests/fixtures/search/quick_song.json` 为真实录制响应。

use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::config::ClientConfig;
use hmp_qqmusic_api::protocol::search::QuickSearch;
use serde_json::json;
use wiremock::matchers::{method, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn load_fixture(name: &str) -> serde_json::Value {
    let path = format!(
        "{}/tests/fixtures/search/{}",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    let text = std::fs::read_to_string(path).expect("fixture file");
    serde_json::from_str(&text).expect("fixture JSON")
}

fn client_for(base_url: &str) -> QqMusicClient {
    let config = ClientConfig {
        base_url: base_url.to_owned(),
        content_base_url: base_url.to_owned(),
        ..Default::default()
    };
    QqMusicClient::with_config(config)
}

#[test]
fn parses_recorded_quick_search_fixture() {
    let body = load_fixture("quick_song.json");
    let quick = QuickSearch::from_value(&body).expect("parse fixture");

    assert_eq!(quick.songs.len(), 4);
    let first = &quick.songs[0];
    assert_eq!(first.mid, "0039MnYb0qxYhV");
    assert_eq!(first.name, "晴天");
    assert_eq!(first.singer, "周杰伦");

    assert_eq!(quick.albums.len(), 2);
    assert_eq!(quick.singers.len(), 2);
}

#[tokio::test]
async fn quick_search_hits_smartbox_endpoint() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("key", "周杰伦"))
        .respond_with(ResponseTemplate::new(200).set_body_json(load_fixture("quick_song.json")))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = client_for(&mock_server.uri());
    let quick = client.quick_search("周杰伦").await.expect("quick search");

    assert_eq!(quick.songs.len(), 4);
    assert_eq!(quick.songs[0].name, "晴天");
}

#[tokio::test]
async fn quick_search_maps_http_error() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let client = client_for(&mock_server.uri());
    let err = client.quick_search("周杰伦").await.unwrap_err();
    assert!(matches!(
        err,
        hmp_qqmusic_api::error::QqMusicError::Http { status: 500, .. }
    ));
}

#[tokio::test]
async fn quick_search_parses_empty_result() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0, "subcode": 0,
            "data": {"song": {"count": 0, "itemlist": []}}
        })))
        .mount(&mock_server)
        .await;

    let client = client_for(&mock_server.uri());
    let quick = client
        .quick_search("不存在的歌")
        .await
        .expect("empty search");
    assert!(quick.songs.is_empty());
}
