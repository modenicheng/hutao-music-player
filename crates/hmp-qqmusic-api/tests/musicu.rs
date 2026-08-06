//! `musicu_request` 集成测试：用 wiremock 模拟 `musicu.fcg` 端点。
//!
//! 行为规范对应上游 `core/client.py::Client.execute`（CGI 分支）与
//! `core/api_context.py::build_api_kwargs` / `prepare_http_kwargs`。

use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::config::ClientConfig;
use hmp_qqmusic_api::credential::Credential;
use hmp_qqmusic_api::error::QqMusicError;
use hmp_qqmusic_api::protocol::cgi::CgiRequest;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client_for(base_url: &str) -> QqMusicClient {
    let config = ClientConfig {
        base_url: base_url.to_owned(),
        ..Default::default()
    };
    QqMusicClient::with_config(config)
}

fn search_request() -> CgiRequest {
    CgiRequest::new(
        "music.search.SearchCgiService",
        "DoSearchForQQMusicDesktop",
        json!({"query": "周杰伦", "num_per_page": 2, "page_num": 1}),
    )
}

fn ok_search_response() -> serde_json::Value {
    json!({
        "code": 0,
        "req_0": {
            "code": 0,
            "data": {
                "song": {
                    "totalnum": 1,
                    "list": [{"songmid": "003aQm4F3GJHZq", "songname": "晴天"}],
                }
            }
        }
    })
}

#[tokio::test]
async fn sends_post_to_musicu_fcg_with_comm_and_req_0() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_search_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = client_for(&mock_server.uri());
    let result = client
        .musicu_request(&search_request(), None)
        .await
        .unwrap();

    assert_eq!(result["data"]["song"]["list"][0]["songname"], "晴天");
}

#[tokio::test]
async fn request_body_contains_default_web_comm() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let comm = &body["comm"];
            // 与 Python 参考实现 WEB 平台默认 comm 一致
            comm["ct"] == json!(24)
                && comm["cv"] == json!(4747474)
                && comm["platform"] == json!("yqq.json")
                && comm["chid"] == json!("0")
                && comm["g_tk"] == json!(5381)
                && comm["g_tk_new_20200303"] == json!(5381)
                && comm["format"] == json!("json")
                && body["req_0"]["module"] == json!("music.search.SearchCgiService")
                && body["req_0"]["method"] == json!("DoSearchForQQMusicDesktop")
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_search_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = client_for(&mock_server.uri());
    client
        .musicu_request(&search_request(), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn injects_user_agent_header() {
    let mock_server = MockServer::start().await;
    // 注意: UA 含逗号, wiremock 的 header() matcher 会按逗号拆分, 需用闭包比较
    Mock::given(method("POST"))
        .and(|req: &wiremock::Request| {
            req.headers.get("user-agent").map(|v| v.as_bytes())
                == Some(hmp_qqmusic_api::config::DEFAULT_USER_AGENT.as_bytes())
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_search_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = client_for(&mock_server.uri());
    client
        .musicu_request(&search_request(), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn injects_credential_cookies() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header(
            "cookie",
            "uin=12345; qqmusic_uin=12345; qm_keyst=secret; qqmusic_key=secret",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_search_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = client_for(&mock_server.uri());
    let credential = Credential {
        uin: "12345".into(),
        music_id: "12345".into(),
        music_key: "secret".into(),
        refresh_key: None,
        login_type: hmp_qqmusic_api::credential::LoginType::Qq,
        raw_cookie: String::new(),
        ..Default::default()
    };
    client
        .musicu_request(&search_request(), Some(&credential))
        .await
        .unwrap();
}

#[tokio::test]
async fn maps_http_error_status() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&mock_server)
        .await;

    let client = client_for(&mock_server.uri());
    let err = client
        .musicu_request(&search_request(), None)
        .await
        .unwrap_err();
    assert!(matches!(err, QqMusicError::Http { status: 503, .. }));
}

#[tokio::test]
async fn maps_global_error_code() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"code": 404, "message": "not found"})),
        )
        .mount(&mock_server)
        .await;

    let client = client_for(&mock_server.uri());
    let err = client
        .musicu_request(&search_request(), None)
        .await
        .unwrap_err();
    match err {
        QqMusicError::QqApi { code, .. } => assert_eq!(code, 404),
        other => panic!("expected global QqApi error, got {other:?}"),
    }
}

#[tokio::test]
async fn maps_business_error_code() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "req_0": {"code": 1000, "data": {}}
        })))
        .mount(&mock_server)
        .await;

    let client = client_for(&mock_server.uri());
    let err = client
        .musicu_request(&search_request(), None)
        .await
        .unwrap_err();
    assert!(matches!(err, QqMusicError::CredentialExpired));
}

#[tokio::test]
async fn require_login_rejects_without_credential() {
    let mock_server = MockServer::start().await;
    // 不应发出任何请求
    let client = client_for(&mock_server.uri());
    let mut req = search_request();
    req.require_login = true;
    let err = client.musicu_request(&req, None).await.unwrap_err();
    assert!(matches!(err, QqMusicError::AuthenticationRequired));
}

#[tokio::test]
async fn require_login_accepts_valid_credential() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_search_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = client_for(&mock_server.uri());
    let mut req = search_request();
    req.require_login = true;
    let credential = Credential {
        uin: "1".into(),
        music_id: "1".into(),
        music_key: "k".into(),
        refresh_key: None,
        login_type: hmp_qqmusic_api::credential::LoginType::Qq,
        raw_cookie: String::new(),
        ..Default::default()
    };
    client
        .musicu_request(&req, Some(&credential))
        .await
        .expect("valid credential passes require_login");
}
