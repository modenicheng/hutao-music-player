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
        Ok(s) => {
            writeln!(out, "{s}")?;
            Ok(true)
        }
        Err(e) => {
            writeln!(out, "二维码渲染失败（{e}），请手动打开: {}", path.display())?;
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
            return Err("登录超时（10 分钟上限）".into());
        };
        let qr = login.get_qrcode(QRLoginType::Qq).await?;
        let qr_path = std::env::temp_dir().join("hmp-qr.png");
        std::fs::write(&qr_path, &qr.data)?;
        print_qr(&qr.data, &qr_path, &mut out)?;
        out.flush()?;
        writeln!(
            out,
            "请用 QQ 手机版扫码并确认登录……（二维码过期将自动刷新）"
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
                            "登录成功! 用户: {} ({}), 凭证已保存到系统密钥环",
                            credential.uin, credential.music_id
                        )?;
                    }
                    BackendKind::File => {
                        writeln!(
                            out,
                            "登录成功! 用户: {} ({}), 凭证已保存到 {}（明文，不安全）",
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
                writeln!(out, "\n二维码已过期，自动刷新…")?;
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
    let is_timeout = matches!(
        err,
        QqMusicError::Login { code: -1, message } if message.contains("超时")
    );
    is_timeout && now < deadline
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_before_deadline_refreshes() {
        let err = hmp_qqmusic_api::QqMusicError::Login {
            code: -1,
            message: "登录二维码已超时".into(),
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
            message: "用户拒绝了登录请求".into(),
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
            message: "登录已取消".into(),
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
            message: "登录二维码已超时".into(),
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
