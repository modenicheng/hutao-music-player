use std::fs::File;
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
    endpoint: PathBuf,
    _lock: File,
}

impl Listener {
    pub async fn bind() -> io::Result<Self> {
        Self::bind_path(endpoint()).await
    }

    pub async fn bind_path(endpoint: PathBuf) -> io::Result<Self> {
        if let Some(parent) = endpoint.parent() {
            std::fs::create_dir_all(parent)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        let lock_path = endpoint.with_extension("sock.lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(lock_path)?;
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "another hmp daemon owns the endpoint",
            ));
        }
        if endpoint.exists() {
            match UnixStream::connect(&endpoint).await {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "another hmp daemon is accepting connections",
                    ));
                }
                Err(_) => std::fs::remove_file(&endpoint)?,
            }
        }
        let inner = UnixListener::bind(&endpoint)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&endpoint, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            inner,
            endpoint,
            _lock: lock,
        })
    }

    pub async fn accept(&mut self) -> io::Result<BoxStream> {
        let (stream, _) = self.inner.accept().await?;
        Ok(Box::new(stream))
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.endpoint);
    }
}

pub async fn connect() -> io::Result<BoxStream> {
    Ok(Box::new(UnixStream::connect(endpoint()).await?))
}
