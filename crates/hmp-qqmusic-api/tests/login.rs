//! 登录流程集成测试：wiremock 模拟 QQ 扫码登录全链路。
//!
//! 行为规范对应上游 `modules/login.py`：
//! - `_get_qq_qr`：GET ptqrshow → Set-Cookie qrsig + PNG 数据
//! - `_check_qq_qr`：GET ptqrlogin → `ptuiCB(...)` 文本，Done 时解析 uin/ptsigx
//! - `_authorize_qq_qr`：GET check_sig → p_skey → POST authorize → Location code
//!   → CGI QQLogin → Credential
//! - `refresh_credential`：CGI Login（按 loginType 分支；musicid 以 int 下发）
//! - `check_expired`：GET fcg_get_profile_homepage.fcg
//! - `logout`：CGI Logout（require_login）
//!
//! `refresh_credential` / `logout` / `authorize_qq_qr` 为会话级写操作，**仅做
//! wiremock 离线验证**（对真实凭证调用会使用户会话失效）；live 探针见
//! `examples/live_login.rs`（仅覆盖只读路径）。

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
        QqMusicError::Login { message, .. } if message.contains("rejected")
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
        QqMusicError::Login { message, .. } if message.contains("canceled")
    ));
}

/// 取消信号应中断进行中的轮询请求（上游 anyio.fail_after 包裹单次操作的语义）：
/// 服务端延迟 2s 应答，100ms 时取消 → 总耗时远小于单次请求时长。
#[tokio::test]
async fn wait_qrcode_login_cancel_interrupts_inflight_request() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ptqrlogin"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(2))
                .set_body_string("ptuiCB('66','0','', '0', '' );"),
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

    let token = tokio_util::sync::CancellationToken::new();
    let cancel_token = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel_token.cancel();
    });

    let start = std::time::Instant::now();
    let err = login
        .wait_qrcode_login(
            &qr,
            PollInterval::default(),
            Duration::from_secs(10),
            Some(&token),
        )
        .await
        .unwrap_err();
    let elapsed = start.elapsed();
    assert!(matches!(
        err,
        QqMusicError::Login { message, .. } if message.contains("canceled")
    ));
    assert!(
        elapsed < Duration::from_secs(1),
        "cancel must abort the in-flight request, took {elapsed:?}"
    );
}

/// 无效 qrsig 的真实服务端行为（2026-09-29 实测）：HTTP 403 Forbidden、无 ptuiCB 文本
/// → [`QqMusicError::Http`] 终态（上游包装为 ApiDataError，同为终态语义）。
#[tokio::test]
async fn check_qrcode_invalid_qrsig_maps_http_error() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ptqrlogin"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);
    let qr = QR {
        data: vec![],
        qr_type: QRLoginType::Qq,
        mimetype: "image/png".into(),
        identifier: "fake-qrsig".into(),
    };
    assert!(matches!(
        login.check_qrcode(&qr).await,
        Err(QqMusicError::Http { status: 403, .. })
    ));
}

/// 取消发生在事件睡眠期时，错误须为 "canceled" 而非 "timeout"（缺陷回归：
/// sleep_before_deadline 的取消与超时两种 false 情形曾被混同）。
#[tokio::test]
async fn wait_qrcode_login_cancel_during_sleep_reports_canceled() {
    let mock = MockServer::start().await;
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
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel_token.cancel();
    });

    let err = login
        .wait_qrcode_login(
            &qr,
            PollInterval {
                default: Duration::from_millis(1500),
                ..Default::default()
            },
            Duration::from_secs(10),
            Some(&token),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        QqMusicError::Login { message, .. } if message.contains("canceled")
    ));
}

/// logout 成功路径：CGI Logout + require_login 凭证 Cookie 注入（仅离线验证）。
#[tokio::test]
async fn logout_sends_logout_cgi_with_credential_cookies() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(header(
            "cookie",
            "uin=12345; qqmusic_uin=12345; qm_keyst=old_mkey; qqmusic_key=old_mkey",
        ))
        .and(|req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let req0 = &body["req_0"];
            req0["module"] == json!("music.login.LoginServer")
                && req0["method"] == json!("Logout")
                && req0["param"] == json!({})
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "req_0": {"code": 0, "data": {}}
        })))
        .expect(1)
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);
    let cred = Credential {
        music_id: "12345".into(),
        music_key: "old_mkey".into(),
        ..Default::default()
    };
    login.logout(&cred).await.unwrap();
}

/// logout 非 allow_error_codes 的业务码应报错（上游 CgiRequest._parse_response）。
#[tokio::test]
async fn logout_maps_non_allowed_business_code() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "req_0": {"code": 500, "data": {}}
        })))
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
    assert!(matches!(
        login.logout(&cred).await,
        Err(QqMusicError::QqApi { code: 500, .. })
    ));
}

/// refresh 微信分支（loginType=1）：param 仅含 openid/refresh_token/str_musicid/
/// musickey/unionid/refresh_key，comm.tmeLoginType=1（仅离线验证）。
#[tokio::test]
async fn refresh_wechat_branch_sends_wechat_params() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let param = &body["req_0"]["param"];
            body["comm"]["tmeLoginType"] == json!(1)
                && param["openid"] == json!("wx-openid")
                && param["refresh_token"] == json!("wx-rt")
                && param["str_musicid"] == json!("8888")
                && param["musickey"] == json!("wx_key")
                && param["unionid"] == json!("uni")
                && param["refresh_key"] == json!("wx-rk")
                && param["loginMode"] == json!(2)
                && param.get("access_token").is_none()
                && param.get("musicid").is_none()
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "req_0": {"code": 0, "data": {
                "musicid": 8888, "musickey": "W_X_new", "str_musicid": "8888",
                "refresh_key": "wx-rk2", "loginType": 1
            }}
        })))
        .expect(1)
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);
    let old = Credential {
        music_id: "8888".into(),
        music_key: "wx_key".into(),
        refresh_key: Some("wx-rk".into()),
        login_type: hmp_qqmusic_api::credential::LoginType::Wechat,
        openid: "wx-openid".into(),
        refresh_token: "wx-rt".into(),
        unionid: "uni".into(),
        ..Default::default()
    };

    let new_cred = login.refresh_credential(&old).await.unwrap();
    assert_eq!(new_cred.music_key, "W_X_new");
    // W_X 前缀 → 微信类型推断兜底
    assert_eq!(
        new_cred.login_type,
        hmp_qqmusic_api::credential::LoginType::Wechat
    );
}

/// refresh 其他来源分支：原始 loginType 数值须还原到 comm.tmeLoginType
/// （上游手机扫码 loginType=6 → comm.tmeLoginType=6），param 含 musicid(int)+str_musicid。
#[tokio::test]
async fn refresh_other_branch_preserves_login_type_int() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let param = &body["req_0"]["param"];
            body["comm"]["tmeLoginType"] == json!(6)
                && param["musicid"] == json!(9999)
                && param["str_musicid"] == json!("9999")
                && param["access_token"] == json!("at")
                && param["expired_in"] == json!(0)
                && param["loginMode"] == json!(2)
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "req_0": {"code": 0, "data": {
                "musicid": 9999, "musickey": "new_key", "loginType": 6
            }}
        })))
        .expect(1)
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);
    let old = Credential {
        music_id: "9999".into(),
        music_key: "k".into(),
        refresh_key: Some("rk".into()),
        login_type: hmp_qqmusic_api::credential::LoginType::Other("6".into()),
        access_token: "at".into(),
        ..Default::default()
    };

    let new_cred = login.refresh_credential(&old).await.unwrap();
    assert_eq!(new_cred.login_type, hmp_qqmusic_api::credential::LoginType::Other("6".into()));
}

/// refresh QQ 分支：musicid 必须按数值下发（上游 Credential.musicid 为 int）。
#[tokio::test]
async fn refresh_qq_branch_sends_musicid_as_int() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let param = &body["req_0"]["param"];
            body["comm"]["tmeLoginType"] == json!(2)
                && param["musicid"].is_number()
                && param["musicid"] == json!(12345)
                && param["str_musicid"].is_null()
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_login_cgi_response()))
        .expect(1)
        .mount(&mock)
        .await;

    let client = client_for(&mock);
    let login = LoginApi::new(&client);
    let old = Credential {
        music_id: "12345".into(),
        music_key: "old_mkey".into(),
        refresh_key: Some("old_rk".into()),
        ..Default::default()
    };
    login.refresh_credential(&old).await.unwrap();
}

/// 子响应 code=0 但内层 data 携带错误码时也须判为刷新失败
/// （上游 _build_cgi 返回内层 data 后 _validate_result 二次校验的语义）。
#[tokio::test]
async fn refresh_wraps_inner_data_code_error() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "req_0": {"code": 0, "data": {"code": 1000}}
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

/// authorize 链路（QQLogin CGI）内层 data 错误码不包装，直接上抛登录域错误
/// （上游 _authorize_qq_qr 无 CredentialRefresh 包装）。
#[tokio::test]
async fn authorize_surfaces_inner_data_code_error_unwrapped() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ptqrlogin"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "ptuiCB('0','0','https://graph.qq.com/oauth2.0/login_jump?ptsigx=sigx&s_url=x&uin=9&service=y', '0', 'ok' );",
        ))
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/check_sig"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Set-Cookie", "p_skey=psk; Path=/; Domain=.qq.com"),
        )
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth2.0/authorize"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "Location",
            "https://y.qq.com/portal/wx_redirect.html?code=C&state=state",
        ))
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "req_0": {"code": 0, "data": {"code": 20279}}
        })))
        .expect(1)
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
    assert!(matches!(
        login.check_qrcode(&qr).await,
        Err(QqMusicError::LoginDeviceLimit)
    ));
}
