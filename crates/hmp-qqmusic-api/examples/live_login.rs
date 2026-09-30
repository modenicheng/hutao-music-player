//! login 域 live 探针（仅覆盖只读/无凭证路径）。
//!
//! 运行：`cargo run -p hmp-qqmusic-api --example live_login`
//!
//! 安全红线：`refresh_credential` / `logout` / `authorize_qq_qr` 为会话级写操作，
//! 对真实凭证调用会使用户会话失效，本探针**不做其 live 调用**（wiremock 覆盖见
//! `tests/login.rs`）。本探针仅验证：
//!
//! 1. `get_qrcode`：真机获取二维码，校验 PNG 文件头（`\x89PNG`）+ Set-Cookie qrsig，
//!    PNG 落临时文件；
//! 2. `check_qrcode`：伪造 qrsig 走一次，记录错误路径/事件解析的实际行为；
//! 3. `wait_qrcode_login`：`CancellationToken` 5s 取消（不实际扫码），验证取消语义
//!    与响应时延；
//! 4. `check_expired`：读取本地存储的真实凭证验证（读操作）；
//! 5. 本地凭证 serde 往返一次（反序列化兼容性）。
//!
//! 全程约 7 个请求，符合登录域网络节奏（间隔 ≥1s）。

use std::time::Duration;

use hmp_qqmusic_api::credential::Credential;
use hmp_qqmusic_api::login::{LoginApi, PollInterval, QR, QRLoginType};
use hmp_qqmusic_api::QqMusicClient;
use tokio_util::sync::CancellationToken;

fn load_credential() -> Option<Credential> {
    hmp_storage::credential::store_from_env()
        .load()
        .ok()
        .flatten()
        .filter(|c| c.is_logged_in())
}

#[tokio::main]
async fn main() {
    let client = QqMusicClient::new();
    let login = LoginApi::new(&client);

    // 1) get_qrcode —— PNG 字节 + qrsig
    let qr = match login.get_qrcode(QRLoginType::Qq).await {
        Ok(qr) => {
            let png_header = qr.data.starts_with(b"\x89PNG");
            println!(
                "PASS get_qrcode: png_header={} bytes={} qrsig_len={} mime={}",
                png_header,
                qr.data.len(),
                qr.identifier.len(),
                qr.mimetype
            );
            let path = std::env::temp_dir().join("hmp_live_login_qr.png");
            match std::fs::write(&path, &qr.data) {
                Ok(()) => println!("  qr saved to {}", path.display()),
                Err(e) => println!("  qr save failed: {e}"),
            }
            qr
        }
        Err(e) => {
            println!("FAIL get_qrcode: {e}");
            return;
        }
    };

    tokio::time::sleep(Duration::from_secs(1)).await;

    // 2) check_qrcode 伪造 qrsig —— 记录错误路径/事件解析实际行为
    let fake = QR {
        data: vec![],
        qr_type: QRLoginType::Qq,
        mimetype: "image/png".into(),
        identifier: "hmp-live-probe-fake-qrsig".into(),
    };
    match login.check_qrcode(&fake).await {
        Ok(r) => println!(
            "BEHAVIOR check_qrcode(fake qrsig): event={:?} credential={}",
            r.event,
            r.credential.is_some()
        ),
        Err(e) => println!("BEHAVIOR check_qrcode(fake qrsig): error={e}"),
    }

    tokio::time::sleep(Duration::from_secs(1)).await;

    // 3) wait_qrcode_login —— 5s 后取消（不实际扫码）
    let token = CancellationToken::new();
    let cancel_token = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        cancel_token.cancel();
    });
    let start = std::time::Instant::now();
    match login
        .wait_qrcode_login(&qr, PollInterval::default(), Duration::from_secs(120), Some(&token))
        .await
    {
        Ok(_) => println!("UNEXPECTED wait_qrcode_login: Done（二维码被扫？）"),
        Err(e) => println!(
            "BEHAVIOR wait_qrcode_login(cancel@5s): error={e} elapsed={:?}",
            start.elapsed()
        ),
    }

    tokio::time::sleep(Duration::from_secs(1)).await;

    // 4) check_expired —— 真实凭证（读操作）
    let Some(cred) = load_credential() else {
        println!("SKIP check_expired: no stored credential");
        return;
    };
    println!(
        "credential: uin={} music_id={} login_type={:?} local_expired={}",
        cred.uin,
        cred.music_id,
        cred.login_type,
        cred.is_expired()
    );

    // 5) 本地凭证 serde 往返一次
    match serde_json::to_string(&cred)
        .ok()
        .and_then(|s| serde_json::from_str::<Credential>(&s).ok())
    {
        Some(back) => println!(
            "PASS credential serde roundtrip: music_id={} musickey_len={} login_type={:?}",
            back.music_id,
            back.music_key.len(),
            back.login_type
        ),
        None => println!("FAIL credential serde roundtrip"),
    }

    match login.check_expired(&cred).await {
        Ok(expired) => println!("PASS check_expired: expired={expired}"),
        Err(e) => println!("FAIL check_expired: {e}"),
    }
}
