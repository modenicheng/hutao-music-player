use std::io;
use std::path::PathBuf;

use tokio::net::{UnixListener, UnixStream};

use super::BoxStream;

pub fn endpoint() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("hmp-{}", unsafe { libc::geteuid() }))
        });
    base.join("hmp.sock")
}

pub struct Listener {
    inner: UnixListener,
}

impl Listener {
    pub async fn bind() -> io::Result<Self> {
        let endpoint = endpoint();
        if let Some(parent) = endpoint.parent() {
            std::fs::create_dir_all(parent)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        let inner = UnixListener::bind(&endpoint)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&endpoint, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self { inner })
    }

    pub async fn accept(&mut self) -> io::Result<BoxStream> {
        let (stream, _) = self.inner.accept().await?;
        Ok(Box::new(stream))
    }
}

pub async fn connect() -> io::Result<BoxStream> {
    Ok(Box::new(UnixStream::connect(endpoint()).await?))
}
