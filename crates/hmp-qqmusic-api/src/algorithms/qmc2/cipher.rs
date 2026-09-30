//! QMC2 流密码实现（移植自 jixunmoe/qmc2-rust）。
//!
//! - 密钥长度 <= 300 → Map 密码（基于旋转的 XOR）
//! - 密钥长度 > 300  → RC4 变体（分段流密码）

use std::sync::Mutex;

use super::key::{Qmc2Error, key_from_ref, parse_ekey};

/// QMC2 流密码 trait。
pub trait Qmc2Cipher: Send + Sync {
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
    /// 当前段密钥流缓存（容量 1 段，5 KiB）：段 id + 该段完整密钥流。
    ///
    /// 流式播放时每个网络 chunk 都会调用 `decrypt`，若每次都克隆 S 盒并重做
    /// 整段丢弃步进，seek 密集场景 CPU 开销显著；改为 miss 时一次性派生整段
    /// 密钥流并缓存，段 id 完全匹配时直接 XOR。首段 0x80 特殊路径不经过缓存。
    /// trait 以 `&self` 解密且可能被并发调用，故用 `Mutex` 提供内部可变性。
    segment_cache: Mutex<Option<(usize, Box<[u8; OTHER_SEGMENT_SIZE]>)>>,
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
            segment_cache: Mutex::new(None),
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
    ///
    /// `buf` 必须完全落在单个 `OTHER_SEGMENT_SIZE` 段内（`decrypt` 的分段
    /// 逻辑保证这一不变量）。RC4 状态从段首开始确定性地演进，因此任意
    /// 段内偏移处的密钥流字节等价于"整段密钥流在相同下标处的字节"，
    /// 据此缓存整段密钥流：段 id 完全匹配则直接 XOR，miss 才派生并替换。
    fn encode_other_segment(&self, offset: usize, buf: &mut [u8]) {
        let seg_id = offset / OTHER_SEGMENT_SIZE;
        let in_seg = offset % OTHER_SEGMENT_SIZE;
        debug_assert!(in_seg + buf.len() <= OTHER_SEGMENT_SIZE);

        // 命中缓存：直接用该段密钥流 XOR。
        {
            let cache = self.lock_segment_cache();
            if let Some((cached_id, keystream)) = cache.as_ref() {
                if *cached_id == seg_id {
                    QmcRc4Cipher::xor_with_keystream(buf, keystream, in_seg);
                    return;
                }
            }
        }

        // miss：从 RC4 初始状态派生整段密钥流（丢弃步进仅含段密钥部分，
        // 段内偏移由下标索引替代，结果与逐次丢弃逐字节一致）。
        let seg_id_small = seg_id & 0x1FF;
        let discard_count = self.calc_segment_key(seg_id, self.rc4_key[seg_id_small]) & 0x1FF;

        let n = self.rc4_key.len();
        let mut s = self.s.clone();
        let mut j = 0usize;
        let mut k = 0usize;
        for _ in 0..discard_count {
            QmcRc4Cipher::rc4_derive(n, &mut s, &mut j, &mut k);
        }

        let mut keystream = Box::new([0u8; OTHER_SEGMENT_SIZE]);
        for byte in keystream.iter_mut() {
            *byte = QmcRc4Cipher::rc4_derive(n, &mut s, &mut j, &mut k);
        }

        QmcRc4Cipher::xor_with_keystream(buf, &keystream, in_seg);
        *self.lock_segment_cache() = Some((seg_id, keystream));
    }

    /// 锁定段密钥流缓存。
    ///
    /// 锁中毒时直接取出内部数据：缓存内容是确定性派生的纯数据，
    /// 不存在半更新的一致性风险，恢复优于 panic（也符合本 crate
    /// 生产代码禁用 unwrap/expect 的约束）。
    #[inline]
    fn lock_segment_cache(
        &self,
    ) -> std::sync::MutexGuard<'_, Option<(usize, Box<[u8; OTHER_SEGMENT_SIZE]>)>> {
        self.segment_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 用段内偏移 `in_seg` 处起的密钥流 XOR `buf`。
    #[inline]
    fn xor_with_keystream(buf: &mut [u8], keystream: &[u8; OTHER_SEGMENT_SIZE], in_seg: usize) {
        let ks = &keystream[in_seg..in_seg + buf.len()];
        for (b, ks) in buf.iter_mut().zip(ks) {
            *b ^= ks;
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
        if key.len() < 512 {
            // 参考实现按 seg_id & 0x1FF 索引，短 RC4 密钥会越界；拒绝畸形输入。
            return Err(Qmc2Error::KeyDerive);
        }
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

    // ---- 段密钥流缓存测试 ----

    /// 生成非平凡明文（避免全 0 数据掩盖密钥流错误）。
    fn sample_data(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8 ^ 0x5A).collect()
    }

    #[test]
    fn rc4_large_buffer_matches_chunked_decrypt() {
        // 顺序大 buffer 一次解密 == 分小块多次解密（跨段一致性回归钉住）。
        // 覆盖：首段 0x80 特殊路径、段内对齐、跨 0x1400 段边界的 chunk。
        let key = rc4_key_255();
        let cipher = QmcRc4Cipher::new(&key);

        let total = FIRST_SEGMENT_SIZE + OTHER_SEGMENT_SIZE * 3 + 0x321;
        let mut whole = sample_data(total);
        cipher.decrypt(0, &mut whole);

        // 块大小故意不整除 0x1400，保证有 chunk 横跨段边界。
        let mut chunked = sample_data(total);
        let chunk_size = 0x371;
        let mut pos = 0;
        while pos < chunked.len() {
            let end = std::cmp::min(pos + chunk_size, chunked.len());
            cipher.decrypt(pos, &mut chunked[pos..end]);
            pos = end;
        }

        assert_eq!(whole, chunked, "大 buffer 一次解密与分块解密必须逐字节一致");
    }

    #[test]
    fn rc4_repeated_decrypt_same_segment_uses_cache() {
        // 同段重复解密两次结果一致（缓存命中路径正确性）。
        let key = rc4_key_255();
        let cipher = QmcRc4Cipher::new(&key);

        let offset = OTHER_SEGMENT_SIZE * 2 + 0x500;
        let len = 0x800;
        let plain = sample_data(len);
        let mut first = plain.clone();
        cipher.decrypt(offset, &mut first); // miss：派生并填充缓存

        let mut second = plain.clone();
        cipher.decrypt(offset, &mut second); // 命中缓存
        assert_eq!(first, second, "同段重复解密（缓存命中）必须与首次解密一致");

        // 同段内不同窗口（不同段内偏移）命中缓存时，也必须对应同一密钥流：
        // 两侧明文同为 plain[0x100..]，密文一致 ⇔ 所用密钥流一致。
        let mut shifted = plain[0x100..].to_vec();
        cipher.decrypt(offset + 0x100, &mut shifted);
        assert_eq!(
            &first[0x100..],
            &shifted[..],
            "缓存命中下不同段内偏移必须索引到相同密钥流"
        );
    }

    #[test]
    fn factory_rc4_and_map_paths_regression() {
        let chunk_size = 0x371;
        let total = FIRST_SEGMENT_SIZE + OTHER_SEGMENT_SIZE * 2 + 0x2A5;

        // 大密钥（>300 字节）→ RC4 路径回归
        let large_key: Vec<u8> = (0..700usize).map(|i| (i * 7 + 3) as u8).collect();
        let ekey = generate_ekey(&large_key);
        let rc4 = decrypt_factory(&ekey).unwrap();

        let mut whole = sample_data(total);
        rc4.decrypt(0, &mut whole);
        assert_ne!(whole, sample_data(total), "RC4 路径应实际改动数据");

        let mut chunked = sample_data(total);
        let mut pos = 0;
        while pos < chunked.len() {
            let end = std::cmp::min(pos + chunk_size, chunked.len());
            rc4.decrypt(pos, &mut chunked[pos..end]);
            pos = end;
        }
        assert_eq!(whole, chunked, "RC4 路径：大 buffer 与分块解密一致");

        // XOR 流密码：同一段密钥流再解一次应还原明文
        let mut restored = whole.clone();
        rc4.decrypt(0, &mut restored);
        assert_eq!(restored, sample_data(total), "RC4 路径：二次解密还原明文");

        // 小密钥（≤300 字节）→ Map 路径回归
        let small_key: Vec<u8> = (0..120usize).map(|i| (i * 11 + 1) as u8).collect();
        let ekey = generate_ekey(&small_key);
        let map = decrypt_factory(&ekey).unwrap();

        let base_offset = 0x1234;
        let mut m_whole = sample_data(total);
        map.decrypt(base_offset, &mut m_whole);

        let mut m_chunked = sample_data(total);
        let mut pos = 0;
        while pos < m_chunked.len() {
            let end = std::cmp::min(pos + chunk_size, m_chunked.len());
            map.decrypt(base_offset + pos, &mut m_chunked[pos..end]);
            pos = end;
        }
        assert_eq!(m_whole, m_chunked, "Map 路径：大 buffer 与分块解密一致");
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

        // 400 字节密钥属于不会触发 RC4 索引越界的畸形区间
        let malformed_key = vec![0u8; 400];
        let ekey = generate_ekey(&malformed_key);
        assert!(matches!(decrypt_factory(&ekey), Err(Qmc2Error::KeyDerive)));

        // 700 字节密钥 → RC4
        let large_key = vec![0u8; 700];
        let ekey = generate_ekey(&large_key);
        let cipher = decrypt_factory(&ekey).unwrap();
        cipher.decrypt(0, &mut buf);
    }
}
