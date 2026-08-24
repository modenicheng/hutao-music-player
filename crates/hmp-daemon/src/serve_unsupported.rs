//! Explicit fallback for platforms where the daemon's Unix-socket transport is unavailable.
//!
//! Native desktop frontends use `AppCore` in-process and do not depend on this transport.

fn unsupported() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "hmp daemon IPC currently requires a Unix platform; use the native desktop application on Windows",
    )
}

pub async fn run_foreground() -> Result<(), Box<dyn std::error::Error>> {
    Err(unsupported().into())
}

pub async fn run_background() -> Result<(), Box<dyn std::error::Error>> {
    Err(unsupported().into())
}

pub fn spawn_detached(_args: &[&str]) -> std::io::Result<()> {
    Err(unsupported())
}

#[cfg(test)]
mod tests {
    #[test]
    fn unsupported_transport_is_reported_explicitly() {
        assert_eq!(
            super::spawn_detached(&[]).unwrap_err().kind(),
            std::io::ErrorKind::Unsupported
        );
    }
}
