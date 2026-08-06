//! 签名算法（对应上游 `qqmusic_api/algorithms/sign.py` 与 `utils/common.py` 的 hash33）。

use base64::Engine;

/// Hash33 哈希（上游 `hash33`），用于 g_tk 等场景。
///
/// `h` 为初始哈希值，默认 0；`g_tk` 场景以 5381 作为初始值。
pub fn hash33(s: &str, h: u32) -> u32 {
    let mut hash = h;
    for c in s.chars() {
        hash = (hash << 5).wrapping_add(hash).wrapping_add(c as u32);
    }
    2_147_483_647 & hash
}

/// zzc 签名（上游 `zzc_sign`），用于 `musics.fcg` 签名请求。
///
/// 结果以 `zzc` 前缀开头，全部小写。
pub fn zzc_sign(payload: &str) -> String {
    use sha1::{Digest, Sha1};

    let hash_hex = hex_uppercase(Sha1::digest(payload.as_bytes()));
    let bytes = hash_hex.as_bytes();

    let part1: String = PART_1_INDEXES.iter().map(|&i| bytes[i] as char).collect();
    let part2: String = PART_2_INDEXES.iter().map(|&i| bytes[i] as char).collect();

    let mut part3 = [0u8; 20];
    for (i, &v) in SCRAMBLE_VALUES.iter().enumerate() {
        // SHA1 十六进制字符串在此索引处恒为两位 ASCII hex，解析不会失败
        let byte = u8::from_str_radix(&hash_hex[i * 2..i * 2 + 2], 16).unwrap_or(0);
        part3[i] = v ^ byte;
    }
    // 上游 b64encode 后剔除 `\` `/` `+` `=` 字符
    let b64_part = base64::engine::general_purpose::STANDARD
        .encode(part3)
        .replace(['\\', '/', '+', '='], "");
    format!("zzc{part1}{b64_part}{part2}").to_lowercase()
}

fn hex_uppercase(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.as_ref().len() * 2);
    for b in bytes.as_ref() {
        // 写入 String 不会失败（fmt::Error 只在不可写目标上发生）
        let _ = write!(out, "{b:02X}");
    }
    out
}

/// 签名 part1 索引（上游 `PART_1_INDEXES`）。
const PART_1_INDEXES: [usize; 7] = [23, 14, 6, 36, 16, 7, 19];
/// 签名 part2 索引（上游 `PART_2_INDEXES`）。
const PART_2_INDEXES: [usize; 8] = [16, 1, 32, 12, 19, 27, 8, 5];
/// 签名置换值（上游 `SCRAMBLE_VALUES`）。
const SCRAMBLE_VALUES: [u8; 20] = [
    89, 39, 179, 150, 218, 82, 58, 252, 177, 52, 186, 123, 120, 64, 242, 133, 143, 161, 121, 179,
];

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle 值由 Python 参考实现（L-1124/QQMusicApi @ 108617f）计算得出，
    // 记录于 docs/QQMUSIC_PORTING.md。

    #[test]
    fn hash33_empty_string() {
        assert_eq!(hash33("", 0), 0);
    }

    #[test]
    fn hash33_basic_string() {
        assert_eq!(hash33("abc", 0), 108_966);
    }

    #[test]
    fn hash33_with_seed_5381() {
        assert_eq!(hash33("abc", 5381), 193_485_963);
    }

    #[test]
    fn zzc_sign_empty_payload() {
        assert_eq!(
            zzc_sign(""),
            "zzcf0e03e5gx4qeiq5cfgdyqwu7sdqfsb5fro3aa45053"
        );
    }

    #[test]
    fn zzc_sign_hello() {
        assert_eq!(
            zzc_sign("hello"),
            "zzcfa14dde89n1iwax0l5rimr0qwjexceiov4daaee8d6"
        );
    }

    #[test]
    fn zzc_sign_json_payload() {
        assert_eq!(
            zzc_sign(r#"{"comm":{"ct":24}}"#),
            "zzce52634cbcpispnllkwa6oyvlivnkbgmhts353ac54b"
        );
    }

    #[test]
    fn g_tk_with_musickey() {
        // g_tk = hash33(musickey, 5381)
        assert_eq!(hash33("test_music_key_123", 5381), 988_047_106);
    }

    #[test]
    fn zzc_sign_deterministic() {
        assert_eq!(zzc_sign("same payload"), zzc_sign("same payload"));
    }

    #[test]
    fn zzc_sign_output_shape() {
        // 结果以 zzc 开头且为小写字母数字
        let out = zzc_sign("payload");
        assert!(out.starts_with("zzc"));
        assert!(
            out.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
    }

    // 供后续 g_tk 测试引用 base64 的正确性
    #[test]
    fn base64_engine_is_available() {
        use base64::engine::general_purpose::STANDARD;
        let encoded = STANDARD.encode([89u8, 39, 179]);
        assert_eq!(encoded, "WSez");
    }
}
