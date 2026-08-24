mod core;
mod playback;

pub use core::{CoreBridge, CoreCommandSender};
pub use playback::{active_lyric_index, elapsed_text, is_playing, progress, remaining_text};
