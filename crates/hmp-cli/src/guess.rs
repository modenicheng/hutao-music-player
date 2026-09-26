//! `hmp guess`：猜你喜欢（需登录，走 daemon）。
//!
//! ```text
//! hmp guess
//! ```

use hmp_core::{Request, Response};

use super::client::DaemonClient;
use super::commands;

/// 猜你喜欢。
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    let resp = commands::send(&mut c, Request::GuessGet { page: 1 }).await?;
    match resp {
        Response::Guess(page) => {
            let mut out = std::io::stdout().lock();
            use std::io::Write;
            if page.songs.is_empty() {
                writeln!(out, "No recommendations (listen more to train radar)")?;
                return Ok(());
            }
            writeln!(out, "== 猜你喜欢（{}）==", page.songs.len())?;
            for (i, s) in page.songs.iter().enumerate() {
                writeln!(
                    out,
                    "{:>3}. {} - {}  [{}]",
                    i + 1,
                    s.name,
                    if s.singer.is_empty() { "-" } else { &s.singer },
                    s.mid
                )?;
            }
            writeln!(out)?;
            writeln!(out, "Play: hmp play <songmid>")?;
            Ok(())
        }
        Response::Err { code, message } => {
            Err(format!("query failed ({code:?}): {message}").into())
        }
        _ => Err("unexpected guess response".into()),
    }
}
