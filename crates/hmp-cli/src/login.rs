//! `hmp login`：QQ 扫码登录。

use hmp_qqmusic_api::{LoginApi, QRLoginType, QqMusicClient};
use std::time::Duration;

use crate::credential_store;

/// 登录流程：取二维码 → 保存 PNG → 轮询扫码 → 保存凭证。
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let client = QqMusicClient::new();
    let login = LoginApi::new(&client);

    let qr = login.get_qrcode(QRLoginType::Qq).await?;
    let qr_path = std::env::temp_dir().join("hmp-qr.png");
    std::fs::write(&qr_path, &qr.data)?;
    println!("二维码已保存: {}", qr_path.display());
    println!("请用 QQ 手机版扫码并确认登录……");

    let credential = login
        .wait_qrcode_login(&qr, Default::default(), Duration::from_secs(120), None)
        .await?;
    credential_store::save(&credential)?;
    println!(
        "登录成功! 用户: {} ({}), 凭证已保存到 {}",
        credential.uin,
        credential.music_id,
        credential_store::credential_path().display()
    );
    Ok(())
}
