//! QMC2 TEA 加解密（移植自 TarsCpp TC_Tea / jixunmoe/qmc2-rust）。
//!
//! 算法为 TEA-CBC 变体（16 轮，delta = 0x9e3779b9，IV 为零向量）。
//! 密文格式：PadLen(1) + Padding(var) + Salt(2) + Body(var) + Zero(7)。

/// TEA 块解密（单块 8 字节，ECB）。
///
/// `v` 为大端序 `[y, z]`，`k` 为大端序 `[k0, k1, k2, k3]`。
#[inline]
pub(crate) fn tea_decrypt_block(v: [u32; 2], k: [u32; 4]) -> [u32; 2] {
    let mut y = v[0];
    let mut z = v[1];
    let mut sum = DELTA << 4;
    for _ in 0..ROUNDS {
        z = z.wrapping_sub(
            ((y << 4).wrapping_add(k[2])) ^ (y.wrapping_add(sum)) ^ ((y >> 5).wrapping_add(k[3])),
        );
        y = y.wrapping_sub(
            ((z << 4).wrapping_add(k[0])) ^ (z.wrapping_add(sum)) ^ ((z >> 5).wrapping_add(k[1])),
        );
        sum = sum.wrapping_sub(DELTA);
    }
    [y, z]
}

/// TEA 块加密（单块 8 字节，ECB）。
#[inline]
pub(crate) fn tea_encrypt_block(v: [u32; 2], k: [u32; 4]) -> [u32; 2] {
    let mut y = v[0];
    let mut z = v[1];
    let mut sum = 0u32;
    for _ in 0..ROUNDS {
        sum = sum.wrapping_add(DELTA);
        y = y.wrapping_add(
            ((z << 4).wrapping_add(k[0])) ^ (z.wrapping_add(sum)) ^ ((z >> 5).wrapping_add(k[1])),
        );
        z = z.wrapping_add(
            ((y << 4).wrapping_add(k[2])) ^ (y.wrapping_add(sum)) ^ ((y >> 5).wrapping_add(k[3])),
        );
    }
    [y, z]
}

const DELTA: u32 = 0x9e3779b9;
const ROUNDS: usize = 16;
const SALT_LEN: usize = 2;
const ZERO_LEN: usize = 7;

/// 将大端字节序 16 字节密钥转换为 4 个 u32。
fn key_to_u32(key: &[u8; 16]) -> [u32; 4] {
    [
        u32::from_be_bytes([key[0], key[1], key[2], key[3]]),
        u32::from_be_bytes([key[4], key[5], key[6], key[7]]),
        u32::from_be_bytes([key[8], key[9], key[10], key[11]]),
        u32::from_be_bytes([key[12], key[13], key[14], key[15]]),
    ]
}

/// TEA-CBC 解密（IV 为零向量）。
///
/// 密文格式：PadLen(1) + Padding(var) + Salt(2) + Body(var) + Zero(7)。
/// 返回 `None` 当长度不是 8 的倍数或不足 16 字节时。
pub(crate) fn tea_cbc_decrypt(data: &[u8], key: &[u8; 16]) -> Option<Vec<u8>> {
    if data.len() % 8 != 0 || data.len() < 16 {
        return None;
    }

    let k = key_to_u32(key);
    let n = data.len();
    let zero_buf = [0u8; 8];
    let mut iv_pre_crypt: &[u8] = &zero_buf;
    let mut iv_cur_crypt: &[u8] = &data[0..];

    // 解密第一个块，获取 PadLen
    let mut dest_buf = tea_decrypt_block_bytes(&data[0..8], &k);
    let pad_len = (dest_buf[0] & 0x7) as usize;

    // 计算明文长度：总长 - 1 (PadLen) - PadLen - SALT_LEN - ZERO_LEN
    let body_len = n.checked_sub(1 + pad_len + SALT_LEN + ZERO_LEN)?;
    let mut out = Vec::with_capacity(body_len);

    // 跳过 PadLen 字节
    let mut dest_i = 1 + pad_len;
    let mut pos = 8; // 读取位置

    // 跳过 Salt
    let mut salt_remain = SALT_LEN;
    while salt_remain > 0 {
        if dest_i < 8 {
            dest_i += 1;
            salt_remain -= 1;
        } else {
            iv_pre_crypt = iv_cur_crypt;
            iv_cur_crypt = &data[pos..];
            for j in 0..8 {
                dest_buf[j] ^= iv_cur_crypt[j];
            }
            dest_buf = tea_decrypt_block_bytes(&dest_buf, &k);
            pos += 8;
            dest_i = 0;
        }
    }

    // 解密 Body
    let mut body_remain = body_len;
    while body_remain > 0 {
        if dest_i < 8 {
            out.push(dest_buf[dest_i] ^ iv_pre_crypt[dest_i]);
            dest_i += 1;
            body_remain -= 1;
        } else {
            iv_pre_crypt = iv_cur_crypt;
            iv_cur_crypt = &data[pos..];
            for j in 0..8 {
                dest_buf[j] ^= iv_cur_crypt[j];
            }
            dest_buf = tea_decrypt_block_bytes(&dest_buf, &k);
            pos += 8;
            dest_i = 0;
        }
    }

    // 校验 Zero
    let mut zero_remain = ZERO_LEN;
    while zero_remain > 0 {
        if dest_i < 8 {
            if dest_buf[dest_i] ^ iv_pre_crypt[dest_i] != 0 {
                return None;
            }
            dest_i += 1;
            zero_remain -= 1;
        } else {
            iv_pre_crypt = iv_cur_crypt;
            iv_cur_crypt = &data[pos..];
            for j in 0..8 {
                dest_buf[j] ^= iv_cur_crypt[j];
            }
            dest_buf = tea_decrypt_block_bytes(&dest_buf, &k);
            pos += 8;
            dest_i = 0;
        }
    }

    Some(out)
}

/// TEA-CBC 加密（IV 为零向量，padding/salt 使用伪随机值）。
pub(crate) fn tea_cbc_encrypt(data: &[u8], key: &[u8; 16]) -> Vec<u8> {
    let k = key_to_u32(key);
    let n_in = data.len();

    // 计算总长度（确保是 8 的倍数）
    let pad_salt_body_zero = n_in + 1 + SALT_LEN + ZERO_LEN;
    let pad_len = (8 - (pad_salt_body_zero % 8)) % 8;
    let total = pad_salt_body_zero + pad_len;

    let mut out = Vec::with_capacity(total);
    let mut src_buf = [0u8; 8];
    let mut iv_plain = [0u8; 8]; // 前一明文块，用于 CBC XOR
    let mut iv_crypt = [0u8; 8]; // 前一密文块，初始为零

    // 使用确定性序列模拟随机（与上游自洽即可）
    let mut pseudo = 0x55u8;

    let mut src_i;

    // 写入 PadLen
    src_buf[0] = pad_len as u8;
    src_i = 1;

    // 写入 Padding
    for _ in 0..pad_len {
        src_buf[src_i] = pseudo;
        pseudo = pseudo.wrapping_mul(17).wrapping_add(1);
        src_i += 1;
    }

    // 写入 Salt（2 字节）
    let mut salt_remain = SALT_LEN;
    while salt_remain > 0 {
        if src_i < 8 {
            src_buf[src_i] = pseudo;
            pseudo = pseudo.wrapping_mul(17).wrapping_add(1);
            src_i += 1;
            salt_remain -= 1;
        }
        if src_i == 8 {
            encrypt_and_output_block(&mut src_buf, &mut iv_plain, &mut iv_crypt, &k, &mut out);
            src_i = 0;
        }
    }

    // 写入 Body
    let mut body_remain = n_in;
    let mut data_pos = 0;
    while body_remain > 0 {
        if src_i < 8 {
            src_buf[src_i] = data[data_pos];
            data_pos += 1;
            src_i += 1;
            body_remain -= 1;
        }
        if src_i == 8 {
            encrypt_and_output_block(&mut src_buf, &mut iv_plain, &mut iv_crypt, &k, &mut out);
            src_i = 0;
        }
    }

    // 写入 Zero（7 字节）
    let mut zero_remain = ZERO_LEN;
    while zero_remain > 0 {
        if src_i < 8 {
            src_buf[src_i] = 0;
            src_i += 1;
            zero_remain -= 1;
        }
        if src_i == 8 {
            encrypt_and_output_block(&mut src_buf, &mut iv_plain, &mut iv_crypt, &k, &mut out);
            src_i = 0;
        }
    }

    out
}

/// 辅助：CBC 加密一个完整块并输出。
fn encrypt_and_output_block(
    src_buf: &mut [u8; 8],
    iv_plain: &mut [u8; 8],
    iv_crypt: &mut [u8; 8],
    k: &[u32; 4],
    out: &mut Vec<u8>,
) {
    // XOR with previous ciphertext (CBC)
    for j in 0..8 {
        src_buf[j] ^= iv_crypt[j];
    }
    // save plaintext for next CBC round
    let saved_plain = *src_buf;
    // encrypt (returns new array, no borrow conflict)
    *src_buf = tea_encrypt_block_bytes(src_buf, k);
    // XOR with previous plaintext
    for j in 0..8 {
        src_buf[j] ^= iv_plain[j];
    }
    out.extend_from_slice(src_buf);
    *iv_plain = saved_plain;
    *iv_crypt = *src_buf;
}

/// 辅助函数：解密 8 字节输入块，返回 8 字节输出。
fn tea_decrypt_block_bytes(input: &[u8], k: &[u32; 4]) -> [u8; 8] {
    let y = u32::from_be_bytes([input[0], input[1], input[2], input[3]]);
    let z = u32::from_be_bytes([input[4], input[5], input[6], input[7]]);
    let [y, z] = tea_decrypt_block([y, z], *k);
    let yb = y.to_be_bytes();
    let zb = z.to_be_bytes();
    let mut out = [0u8; 8];
    out[0..4].copy_from_slice(&yb);
    out[4..8].copy_from_slice(&zb);
    out
}

/// 辅助函数：加密 8 字节输入块，返回 8 字节输出。
fn tea_encrypt_block_bytes(input: &[u8; 8], k: &[u32; 4]) -> [u8; 8] {
    let y = u32::from_be_bytes([input[0], input[1], input[2], input[3]]);
    let z = u32::from_be_bytes([input[4], input[5], input[6], input[7]]);
    let [y, z] = tea_encrypt_block([y, z], *k);
    let yb = y.to_be_bytes();
    let zb = z.to_be_bytes();
    let mut out = [0u8; 8];
    out[0..4].copy_from_slice(&yb);
    out[4..8].copy_from_slice(&zb);
    out
}

/// 生成简单密钥（上游 `SimpleMakeKey`）。
///
/// 公式：`fabs(tan(seed + i * 0.1)) * 100`，截断为 `u8`。
pub(crate) fn simple_make_key(seed: u8, len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    for (i, b) in buf.iter_mut().enumerate() {
        let value = ((seed as f64 + i as f64 * 0.1).tan().abs() * 100.0) as u8;
        *b = value;
    }
    buf
}

/// 从 ekey 前 8 字节导出 TEA 密钥（上游 `derive_tea_key`）。
///
/// 将 `simple_make_key(106, 8)` 与 ekey 头部交叉排列。
pub(crate) fn derive_tea_key(header: &[u8]) -> [u8; 16] {
    let simple = simple_make_key(106, 8);
    let mut tea_key = [0u8; 16];
    for i in (0..16).step_by(2) {
        tea_key[i] = simple[i / 2];
        tea_key[i + 1] = header[i / 2];
    }
    tea_key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_key_matches_reference() {
        let key = simple_make_key(106, 8);
        assert_eq!(key, vec![0x69, 0x56, 0x46, 0x38, 0x2b, 0x20, 0x15, 0x0b]);
    }

    #[test]
    fn tea_key_interleaves_header() {
        let header = [0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8];
        let tea_key = derive_tea_key(&header);
        assert_eq!(
            tea_key,
            [
                0x69, 0xf1, 0x56, 0xf2, 0x46, 0xf3, 0x38, 0xf4, 0x2b, 0xf5, 0x20, 0xf6, 0x15, 0xf7,
                0x0b, 0xf8,
            ]
        );
    }

    #[test]
    fn tea_cbc_roundtrip() {
        let key = [0x01u8; 16];

        // 测试不同长度的正文
        for &len in &[0, 1, 7, 8, 15, 16, 31, 64, 100] {
            let body: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(7)).collect();
            let encrypted = tea_cbc_encrypt(&body, &key);
            let decrypted = tea_cbc_decrypt(&encrypted, &key).expect("decrypt should succeed");
            assert_eq!(decrypted, body, "roundtrip failed at len={}", len);
        }
    }

    #[test]
    fn tea_cbc_rejects_misaligned() {
        let key = [0x01u8; 16];
        // 长度不足 16
        assert!(tea_cbc_decrypt(&[0u8; 8], &key).is_none());
        // 长度非 8 倍数
        assert!(tea_cbc_decrypt(&[0u8; 17], &key).is_none());
    }

    #[test]
    fn tea_cbc_rejects_truncated_padding_without_panicking() {
        let key = [0x01u8; 16];
        // 7-byte body produces pad_len=7 and 24 bytes; truncating to 16 used to underflow.
        let ciphertext = tea_cbc_encrypt(&[0u8; 7], &key);
        assert_eq!(ciphertext.len(), 24);
        assert!(tea_cbc_decrypt(&ciphertext[..16], &key).is_none());
    }

    #[test]
    fn tea_cbc_decrypts_real_vector() {
        let key = [0x01u8; 16];
        let body = b"hello world".to_vec();
        let encrypted = tea_cbc_encrypt(&body, &key);
        let decrypted = tea_cbc_decrypt(&encrypted, &key).expect("decrypt should succeed");
        assert_eq!(decrypted, body);
    }
}
