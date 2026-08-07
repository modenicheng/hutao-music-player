# QMC2 加密音质解密播放实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `hmp play`（CLI）与 hmp-desktop 能够直接播放 QQ 音乐加密音质（`.mflac`/`.mgg`/`.mmp4`/`.mnac`），通过 GetEVkey 返回的 `ekey` 做 QMC2 解密，恢复无损音质回退链（docs/PROJECT.md §7.3），并把无损（FLAC）作为默认可取流音质。

**Architecture:** 解密算法（TEA-CBC、ekey 派生、map/RC4 流密码、STag/QTag 尾部检测）放进 `hmp-qqmusic-api::algorithms::qmc2`（与现有 `algorithms/qrc.rs`、`algorithms/tripledes.rs` 并列）。新增独立 crate `hmp-media`：下载加密流 → 检测/剥离尾部 → 流式解密 → 写入 XDG 缓存 → 返回 `file://` URI；CLI 与桌面共用。播放器（hmp-player-gst）不变——它本就支持 `file://` URI。

**Tech Stack:** Rust 2024, reqwest 0.12（新增 `stream` feature）, tokio, base64, sha1, hmp-core, hmp-storage（XDG 路径）, wiremock（测试）。

## Global Constraints

- 算法移植自 jixunmoe/qmc2-rust（MIT）与 bczhc/qmc-decode + qmc-decrypt（GPL-3.0）；TEA 核心移植自 TarsCpp `tc_tea`（BSD-3-Clause）。三者均与 HMP 的 GPL-3.0-or-later 兼容。**必须**在 README.md 增加“鸣谢 / Acknowledgements”节并写入 `docs/QQMUSIC_PORTING.md` 模块映射（Task 5）。
- 加密音质取流仍走 `CgiGetEVkey`（`song::SongApi::get_song_urls` 已实现）；本计划**不修改** QQ 协议请求层。
- 不新增 `AudioQuality` 枚举变体（OGG 系列 `O8M1`/`O8M0` 等仍不进入回退链，维持现状，文档中记为后续项）；回退链维持 `Master → HiRes → Atmos → Flac → Mp3_320 → Mp3_128`，其中 Master/Atmos/Flac 现在可解密播放。
- 解密密钥只来自接口 `ekey`；文件内嵌 ekey 仅在接口 ekey 缺失时作为回退（STag 尾部）。
- `hmp-core` 领域模型、`hmp-player-gst`、`hmp-mpris` 三个 crate 除 CLI/desktop 接线外**不得修改**。
- 所有编辑保持 ASCII 注释，除非已有中文产品文案或源文件字符集明确需要中文（本仓库文档/注释惯例为中文，代码注释沿用中文）。
- 每个 Task 一个原子 commit；`cargo fmt --all`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace` 必须通过。
- 缓存目录：`hmp_storage::cache_dir()/decrypted/`；缓存键 = `sha1(url|ekey)` 十六进制前 16 位；eviction：新建文件时若总大小超阈值（默认 2048 MiB，环境变量 `HMP_DECRYPT_CACHE_MIB` 可覆盖）则删除 mtime 最旧的文件直至达标。

---

## 算法参考（Task 1 的完整实现依据，直接移植以下代码）

### 1. TEA（TarsCpp `oi_symmetry_decrypt2` 变体，用于 ekey 解密）

```rust
/// 标准 TEA ECB 单块解密（16 轮，大端字节序）。
/// block: &[u8; 8], key: &[u8; 16]
fn tea_decrypt_block(block: &[u8; 8], key: &[u8; 16]) -> [u8; 8] {
    const DELTA: u32 = 0x9e37_79b9;
    let mut y = u32::from_be_bytes(block[0..4].try_into().unwrap());
    let mut z = u32::from_be_bytes(block[4..8].try_into().unwrap());
    let k = [
        u32::from_be_bytes(key[0..4].try_into().unwrap()),
        u32::from_be_bytes(key[4..8].try_into().unwrap()),
        u32::from_be_bytes(key[8..12].try_into().unwrap()),
        u32::from_be_bytes(key[12..16].try_into().unwrap()),
    ];
    let mut sum = DELTA.wrapping_shl(4);
    for _ in 0..16 {
        z = z.wrapping_sub(
            (y.wrapping_shl(4).wrapping_add(k[2]))
                ^ y.wrapping_add(sum)
                ^ (y.wrapping_shr(5).wrapping_add(k[3])),
        );
        y = y.wrapping_sub(
            (z.wrapping_shl(4).wrapping_add(k[0]))
                ^ z.wrapping_add(sum)
                ^ (z.wrapping_shr(5).wrapping_add(k[1])),
        );
        sum = sum.wrapping_sub(DELTA);
    }
    let mut out = [0u8; 8];
    out[0..4].copy_from_slice(&y.to_be_bytes());
    out[4..8].copy_from_slice(&z.to_be_bytes());
    out
}

/// 标准 TEA ECB 单块加密（16 轮，大端字节序；与解密方向互为逆运算）。
fn tea_encrypt_block(block: &[u8; 8], key: &[u8; 16]) -> [u8; 8] {
    const DELTA: u32 = 0x9e37_79b9;
    let mut y = u32::from_be_bytes(block[0..4].try_into().unwrap());
    let mut z = u32::from_be_bytes(block[4..8].try_into().unwrap());
    let k = [
        u32::from_be_bytes(key[0..4].try_into().unwrap()),
        u32::from_be_bytes(key[4..8].try_into().unwrap()),
        u32::from_be_bytes(key[8..12].try_into().unwrap()),
        u32::from_be_bytes(key[12..16].try_into().unwrap()),
    ];
    let mut sum = 0u32;
    for _ in 0..16 {
        sum = sum.wrapping_add(DELTA);
        y = y.wrapping_add(
            (z.wrapping_shl(4).wrapping_add(k[0]))
                ^ z.wrapping_add(sum)
                ^ (z.wrapping_shr(5).wrapping_add(k[1])),
        );
        z = z.wrapping_add(
            (y.wrapping_shl(4).wrapping_add(k[2]))
                ^ y.wrapping_add(sum)
                ^ (y.wrapping_shr(5).wrapping_add(k[3])),
        );
    }
    let mut out = [0u8; 8];
    out[0..4].copy_from_slice(&y.to_be_bytes());
    out[4..8].copy_from_slice(&z.to_be_bytes());
    out
}
```

TEA-CBC 解密（TarsCpp `oi_symmetry_decrypt2` 逻辑）——密文格式：
`PadLen(1byte: pad_len 低 3 位) + Padding(pad_len bytes) + Salt(2 bytes) + Body + Zero(7 bytes)`，总长 8 字节对齐：

```rust
/// 解密 TarsCpp TEA-CBC 密文，返回 Body。len 必须为 8 的倍数且 >= 16，否则返回 None。
fn tea_cbc_decrypt(data: &[u8], key: &[u8; 16]) -> Option<Vec<u8>> {
    if data.len() % 8 != 0 || data.len() < 16 {
        return None;
    }
    let mut dest = tea_decrypt_block(data[0..8].try_into().unwrap(), key);
    let pad_len = usize::from(dest[0] & 0x7);
    let plain_len = data.len() as isize - 1 - pad_len as isize - 2 - 7;
    if plain_len < 0 {
        return None;
    }
    let mut out = Vec::with_capacity(plain_len as usize);
    let mut iv_pre = [0u8; 8]; // 前一块密文（初始 0）
    let mut iv_cur = &data[0..8]; // 当前块密文
    let mut pos = 8usize; // data 已消费位置
    let mut dest_i = 1 + pad_len; // dest 缓冲内游标（跳过 PadLen + Padding）
    // 跳过 Salt（2 字节）
    let mut salt_remaining = 2usize;
    while salt_remaining > 0 {
        if dest_i < 8 {
            dest_i += 1;
            salt_remaining -= 1;
        } else {
            iv_pre = iv_cur;
            iv_cur = &data[pos..pos + 8];
            for j in 0..8 {
                dest[j] ^= data[pos + j];
            }
            dest = tea_decrypt_block(&dest, key);
            pos += 8;
            dest_i = 0;
        }
    }
    // 读出 Body
    let mut remaining = plain_len as usize;
    while remaining > 0 {
        if dest_i < 8 {
            out.push(dest[dest_i] ^ iv_pre[dest_i]);
            dest_i += 1;
            remaining -= 1;
        } else {
            iv_pre = iv_cur;
            iv_cur = &data[pos..pos + 8];
            for j in 0..8 {
                dest[j] ^= data[pos + j];
            }
            dest = tea_decrypt_block(&dest, key);
            pos += 8;
            dest_i = 0;
        }
    }
    Some(out)
}
```

TEA 加密方向（仅测试/构造 fixture 用；salt/padding 用 0 填充即可，解密端会剥离）：

```rust
/// 加密 Body 为 TarsCpp TEA-CBC 密文（salt/padding 固定为 0，仅供测试与密钥生成）。
fn tea_cbc_encrypt(body: &[u8], key: &[u8; 16]) -> Vec<u8> {
    // 密文 = PadLen(1) + Padding(0..7) + Salt(2) + Body + Zero(7)，8 对齐
    let prefix_len = 1 + 2 + 7; // PadLen + Salt + Zero
    let pad_len = (8 - (prefix_len + body.len()) % 8) % 8;
    let total = prefix_len + pad_len + body.len();
    let mut plain = vec![0u8; total];
    plain[0] = pad_len as u8 & 0x7;
    plain[1 + pad_len + 2..1 + pad_len + 2 + body.len()].copy_from_slice(body);
    // 分块 CBC：out = ECB(plain ^ prev_cipher) ^ prev_plain
    let mut out = Vec::with_capacity(total);
    let mut prev_plain = [0u8; 8];
    let mut prev_cipher = [0u8; 8];
    for chunk in plain.chunks_exact(8) {
        let mut block = [0u8; 8];
        for j in 0..8 {
            block[j] = chunk[j] ^ prev_cipher[j];
        }
        let ecb = tea_decrypt_block(&block, key); // TEA 对称：解密 == 加密
        for j in 0..8 {
            out.push(ecb[j] ^ prev_plain[j]);
        }
        prev_plain.copy_from_slice(chunk);
        prev_cipher.copy_from_slice(&ecb);
    }
    out
}
```

> 注：`tea_decrypt_block` 的 16 轮结构在解密与加密方向相同（Feistel 对称），`tea_cbc_encrypt` 复用解密块函数即可。加密测试只需保证 `tea_cbc_decrypt(tea_cbc_encrypt(body, key), key) == body`。

### 2. ekey 派生（jixunmoe/qmc2-rust `key_dec.rs`）

```rust
/// 简单密钥生成（seed=106, size=8），与上游逐字节一致。
fn simple_make_key(seed: u8, size: usize) -> Vec<u8> {
    (0..size)
        .map(|i| {
            let value = f32::from(seed) + i as f32 * 0.1;
            (100.0 * value.tan().abs()) as u8
        })
        .collect()
}

/// 由 ekey 前 8 字节派生 TEA 密钥：偶数位 = simple_key[i]，奇数位 = ekey[i]。
fn derive_tea_key(ekey_header: &[u8]) -> [u8; 16] {
    let simple = simple_make_key(106, 8);
    let mut tea_key = [0u8; 16];
    for i in 0..8 {
        tea_key[2 * i] = simple[i];
        tea_key[2 * i + 1] = ekey_header[i];
    }
    tea_key
}

const ENCV2_PREFIX: &[u8] = b"QQMusic EncV2,Key:";
const ENCV2_STAGE1_KEY: &[u8] = b"386ZJY!@#*$%^&)(";
const ENCV2_STAGE2_KEY: &[u8] = b"**#!(#$%&^a1cZ,T";

/// 解析 ekey（base64 字符串）→ 真实 QMC2 密钥（`key = header ++ tea_decrypt(body)`）。
/// 失败返回 Err（bad base64 / 长度不足 / TEA 解密失败）。
pub fn parse_ekey(ekey: &str) -> Result<Vec<u8>, Qmc2Error> {
    let ekey = ekey.trim_matches('\0');
    let mut decoded = base64::decode(ekey).map_err(|_| Qmc2Error::EKeyParse)?;
    if decoded.len() < 8 {
        return Err(Qmc2Error::EKeyParse);
    }
    if decoded.starts_with(ENCV2_PREFIX) {
        let blob = &decoded[ENCV2_PREFIX.len()..];
        let stage1 = tea_cbc_decrypt(blob, &key_from_ref(ENCV2_STAGE1_KEY))
            .ok_or(Qmc2Error::KeyDerive)?;
        let stage2 = tea_cbc_decrypt(&stage1, &key_from_ref(ENCV2_STAGE2_KEY))
            .ok_or(Qmc2Error::KeyDerive)?;
        decoded = base64::decode(&stage2).map_err(|_| Qmc2Error::EKeyParse)?;
        if decoded.len() < 8 {
            return Err(Qmc2Error::EKeyParse);
        }
    }
    let (header, body) = decoded.split_at(8);
    let tea_key = derive_tea_key(header);
    let body = tea_cbc_decrypt(body, &tea_key).ok_or(Qmc2Error::KeyDerive)?;
    let mut key = Vec::with_capacity(8 + body.len());
    key.extend_from_slice(header);
    key.extend_from_slice(&body);
    Ok(key)
}
```

其中 `key_from_ref(k: &[u8]) -> [u8; 16]` 把 `&[u8]` 拷贝为定长数组（`k.try_into().unwrap()`，两阶段密钥恰为 16 字节）。

加密方向（测试用，对照参考 `generate_ekey`）：

```rust
/// 由真实密钥生成 ekey 字符串（测试/构造 fixture 用）。
pub fn generate_ekey(key: &[u8]) -> String {
    let (header, body) = key.split_at(8);
    let tea_key = derive_tea_key(header);
    let encrypted = tea_cbc_encrypt(body, &tea_key);
    let mut blob = Vec::with_capacity(8 + encrypted.len());
    blob.extend_from_slice(header);
    blob.extend_from_slice(&encrypted);
    base64::encode(blob)
}
```

### 3. 流密码（jixunmoe/qmc2-rust `qmc2_rc4.rs` + `qmc2_map.rs`）

密钥长度 `> 300` 用 RC4 分段密码，否则用 map 异或密码。两者 XOR 对称（加密 == 解密）。

```rust
pub trait Qmc2Cipher {
    fn decrypt(&self, offset: usize, buf: &mut [u8]);
}

// ---------- map 密码（key.len() <= 300）----------
pub struct QmcMapCipher { key: Vec<u8> }

impl QmcMapCipher {
    fn scramble(value: u8, index: usize) -> u8 {
        let rotation = (index as u32).wrapping_add(4) & 0b111;
        let left = value.wrapping_shl(rotation);
        let right = value.wrapping_shr(rotation);
        left | right
    }
    fn map_l(&self, offset: usize) -> u8 {
        let off = if offset > 0x7FFF { offset % 0x7FFF } else { offset };
        let index = ((off as u64 * off as u64 + 71214) % self.key.len() as u64) as usize;
        Self::scramble(self.key[index], index)
    }
}
impl Qmc2Cipher for QmcMapCipher {
    fn decrypt(&self, offset: usize, buf: &mut [u8]) {
        for (i, b) in buf.iter_mut().enumerate() {
            *b ^= self.map_l(offset + i);
        }
    }
}

// ---------- RC4 分段密码（key.len() > 300）----------
pub struct QmcRc4Cipher {
    s: Vec<u8>,       // KSA 后的 S 盒（每段克隆后推进）
    hash: u32,        // hash_base
    rc4_key: Vec<u8>, // 原始密钥
}

const FIRST_SEGMENT_SIZE: usize = 0x80;
const OTHER_SEGMENT_SIZE: usize = 0x1400;

impl QmcRc4Cipher {
    pub fn new(rc4_key: &[u8]) -> Self {
        let n = rc4_key.len();
        let mut s: Vec<u8> = (0..n).map(|i| i as u8).collect();
        let mut j = 0usize;
        for (i, &key_byte) in rc4_key.iter().enumerate() {
            j = (j + s[i] as usize + key_byte as usize) % n;
            s.swap(i, j);
        }
        Self { s, hash: calc_hash_base(rc4_key), rc4_key: rc4_key.to_vec() }
    }

    /// 参考实现：`hash / ((id+1) * seed) * 100.0` 截断为整数。
    fn calc_segment_key(&self, id: usize, seed: u8) -> usize {
        let dividend = f64::from(self.hash);
        let divisor = ((id + 1) * usize::from(seed)) as f64;
        (dividend / divisor * 100.0) as u64 as usize
    }

    fn rc4_derive(n: usize, s: &mut Vec<u8>, j: &mut usize, k: &mut usize) -> u8 {
        *j = (*j + 1) % n;
        *k = (usize::from(s[*j]) + *k) % n;
        s.swap(*j, *k);
        let index = usize::from(s[*j]) + usize::from(s[*k]);
        s[index % n]
    }

    fn encode_first_segment(&self, offset: usize, buf: &mut [u8]) {
        let n = self.rc4_key.len();
        let mut off = offset;
        for b in buf.iter_mut() {
            let key1 = self.rc4_key[off % n];
            let key2 = self.calc_segment_key(off, key1);
            *b ^= self.rc4_key[key2 % n];
            off += 1;
        }
    }

    fn encode_other_segment(&self, offset: usize, buf: &mut [u8]) {
        let seg_id = offset / OTHER_SEGMENT_SIZE;
        let seg_id_small = seg_id & 0x1FF;
        let mut discard = self.calc_segment_key(seg_id, self.rc4_key[seg_id_small]) & 0x1FF;
        discard += offset % OTHER_SEGMENT_SIZE;
        let n = self.rc4_key.len();
        let mut s = self.s.clone();
        let mut j = 0usize;
        let mut k = 0usize;
        for _ in 0..discard {
            Self::rc4_derive(n, &mut s, &mut j, &mut k);
        }
        for b in buf.iter_mut() {
            *b ^= Self::rc4_derive(n, &mut s, &mut j, &mut k);
        }
    }
}

fn calc_hash_base(data: &[u8]) -> u32 {
    let mut hash: u32 = 1;
    for &value in data {
        let value = u32::from(value);
        if value == 0 {
            continue;
        }
        let next = hash.wrapping_mul(value);
        if next == 0 || next <= hash {
            break;
        }
        hash = next;
    }
    hash
}

impl Qmc2Cipher for QmcRc4Cipher {
    fn decrypt(&self, offset: usize, buf: &mut [u8]) {
        let mut off = offset;
        let mut len = buf.len();
        let mut i = 0usize;
        if off < FIRST_SEGMENT_SIZE {
            let seg = std::cmp::min(len, FIRST_SEGMENT_SIZE - off);
            self.encode_first_segment(off, &mut buf[i..i + seg]);
            i += seg;
            len -= seg;
            off += seg;
        }
        let to_align = off % OTHER_SEGMENT_SIZE;
        if to_align != 0 {
            let seg = std::cmp::min(len, OTHER_SEGMENT_SIZE - to_align);
            self.encode_other_segment(off, &mut buf[i..i + seg]);
            i += seg;
            len -= seg;
            off += seg;
        }
        while len > OTHER_SEGMENT_SIZE {
            self.encode_other_segment(off, &mut buf[i..i + OTHER_SEGMENT_SIZE]);
            i += OTHER_SEGMENT_SIZE;
            len -= OTHER_SEGMENT_SIZE;
            off += OTHER_SEGMENT_SIZE;
        }
        if len > 0 {
            self.encode_other_segment(off, &mut buf[i..i + len]);
        }
    }
}

/// 工厂：按密钥长度选择密码。
pub fn decrypt_factory(ekey: &str) -> Result<Box<dyn Qmc2Cipher>, Qmc2Error> {
    let key = parse_ekey(ekey)?;
    Ok(if key.len() > 300 {
        Box::new(QmcRc4Cipher::new(&key))
    } else {
        Box::new(QmcMapCipher::new(key))
    })
}
```

### 4. 尾部检测（bczhc/qmc-decode `QMCDetection`，QTag + v1 布局）

```rust
/// 文件尾部检测结果。
pub enum Footer {
    /// QTag 布局：末尾 `[meta_len BE u32]["QTag"]`，meta_len 为 ekey/songid/版本文本区长度。
    QTag { audio_len: usize },
    /// v1/STag 布局：末尾 `[ekey bytes][ekey_len LE u32]`。
    V1 { audio_len: usize },
}

/// 依据文件总大小与末尾 0x40 字节检测尾部。
pub fn detect_footer(total_len: usize, tail: &[u8]) -> Option<Footer> {
    if tail.len() < 8 {
        return None;
    }
    let magic = u32::from_le_bytes(tail[tail.len() - 4..].try_into().unwrap());
    if magic == u32::from_le_bytes(*b"QTag") {
        let meta_len = u32::from_be_bytes(tail[tail.len() - 8..tail.len() - 4].try_into().unwrap())
            as usize;
        if meta_len + 8 <= total_len {
            return Some(Footer::QTag { audio_len: total_len - 8 - meta_len });
        }
        return None;
    }
    // v1 布局：最后 4 字节 LE = ekey 长度（< 0x400）
    if (1..=0x400).contains(&magic) {
        let key_size = magic as usize;
        if key_size + 4 <= total_len {
            return Some(Footer::V1 { audio_len: total_len - 4 - key_size });
        }
    }
    None
}
```

> 重要：`detect_footer` 对“无尾部文件”可能误判（最后 4 字节恰为小整数）。调用方必须用“解密后头部魔数校验 + 无剥离重试”兜底（见 Task 2 §2）。

### 5. 参考测试向量（移植进 Task 1 的测试，均来自 jixunmoe/qmc2-rust）

```rust
// simple_make_key(106, 8) == [0x69, 0x56, 0x46, 0x38, 0x2b, 0x20, 0x15, 0x0b]
// derive_tea_key([f1..f8]) == [0x69,0xf1, 0x56,0xf2, 0x46,0xf3, 0x38,0xf4, 0x2b,0xf5, 0x20,0xf6, 0x15,0xf7, 0x0b,0xf8]
// parse_ekey("VGhpcyBpcyBHFWEh4cjZ1Vi7rJ56XeoPlqGM1sxBGPg7mt89umKclFBr9iqfmFdS")
//   == b"This is a test key for test purpose :D"
// generate_ekey(b"12345678...test data by Jixun") -> parse_ekey 往返一致
// QmcRc4Cipher::new([0..255])：
//   decrypt(0, [0u8;16]) == [0,50,16,8,5,3,2,1,1,1,0,0,0,0,0,0]
//   decrypt(0x80-8, [0u8;16]) == [0;8] ++ [141,97,122,193,166,101,233,214]
//   decrypt(0x1400-8, [0u8;16]) == [118,193,176,83,10,98,105,234] ++ [151,56,198,1,226,173,127,4]
//   decrypt(0x1400, [0u8;16]) == [151,56,198,1,226,173,127,4] ++ [181,165,171,21,82,152,195,210]
// QmcMapCipher::new([0x41..0x50])：
//   decrypt(0, [0u8;16]) == [0x3F,0x8A,0xC1,0x49,0x3F,0x49,0xC1,0x8A, 0x3F,0x8A,0xC1,0x49,0x3F,0x49,0xC1,0x8A]
//   decrypt(0x7FFF-8, [0u8;16]) == [0x8A,0x3F,0x8A,0xC1,0x49,0x3F,0x49,0xC1, 0x8A,0x8A,0xC1,0x49,0x3F,0x49,0xC1,0x8A]
// calc_hash_base([0xff;16]) == 0xfc05fc01；含 0 字节时跳过 0 继续累乘
// detect_footer：
//   [b"aaaa", 4,0,0,0] => V1 { audio_len: 4 }（4 字节 ekey）
//   [b"aaaa,", b"18,", b"2,", 10_i32.to_be_bytes(), b"QTag"] 共 24 字节 => QTag { audio_len: 0 }
//   不足 8 字节 => None；最后 4 字节为 0 => None
```

---

## File Structure

- Task 1（解密算法）：
  - Create: `crates/hmp-qqmusic-api/src/algorithms/qmc2/mod.rs`（`parse_ekey`/`generate_ekey`/`decrypt_factory`/`Qmc2Cipher` 与错误类型）
  - Create: `crates/hmp-qqmusic-api/src/algorithms/qmc2/tea.rs`（TEA-CBC 加解密 + `simple_make_key` + `derive_tea_key`）
  - Create: `crates/hmp-qqmusic-api/src/algorithms/qmc2/key.rs`（ekey 派生 EncV1/EncV2）
  - Create: `crates/hmp-qqmusic-api/src/algorithms/qmc2/cipher.rs`（map + RC4 密码）
  - Create: `crates/hmp-qqmusic-api/src/algorithms/qmc2/detect.rs`（尾部检测）
  - Modify: `crates/hmp-qqmusic-api/src/algorithms/mod.rs`（`pub mod qmc2;` 或 `pub use qmc2::...`）
- Task 2（媒体准备）：
  - Create: `crates/hmp-media/Cargo.toml`
  - Create: `crates/hmp-media/src/lib.rs`（`prepare_playable` 主入口 + 错误类型）
  - Create: `crates/hmp-media/src/decrypt.rs`（下载 → 尾部处理 → 流式解密 → 魔数校验）
  - Create: `crates/hmp-media/src/cache.rs`（缓存路径/命中/eviction）
  - Modify: `Cargo.toml`（workspace members 增 `crates/hmp-media`；workspace reqwest 增 `"stream"` feature）
- Task 3（CLI）：
  - Modify: `crates/hmp-cli/Cargo.toml`（增 `hmp-media` 依赖）
  - Modify: `crates/hmp-cli/src/play.rs`（不再跳过加密音质；ekey → `prepare_playable` → file URI）
- Task 4（桌面）：
  - Modify: `crates/hmp-desktop/Cargo.toml`（增 `hmp-media` 依赖）
  - Modify: `crates/hmp-desktop/src/app.rs`（`resolve_stream` 返回 ekey；`resolve_play_request` 内解密准备）
- Task 5（文档）：
  - Modify: `docs/PROJECT.md`（§7.3 恢复无损链说明；§20 里程碑勾选；功能状态）
  - Modify: `docs/QQMUSIC_PORTING.md`（新增 qmc2 模块映射 + 实测记录）
  - Modify: `README.md`（新增“鸣谢 / Acknowledgements”节）

---

### Task 1: QMC2 解密算法（hmp-qqmusic-api::algorithms::qmc2）

**Files:**
- Create: `crates/hmp-qqmusic-api/src/algorithms/qmc2/tea.rs`
- Create: `crates/hmp-qqmusic-api/src/algorithms/qmc2/key.rs`
- Create: `crates/hmp-qqmusic-api/src/algorithms/qmc2/cipher.rs`
- Create: `crates/hmp-qqmusic-api/src/algorithms/qmc2/detect.rs`
- Create: `crates/hmp-qqmusic-api/src/algorithms/qmc2/mod.rs`
- Modify: `crates/hmp-qqmusic-api/src/algorithms/mod.rs`

**Interfaces:**
- Consumes: `base64`（workspace 依赖已存在）
- Produces（Task 2 依赖的公共接口，签名必须一致）：
  - `qmc2::Qmc2Error`（`#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]`，变体 `EKeyParse`、`KeyDerive`）
  - `qmc2::Qmc2Cipher` trait：`fn decrypt(&self, offset: usize, buf: &mut [u8]);`
  - `qmc2::parse_ekey(ekey: &str) -> Result<Vec<u8>, Qmc2Error>`
  - `qmc2::decrypt_factory(ekey: &str) -> Result<Box<dyn Qmc2Cipher>, Qmc2Error>`
  - `qmc2::detect_footer(total_len: usize, tail: &[u8]) -> Option<qmc2::Footer>`（`Footer::{QTag{audio_len}, V1{audio_len}}`）
  - `qmc2::generate_ekey(key: &[u8]) -> String`（测试/构造 fixture 用，rustdoc 注明）

- [ ] **Step 1: 写失败测试** — 在 `crates/hmp-qqmusic-api/src/algorithms/qmc2/tea.rs` 底部 `#[cfg(test)]` 模块写入：
  - `simple_key_matches_reference`：`simple_make_key(106, 8) == [0x69,0x56,0x46,0x38,0x2b,0x20,0x15,0x0b]`
  - `tea_key_interleaves_header`：`derive_tea_key(&[0xf1,0xf2,0xf3,0xf4,0xf5,0xf6,0xf7,0xf8]) == [0x69,0xf1,0x56,0xf2,0x46,0xf3,0x38,0xf4,0x2b,0xf5,0x20,0xf6,0x15,0xf7,0x0b,0xf8]`
  - `tea_cbc_roundtrip`：`tea_cbc_decrypt(&tea_cbc_encrypt(body, &key), &key) == body`（body 取 8/15/16/31 字节等长度）
  - `tea_cbc_rejects_misaligned`：长度非 8 倍数或 `< 16` 返回 `None`
  - `tea_cbc_decrypts_real_vector`：用参考 TarsCpp 输出验证——密钥 `[0x01;16]`、密文为 `tea_cbc_encrypt(b"hello world", &[0x01;16])` 的结果再解密回原文（自洽即可，不强求外部向量）。

- [ ] **Step 2: 运行测试确认失败** — `cargo test -p hmp-qqmusic-api algorithms::qmc2` — 预期编译失败（模块不存在）。

- [ ] **Step 3: 实现 tea.rs** — 按“算法参考 §1”实现 `tea_decrypt_block`、`tea_cbc_decrypt`、`tea_cbc_encrypt`、`simple_make_key`、`derive_tea_key`（`tea_decrypt_block`/`tea_cbc_decrypt` 为 `pub(crate)`，`tea_cbc_encrypt` 为 `pub(crate)` 供 key.rs 测试与 generate_ekey 使用；`simple_make_key`/`derive_tea_key` 为 `pub(crate)`）。

- [ ] **Step 4: 运行测试确认通过** — 同上命令，全部 PASS。

- [ ] **Step 5: 实现 key.rs（含测试）** — 按“算法参考 §2”实现 `parse_ekey`/`generate_ekey`/`key_from_ref`。测试：
  - `parse_ekey_decodes_reference_vector`：`parse_ekey("VGhpcyBpcyBHFWEh4cjZ1Vi7rJ56XeoPlqGM1sxBGPg7mt89umKclFBr9iqfmFdS") == b"This is a test key for test purpose :D"`
  - `generate_parse_roundtrip`：`parse_ekey(&generate_ekey(b"12345678...test data by Jixun")) == 该原文`
  - `parse_ekey_rejects_bad_base64` / `parse_ekey_rejects_short`（`"aGk="` 解码 < 8 字节）
  - `parse_ekey_trims_nul_padding`：`format!("{}\0\0", ekey)` 仍解析成功

- [ ] **Step 6: 实现 cipher.rs（含测试）** — 按“算法参考 §3”实现。测试：
  - `map_cipher_decrypts_zeroes`：`QmcMapCipher::new(KEY)` 在 offset 0 与 0x7FFF-8 的输出等于参考 EXPECTED1/EXPECTED2
  - `rc4_cipher_first_segment` / `rc4_boundary_segments` / `rc4_entire_segment`：255 字节密钥 `[0..255]` 的四个参考输出
  - `hash_base_ignores_zero_bytes`：`calc_hash_base(&[0xff;16]) == 0xfc05fc01`，含 `0x00` 时跳过仍等于该值
  - `decrypt_factory_picks_map_or_rc4`：20 字节密钥 → map；700 字节密钥 → rc4（`generate_ekey` 构造后 `parse_ekey` 取回）

- [ ] **Step 7: 实现 detect.rs（含测试）** — 按“算法参考 §4”实现。测试：
  - `detect_qtag_layout`：`[b"aaaa,", b"18,", b"2,", 10_i32.to_be_bytes(), b"QTag"]`（24 字节）→ `QTag { audio_len: 0 }`；再接 16 字节音频前缀 → `audio_len: 16`
  - `detect_v1_layout`：`[b"aaaa", 4u32.to_le_bytes()]` → `V1 { audio_len: 4 }`
  - `detect_v1_key_size_upper_bound`：key_size 0x400 通过、0x401 拒绝
  - `detect_rejects_tiny_and_zero`：`< 8` 字节 → `None`；`[0u8; 8]` → `None`

- [ ] **Step 8: 实现 mod.rs** — 聚合导出：`pub mod detect; pub mod key; pub mod tea; pub mod cipher;` + `pub use key::{parse_ekey, generate_ekey};` + `pub use cipher::{decrypt_factory, Qmc2Cipher};` + `pub use detect::{detect_footer, Footer};` + `pub use key::Qmc2Error`（错误类型定义在 key.rs 或 mod.rs，任选但须被 `thiserror` 正确派生）。**错误类型必须实现 `std::fmt::Display` 与 `std::error::Error`**（Task 2 会把 `?` 传播到自定义错误）。

- [ ] **Step 9: 更新 algorithms/mod.rs** — 在 `pub mod qrc; pub mod tripledes;` 之后加 `pub mod qmc2;`（模块级文档注明移植来源：jixunmoe/qmc2-rust MIT、bczhc/qmc-decode GPL-3.0、TarsCpp BSD-3-Clause）。

- [ ] **Step 10: 全量校验 + commit**
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  cargo test --workspace
  git add crates/hmp-qqmusic-api/src/algorithms/
  git commit -m "feat(qqmusic): add QMC2 decryption algorithms (TEA, ekey derivation, stream ciphers, footer detection)"
  ```

---

### Task 2: hmp-media crate（下载 → 解密 → 缓存 → file URI）

**Files:**
- Create: `crates/hmp-media/Cargo.toml`
- Create: `crates/hmp-media/src/lib.rs`
- Create: `crates/hmp-media/src/decrypt.rs`
- Create: `crates/hmp-media/src/cache.rs`
- Modify: `Cargo.toml`（workspace）

**Interfaces:**
- Consumes: `hmp_qqmusic_api::algorithms::qmc2::{parse_ekey, decrypt_factory, Qmc2Cipher, detect_footer, Footer}`（Task 1）
- Produces（Task 3/4 依赖）：
  - `hmp_media::prepare_playable(url: &str, ekey: Option<&str>, progress: Option<tokio::sync::watch::Sender<Option<f64>>>) -> Result<String, MediaError>`
    - `url` 为 `https://isure.stream.qqmusic.qq.com/<purl>`；`ekey` 为接口返回的 ekey（加密音质非空）
    - `ekey` 为 `None`/空串 → 直接返回原 `url`（明文音质，不做解密）
    - 返回 `file://` URI（已解密缓存文件）或原 https URI
    - `progress`：下载+解密进度 `0.0..=1.0`（无 Content-Length 时每块发 `None`）
  - `hmp_media::MediaError`（`thiserror`；变体：`Network(String)`、`HttpStatus(u16)`、`Io(std::io::Error)`、`Key(Qmc2Error)`、`Unsupported(String)`（魔数无法识别）、`Cache(String)`）
  - `hmp_media::prepare_playable_at(cache_root: &Path, url: &str, ekey: Option<&str>, progress: Option<...>) -> Result<String, MediaError>`——**测试用**：显式指定缓存根目录；生产入口 `prepare_playable` 内部用 `hmp_storage::cache_dir().join("decrypted")` 调用它。

- [ ] **Step 1: workspace 接线** — `Cargo.toml`：
  - `members` 数组追加 `"crates/hmp-media"`
  - workspace `reqwest` features 追加 `"stream"`
  - 创建 `crates/hmp-media/Cargo.toml`：
    ```toml
    [package]
    name = "hmp-media"
    description = "HMP 媒体准备：QMC2 加密流下载/解密/缓存，产出可播放 file URI"
    version = "0.1.0"
    edition.workspace = true
    license.workspace = true
    repository.workspace = true
    rust-version.workspace = true

    [dependencies]
    base64.workspace = true
    hmp-core.workspace = true
    hmp-qqmusic-api = { path = "../hmp-qqmusic-api" }
    hmp-storage = { path = "../hmp-storage" }
    reqwest.workspace = true
    sha1.workspace = true
    thiserror.workspace = true
    tokio.workspace = true
    tracing.workspace = true

    [dev-dependencies]
    wiremock.workspace = true
    ```
    > 注意：`hmp-core.workspace = true` 需在根 `[workspace.dependencies]` 中补 `hmp-core = { path = "crates/hmp-core" }`（若不存在）；同样 `hmp-storage` 已有 path 依赖但可能未列入 workspace.dependencies——以根清单现状为准，缺失则补。运行 `cargo check -p hmp-media` 验证。

- [ ] **Step 2: 写失败测试（cache.rs）** — `crates/hmp-media/src/cache.rs` 底部测试：
  - `cache_key_is_stable_and_distinct`：`cache_key("https://a/1.mflac", "ekey1")` 两次一致；换 url 或 ekey 不一致；长度 == 16（hex 前缀）
  - `extension_from_magic`：`b"fLaC"`→`"flac"`、`b"OggS"`→`"ogg"`、`b"ftyp"`→`"m4a"`、`b"ID3"`→`"mp3"`、`[0xff, 0xfb]`→`"mp3"`、未知→`None`
  - `evict_keeps_under_cap`：在临时目录创建 3 个文件（mtime 错开），cap=1000 字节，触发 evict 后目录总大小 <= cap 且最旧文件被删
  - 缓存根目录由测试传入 `std::env::temp_dir().join(format!("hmp-media-test-{}", std::process::id()))`，测试结束删除目录（`std::fs::remove_dir_all`，`let _ =` 忽略）。

- [ ] **Step 3: 实现 cache.rs** — 函数（均接收 `root: &Path`）：
  - `pub fn cache_key(url: &str, ekey: &str) -> String`：`sha1(format!("{url}|{ekey}"))` 十六进制前 16 位
  - `pub fn extension_from_magic(head: &[u8]) -> Option<&'static str>`：匹配 `fLaC`/`OggS`/`ftyp`（`head[4..8] == b"isom"` 不要求）/`ID3`/`\xff\xfb`；否则 `None`
  - `pub fn final_path(root: &Path, key: &str, ext: &str) -> PathBuf`：`root.join(format!("{key}.{ext}"))`
  - `pub fn tmp_path(root: &Path, key: &str) -> PathBuf`：`root.join(format!(".{key}.{}.tmp", std::process::id()))`
  - `pub fn evict_if_needed(root: &Path) -> Result<(), std::io::Error>`：累计 `root` 内 `*.tmp` 以外的文件大小；超 `HMP_DECRYPT_CACHE_MIB`（默认 2048，解析失败用默认）MiB 时按 mtime 升序删除直到达标；无文件时 `Ok(())`。失败（如目录不存在）不阻塞主流程——调用方 `let _ = evict_if_needed(&root);`

- [ ] **Step 4: 写失败测试（decrypt.rs）** — 用 wiremock 起本地 HTTP 服务：
  ```rust
  // 构造：真实密钥 20 字节 -> map 密码；明文 = b"fLaC" + 2048 字节伪随机；
  // 加密 = 用 decrypt_factory(ekey) 对明文整体 decrypt 一遍（XOR 对称）；
  // 无尾部版本直接返回加密流；带尾部版本在末尾附加 [ekey_bytes][ekey_len LE u32]。
  ```
  - `prepare_decrypts_plain_stream`：无尾部 → 返回 `file://`，读文件内容 == 明文
  - `prepare_strips_stag_footer`：带尾部 → 返回 `file://`，文件内容 == 明文（尾部已剥）
  - `prepare_returns_url_when_no_ekey`：`ekey = None` → 返回值 == 原 url（不发请求）
  - `prepare_cache_hit_skips_download`：首次调用后删除 wiremock 路由仍能命中（第二次返回相同 file URI，且不重新请求——通过 mock 计数器断言）
  - `prepare_retries_without_strip_on_magic_mismatch`：文件尾部 4 字节 LE 恰为小整数（如 `42u32.to_le_bytes()`）触发误判，但整体解密后头部魔数仍 `fLaC` → 走“无剥离”重试成功
  - `prepare_reports_progress`：progress watch 收到过 `Some(p)` 且最终 `Some(1.0)`
  - 每个测试显式传入临时缓存根目录（见 Step 2 的模式），wiremock 用 `MockServer::start().await` + `Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(...))`

- [ ] **Step 5: 实现 decrypt.rs** — 主流程（`prepare_playable_at`）：
  1. `let ekey = ekey.filter(|e| !e.is_empty());` 为空 → `return Ok(url.to_owned())`
  2. `let key = cache_key(url, ekey);` 若 `final_path` 存在 → 返回 `file://` URI（缓存命中）
  3. `let tmp = tmp_path(root, &key);` `let _ = std::fs::remove_file(&tmp);` 下载：`reqwest::get(url)` → 状态码非 2xx 报 `HttpStatus` → `response.bytes_stream()` 逐块写入 `tmp`（`tokio::fs::File`）；`content_length` 有值则按累计字节发进度 `Some(bytes/total)`，每块发一次；结束后发 `Some(1.0)`。任一步失败：删除 `tmp` 并返回错误。
  4. `let total_len = metadata.len();` 读尾部 0x40 字节 → `detect_footer(total_len, &tail)` → `strip_len`（`audio_len` 或 `None`）
  5. 开 `tmp` 读、开 `final_path` 写：逐块（建议 256 KiB）读 → `decrypt_factory(ekey)` 的 cipher `decrypt(offset, &mut chunk)` → 若 `strip_len` 存在则裁剪超出的尾部字节（`chunk.truncate(strip_len - written)`）→ 写入；`offset += len`。完成后 flush。
  6. 魔数校验：读 `final_path` 前 8 字节 → `extension_from_magic`；`Some(ext)` → 若最终文件名后缀 ≠ ext（缓存命中检查与最终命名按最终 ext 处理：命中检查在 Step 2 之前用 `extension_from_magic` 未知时先用源文件名后缀推断）→ **重命名** `final_path` 为 `final_path_with(ext)` 并返回其 `file://` URI；`None` → 若 `strip_len` 存在则**删除 final、不带剥离重试一次**（回到步骤 5 但 `strip_len = None`）；仍失败 → `Unsupported` 错误（附前 8 字节 hex）。
  7. `let _ = std::fs::remove_file(&tmp);` `let _ = evict_if_needed(root);` 返回 `file://{final_path}`。
  - 缓存命中检查优化：因 ext 在解密前未知，命中检查分两步——先查 `final_path(root, key, ext_guess)`（`ext_guess` 由 url 后缀映射：`mflac→flac`、`mgg→ogg`、`mmp4→m4a`、`mnac→m4a`、其他→`bin`），再在解密后按实际 ext 归一化命名；为控制范围，**v1 约定**：命中检查仅按 `ext_guess` 查一次，解密后实际 ext 与猜测不同时以实际 ext 重命名并返回新路径（不重复下载）。
  - 进度：`progress: Option<&watch::Sender<Option<f64>>>`，发送用 `let _ = tx.send(Some(p));`

- [ ] **Step 6: 实现 lib.rs** — `pub mod cache; pub mod decrypt;` + `MediaError`（thiserror）+ `prepare_playable(url, ekey, progress)`（内部 `prepare_playable_at(&hmp_storage::cache_dir().join("decrypted"), ...)`，`std::fs::create_dir_all` 前置）。模块文档注明依赖 Task 1 的 qmc2 模块。

- [ ] **Step 7: 运行测试** — `cargo test -p hmp-media` 全绿；`cargo fmt --all`、`cargo clippy -p hmp-media --all-targets -- -D warnings`。

- [ ] **Step 8: 全量校验 + commit**
  ```bash
  cargo check --workspace
  cargo test --workspace
  git add Cargo.toml Cargo.lock crates/hmp-media/
  git commit -m "feat(media): add hmp-media decrypted playback preparation (download, QMC2 decrypt, XDG cache, file URI)"
  ```

---

### Task 3: CLI 接线（hmp play 播放加密音质）

**Files:**
- Modify: `crates/hmp-cli/Cargo.toml`
- Modify: `crates/hmp-cli/src/play.rs`

**Interfaces:**
- Consumes: `hmp_media::prepare_playable(url, ekey, progress)`（Task 2）
- Produces: 无（最终用户行为）

- [ ] **Step 1: 加依赖** — `crates/hmp-cli/Cargo.toml` `[dependencies]` 加 `hmp-media = { path = "../hmp-media" }`。

- [ ] **Step 2: 改取流回退循环** — `play.rs` 中删除加密跳过分支：
  ```rust
  // 删除这两行（原实现跳过加密音质）：
  // if file_type.is_encrypted { tracing::info!(quality = ?quality, "跳过加密音质（暂不支持解密）"); continue; }
  ```
  并在 `chosen` 元组中携带 ekey：
  ```rust
  let mut chosen: Option<(SongFileType, String, Option<String>)> = None;
  ...
  if item.result == 0 && !item.purl.is_empty() {
      let ekey = file_type.is_encrypted.then(|| item.ekey.clone()).flatten();
      // 加密音质必须有 ekey，否则视为不可用继续回退
      if file_type.is_encrypted && ekey.as_deref().map_or(true, str::is_empty) {
          last_error = Some(format!("encrypted but no ekey (result={})", item.result));
          continue;
      }
      chosen = Some((file_type, item.purl.clone(), ekey));
      println!("音质: {quality:?} ({}{})", file_type.s, file_type.e);
      break 'quality;
  }
  ```

- [ ] **Step 3: 解密准备** — 解构 `chosen` 后：
  ```rust
  let (file_type, purl, ekey) = chosen.ok_or_else(...)?; // 原错误逻辑保留
  let remote_uri = format!("https://isure.stream.qqmusic.qq.com/{purl}");
  let uri = match &ekey {
      Some(key) => {
          println!("解密中…（QMC2）");
          let (progress_tx, progress_rx) = tokio::sync::watch::channel(Some(0.0f64));
          let progress_handle = {
              let mut rx = progress_rx;
              tokio::spawn(async move {
                  while rx.changed().await.is_ok() {
                      if let Some(p) = *rx.borrow() {
                          print!("\r解密进度: {:.0}%", p * 100.0);
                      }
                  }
              })
          };
          let prepared = hmp_media::prepare_playable(&remote_uri, Some(key), Some(progress_tx))
              .await
              .map_err(|e| e.to_string())?;
          let _ = progress_handle.await;
          println!("\r解密完成，播放本地缓存: {prepared}");
          prepared
      }
      None => remote_uri,
  };
  ```
  > 说明：`ekey` 借用在 `await` 前已转为 owned `Some(key)`，避免 borrow 冲突；`watch::channel` 初值 `Some(0.0)` 避免 CLI 进度循环空转。`uri` 变量名覆盖原 `uri`，后续 `LoadRequest { uri, ... }` 与 `Track.url` 不变。

- [ ] **Step 4: 校验** — `cargo build -p hmp-cli`、`cargo clippy -p hmp-cli --all-targets -- -D warnings`、`cargo test -p hmp-cli`。手动冒烟（可选，需账号）：`cargo run -p hmp-cli -- play <已购歌曲 id>` 应输出“解密中…”并最终播放 FLAC。

- [ ] **Step 5: commit**
  ```bash
  git add crates/hmp-cli/
  git commit -m "feat(cli): play encrypted lossless formats via QMC2 decryption"
  ```

---

### Task 4: 桌面接线（hmp-desktop 播放加密音质）

**Files:**
- Modify: `crates/hmp-desktop/Cargo.toml`
- Modify: `crates/hmp-desktop/src/app.rs`

**Interfaces:**
- Consumes: `hmp_media::prepare_playable(url, ekey, None)`（Task 2）
- Produces: 无（最终用户行为）

- [ ] **Step 1: 加依赖** — `crates/hmp-desktop/Cargo.toml` `[dependencies]` 加 `hmp-media = { path = "../hmp-media" }`。

- [ ] **Step 2: `resolve_stream` 返回 ekey** — 签名改为
  ```rust
  async fn resolve_stream(
      client: &QqMusicClient,
      credential: Option<&Credential>,
      mid: &str,
      media_mid: &str,
      song_type: i64,
  ) -> Option<(SongFileType, String, Option<String>)>
  ```
  删除加密跳过分支（两行 `if file_type.is_encrypted { ... continue; }`），成功分支改为：
  ```rust
  if item.result == 0 && !item.purl.is_empty() {
      let ekey = file_type.is_encrypted.then(|| item.ekey.clone()).flatten();
      if file_type.is_encrypted && ekey.as_deref().map_or(true, str::is_empty) {
          tracing::debug!(quality = ?quality, "encrypted stream without ekey");
          continue;
      }
      let uri = format!("https://isure.stream.qqmusic.qq.com/{}", item.purl);
      tracing::info!(quality = ?quality, "stream resolved");
      return Some((file_type, uri, ekey));
  }
  ```

- [ ] **Step 3: `resolve_play_request` 解密准备** — 两个 `PlayRequest` 分支的调用点改为：
  ```rust
  let (file_type, uri, ekey) = resolve_stream(...)
      .await
      .ok_or_else(|| format!("all qualities unavailable for {}", item.mid))?;
  let uri = match &ekey {
      Some(key) => hmp_media::prepare_playable(&uri, Some(key), None)
          .await
          .map_err(|e| format!("QMC2 decrypt failed for {}: {e}", item.mid))?,
      None => uri,
  };
  ```
  注意 `resolve_stream` 返回的 `uri` 是完整 https URL；`prepare_playable` 返回 `file://` 或原 https。`ResolvedPlayback { uri, ... }` 其余不变（`item.track.url = Some(uri)` 供 MPRIS `xesam:url`，file:// 同样有效）。

- [ ] **Step 4: 校验** — `cargo build -p hmp-desktop`、`cargo clippy -p hmp-desktop --all-targets -- -D warnings`、`cargo test -p hmp-desktop`。桌面已有 `app.rs` 单元测试（`resolve_stream` 相关不直接测网络），跑通即可。

- [ ] **Step 5: commit**
  ```bash
  git add crates/hmp-desktop/
  git commit -m "feat(desktop): play encrypted lossless formats via QMC2 decryption"
  ```

---

### Task 5: 文档与鸣谢

**Files:**
- Modify: `docs/PROJECT.md`
- Modify: `docs/QQMUSIC_PORTING.md`
- Modify: `README.md`

- [ ] **Step 1: PROJECT.md** — 修改 §7.3 加密音质段，替换“当前播放器尚未实现解密”为：
  > **加密音质**：QQ 音乐的无损及以上音质（FLAC/HiRes/Atmos/Master，即 `.mflac`/`.mgg`/`.mmp4` 等）为加密文件，需要客户端用接口返回的 `ekey` 解密后才能播放。HMP 已实现 QMC2 解密（`hmp-qqmusic-api::algorithms::qmc2` + `hmp-media` 下载/解密/缓存），取流后解密为本地缓存文件播放，无损链已恢复（`Master → HiRes → Atmos → Flac → Mp3_320 → Mp3_128`）；OGG 系列（`.mgg`，`O8M1` 等）尚未纳入回退链，属后续项。
  另在 §20 当前优先任务 7 后补一行：`8. ✅ 实现 QMC2 加密音质解密播放（CLI + 桌面）`。

- [ ] **Step 2: QQMUSIC_PORTING.md** — 在“模块映射”表追加：
  | Python 源文件 | Rust 目标模块 | 状态 | 备注 |
  | --- | --- | --- | --- |
  | （无上游对应；独立实现） | `algorithms/qmc2` | ✅ 已移植 | QMC2 解密：TEA-CBC、ekey 派生（EncV1/EncV2）、map/RC4 流密码、STag/QTag 尾部检测 |
  | （无上游对应；独立实现） | `crates/hmp-media` | ✅ 已移植 | 加密流下载→解密→XDG 缓存→file URI（CLI/桌面共用） |
  并在文件末尾“加密取流”节补充实测记录：`CgiGetEVkey` 返回 `ekey` 后解密播放链路（Task 3/4 已接线）。

- [ ] **Step 3: README.md** — 新增“鸣谢 / Acknowledgements”节（置于“许可证”之前）：
  ```markdown
  ## 鸣谢 / Acknowledgements

  HMP 的 QMC2 加密音质解密实现基于以下开源项目的研究与代码（许可证均与 GPL-3.0-or-later 兼容）：

  - [jixunmoe/qmc2-rust](https://github.com/jixunmoe/qmc2-rust)（MIT）——ekey 派生（含 EncV2 两段 TEA）与 map/RC4 流密码的 Rust 参考实现及测试向量；
  - [bczhc/qmc-decode](https://github.com/bczhc/qmc-decode)（GPL-3.0）——QMC2 文件尾部（QTag/STag）检测与格式研究；
  - [bczhc/qmc-decrypt](https://github.com/bczhc/qmc-decrypt)（GPL-3.0）——STag 解密流程与 ekey 用法；
  - TarsCpp [`tc_tea`](https://github.com/TarsCloud/TarsCpp)（BSD-3-Clause）——TEA-CBC 加解密变体（`oi_symmetry_encrypt2/decrypt2`）；
  - [unlock-music](https://github.com/ix64/unlock-music) 研究（GPL-3.0-or-later）——QMC 格式的早期研究与文档（仓库现因 DMCA 不可访问，本实现基于上述维护中的衍生项目）。
  ```

- [ ] **Step 4: 校验 + commit**
  ```bash
  git add docs/PROJECT.md docs/QQMUSIC_PORTING.md README.md
  git commit -m "docs: restore lossless playback chain, document QMC2 decryption and acknowledgements"
  ```

---

## 自检（Self-Review）

- 覆盖范围：Task 1 覆盖全部算法需求（TEA/ekey/双密码/尾部检测）；Task 2 覆盖下载/解密/缓存/魔数校验/进度/兜底重试；Task 3/4 覆盖 CLI 与桌面播放接线；Task 5 覆盖 PROJECT.md/QQMUSIC_PORTING.md/README 鸣谢。
- 类型一致性：`parse_ekey(&str) -> Result<Vec<u8>, Qmc2Error>`、`decrypt_factory(&str) -> Result<Box<dyn Qmc2Cipher>, Qmc2Error>`、`detect_footer(usize, &[u8]) -> Option<Footer>`、`prepare_playable(&str, Option<&str>, Option<watch::Sender<Option<f64>>>) -> Result<String, MediaError>` 在 Task 1→4 中签名一致。
- 已知限制（非缺陷，文档已记）：OGG 加密音质不入回退链；桌面解密期间 UI 无独立进度提示（CLI 有）；`.mnac`（AICodec）解密后容器由 GStreamer typefind 探测。
