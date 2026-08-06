//! `hmp login`：QQ 扫码登录。

use hmp_qqmusic_api::{LoginApi, QRLoginType, QqMusicClient};
use hmp_storage::credential::{BackendKind, store_from_env};
use std::time::Duration;

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
    let backend = BackendKind::from_env();
    let store = store_from_env();
    store.save(&credential)?;
    match backend {
        BackendKind::SecretService => {
            println!(
                "登录成功! 用户: {} ({}), 凭证已保存到系统密钥环",
                credential.uin, credential.music_id
            );
        }
        BackendKind::File => {
            println!(
                "登录成功! 用户: {} ({}), 凭证已保存到 {}（明文，不安全）",
                credential.uin,
                credential.music_id,
                hmp_storage::xdg::config_dir()
                    .join("credential.json")
                    .display()
            );
        }
    }
    Ok(())
}
