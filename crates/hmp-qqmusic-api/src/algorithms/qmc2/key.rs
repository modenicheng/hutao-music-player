//! QMC2 ekey 解析与生成（移植自 jixunmoe/qmc2-rust）。
//!
//! 支持 EncV1 与 EncV2 两种 ekey 格式。

use base64::Engine;
use thiserror::Error;

use super::tea::{derive_tea_key, tea_cbc_decrypt, tea_cbc_encrypt};

/// QMC2 解密错误类型。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Qmc2Error {
    /// ekey 解析失败（Base64 解码错误或格式不符）。
    #[error("ekey 解析失败")]
    EKeyParse,
    /// 密钥派生失败（TEA 解密错误）。
    #[error("QMC2 密钥派生失败")]
    KeyDerive,
}

/// EncV2 前缀。
const ENCV2_PREFIX: &[u8] = b"QQMusic EncV2,Key:";

/// EncV2 第一层 TEA 密钥。
const ENCV2_STAGE1_KEY: &[u8; 16] = b"386ZJY!@#*$%^&)(";

/// EncV2 第二层 TEA 密钥。
const ENCV2_STAGE2_KEY: &[u8; 16] = b"**#!(#$%&^a1cZ,T";

/// 从 Base64 编码的 ekey 字符串解析出原始密钥。
///
/// 自动检测 EncV1 / EncV2 格式。调用方通过 `[u8]` 长度（>300 → RC4，
/// <=300 → Map）选择对应的流密码。
pub fn parse_ekey(ekey: &str) -> Result<Vec<u8>, Qmc2Error> {
    // 去除末尾 NUL 填充
    let ekey = ekey.trim_end_matches('\0');

    let ekey_decoded = base64::engine::general_purpose::STANDARD
        .decode(ekey)
        .map_err(|_| Qmc2Error::EKeyParse)?;

    if ekey_decoded.len() < 8 {
        return Err(Qmc2Error::EKeyParse);
    }

    // 检测 EncV2 格式
    let ekey_decoded = if ekey_decoded.starts_with(ENCV2_PREFIX) {
        // EncV2：两层 TEA 解密 + Base64 解码
        let encv2_blob = &ekey_decoded[ENCV2_PREFIX.len()..];
        let stage1 = tea_cbc_decrypt(encv2_blob, ENCV2_STAGE1_KEY).ok_or(Qmc2Error::KeyDerive)?;
        let stage2 = tea_cbc_decrypt(&stage1, ENCV2_STAGE2_KEY).ok_or(Qmc2Error::KeyDerive)?;
        base64::engine::general_purpose::STANDARD
            .decode(stage2)
            .map_err(|_| Qmc2Error::EKeyParse)?
    } else {
        ekey_decoded
    };

    if ekey_decoded.len() < 8 {
        return Err(Qmc2Error::EKeyParse);
    }

    let (header, body) = ekey_decoded.split_at(8);
    let tea_key = derive_tea_key(header);
    let body = tea_cbc_decrypt(body, &tea_key).ok_or(Qmc2Error::KeyDerive)?;

    Ok([header, &body].concat())
}

/// 从原始密钥生成 Base64 编码的 ekey 字符串（EncV1 格式）。
///
/// 主要供测试与构造 fixture 使用，生产环境不应生成 ekey。
pub fn generate_ekey(key: &[u8]) -> String {
    assert!(key.len() >= 8, "key must be at least 8 bytes");
    let (key_header, key_body) = key.split_at(8);
    let tea_key = derive_tea_key(key_header);
    let encrypted_body = tea_cbc_encrypt(key_body, &tea_key);
    let ekey_encoded = [key_header, &encrypted_body].concat();
    base64::engine::general_purpose::STANDARD.encode(ekey_encoded)
}

/// 从 `&[u8]` 引用复制出密钥（供 `decrypt_factory` 使用）。
pub(crate) fn key_from_ref(key: &[u8]) -> Vec<u8> {
    key.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ekey_decodes_reference_vector() {
        let ekey = "VGhpcyBpcyBHFWEh4cjZ1Vi7rJ56XeoPlqGM1sxBGPg7mt89umKclFBr9iqfmFdS";
        let decoded = parse_ekey(ekey).unwrap();
        assert_eq!(decoded, b"This is a test key for test purpose :D".to_vec());
    }

    #[test]
    fn generate_parse_roundtrip() {
        let original = b"12345678...test data by Jixun".to_vec();
        let ekey = generate_ekey(&original);
        let parsed = parse_ekey(&ekey).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn parse_ekey_rejects_bad_base64() {
        assert!(matches!(
            parse_ekey("!!!not-valid-base64!!!"),
            Err(Qmc2Error::EKeyParse)
        ));
    }

    #[test]
    fn parse_ekey_rejects_short() {
        // "aGk=" 解码为 "hi"（2 字节，不足 8）
        assert!(matches!(parse_ekey("aGk="), Err(Qmc2Error::EKeyParse)));
    }

    #[test]
    fn parse_ekey_encv2_empty_blob_does_not_panic() {
        // "QQMusic EncV2,Key:" with empty blob — stage1 TEA fails, no panic.
        let ekey = "UVFNdXNpYyBFbmNWMixLZXk6";
        assert!(matches!(parse_ekey(ekey), Err(Qmc2Error::KeyDerive)));
    }

    #[test]
    fn parse_ekey_encv2_short_final_decode_rejected() {
        // Encrypt a 1-byte base64 string through both TEA layers. After
        // double-decryption the final base64 decodes to 1 byte < 8 → EKeyParse.
        let inner = b"AQ==";
        let stage2 = tea_cbc_encrypt(inner, ENCV2_STAGE2_KEY);
        let stage1 = tea_cbc_encrypt(&stage2, ENCV2_STAGE1_KEY);
        let payload = [ENCV2_PREFIX, &stage1].concat();
        let ekey = base64::engine::general_purpose::STANDARD.encode(&payload);
        assert!(matches!(parse_ekey(&ekey), Err(Qmc2Error::EKeyParse)));
    }

    #[test]
    fn parse_ekey_trims_nul_padding() {
        let original = b"12345678...test data by Jixun".to_vec();
        let ekey = generate_ekey(&original);
        // 添加 NUL 填充
        let padded = format!("{}\0\0", ekey);
        let parsed = parse_ekey(&padded).unwrap();
        assert_eq!(parsed, original);
    }
}
