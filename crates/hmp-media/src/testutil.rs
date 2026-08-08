//! 测试辅助工具：构造 QMC2 加密数据、缓存路径等。
//!
//! 供 `decrypt` 与 `proxy` 模块测试共用。
//!
//! 仅在 `#[cfg(test)]` 时编译。

use std::path::PathBuf;

use hmp_qqmusic_api::algorithms::qmc2::{decrypt_factory, key::generate_ekey};

/// 构造 QMC2 加密测试数据。
///
/// - `plaintext`：明文音频数据（应以已知魔数开头，如 `b"fLaC"`）
/// - `key`：原始密钥字节
/// - `with_footer`：是否在末尾附加 V1 尾部 `[key_bytes][key_len LE u32]`
///
/// 返回 `(encrypted_data, ekey)`。
pub(crate) fn make_encrypted(plaintext: &[u8], key: &[u8], with_footer: bool) -> (Vec<u8>, String) {
    let ekey = generate_ekey(key);
    let cipher = decrypt_factory(&ekey).unwrap();

    let mut encrypted = plaintext.to_vec();
    cipher.decrypt(0, &mut encrypted);

    if with_footer {
        let key_len = key.len() as u32;
        encrypted.extend_from_slice(key);
        encrypted.extend_from_slice(&key_len.to_le_bytes());
    }

    (encrypted, ekey)
}

/// 为当前进程创建唯一的临时缓存根目录路径。
pub(crate) fn test_cache_root() -> PathBuf {
    std::env::temp_dir().join(format!("hmp-media-test-{}", std::process::id()))
}
