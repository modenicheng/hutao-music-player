//! `hmp login`：QQ 扫码登录（终端 ASCII 二维码 + 过期自动刷新）。
//!
//! 输出约定：二维码与提示全部 `write!` + `stdout().flush()`（spec 全局约束），
//! 禁止裸 `println!`。

use std::io::Write;
use std::time::{Duration, Instant};

use hmp_qqmusic_api::{LoginApi, QRLoginType, QqMusicClient};
use hmp_storage::credential::{BackendKind, store_from_env};

mod qr_ascii;

/// 总墙钟上限：二维码无限过期也不死循环（10 分钟）。
const OVERALL_LIMIT: Duration = Duration::from_secs(600);
/// 单个二维码等待上限。
const QR_TIMEOUT: Duration = Duration::from_secs(120);

/// 渲染二维码到 stdout（失败时打印兜底路径）。返回是否渲染成功。
fn print_qr(data: &[u8], path: &std::path::Path, out: &mut impl Write) -> std::io::Result<bool> {
    match qr_ascii::render_qr(data, qr_ascii::terminal_width()) {
        Ok(render) => {
            writeln!(out, "{}", render.text)?;
            // 每次扫码前校验尺寸：非标准 111×111 → 提示显示可能有误
            if !render.is_expected_size {
                writeln!(
                    out,
                    "warning: QR image size is {}x{} (expected {}x{}); display may be wrong. If scanning fails, open: {}",
                    render.size.0,
                    render.size.1,
                    qr_ascii::EXPECTED_QR_SIZE,
                    qr_ascii::EXPECTED_QR_SIZE,
                    path.display()
                )?;
            }
            Ok(true)
        }
        Err(e) => {
            writeln!(
                out,
                "QR render failed ({e}); please open manually: {}",
                path.display()
            )?;
            Ok(false)
        }
    }
}

/// 登录主流程。
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let client = QqMusicClient::new();
    let login = LoginApi::new(&client);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let overall_deadline = Instant::now() + OVERALL_LIMIT;

    loop {
        // 剩余墙钟时间为零 → 不再等待（final review Finding 10）。
        let Some(wait_timeout) = wait_timeout(&overall_deadline) else {
            return Err("login timed out (10 minute limit)".into());
        };
        let qr = login.get_qrcode(QRLoginType::Qq).await?;
        let qr_path = std::env::temp_dir().join("hmp-qr.png");
        std::fs::write(&qr_path, &qr.data)?;
        print_qr(&qr.data, &qr_path, &mut out)?;
        out.flush()?;
        writeln!(
            out,
            "Scan the QR code with the QQ mobile app and confirm login... (expired codes refresh automatically)"
        )?;
        out.flush()?;

        match login
            .wait_qrcode_login(&qr, Default::default(), wait_timeout, None)
            .await
        {
            Ok(credential) => {
                let backend = BackendKind::from_env();
                let store = store_from_env();
                store.save(&credential)?;
                match backend {
                    BackendKind::SecretService => {
                        writeln!(
                            out,
                            "Login successful! User: {} ({}), credentials saved to the system keyring",
                            credential.uin, credential.music_id
                        )?;
                    }
                    BackendKind::File => {
                        writeln!(
                            out,
                            "Login successful! User: {} ({}), credentials saved to {} (plaintext, insecure)",
                            credential.uin,
                            credential.music_id,
                            hmp_storage::xdg::config_dir()
                                .join("credential.json")
                                .display()
                        )?;
                    }
                }
                out.flush()?;
                return Ok(());
            }
            Err(e) if should_refresh(&e, Instant::now(), overall_deadline) => {
                // 二维码过期/超时 → 自动刷新（不重跑命令）
                writeln!(out, "\nQR code expired, refreshing...")?;
                out.flush()?;
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// 单次等待上限：`QR_TIMEOUT` 与总墙钟剩余时间的较小值（final review Finding 10）。
/// 剩余时间为零时返回 None → 调用方直接退出循环（不再等待）。
fn wait_timeout(deadline: &Instant) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return None;
    }
    Some(remaining.min(QR_TIMEOUT))
}

/// 判定是否应自动刷新二维码（仅超时类错误且未到总墙钟上限；
/// final review Finding 10：用户拒绝/取消不刷新）。
fn should_refresh(err: &hmp_qqmusic_api::QqMusicError, now: Instant, deadline: Instant) -> bool {
    use hmp_qqmusic_api::QqMusicError;
    // QQ 服务端超时消息为中文（「二维码已超时」）；容错匹配英文小写形式，
    // 防服务端文案切换语言后自动刷新失效。
    let msg = match err {
        QqMusicError::Login { code: -1, message } => message.to_lowercase(),
        _ => return false,
    };
    (msg.contains("超时") || msg.contains("timeout") || msg.contains("timed out")) && now < deadline
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 标准 111×111 不提示；非标准尺寸提示显示可能有误并给出兑底路径。
    #[test]
    fn print_qr_warns_on_unexpected_size() {
        let png = |size: u32| {
            let mut img = image::RgbaImage::new(size, size);
            for p in img.pixels_mut() {
                *p = image::Rgba([255, 255, 255, 255]);
            }
            let mut buf = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut buf, image::ImageFormat::Png)
                .unwrap();
            buf.into_inner()
        };
        let path = std::path::Path::new("/tmp/hmp-qr.png");
        // 标准尺寸 → 无警告
        let mut out = Vec::new();
        print_qr(&png(111), path, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(!s.contains("warning"));
        // 99×99 → 警告 + 兑底路径
        let mut out = Vec::new();
        print_qr(&png(99), path, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("warning"));
        assert!(s.contains("99x99"));
        assert!(s.contains("/tmp/hmp-qr.png"));
    }

    #[test]
    fn timeout_before_deadline_refreshes() {
        let err = hmp_qqmusic_api::QqMusicError::Login {
            code: -1,
            message: "login QR code has timed out".into(),
        };
        assert!(should_refresh(
            &err,
            Instant::now(),
            Instant::now() + Duration::from_secs(100)
        ));
    }

    /// Finding 10：用户拒绝（非超时）不得刷新。
    #[test]
    fn refusal_does_not_refresh() {
        let err = hmp_qqmusic_api::QqMusicError::Login {
            code: -1,
            message: "user rejected the login request".into(),
        };
        assert!(!should_refresh(
            &err,
            Instant::now(),
            Instant::now() + Duration::from_secs(100)
        ));
    }

    /// Finding 10：用户取消（非超时）不得刷新。
    #[test]
    fn cancel_does_not_refresh() {
        let err = hmp_qqmusic_api::QqMusicError::Login {
            code: -1,
            message: "login canceled".into(),
        };
        assert!(!should_refresh(
            &err,
            Instant::now(),
            Instant::now() + Duration::from_secs(100)
        ));
    }

    /// Finding 10：单次等待上限被剩余墙钟时间截断；剩余为零 → None。
    #[test]
    fn wait_timeout_capped_by_overall_remaining() {
        // 剩余远超 QR_TIMEOUT → 上限即 QR_TIMEOUT
        let far = Instant::now() + Duration::from_secs(1000);
        assert_eq!(wait_timeout(&far), Some(QR_TIMEOUT));
        // 剩余不足 QR_TIMEOUT → 截断为剩余值
        let near = Instant::now() + Duration::from_secs(30);
        let t = wait_timeout(&near).expect("剩余非零应有超时");
        assert!(t <= Duration::from_secs(30) && t > Duration::ZERO);
        // 已过上限 → None（直接退出循环）
        let past = Instant::now() - Duration::from_secs(1);
        assert_eq!(wait_timeout(&past), None);
    }

    #[test]
    fn timeout_after_deadline_stops() {
        let err = hmp_qqmusic_api::QqMusicError::Login {
            code: -1,
            message: "login QR code has timed out".into(),
        };
        assert!(!should_refresh(
            &err,
            Instant::now(),
            Instant::now() - Duration::from_secs(1)
        ));
    }

    #[test]
    fn non_timeout_error_stops() {
        let err = hmp_qqmusic_api::QqMusicError::Network("断网".into());
        assert!(!should_refresh(
            &err,
            Instant::now(),
            Instant::now() + Duration::from_secs(100)
        ));
    }
}
