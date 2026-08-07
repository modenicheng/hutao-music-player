//! QMC2 文件尾部检测（移植自 jixunmoe/qmc2-rust / bczhc/qmc-decode）。
//!
//! 检测加密文件尾部的 ekey 元数据，支持 QTag（v2）与 V1 两种格式。

/// 文件尾部元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Footer {
    /// QTag 格式（v2）：audio_len 为加密音频的字节数。
    QTag {
        /// 加密音频的字节数。
        audio_len: u32,
    },
    /// V1 格式：audio_len 为加密音频的字节数（文件总长 − 4 − key_size）。
    V1 {
        /// 加密音频的字节数。
        audio_len: u32,
    },
}

/// QTag 魔数（小端序 u32 的 "QTag"）。
const MAGIC_QTAG: u32 = 0x6761_5451;

/// 已知最大 V1 密钥长度（0x400）。
const MAX_V1_KEY_SIZE: usize = 0x400;

/// 检测文件尾部中的 ekey 元数据。
///
/// - `total_len`：文件总长（用于计算音频部分长度）。
/// - `tail`：文件尾部字节（至少 8 字节，通常取最后 0x40 字节）。
///
/// 返回 `None` 表示未识别到已知格式或数据不足。
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

        // 音频长度 = 文件总长 - 8 (QTag+size) - payload_size
        let audio_len = total_len.saturating_sub(8 + payload_size) as u32;
        return Some(Footer::QTag { audio_len });
    }

    // 尝试 V1 格式：最后 4 字节为小端序 key_size
    let key_size = eof_magic as usize;
    // 保证尾部不会延伸到文件起始位置之前
    if key_size > 0 && key_size <= MAX_V1_KEY_SIZE && key_size + 4 <= total_len {
        // 音频长度 = 文件总长 − 4（尾部长度值） − key_size
        let audio_len = total_len.saturating_sub(4 + key_size) as u32;
        return Some(Footer::V1 { audio_len });
    }

    // eof_magic == 0 或无法识别 → None
    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn detect_v1_layout() {
        // V1 尾部：[key_data "aaaa", key_size 4u32 LE]
        // total 8 bytes, key_size=4 → audio_len = 8 − 4 − 4 = 0
        let tail: Vec<u8> = [b"aaaa".as_slice(), &4u32.to_le_bytes()].concat();
        let len = tail.len();
        let result = detect_footer(len, &tail);
        assert_eq!(result, Some(Footer::V1 { audio_len: 0 }));
    }

    #[test]
    fn detect_v1_key_size_upper_bound() {
        // key_size = 0x400 (1024)，应通过
        // tail: 0x400 key bytes + 4-byte length → total=0x404, guard passes, audio_len=0
        let mut tail = vec![0x41u8; 0x400];
        tail.extend_from_slice(&(0x400u32).to_le_bytes());
        let len = tail.len();
        assert!(matches!(detect_footer(len, &tail), Some(Footer::V1 { .. })));

        // key_size = 0x401 (1025)，应拒绝
        let mut tail = vec![0x41u8; 0x401];
        tail.extend_from_slice(&(0x401u32).to_le_bytes());
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
        // 16 音频字节 + key_data + key_size:
        // total=24, key_size=4 → audio_len = 24 − 4 − 4 = 16
        let mut data = vec![0u8; 16];
        data.extend_from_slice(b"aaaa");
        data.extend_from_slice(&4u32.to_le_bytes());
        let total = data.len();
        let result = detect_footer(total, &data);
        assert_eq!(result, Some(Footer::V1 { audio_len: 16 }));
    }
}
