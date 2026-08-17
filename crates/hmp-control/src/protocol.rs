//! Versioned controller protocol.
//!
//! Playback requests remain kernel interfaces in `hmp-core`; this module wraps
//! them with host-only messages such as protocol negotiation and frontend leases.

use serde::{Deserialize, Serialize};

/// Current controller protocol version.
pub const PROTOCOL_VERSION: u16 = 1;

/// Maximum frame size including the four-byte length prefix.
pub const MAX_FRAME: usize = 1 << 20;

/// Controller-to-daemon message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// Negotiate the wire protocol before any other message.
    Hello { protocol: u16 },
    /// Subscribe to state changes and optionally hold a desktop frontend lease.
    Subscribe { frontend_lease: bool },
    /// Forward a transport-agnostic request to the playback runtime.
    Engine(hmp_core::Request),
}

/// Daemon-to-controller response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Response {
    /// Accepted protocol version.
    Hello { protocol: u16 },
    /// Response produced by the playback runtime or daemon query adapter.
    Engine(hmp_core::Response),
    /// Host-level protocol error.
    ProtocolError { message: String },
}

/// Daemon-to-controller subscription event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// Playback runtime state event.
    Engine(hmp_core::Event),
}

/// Frame encoding/decoding error.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame size {0} exceeds limit {MAX_FRAME}")]
    TooLarge(usize),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Encode a JSON message with a four-byte little-endian length prefix.
pub fn encode_frame<T: Serialize>(message: &T) -> Result<Vec<u8>, FrameError> {
    let payload = serde_json::to_vec(message)?;
    let total = payload.len() + 4;
    if total > MAX_FRAME {
        return Err(FrameError::TooLarge(total));
    }
    let mut frame = Vec::with_capacity(total);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decode one complete length-prefixed JSON frame.
pub fn decode_frame<T: serde::de::DeserializeOwned>(frame: &[u8]) -> Result<T, FrameError> {
    if frame.len() < 4 {
        return Err(invalid_frame("frame is shorter than its length prefix"));
    }
    let payload_len = u32::from_le_bytes(frame[..4].try_into().expect("four-byte prefix")) as usize;
    if payload_len > MAX_FRAME || payload_len + 4 != frame.len() {
        return Err(invalid_frame("frame length prefix does not match payload"));
    }
    serde_json::from_slice(&frame[4..]).map_err(FrameError::Json)
}

fn invalid_frame(message: &'static str) -> FrameError {
    FrameError::Json(serde_json::Error::io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_messages_roundtrip() {
        let messages = [
            Request::Hello {
                protocol: PROTOCOL_VERSION,
            },
            Request::Subscribe {
                frontend_lease: true,
            },
            Request::Subscribe {
                frontend_lease: false,
            },
        ];
        for message in messages {
            let frame = encode_frame(&message).unwrap();
            assert_eq!(decode_frame::<Request>(&frame).unwrap(), message);
        }
        assert_eq!(
            decode_frame::<Response>(
                &encode_frame(&Response::Hello {
                    protocol: PROTOCOL_VERSION,
                })
                .unwrap(),
            )
            .unwrap(),
            Response::Hello {
                protocol: PROTOCOL_VERSION,
            },
        );
    }
}
