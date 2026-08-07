//! 算法模块（对应上游 `algorithms/`）。

pub mod qrc;
pub mod tripledes;

/// QMC2 加密音频解密（移植自 jixunmoe/qmc2-rust MIT、bczhc/qmc-decode GPL-3.0、
/// TarsCpp TC_Tea BSD-3-Clause）。
pub mod qmc2;

pub use qrc::qrc_decrypt;
