//! 登录流程集成测试：wiremock 模拟 QQ 扫码登录全链路。
//!
//! 行为规范对应上游 `modules/login.py`：
//! - `_get_qq_qr`：GET ptqrshow → Set-Cookie qrsig + PNG 数据
//! - `_check_qq_qr`：GET ptqrlogin → `ptuiCB(...)` 文本，Done 时解析 uin/ptsigx
//! - `_authorize_qq_qr`：GET check_sig → p_skey → POST authorize → Location code
//!   → CGI QQLogin → Credential
//! - `refresh_credential`：CGI Login → 新 Credential
//! - `check_expired`：GET fcg_get_profile_homepage.fcg

use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::config::ClientConfig;
use hmp_qqmusic_api::credential::Credential;
use hmp_qqmusic_api::error::QqMusicError;
use hmp_qqmusic_api::login::{LoginApi, PollInterval, QR, QRCodeLoginEvents, QRLoginType};
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client_for(mock: &MockServer) -> QqMusicClient {
    let config = ClientConfig {
        base_url: mock.uri(),
        login_ptlogin2_url: mock.uri(),
        login_graph_url: mock.uri(),
        login_oauth_url: mock.uri(),
        login_profile_url: mock.uri(),
        ..Default::default()
    };
    QqMusicClient::with_config(config)
}

/// ptqrlogin 返回扫码确认状态（67=CONF）
fn conf_ptui_response() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_string("ptuiCB('67','0','', '0', '二维码未失效' );")
}

fn ok_login_cgi_response() -> serde_json::Value {
    json!({
        "code": 0,
        "req_0": {
            "code": 0,
            "data": {
                "musicid": 12345,
                "musickey": "mkey_abc",
                "str_musicid": "12345",
                "refresh_key": "rk_xyz",
                "loginType": 2,
                "musickeyCreateTime": 1700000000,
                "keyExpiresIn": 86400
            }
        }
    })
}

#[tokio::test]
async fn get_qrcode_extracts_qrsig_and_png() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ptqrshow"))
        .and(query_param("appid", "716027609"))
        .and(query_param("pt_3rd_aid", "100497308"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Set-Cookie", "qrsig=abc123; Path=/; Domain=.qq.com")
                .set_body_bytes(vec![0x89, 0x50, 0x4e, 0x47]),
        )
        .expect(1)
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);
    let qr = login.get_qrcode(QRLoginType::Qq).await.unwrap();

    assert_eq!(qr.qr_type, QRLoginType::Qq);
    assert_eq!(qr.identifier, "abc123");
    assert_eq!(qr.mimetype, "image/png");
    assert_eq!(qr.data, vec![0x89, 0x50, 0x4e, 0x47]);
}

#[tokio::test]
async fn check_qrcode_scan_and_conf_states() {
    let mock = MockServer::start().await;
    // 第一次：未扫描（66）
    Mock::given(method("GET"))
        .and(path("/ptqrlogin"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ptuiCB('66','0','', '0', '' );"))
        .up_to_n_times(1)
        .mount(&mock)
        .await;
    // 第二次：已扫描待确认（67）
    Mock::given(method("GET"))
        .and(path("/ptqrlogin"))
        .respond_with(conf_ptui_response())
        .up_to_n_times(1)
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);
    let qr = QR {
        data: vec![],
        qr_type: QRLoginType::Qq,
        mimetype: "image/png".into(),
        identifier: "qrsig123".into(),
    };

    let r1 = login.check_qrcode(&qr).await.unwrap();
    assert_eq!(r1.event, QRCodeLoginEvents::Scan);
    assert!(r1.credential.is_none());

    let r2 = login.check_qrcode(&qr).await.unwrap();
    assert_eq!(r2.event, QRCodeLoginEvents::Conf);
    assert!(!r2.done());
}

#[tokio::test]
async fn check_qrcode_done_authorizes_and_returns_credential() {
    let mock = MockServer::start().await;

    // 1) ptqrlogin → Done 状态 + uin/ptsigx
    Mock::given(method("GET"))
        .and(path("/ptqrlogin"))
        .and(query_param("ptqrtoken", "610575516"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "ptuiCB('0','0','https://graph.qq.com/oauth2.0/login_jump?ptsigx=abcdef12&s_url=x&uin=123456&service=y', '0', '登录成功' );",
        ))
        .expect(1)
        .mount(&mock)
        .await;

    // 2) check_sig → p_skey
    Mock::given(method("GET"))
        .and(path("/check_sig"))
        .and(query_param("uin", "123456"))
        .and(query_param("ptsigx", "abcdef12"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Set-Cookie", "p_skey=psk123; Path=/; Domain=.qq.com"),
        )
        .expect(1)
        .mount(&mock)
        .await;

    // 3) authorize → 302 Location 带 code
    // 注意：上游以表单（form）发送 authorize 参数，wiremock 用闭包检查 body
    Mock::given(method("POST"))
        .and(path("/oauth2.0/authorize"))
        .and(header("cookie", "p_skey=psk123"))
        .and(|req: &wiremock::Request| {
            let body = String::from_utf8_lossy(&req.body);
            body.contains("response_type=code")
                && body.contains("client_id=100497308")
                && body.contains("state=state")
        })
        .respond_with(ResponseTemplate::new(302).insert_header(
            "Location",
            "https://y.qq.com/portal/wx_redirect.html?login_type=1&code=QQCODE123&state=state",
        ))
        .expect(1)
        .mount(&mock)
        .await;

    // 4) QQLogin CGI
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_login_cgi_response()))
        .expect(1)
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);
    let qr = QR {
        data: vec![],
        qr_type: QRLoginType::Qq,
        mimetype: "image/png".into(),
        identifier: "qrsig123".into(),
    };

    let result = login.check_qrcode(&qr).await.unwrap();
    assert!(result.done());
    let cred = result.credential.expect("done carries credential");
    assert_eq!(cred.music_id, "12345");
    assert_eq!(cred.music_key, "mkey_abc");
    assert_eq!(cred.login_type, hmp_qqmusic_api::credential::LoginType::Qq);
}

#[tokio::test]
async fn refresh_credential_returns_new_credential() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let req0 = &body["req_0"];
            req0["module"] == json!("music.login.LoginServer")
                && req0["method"] == json!("Login")
                && body["comm"]["tmeLoginType"] == json!(2)
                && req0["param"]["loginMode"] == json!(2)
                && req0["param"]["musickey"] == json!("old_mkey")
                && req0["param"]["refresh_key"] == json!("old_rk")
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_login_cgi_response()))
        .expect(1)
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);

    let old = Credential {
        uin: "12345".into(),
        music_id: "12345".into(),
        music_key: "old_mkey".into(),
        refresh_key: Some("old_rk".into()),
        ..Default::default()
    };

    let new_cred = login.refresh_credential(&old).await.unwrap();
    assert_eq!(new_cred.music_key, "mkey_abc");
    assert_eq!(new_cred.refresh_key.as_deref(), Some("rk_xyz"));
}

#[tokio::test]
async fn refresh_credential_maps_login_error_to_credential_refresh() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "req_0": {"code": 1000, "data": {}}
        })))
        .expect(1)
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);
    let cred = Credential {
        music_id: "1".into(),
        music_key: "k".into(),
        ..Default::default()
    };
    let err = login.refresh_credential(&cred).await.unwrap_err();
    match err {
        QqMusicError::CredentialRefresh { code, .. } => assert_eq!(code, 1000),
        other => panic!("expected CredentialRefresh, got {other:?}"),
    }
}

#[tokio::test]
async fn check_expired_queries_profile_homepage() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rsc/fcgi-bin/fcg_get_profile_homepage.fcg"))
        .and(query_param("loginUin", "12345"))
        .and(query_param("g_tk", "988047106"))
        .and(header(
            "cookie",
            "uin=12345; qqmusic_uin=12345; qm_keyst=test_music_key_123; qqmusic_key=test_music_key_123",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": 0})))
        .expect(1)
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);
    let cred = Credential {
        uin: "12345".into(),
        music_id: "12345".into(),
        music_key: "test_music_key_123".into(),
        ..Default::default()
    };
    assert!(!login.check_expired(&cred).await.unwrap());
}

#[tokio::test]
async fn check_expired_true_when_code_nonzero() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rsc/fcgi-bin/fcg_get_profile_homepage.fcg"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": -3001})))
        .expect(1)
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);
    let cred = Credential {
        music_id: "12345".into(),
        music_key: "k".into(),
        ..Default::default()
    };
    assert!(login.check_expired(&cred).await.unwrap());
}

#[tokio::test]
async fn wait_qrcode_login_loops_until_done() {
    let mock = MockServer::start().await;
    // 66(Scan) → 67(Conf) → Done
    Mock::given(method("GET"))
        .and(path("/ptqrlogin"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ptuiCB('66','0','', '0', '' );"))
        .up_to_n_times(1)
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/ptqrlogin"))
        .respond_with(conf_ptui_response())
        .up_to_n_times(1)
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/ptqrlogin"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "ptuiCB('0','0','https://graph.qq.com/oauth2.0/login_jump?ptsigx=sig&s_url=x&uin=1&service=y', '0', 'ok' );",
        ))
        .up_to_n_times(1)
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/check_sig"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Set-Cookie", "p_skey=psk; Path=/; Domain=.qq.com"),
        )
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth2.0/authorize"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "Location",
            "https://y.qq.com/portal/wx_redirect.html?code=CODE&state=state",
        ))
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_login_cgi_response()))
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);
    let qr = QR {
        data: vec![],
        qr_type: QRLoginType::Qq,
        mimetype: "image/png".into(),
        identifier: "qrsig".into(),
    };

    let interval = PollInterval {
        default: Duration::from_millis(10),
        scanned: Some(Duration::from_millis(10)),
        error: Some(Duration::from_millis(10)),
    };
    let cred = login
        .wait_qrcode_login(&qr, interval, Duration::from_secs(5), None)
        .await
        .unwrap();
    assert_eq!(cred.music_key, "mkey_abc");
}

#[tokio::test]
async fn wait_qrcode_login_refuse_errors() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ptqrlogin"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("ptuiCB('68','0','', '0', '用户拒绝' );"),
        )
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);
    let qr = QR {
        data: vec![],
        qr_type: QRLoginType::Qq,
        mimetype: "image/png".into(),
        identifier: "qrsig".into(),
    };
    let err = login
        .wait_qrcode_login(&qr, PollInterval::default(), Duration::from_secs(5), None)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        QqMusicError::Login { message, .. } if message.contains("拒绝")
    ));
}

#[tokio::test]
async fn wait_qrcode_login_cancel() {
    let mock = MockServer::start().await;
    // 永远返回 Scan，不进入 Done
    Mock::given(method("GET"))
        .and(path("/ptqrlogin"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ptuiCB('66','0','', '0', '' );"))
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);
    let qr = QR {
        data: vec![],
        qr_type: QRLoginType::Qq,
        mimetype: "image/png".into(),
        identifier: "qrsig".into(),
    };

    let token = tokio_util::sync::CancellationToken::new();
    let cancel_token = token.clone();
    let handle = tokio::spawn(async move {
        cancel_token.cancel();
    });
    handle.await.unwrap();

    let err = login
        .wait_qrcode_login(
            &qr,
            PollInterval {
                default: Duration::from_millis(20),
                ..Default::default()
            },
            Duration::from_secs(5),
            Some(&token),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        QqMusicError::Login { message, .. } if message.contains("取消")
    ));
}
