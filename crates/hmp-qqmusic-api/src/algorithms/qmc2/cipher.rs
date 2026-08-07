//! QMC2 流密码实现（移植自 jixunmoe/qmc2-rust）。
//!
//! - 密钥长度 <= 300 → Map 密码（基于旋转的 XOR）
//! - 密钥长度 > 300  → RC4 变体（分段流密码）

use super::key::{Qmc2Error, key_from_ref, parse_ekey};

/// QMC2 流密码 trait。
pub trait Qmc2Cipher {
    /// 解密从 `offset` 开始的 `buf` 字节（原地修改）。
    fn decrypt(&self, offset: usize, buf: &mut [u8]);
}

// ---------------------------------------------------------------------------
// Map 密码（密钥长度 ≤ 300）
// ---------------------------------------------------------------------------

/// 基于 Map 旋转的流密码。
struct QmcMapCipher {
    key: Vec<u8>,
}

impl QmcMapCipher {
    fn new(key: &[u8]) -> Self {
        QmcMapCipher { key: key.to_vec() }
    }

    /// 根据索引扰乱密钥字节（上游 `scramble_by_index`）。
    #[inline]
    fn scramble(value: u8, index: usize) -> u8 {
        let rotation = ((index as u32).wrapping_add(4)) & 0b111;
        let left = value.wrapping_shl(rotation);
        let right = value.wrapping_shr(rotation);
        left | right
    }

    /// 根据 offset 计算 XOR 字节（上游 `mapL`）。
    #[inline]
    fn map_l(&self, offset: usize) -> u8 {
        let mut offset_local = offset;
        if offset_local > 0x7FFF {
            offset_local %= 0x7FFF;
        }
        let index = (offset_local * offset_local + 71214) % self.key.len();
        QmcMapCipher::scramble(self.key[index], index)
    }
}

impl Qmc2Cipher for QmcMapCipher {
    fn decrypt(&self, offset: usize, buf: &mut [u8]) {
        for (i, byte) in buf.iter_mut().enumerate() {
            *byte ^= self.map_l(offset + i);
        }
    }
}

// ---------------------------------------------------------------------------
// RC4 变体（密钥长度 > 300）
// ---------------------------------------------------------------------------

/// 第一段大小（特殊算法）。
const FIRST_SEGMENT_SIZE: usize = 0x80;
/// 其余段大小。
const OTHER_SEGMENT_SIZE: usize = 0x1400;

/// RC4 变体流密码。
struct QmcRc4Cipher {
    /// RC4 S 盒（初始化后的状态）。
    s: Vec<u8>,
    /// 哈希基值，用于分段密钥计算。
    hash: u32,
    /// RC4 原始密钥。
    rc4_key: Vec<u8>,
}

impl QmcRc4Cipher {
    fn new(rc4_key: &[u8]) -> Self {
        let n = rc4_key.len();
        let mut s = vec![0u8; n];
        for (i, b) in s.iter_mut().enumerate() {
            *b = i as u8;
        }

        let mut j = 0usize;
        for (i, &key_byte) in rc4_key.iter().enumerate() {
            j = j
                .wrapping_add(s[i] as usize)
                .wrapping_add(key_byte as usize)
                % n;
            s.swap(i, j);
        }

        let hash = QmcRc4Cipher::calc_hash_base(rc4_key);

        QmcRc4Cipher {
            s,
            hash,
            rc4_key: rc4_key.to_vec(),
        }
    }

    /// 计算哈希基值（上游 `calc_hash_base`）。
    fn calc_hash_base(data: &[u8]) -> u32 {
        let mut hash: u32 = 1;
        for &value in data {
            let value = u32::from(value);
            if value == 0 {
                continue;
            }
            let next_hash = hash.wrapping_mul(value);
            if next_hash == 0 || next_hash <= hash {
                break;
            }
            hash = next_hash;
        }
        hash
    }

    /// 计算分段密钥（上游 `calc_segment_key`）。
    #[inline]
    fn calc_segment_key(&self, id: usize, seed: u8) -> usize {
        let dividend = f64::from(self.hash);
        let divisor = ((id + 1) * usize::from(seed)) as f64;
        let key = dividend / divisor * 100.0;
        key as u64 as usize
    }

    /// RC4 单步推导（上游 `rc4_derive`）。
    #[inline]
    fn rc4_derive(n: usize, s: &mut [u8], j: &mut usize, k: &mut usize) -> u8 {
        *j = (*j + 1) % n;
        *k = (usize::from(s[*j]) + *k) % n;
        s.swap(*j, *k);
        let index = usize::from(s[*j]) + usize::from(s[*k]);
        s[index % n]
    }

    /// 加密第一段（offset < 0x80）。
    fn encode_first_segment(&self, offset: usize, buf: &mut [u8]) {
        let n = self.rc4_key.len();
        for (i, b) in buf.iter_mut().enumerate() {
            let off = offset + i;
            let key1 = self.rc4_key[off % n];
            let key2 = self.calc_segment_key(off, key1);
            *b ^= self.rc4_key[key2 % n];
        }
    }

    /// 加密其余段。
    fn encode_other_segment(&self, offset: usize, buf: &mut [u8]) {
        let seg_id = offset / OTHER_SEGMENT_SIZE;
        let seg_id_small = seg_id & 0x1FF;

        let mut discard_count = self.calc_segment_key(seg_id, self.rc4_key[seg_id_small]) & 0x1FF;
        discard_count += offset % OTHER_SEGMENT_SIZE;

        let n = self.rc4_key.len();
        let mut s = self.s.clone();
        let mut j = 0usize;
        let mut k = 0usize;
        for _ in 0..discard_count {
            QmcRc4Cipher::rc4_derive(n, &mut s, &mut j, &mut k);
        }

        for b in buf.iter_mut() {
            *b ^= QmcRc4Cipher::rc4_derive(n, &mut s, &mut j, &mut k);
        }
    }
}

impl Qmc2Cipher for QmcRc4Cipher {
    fn decrypt(&self, offset: usize, buf: &mut [u8]) {
        let mut offset = offset;
        let mut len = buf.len();
        let mut i = 0usize;

        // 第一段（特殊算法）
        if offset < FIRST_SEGMENT_SIZE {
            let len_processed = std::cmp::min(len, FIRST_SEGMENT_SIZE - offset);
            self.encode_first_segment(offset, &mut buf[i..i + len_processed]);
            i += len_processed;
            len -= len_processed;
            offset += len_processed;
        }

        // 对齐段
        let to_align = offset % OTHER_SEGMENT_SIZE;
        if to_align != 0 {
            let len_processed = std::cmp::min(len, OTHER_SEGMENT_SIZE - to_align);
            self.encode_other_segment(offset, &mut buf[i..i + len_processed]);
            i += len_processed;
            len -= len_processed;
            offset += len_processed;
        }

        // 批量处理完整段
        while len > OTHER_SEGMENT_SIZE {
            self.encode_other_segment(offset, &mut buf[i..i + OTHER_SEGMENT_SIZE]);
            i += OTHER_SEGMENT_SIZE;
            len -= OTHER_SEGMENT_SIZE;
            offset += OTHER_SEGMENT_SIZE;
        }

        // 末尾不完整段
        if len > 0 {
            self.encode_other_segment(offset, &mut buf[i..i + len]);
        }
    }
}

// ---------------------------------------------------------------------------
// 工厂函数
// ---------------------------------------------------------------------------

/// 根据 ekey 字符串创建对应的流密码。
///
/// 自动根据密钥长度选择 Map（<=300）或 RC4（>300）密码。
pub fn decrypt_factory(ekey: &str) -> Result<Box<dyn Qmc2Cipher>, Qmc2Error> {
    let key = parse_ekey(ekey)?;
    let key = key_from_ref(&key);
    if key.len() > 300 {
        Ok(Box::new(QmcRc4Cipher::new(&key)))
    } else {
        Ok(Box::new(QmcMapCipher::new(&key)))
    }
}

#[cfg(test)]
mod tests {
    use super::super::key::generate_ekey;
    use super::*;

    // ---- Map 密码测试 ----

    const MAP_KEY: [u8; 16] = [
        0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F,
        0x50,
    ];

    #[test]
    fn map_cipher_decrypts_zeroes() {
        let cipher = QmcMapCipher::new(&MAP_KEY);

        // offset 0
        let mut data = [0u8; 16];
        cipher.decrypt(0, &mut data);
        assert_eq!(
            data,
            [
                0x3F, 0x8A, 0xC1, 0x49, 0x3F, 0x49, 0xC1, 0x8A, 0x3F, 0x8A, 0xC1, 0x49, 0x3F, 0x49,
                0xC1, 0x8A
            ]
        );

        // offset 0x7FFF - 8
        let mut data = [0u8; 16];
        cipher.decrypt(0x7FFF - 8, &mut data);
        assert_eq!(
            data,
            [
                0x8A, 0x3F, 0x8A, 0xC1, 0x49, 0x3F, 0x49, 0xC1, 0x8A, 0x8A, 0xC1, 0x49, 0x3F, 0x49,
                0xC1, 0x8A
            ]
        );
    }

    // ---- RC4 密码测试 ----

    fn rc4_key_255() -> Vec<u8> {
        (0u8..=254).collect()
    }

    #[test]
    fn rc4_cipher_first_segment() {
        let key = rc4_key_255();
        let cipher = QmcRc4Cipher::new(&key);
        let mut data = [0u8; 16];
        cipher.decrypt(0, &mut data);
        assert_eq!(data, [0, 50, 16, 8, 5, 3, 2, 1, 1, 1, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn rc4_boundary_segments() {
        let key = rc4_key_255();
        let cipher = QmcRc4Cipher::new(&key);

        // 第一段末尾 + 第二段开头
        let mut data = [0u8; 16];
        cipher.decrypt(FIRST_SEGMENT_SIZE - 8, &mut data);
        assert_eq!(
            data,
            [
                0, 0, 0, 0, 0, 0, 0, 0, 141, 97, 122, 193, 166, 101, 233, 214
            ]
        );

        // 段边界
        let mut data = [0u8; 16];
        cipher.decrypt(OTHER_SEGMENT_SIZE - 8, &mut data);
        assert_eq!(
            data,
            [
                118, 193, 176, 83, 10, 98, 105, 234, 151, 56, 198, 1, 226, 173, 127, 4
            ]
        );
    }

    #[test]
    fn rc4_entire_segment() {
        let key = rc4_key_255();
        let cipher = QmcRc4Cipher::new(&key);

        // 第二段开头
        let mut data = [0u8; 16];
        cipher.decrypt(OTHER_SEGMENT_SIZE, &mut data);
        assert_eq!(
            data,
            [
                151, 56, 198, 1, 226, 173, 127, 4, 181, 165, 171, 21, 82, 152, 195, 210
            ]
        );

        // 完整段 + 1（确认 segment 循环）
        let mut data = vec![0u8; OTHER_SEGMENT_SIZE + 1];
        cipher.decrypt(OTHER_SEGMENT_SIZE, &mut data);
        assert_eq!(
            data[0..16],
            [
                151, 56, 198, 1, 226, 173, 127, 4, 181, 165, 171, 21, 82, 152, 195, 210
            ]
        );
    }

    // ---- 哈希基值测试 ----

    #[test]
    fn hash_base_ignores_zero_bytes() {
        let hash = QmcRc4Cipher::calc_hash_base(&[0xffu8; 16]);
        assert_eq!(hash, 0xfc05fc01);

        // 含 0x00 字节应被跳过，结果相同
        let hash_with_zeros = QmcRc4Cipher::calc_hash_base(&[
            0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff,
        ]);
        assert_eq!(hash_with_zeros, 0xfc05fc01);
    }

    // ---- 工厂测试 ----

    #[test]
    fn decrypt_factory_picks_map_or_rc4() {
        // 20 字节密钥 → Map
        let small_key = vec![0u8; 20];
        let ekey = generate_ekey(&small_key);
        let cipher = decrypt_factory(&ekey).unwrap();
        // 简单烟雾测试：解密一段零数据不 panic
        let mut buf = [0u8; 16];
        cipher.decrypt(0, &mut buf);

        // 700 字节密钥 → RC4
        let large_key = vec![0u8; 700];
        let ekey = generate_ekey(&large_key);
        let cipher = decrypt_factory(&ekey).unwrap();
        cipher.decrypt(0, &mut buf);
    }
}
