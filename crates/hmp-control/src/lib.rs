pub mod protocol;

pub use protocol::{
    Event, FrameError, MAX_FRAME, PROTOCOL_VERSION, Request, Response, decode_frame, encode_frame,
};
