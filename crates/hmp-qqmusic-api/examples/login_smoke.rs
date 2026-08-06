//! 登录冒烟：获取真实 QQ 二维码并检查一次状态（不完成登录）。
//!
//! 运行：`cargo run -p hmp-qqmusic-api --example login_smoke`

use hmp_qqmusic_api::QqMusicClient;
use hmp_qqmusic_api::login::{LoginApi, QRLoginType};

#[tokio::main]
async fn main() {
    let client = QqMusicClient::new();
    let login = LoginApi::new(&client);

    let qr = login.get_qrcode(QRLoginType::Qq).await.expect("get qrcode");
    println!(
        "qrcode: type={:?} mimetype={} identifier={} data_bytes={}",
        qr.qr_type,
        qr.mimetype,
        qr.identifier,
        qr.data.len()
    );

    let result = login.check_qrcode(&qr).await.expect("check qrcode");
    println!(
        "status: event={:?} has_credential={}",
        result.event,
        result.credential.is_some()
    );
}
