//! QMC2 加密音频解密算法。
//!
//! 移植自：
//! - [jixunmoe/qmc2-rust](https://github.com/jixunmoe/qmc2-rust)（MIT）
//! - [bczhc/qmc-decode](https://github.com/bczhc/qmc-decode)（GPL-3.0）
//! - TarsCpp TC_Tea（BSD-3-Clause）

pub mod cipher;
pub mod detect;
pub mod key;
pub mod tea;

pub use cipher::{Qmc2Cipher, decrypt_factory};
pub use detect::{Footer, detect_footer};
pub use key::{Qmc2Error, generate_ekey, parse_ekey, parse_ekey_decoded};
