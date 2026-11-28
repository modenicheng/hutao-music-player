//! QMC2 文件尾部检测（移植自 jixunmoe/qmc2-rust / bczhc/qmc-decode）。
//!
//! 检测加密文件尾部的 ekey 元数据，支持 QTag（v2）与 V1 两种格式。
//! V1 的 key 区可以是 Base64 ekey 文本，也可以是历史形态的已解码 ekey
//! blob（原始字节）；检测时会校验两种形态的可解析性（见 [`detect_footer`]）。

use super::key::{parse_ekey, parse_ekey_decoded};

/// 文件尾部元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Footer {
    /// QTag 格式（v2）：audio_len 为加密音频的字节数。
    QTag {
        /// 加密音频的字节数。
        audio_len: usize,
    },
    /// V1 格式：audio_len 为加密音频的字节数（文件总长 − 4 − key_size）。
    ///
    /// 只有在 key 区能解析出合法 ekey（文本或已解码 blob）时才会被
    /// [`detect_footer`] 返回。
    V1 {
        /// 加密音频的字节数。
        audio_len: usize,
    },
}

/// QTag 魔数（小端序 u32 的 "QTag"）。
const MAGIC_QTAG: u32 = 0x6761_5451;

/// 已知最大 V1 密钥长度（0x400 = 1024 字节）。
///
/// V1 的 key 区通常是 Base64 ekey 文本；只探测了短尾部（如最后 0x40
/// 字节）的调用方需要按 [`v1_key_size_hint`] 补拉完整 key 区后再调用
/// [`detect_footer`]。
pub const MAX_V1_KEY_SIZE: usize = 0x400;

/// 校验 V1 key 区能否解析出合法 ekey。
///
/// 兼容两种历史形态：
/// 1. UTF-8 文本且 [`parse_ekey`] 成功（当前 V1/QTag 的 Base64 文本）；
/// 2. [`parse_ekey_decoded`] 成功（key 区直接存已解码的 ekey blob）。
///
/// 两者都失败说明这 4 字节只是恰好像 key_size 的音频数据，不能当 footer。
/// 两种解析都有 TEA padding / 长度校验，随机字节通过的概率可忽略。
fn v1_key_is_valid(key_area: &[u8]) -> bool {
    std::str::from_utf8(key_area).is_ok_and(|text| parse_ekey(text).is_ok())
        || parse_ekey_decoded(key_area).is_ok()
}

/// 读取最后 4 字节给出的 V1 key 区长度（合法范围 `1..=MAX_V1_KEY_SIZE`）。
///
/// 这里**只做长度判据、不校验内容**：0x40 探测窗可能不覆盖完整 key 区，
/// 此时 [`detect_footer`] 会因无法校验而返回 `None`；调用方可据此补拉
/// `4 + key_size` 字节的精确尾部后重试。`key_size == 0` 或超界返回 `None`。
pub fn v1_key_size_hint(tail: &[u8]) -> Option<usize> {
    let start = tail.len().checked_sub(4)?;
    let key_size = u32::from_le_bytes([
        tail[start],
        tail[start + 1],
        tail[start + 2],
        tail[start + 3],
    ]) as usize;
    (key_size > 0 && key_size <= MAX_V1_KEY_SIZE).then_some(key_size)
}

/// 检测文件尾部中的 ekey 元数据。
///
/// - `total_len`：文件总长（用于计算音频部分长度）。
/// - `tail`：文件尾部字节（至少 8 字节）。QTag 只看末尾 8 字节；V1 只有在
///   `tail` 完整覆盖末尾 `4 + key_size` 字节、且 key 区能解析出合法 ekey
///   （Base64 文本或已解码 blob）时才成立——覆盖不足或两种解析都失败一律
///   按「无 footer」返回 `None`，绝不静默截断。
///
/// 返回 `None` 表示未识别到已知格式、数据不足以校验，或 ekey 不可解析。
pub fn detect_footer(total_len: usize, tail: &[u8]) -> Option<Footer> {
    if tail.len() < 8 {
        return None;
    }

    // 读取最后 4 字节，尝试匹配 QTag 魔数
    let eof_magic = u32::from_le_bytes([
        tail[tail.len() - 4],
        tail[tail.len() - 3],
        tail[tail.len() - 2],
        tail[tail.len() - 1],
    ]);

    if eof_magic == MAGIC_QTAG {
        // QTag 格式：前 4 字节为大端序 payload_size
        let payload_size = u32::from_be_bytes([
            tail[tail.len() - 8],
            tail[tail.len() - 7],
            tail[tail.len() - 6],
            tail[tail.len() - 5],
        ]) as usize;

        let footer_len = payload_size.checked_add(8)?;
        if footer_len <= total_len {
            return Some(Footer::QTag {
                audio_len: total_len - footer_len,
            });
        }
        return None;
    }

    // 尝试 V1 格式：最后 4 字节为小端序 key_size
    let key_size = v1_key_size_hint(tail)?;
    // 保证尾部不会延伸到文件起始位置之前
    if key_size + 4 > total_len {
        return None;
    }
    // 探测窗必须覆盖完整 key 区：否则无法确认这 4 字节不是音频数据。
    if tail.len() < key_size + 4 {
        return None;
    }
    let key_start = tail.len() - key_size - 4;
    if !v1_key_is_valid(&tail[key_start..key_start + key_size]) {
        return None;
    }

    // 音频长度 = 文件总长 − 4（尾部长度值） − key_size
    Some(Footer::V1 {
        audio_len: total_len - 4 - key_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algorithms::qmc2::key::generate_ekey;

    /// 构造 V1 尾部：合法 ekey 文本 + 其长度（LE u32）。
    fn v1_tail(ekey: &str) -> Vec<u8> {
        let mut tail = ekey.as_bytes().to_vec();
        tail.extend_from_slice(&(ekey.len() as u32).to_le_bytes());
        tail
    }

    /// 把 ekey 文本解码回「已解码 ekey blob」（历史 V1 key 区形态）。
    fn decoded_ekey_blob(ekey: &str) -> Vec<u8> {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(ekey)
            .unwrap()
    }

    fn test_ekey() -> String {
        generate_ekey(b"0123456789abcdefghij")
    }

    #[test]
    fn detect_qtag_layout() {
        // 构造 QTag 尾部：[ekey "aaaa,", song_id "18,", version "2,", big-endian 10u32, "QTag"]
        let tail = [
            b"aaaa,".as_slice(),
            b"18,",
            b"2,",
            &10u32.to_be_bytes(),
            b"QTag",
        ]
        .concat();
        let len = tail.len();
        let result = detect_footer(len, &tail);
        assert_eq!(result, Some(Footer::QTag { audio_len: 0 }));

        // 在前方添加 16 字节音频前缀
        let mut with_audio = vec![0u8; 16];
        with_audio.extend_from_slice(&tail);
        let total = with_audio.len();
        let result = detect_footer(total, &with_audio);
        assert_eq!(result, Some(Footer::QTag { audio_len: 16 }));
    }

    #[test]
    fn detect_qtag_rejects_oversized_metadata() {
        let tail = [&20u32.to_be_bytes()[..], &b"QTag"[..]].concat();
        assert!(detect_footer(8, &tail).is_none());
    }

    #[test]
    fn detect_qtag_rejects_u32_max_payload() {
        // payload_size = u32::MAX (0xFFFFFFFF) 会导致 checked_add(8) 溢出，
        // 应返回 None 而不是 panic（防御 32 位目标）。
        let tail = [&u32::MAX.to_be_bytes()[..], b"QTag"].concat();
        assert!(detect_footer(8, &tail).is_none());
    }

    #[test]
    fn detect_v1_layout() {
        // V1 尾部：[ekey 文本, key_size u32 LE]；total = 尾部长度 → audio_len 0
        let ekey = test_ekey();
        let tail = v1_tail(&ekey);
        let result = detect_footer(tail.len(), &tail);
        assert_eq!(result, Some(Footer::V1 { audio_len: 0 }));
    }

    #[test]
    fn detect_v1_rejects_unparseable_key_area() {
        // 普通字节流：尾 4 字节恰为 1u32 LE，但前 1 字节不是 ekey →
        // 必须按「无 footer」处理，调用方因此使用 audio_len == total_len。
        let mut tail = vec![0u8; 4]; // 音频前缀，让尾部长度过 8 字节门槛
        tail.push(0x00); // 1 字节「key 区」，不可能是 Base64 ekey
        tail.extend_from_slice(&1u32.to_le_bytes());
        assert!(detect_footer(tail.len(), &tail).is_none());

        // key 区长度合法（64）但内容是 NUL，不可能是 Base64 ekey。
        let mut tail = vec![0u8; 64];
        tail.extend_from_slice(&64u32.to_le_bytes());
        assert!(detect_footer(tail.len(), &tail).is_none());
    }

    #[test]
    fn detect_v1_accepts_decoded_ekey_blob_key_area() {
        // 历史 V1：key 区直接存已解码 ekey blob（非 Base64 文本）。
        let key = b"0123456789abcdefghij";
        let ekey = test_ekey();
        let blob = decoded_ekey_blob(&ekey);
        if let Ok(text) = std::str::from_utf8(&blob) {
            assert!(
                parse_ekey(text).is_err(),
                "fixture must exercise the decoded-blob branch"
            );
        }

        let mut tail = vec![0u8; 16];
        tail.extend_from_slice(&blob);
        tail.extend_from_slice(&(blob.len() as u32).to_le_bytes());
        assert_eq!(
            detect_footer(tail.len(), &tail),
            Some(Footer::V1 { audio_len: 16 })
        );

        // 同一份 ekey 的两种形态都应由 parse_ekey_decoded 还原成同一 raw key。
        assert_eq!(parse_ekey_decoded(&blob).unwrap(), key.to_vec());
    }

    #[test]
    fn detect_v1_rejects_random_bytes_key_area() {
        // 固定内容：既不是合法 UTF-8/Base64，也不应通过 TEA padding 校验。
        // （避免用「恰好能过 padding 的随机种子」当夹具。）
        let key_area: Vec<u8> = (0..64u8)
            .map(|i| i.wrapping_mul(37).wrapping_add(11))
            .collect();
        assert!(std::str::from_utf8(&key_area).is_err());

        let mut tail = vec![0u8; 16];
        tail.extend_from_slice(&key_area);
        tail.extend_from_slice(&(key_area.len() as u32).to_le_bytes());
        assert!(detect_footer(tail.len(), &tail).is_none());
    }

    #[test]
    fn detect_v1_requires_full_key_region() {
        // 尾部只给最后 8 字节：key 区（44 字节）未覆盖 → 无法校验 → None。
        // 调用方需按 v1_key_size_hint 补拉后再检测。
        let full = v1_tail(&test_ekey());
        let total = 100 + full.len();
        let short = &full[full.len() - 8..];
        assert!(detect_footer(total, short).is_none());
        assert_eq!(v1_key_size_hint(short), Some(full.len() - 4));

        // 覆盖完整 key 区后成立。
        assert_eq!(
            detect_footer(total, &full),
            Some(Footer::V1 { audio_len: 100 })
        );
    }

    #[test]
    fn detect_v1_key_size_upper_bound() {
        // 756 字节原始密钥生成的 EncV1 ekey 恰好 1024 字符（key_size 上限）。
        let ekey = generate_ekey(&vec![0x41u8; 756]);
        assert_eq!(ekey.len(), MAX_V1_KEY_SIZE);
        let mut tail = v1_tail(&ekey);
        let total = tail.len();
        assert_eq!(
            detect_footer(total, &tail),
            Some(Footer::V1 { audio_len: 0 })
        );

        // key_size = 0x401 (1025)，应拒绝（长度判据即可，无需合法内容）
        tail = vec![0x41u8; MAX_V1_KEY_SIZE + 1];
        tail.extend_from_slice(&((MAX_V1_KEY_SIZE + 1) as u32).to_le_bytes());
        let len = tail.len();
        assert!(detect_footer(len, &tail).is_none());
    }

    #[test]
    fn detect_v1_key_size_exceeds_file() {
        // key_size + 4 > total_len → None
        // 4-byte tail with key_size=1, but total_len=3 → guard fails
        let tail = 1u32.to_le_bytes();
        assert!(detect_footer(3, &tail).is_none());
    }

    #[test]
    fn detect_rejects_tiny_and_zero() {
        // 不足 8 字节 → None
        assert!(detect_footer(7, &[0u8; 7]).is_none());

        // 8 字节全零 → None（V1 key_size = 0 不通过，QTag 魔数不匹配）
        assert!(detect_footer(8, &[0u8; 8]).is_none());
    }

    #[test]
    fn detect_v1_with_audio_prefix() {
        // 16 音频字节 + 合法 ekey 文本 + key_size:
        // total = 16 + 48，key_size = 44 → audio_len = 16
        let ekey = test_ekey();
        let mut data = vec![0u8; 16];
        data.extend_from_slice(&v1_tail(&ekey));
        let total = data.len();
        let result = detect_footer(total, &data);
        assert_eq!(result, Some(Footer::V1 { audio_len: 16 }));
    }

    #[test]
    fn v1_key_size_hint_is_length_only() {
        assert_eq!(v1_key_size_hint(&1u32.to_le_bytes()), Some(1));
        assert_eq!(
            v1_key_size_hint(&(MAX_V1_KEY_SIZE as u32).to_le_bytes()),
            Some(MAX_V1_KEY_SIZE)
        );
        assert_eq!(v1_key_size_hint(&0u32.to_le_bytes()), None);
        assert_eq!(
            v1_key_size_hint(&((MAX_V1_KEY_SIZE + 1) as u32).to_le_bytes()),
            None
        );
        assert_eq!(v1_key_size_hint(&[0u8; 3]), None);
    }
}
