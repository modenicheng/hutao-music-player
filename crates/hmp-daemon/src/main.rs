//! Standalone HMP playback daemon.

use clap::{ArgGroup, Parser};

#[derive(Debug, Parser)]
#[command(name = "hmpd")]
#[command(group(
    ArgGroup::new("ownership")
        .required(true)
        .multiple(false)
        .args(["frontend_owned", "autonomous"])
))]
struct Args {
    /// Exit 30 seconds after the last desktop frontend lease disappears.
    #[arg(long)]
    frontend_owned: bool,
    /// Continue running without a desktop frontend (CLI/headless mode).
    #[arg(long)]
    autonomous: bool,
    /// Override the configured GStreamer audio sink.
    #[arg(long)]
    sink: Option<String>,
}

impl Args {
    fn mode(&self) -> hmp_control::LifecycleMode {
        if self.frontend_owned {
            hmp_control::LifecycleMode::FrontendOwned {
                orphan_grace: std::time::Duration::from_secs(30),
            }
        } else {
            hmp_control::LifecycleMode::Autonomous
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();
    let args = Args::parse();
    if let Err(error) = hmp_daemon::serve::run(args.sink.as_deref(), args.mode()).await {
        eprintln!("hmpd: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_mode_cli_is_explicit() {
        assert_eq!(
            Args::try_parse_from(["hmpd", "--frontend-owned"])
                .unwrap()
                .mode(),
            hmp_control::LifecycleMode::FrontendOwned {
                orphan_grace: std::time::Duration::from_secs(30),
            }
        );
        assert_eq!(
            Args::try_parse_from(["hmpd", "--autonomous"])
                .unwrap()
                .mode(),
            hmp_control::LifecycleMode::Autonomous
        );
        assert!(Args::try_parse_from(["hmpd"]).is_err());
        assert!(Args::try_parse_from(["hmpd", "--frontend-owned", "--autonomous"]).is_err());
    }
}
