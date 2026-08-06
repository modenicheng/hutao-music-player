//! QRC 加密歌词解密（对应上游 `algorithms/__init__.py::qrc_decrypt`）。
//!
//! 算法：hex 解码 → 自定义 3DES 逐 8 字节块解密（ECB 模式，
//! 密钥 `!@#)(*$%123ZXC!@!@#)(NHL`）→ zlib 解压 → UTF-8。
//! 3DES 为上游自定义变体（见 `tripledes` 模块），与标准 DES 不兼容。

use crate::algorithms::tripledes::{DECRYPT, tripledes_crypt, tripledes_key_setup};
use crate::error::QqMusicError;

/// QRC 3DES 密钥（上游 `_QRC_3DES_KEY`）。
const QRC_3DES_KEY: &[u8; 24] = b"!@#)(*$%123ZXC!@!@#)(NHL";

/// QRC 解码（上游 `qrc_decrypt`）。
///
/// 输入为加密歌词 hex 字符串，解密失败时返回
/// [`QqMusicError::InvalidResponse`]。
pub fn qrc_decrypt(encrypted: &str) -> Result<String, QqMusicError> {
    if encrypted.is_empty() {
        return Ok(String::new());
    }

    let encrypted_bytes = hex_decode(encrypted)
        .map_err(|_| QqMusicError::InvalidResponse("QRC 解密失败: 无效的 hex 数据".into()))?;
    let plain = decrypt_3des_ecb(&encrypted_bytes)?;

    // zlib 解压
    let mut out = Vec::with_capacity(plain.len() * 2);
    let mut decompress = flate2::Decompress::new(true);
    decompress
        .decompress_vec(&plain, &mut out, flate2::FlushDecompress::Finish)
        .map_err(|e| QqMusicError::InvalidResponse(format!("QRC 解密失败: {e}")))?;

    String::from_utf8(out)
        .map_err(|_| QqMusicError::InvalidResponse("QRC 解密失败: 非 UTF-8 数据".into()))
}

/// hex 字符串解码（大小写不敏感）。
fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for chunk in bytes.chunks(2) {
        if chunk.len() != 2 {
            return Err(());
        }
        let hi = hex_val(chunk[0]).ok_or(())?;
        let lo = hex_val(chunk[1]).ok_or(())?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// 3DES-EDE ECB 逐块解密（上游 `qrc_decrypt` 循环）。
fn decrypt_3des_ecb(data: &[u8]) -> Result<Vec<u8>, QqMusicError> {
    if data.is_empty() || data.len() % 8 != 0 {
        return Err(QqMusicError::InvalidResponse(
            "QRC 解密失败: 数据长度非 8 的倍数".into(),
        ));
    }

    let schedule = tripledes_key_setup(QRC_3DES_KEY, DECRYPT);
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks(8) {
        out.extend_from_slice(&tripledes_crypt(chunk, &schedule));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 从真实 fixture 读取加密歌词 hex。
    fn fixture_encrypted() -> String {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/lyric/encrypted.json"
        );
        let body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        body["req_0"]["data"]["lyric"]
            .as_str()
            .expect("fixture lyric")
            .to_owned()
    }

    #[test]
    fn decrypts_real_fixture_lyric() {
        let encrypted = fixture_encrypted();
        let plain = qrc_decrypt(&encrypted).expect("decrypt fixture");
        // 解密后应为标准 LRC：含 [ti:] 元数据与 [mm:ss.xx] 时间戳行
        assert!(
            plain.contains('[') && plain.contains(']'),
            "decrypted lyric should be LRC format, got: {}",
            &plain[..plain.len().min(80)]
        );
        assert!(plain.contains('\n'), "LRC 歌词应为多行");
    }

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(qrc_decrypt("").unwrap(), "");
    }

    #[test]
    fn invalid_hex_rejected() {
        assert!(matches!(
            qrc_decrypt("zzz"),
            Err(QqMusicError::InvalidResponse(_))
        ));
    }

    #[test]
    fn non_multiple_of_8_rejected() {
        assert!(qrc_decrypt("001122334455").is_err());
    }
}
