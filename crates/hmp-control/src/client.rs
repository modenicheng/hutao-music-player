use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::protocol::{Event, FrameError, MAX_FRAME, PROTOCOL_VERSION, Request, Response};
use crate::transport::BoxStream;

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame error: {0}")]
    Frame(#[from] FrameError),
    #[error("protocol error: {0}")]
    Protocol(String),
}

pub async fn write_frame<T, W>(writer: &mut W, message: &T) -> Result<(), ControlError>
where
    T: Serialize,
    W: AsyncWrite + Unpin + ?Sized,
{
    writer.write_all(&crate::encode_frame(message)?).await?;
    Ok(())
}

pub async fn read_frame<T, R>(reader: &mut R) -> Result<T, ControlError>
where
    T: DeserializeOwned,
    R: AsyncRead + Unpin + ?Sized,
{
    let mut prefix = [0_u8; 4];
    reader.read_exact(&mut prefix).await?;
    let payload_len = u32::from_le_bytes(prefix) as usize;
    if payload_len == 0 || payload_len + 4 > MAX_FRAME {
        return Err(ControlError::Protocol(format!(
            "invalid payload length {payload_len}"
        )));
    }
    let mut frame = Vec::with_capacity(payload_len + 4);
    frame.extend_from_slice(&prefix);
    frame.resize(payload_len + 4, 0);
    reader.read_exact(&mut frame[4..]).await?;
    Ok(crate::decode_frame(&frame)?)
}

pub struct ControlClient {
    stream: BoxStream,
}

impl ControlClient {
    pub async fn connect() -> Result<Self, ControlError> {
        Self::from_stream(crate::transport::connect().await?).await
    }

    pub async fn from_stream(mut stream: BoxStream) -> Result<Self, ControlError> {
        negotiate(&mut *stream).await?;
        Ok(Self { stream })
    }

    pub async fn request(
        &mut self,
        request: hmp_core::Request,
    ) -> Result<hmp_core::Response, ControlError> {
        write_frame(&mut *self.stream, &Request::Engine(request)).await?;
        match read_frame::<Response, _>(&mut *self.stream).await? {
            Response::Engine(response) => Ok(response),
            Response::ProtocolError { message } => Err(ControlError::Protocol(message)),
            Response::Hello { .. } => Err(ControlError::Protocol(
                "unexpected duplicate protocol handshake".into(),
            )),
        }
    }
}

pub struct Subscription {
    stream: BoxStream,
}

impl Subscription {
    pub async fn connect(frontend_lease: bool) -> Result<Self, ControlError> {
        Self::from_stream(crate::transport::connect().await?, frontend_lease).await
    }

    pub async fn from_stream(
        mut stream: BoxStream,
        frontend_lease: bool,
    ) -> Result<Self, ControlError> {
        negotiate(&mut *stream).await?;
        write_frame(&mut *stream, &Request::Subscribe { frontend_lease }).await?;
        Ok(Self { stream })
    }

    pub async fn next(&mut self) -> Result<Event, ControlError> {
        read_frame(&mut *self.stream).await
    }
}

async fn negotiate(stream: &mut dyn crate::transport::AsyncStream) -> Result<(), ControlError> {
    write_frame(
        stream,
        &Request::Hello {
            protocol: PROTOCOL_VERSION,
        },
    )
    .await?;
    match read_frame::<Response, _>(stream).await? {
        Response::Hello { protocol } if protocol == PROTOCOL_VERSION => Ok(()),
        Response::Hello { protocol } => Err(ControlError::Protocol(format!(
            "daemon selected unsupported protocol {protocol}"
        ))),
        Response::ProtocolError { message } => Err(ControlError::Protocol(message)),
        Response::Engine(_) => Err(ControlError::Protocol(
            "daemon did not answer protocol handshake".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::duplex;

    use super::*;
    use crate::{PROTOCOL_VERSION, Request, Response};

    #[tokio::test]
    async fn client_handshakes_before_first_request() {
        let (client_io, mut server_io) = duplex(4096);
        let server = tokio::spawn(async move {
            assert_eq!(
                read_frame::<Request, _>(&mut server_io).await.unwrap(),
                Request::Hello {
                    protocol: PROTOCOL_VERSION,
                }
            );
            write_frame(
                &mut server_io,
                &Response::Hello {
                    protocol: PROTOCOL_VERSION,
                },
            )
            .await
            .unwrap();
            assert_eq!(
                read_frame::<Request, _>(&mut server_io).await.unwrap(),
                Request::Engine(hmp_core::Request::Status),
            );
            write_frame(
                &mut server_io,
                &Response::Engine(hmp_core::Response::Status(Default::default())),
            )
            .await
            .unwrap();
        });

        let mut client = ControlClient::from_stream(Box::new(client_io))
            .await
            .unwrap();
        assert!(matches!(
            client.request(hmp_core::Request::Status).await.unwrap(),
            hmp_core::Response::Status(_)
        ));
        server.await.unwrap();
    }
}
