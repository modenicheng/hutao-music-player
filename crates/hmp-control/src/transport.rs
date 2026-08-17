//! Cross-platform local transport.

use tokio::io::{AsyncRead, AsyncWrite};

pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> AsyncStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

pub type BoxStream = Box<dyn AsyncStream>;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::{Listener, connect, endpoint};
#[cfg(windows)]
pub use windows::{Listener, connect, connect_named, endpoint};

#[cfg(all(test, windows))]
mod windows_tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[tokio::test]
    async fn named_pipe_accepts_clients_and_excludes_second_listener() {
        let endpoint = format!(r"\\.\pipe\hmp-control-test-{}", std::process::id());
        let mut listener = Listener::bind_named(&endpoint).await.unwrap();
        assert!(Listener::bind_named(&endpoint).await.is_err());

        for byte in [7_u8, 9_u8] {
            let endpoint = endpoint.clone();
            let client = tokio::spawn(async move {
                let mut stream = connect_named(&endpoint).await.unwrap();
                stream.write_all(&[byte]).await.unwrap();
            });
            let mut server = listener.accept().await.unwrap();
            let mut received = [0_u8; 1];
            server.read_exact(&mut received).await.unwrap();
            assert_eq!(received, [byte]);
            client.await.unwrap();
        }
    }
}
