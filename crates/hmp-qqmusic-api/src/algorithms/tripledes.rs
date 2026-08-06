//! 自定义 Triple-DES 实现（对应上游 `algorithms/tripledes.py`）。
//!
//! 上游为兼容 QQ 音乐 QRC 歌词解密的 3DES 变体（含自定义 PC-2 偏移），
//! 与标准 DES crate 不兼容，故整体移植。行为以 Python 参考实现为 Oracle。
//!
//! 位运算表达式与上游逐项保真（含 `>> 0`、冗余括号等），故放行
//! 对应 lint；这些"冗余"源于原始实现而非笔误。

#![allow(unused_parens)]
#![allow(clippy::identity_op)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::assign_op_pattern)]

/// 8 个 S 盒（上游 `sbox`）。
pub(crate) static SBOX: [[u8; 64]; 8] = [
    // sbox[0]
    [
        14, 4, 13, 1, 2, 15, 11, 8, 3, 10, 6, 12, 5, 9, 0, 7, 0, 15, 7, 4, 14, 2, 13, 1, 10, 6, 12,
        11, 9, 5, 3, 8, 4, 1, 14, 8, 13, 6, 2, 11, 15, 12, 9, 7, 3, 10, 5, 0, 15, 12, 8, 2, 4, 9,
        1, 7, 5, 11, 3, 14, 10, 0, 6, 13,
    ],
    // sbox[1]
    [
        15, 1, 8, 14, 6, 11, 3, 4, 9, 7, 2, 13, 12, 0, 5, 10, 3, 13, 4, 7, 15, 2, 8, 15, 12, 0, 1,
        10, 6, 9, 11, 5, 0, 14, 7, 11, 10, 4, 13, 1, 5, 8, 12, 6, 9, 3, 2, 15, 13, 8, 10, 1, 3, 15,
        4, 2, 11, 6, 7, 12, 0, 5, 14, 9,
    ],
    // sbox[2]
    [
        10, 0, 9, 14, 6, 3, 15, 5, 1, 13, 12, 7, 11, 4, 2, 8, 13, 7, 0, 9, 3, 4, 6, 10, 2, 8, 5,
        14, 12, 11, 15, 1, 13, 6, 4, 9, 8, 15, 3, 0, 11, 1, 2, 12, 5, 10, 14, 7, 1, 10, 13, 0, 6,
        9, 8, 7, 4, 15, 14, 3, 11, 5, 2, 12,
    ],
    // sbox[3]
    [
        7, 13, 14, 3, 0, 6, 9, 10, 1, 2, 8, 5, 11, 12, 4, 15, 13, 8, 11, 5, 6, 15, 0, 3, 4, 7, 2,
        12, 1, 10, 14, 9, 10, 6, 9, 0, 12, 11, 7, 13, 15, 1, 3, 14, 5, 2, 8, 4, 3, 15, 0, 6, 10,
        10, 13, 8, 9, 4, 5, 11, 12, 7, 2, 14,
    ],
    // sbox[4]
    [
        2, 12, 4, 1, 7, 10, 11, 6, 8, 5, 3, 15, 13, 0, 14, 9, 14, 11, 2, 12, 4, 7, 13, 1, 5, 0, 15,
        10, 3, 9, 8, 6, 4, 2, 1, 11, 10, 13, 7, 8, 15, 9, 12, 5, 6, 3, 0, 14, 11, 8, 12, 7, 1, 14,
        2, 13, 6, 15, 0, 9, 10, 4, 5, 3,
    ],
    // sbox[5]
    [
        12, 1, 10, 15, 9, 2, 6, 8, 0, 13, 3, 4, 14, 7, 5, 11, 10, 15, 4, 2, 7, 12, 9, 5, 6, 1, 13,
        14, 0, 11, 3, 8, 9, 14, 15, 5, 2, 8, 12, 3, 7, 0, 4, 10, 1, 13, 11, 6, 4, 3, 2, 12, 9, 5,
        15, 10, 11, 14, 1, 7, 6, 0, 8, 13,
    ],
    // sbox[6]
    [
        4, 11, 2, 14, 15, 0, 8, 13, 3, 12, 9, 7, 5, 10, 6, 1, 13, 0, 11, 7, 4, 9, 1, 10, 14, 3, 5,
        12, 2, 15, 8, 6, 1, 4, 11, 13, 12, 3, 7, 14, 10, 15, 6, 8, 0, 5, 9, 2, 6, 11, 13, 8, 1, 4,
        10, 7, 9, 5, 0, 15, 14, 2, 3, 12,
    ],
    // sbox[7]
    [
        13, 2, 8, 4, 6, 15, 11, 1, 10, 9, 3, 14, 5, 0, 12, 7, 1, 15, 13, 8, 10, 3, 7, 4, 12, 5, 6,
        11, 0, 14, 9, 2, 7, 11, 4, 1, 9, 12, 14, 2, 0, 6, 10, 13, 15, 3, 5, 8, 2, 1, 14, 7, 4, 10,
        8, 13, 15, 12, 9, 0, 3, 5, 6, 11,
    ],
];

pub(crate) const ENCRYPT: u8 = 1;
pub(crate) const DECRYPT: u8 = 0;

/// S 盒位重组（上游 `sbox_bit`）。
fn sbox_bit(a: u8) -> usize {
    ((a & 32) | ((a & 31) >> 1) | ((a & 1) << 4)) as usize
}

/// 初始置换（上游 `initial_permutation`），返回左右两个 32 位整数。
fn initial_permutation(input: &[u8]) -> (u32, u32) {
    let v0 = u32::from(input[0])
        | (u32::from(input[1]) << 8)
        | (u32::from(input[2]) << 16)
        | (u32::from(input[3]) << 24);
    let v1 = u32::from(input[4])
        | (u32::from(input[5]) << 8)
        | (u32::from(input[6]) << 16)
        | (u32::from(input[7]) << 24);

    let s0 = (((v1 >> 6) & 1) << 31
        | ((v1 >> 14) & 1) << 30
        | ((v1 >> 22) & 1) << 29
        | ((v1 >> 30) & 1) << 28
        | ((v0 >> 6) & 1) << 27
        | ((v0 >> 14) & 1) << 26
        | ((v0 >> 22) & 1) << 25
        | ((v0 >> 30) & 1) << 24
        | ((v1 >> 4) & 1) << 23
        | ((v1 >> 12) & 1) << 22
        | ((v1 >> 20) & 1) << 21
        | ((v1 >> 28) & 1) << 20
        | ((v0 >> 4) & 1) << 19
        | ((v0 >> 12) & 1) << 18
        | ((v0 >> 20) & 1) << 17
        | ((v0 >> 28) & 1) << 16
        | ((v1 >> 2) & 1) << 15
        | ((v1 >> 10) & 1) << 14
        | ((v1 >> 18) & 1) << 13
        | ((v1 >> 26) & 1) << 12
        | ((v0 >> 2) & 1) << 11
        | ((v0 >> 10) & 1) << 10
        | ((v0 >> 18) & 1) << 9
        | ((v0 >> 26) & 1) << 8
        | ((v1 >> 0) & 1) << 7
        | ((v1 >> 8) & 1) << 6
        | ((v1 >> 16) & 1) << 5
        | ((v1 >> 24) & 1) << 4
        | ((v0 >> 0) & 1) << 3
        | ((v0 >> 8) & 1) << 2
        | ((v0 >> 16) & 1) << 1
        | ((v0 >> 24) & 1));
    let s1 = (((v1 >> 7) & 1) << 31
        | ((v1 >> 15) & 1) << 30
        | ((v1 >> 23) & 1) << 29
        | ((v1 >> 31) & 1) << 28
        | ((v0 >> 7) & 1) << 27
        | ((v0 >> 15) & 1) << 26
        | ((v0 >> 23) & 1) << 25
        | ((v0 >> 31) & 1) << 24
        | ((v1 >> 5) & 1) << 23
        | ((v1 >> 13) & 1) << 22
        | ((v1 >> 21) & 1) << 21
        | ((v1 >> 29) & 1) << 20
        | ((v0 >> 5) & 1) << 19
        | ((v0 >> 13) & 1) << 18
        | ((v0 >> 21) & 1) << 17
        | ((v0 >> 29) & 1) << 16
        | ((v1 >> 3) & 1) << 15
        | ((v1 >> 11) & 1) << 14
        | ((v1 >> 19) & 1) << 13
        | ((v1 >> 27) & 1) << 12
        | ((v0 >> 3) & 1) << 11
        | ((v0 >> 11) & 1) << 10
        | ((v0 >> 19) & 1) << 9
        | ((v0 >> 27) & 1) << 8
        | ((v1 >> 1) & 1) << 7
        | ((v1 >> 9) & 1) << 6
        | ((v1 >> 17) & 1) << 5
        | ((v1 >> 25) & 1) << 4
        | ((v0 >> 1) & 1) << 3
        | ((v0 >> 9) & 1) << 2
        | ((v0 >> 17) & 1) << 1
        | ((v0 >> 25) & 1));
    (s0, s1)
}

/// 逆初始置换（上游 `inverse_permutation`）。
fn inverse_permutation(s0: u32, s1: u32) -> [u8; 8] {
    let mut data = [0u8; 8];
    data[3] = (((s1 >> 24) & 1) << 7
        | ((s0 >> 24) & 1) << 6
        | ((s1 >> 16) & 1) << 5
        | ((s0 >> 16) & 1) << 4
        | ((s1 >> 8) & 1) << 3
        | ((s0 >> 8) & 1) << 2
        | ((s1 >> 0) & 1) << 1
        | ((s0 >> 0) & 1)) as u8;
    data[2] = (((s1 >> 25) & 1) << 7
        | ((s0 >> 25) & 1) << 6
        | ((s1 >> 17) & 1) << 5
        | ((s0 >> 17) & 1) << 4
        | ((s1 >> 9) & 1) << 3
        | ((s0 >> 9) & 1) << 2
        | ((s1 >> 1) & 1) << 1
        | ((s0 >> 1) & 1)) as u8;
    data[1] = (((s1 >> 26) & 1) << 7
        | ((s0 >> 26) & 1) << 6
        | ((s1 >> 18) & 1) << 5
        | ((s0 >> 18) & 1) << 4
        | ((s1 >> 10) & 1) << 3
        | ((s0 >> 10) & 1) << 2
        | ((s1 >> 2) & 1) << 1
        | ((s0 >> 2) & 1)) as u8;
    data[0] = (((s1 >> 27) & 1) << 7
        | ((s0 >> 27) & 1) << 6
        | ((s1 >> 19) & 1) << 5
        | ((s0 >> 19) & 1) << 4
        | ((s1 >> 11) & 1) << 3
        | ((s0 >> 11) & 1) << 2
        | ((s1 >> 3) & 1) << 1
        | ((s0 >> 3) & 1)) as u8;
    data[7] = (((s1 >> 28) & 1) << 7
        | ((s0 >> 28) & 1) << 6
        | ((s1 >> 20) & 1) << 5
        | ((s0 >> 20) & 1) << 4
        | ((s1 >> 12) & 1) << 3
        | ((s0 >> 12) & 1) << 2
        | ((s1 >> 4) & 1) << 1
        | ((s0 >> 4) & 1)) as u8;
    data[6] = (((s1 >> 29) & 1) << 7
        | ((s0 >> 29) & 1) << 6
        | ((s1 >> 21) & 1) << 5
        | ((s0 >> 21) & 1) << 4
        | ((s1 >> 13) & 1) << 3
        | ((s0 >> 13) & 1) << 2
        | ((s1 >> 5) & 1) << 1
        | ((s0 >> 5) & 1)) as u8;
    data[5] = (((s1 >> 30) & 1) << 7
        | ((s0 >> 30) & 1) << 6
        | ((s1 >> 22) & 1) << 5
        | ((s0 >> 22) & 1) << 4
        | ((s1 >> 14) & 1) << 3
        | ((s0 >> 14) & 1) << 2
        | ((s1 >> 6) & 1) << 1
        | ((s0 >> 6) & 1)) as u8;
    data[4] = (((s1 >> 31) & 1) << 7
        | ((s0 >> 31) & 1) << 6
        | ((s1 >> 23) & 1) << 5
        | ((s0 >> 23) & 1) << 4
        | ((s1 >> 15) & 1) << 3
        | ((s0 >> 15) & 1) << 2
        | ((s1 >> 7) & 1) << 1
        | ((s0 >> 7) & 1)) as u8;
    data
}

/// F 函数（上游 `f`）。
fn f(state: u32, key: &[u8; 6]) -> u32 {
    let t1 = ((state & 1) << 31)
        | ((state & 0xF800_0000) >> 1)
        | ((state & 0x1F80_0000) >> 3)
        | ((state & 0x01F8_0000) >> 5)
        | ((state & 0x001F_8000) >> 7);
    let t2 = ((state & 0x0001_F800) << 15)
        | ((state & 0x0000_1F80) << 13)
        | ((state & 0x0000_01F8) << 11)
        | ((state & 0x0000_001F) << 9)
        | ((state & 0x8000_0000) >> 23);

    let k0 = ((t1 >> 24) & 0xFF) ^ u32::from(key[0]);
    let k1 = ((t1 >> 16) & 0xFF) ^ u32::from(key[1]);
    let k2 = ((t1 >> 8) & 0xFF) ^ u32::from(key[2]);
    let k3 = ((t2 >> 24) & 0xFF) ^ u32::from(key[3]);
    let k4 = ((t2 >> 16) & 0xFF) ^ u32::from(key[4]);
    let k5 = ((t2 >> 8) & 0xFF) ^ u32::from(key[5]);

    let state = (u32::from(SBOX[0][sbox_bit((k0 >> 2) as u8)]) << 28)
        | (u32::from(SBOX[1][sbox_bit((((k0 & 0x03) << 4) | (k1 >> 4)) as u8)]) << 24)
        | (u32::from(SBOX[2][sbox_bit((((k1 & 0x0F) << 2) | (k2 >> 6)) as u8)]) << 20)
        | (u32::from(SBOX[3][sbox_bit((k2 & 0x3F) as u8)]) << 16)
        | (u32::from(SBOX[4][sbox_bit((k3 >> 2) as u8)]) << 12)
        | (u32::from(SBOX[5][sbox_bit((((k3 & 0x03) << 4) | (k4 >> 4)) as u8)]) << 8)
        | (u32::from(SBOX[6][sbox_bit((((k4 & 0x0F) << 2) | (k5 >> 6)) as u8)]) << 4)
        | u32::from(SBOX[7][sbox_bit((k5 & 0x3F) as u8)]);

    ((state >> 16) & 1) << 31
        | ((state >> 25) & 1) << 30
        | ((state >> 12) & 1) << 29
        | ((state >> 11) & 1) << 28
        | ((state >> 3) & 1) << 27
        | ((state >> 20) & 1) << 26
        | ((state >> 4) & 1) << 25
        | ((state >> 15) & 1) << 24
        | ((state >> 31) & 1) << 23
        | ((state >> 17) & 1) << 22
        | ((state >> 9) & 1) << 21
        | ((state >> 6) & 1) << 20
        | ((state >> 27) & 1) << 19
        | ((state >> 14) & 1) << 18
        | ((state >> 1) & 1) << 17
        | ((state >> 22) & 1) << 16
        | ((state >> 30) & 1) << 15
        | ((state >> 24) & 1) << 14
        | ((state >> 8) & 1) << 13
        | ((state >> 18) & 1) << 12
        | ((state >> 0) & 1) << 11
        | ((state >> 5) & 1) << 10
        | ((state >> 29) & 1) << 9
        | ((state >> 23) & 1) << 8
        | ((state >> 13) & 1) << 7
        | ((state >> 19) & 1) << 6
        | ((state >> 2) & 1) << 5
        | ((state >> 26) & 1) << 4
        | ((state >> 10) & 1) << 3
        | ((state >> 21) & 1) << 2
        | ((state >> 28) & 1) << 1
        | ((state >> 7) & 1)
}

/// 单块 DES 加/解密（上游 `crypt`）。
pub(crate) fn crypt(input: &[u8], schedule: &[[u8; 6]; 16]) -> [u8; 8] {
    let (mut s0, mut s1) = initial_permutation(input);

    for idx in 0..15 {
        let previous_s1 = s1;
        s1 = f(s1, &schedule[idx]) ^ s0;
        s0 = previous_s1;
    }
    s0 = f(s1, &schedule[15]) ^ s0;

    inverse_permutation(s0, s1)
}

/// 密钥扩展（上游 `key_schedule`，含自定义 PC-2 偏移 Bug）。
fn key_schedule(key: &[u8], mode: u8) -> [[u8; 6]; 16] {
    let mut schedule = [[0u8; 6]; 16];
    let key_rnd_shift: [u8; 16] = [1, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 1];
    let key_perm_c: [u8; 28] = [
        56, 48, 40, 32, 24, 16, 8, 0, 57, 49, 41, 33, 25, 17, 9, 1, 58, 50, 42, 34, 26, 18, 10, 2,
        59, 51, 43, 35,
    ];
    let key_perm_d: [u8; 28] = [
        62, 54, 46, 38, 30, 22, 14, 6, 61, 53, 45, 37, 29, 21, 13, 5, 60, 52, 44, 36, 28, 20, 12,
        4, 27, 19, 11, 3,
    ];
    let key_compression: [u8; 48] = [
        13, 16, 10, 23, 0, 4, 2, 27, 14, 5, 20, 9, 22, 18, 11, 3, 25, 7, 15, 6, 26, 19, 12, 1, 40,
        51, 30, 36, 46, 54, 29, 39, 50, 44, 32, 47, 43, 48, 38, 55, 33, 52, 45, 41, 49, 35, 28, 31,
    ];

    let v0 = u32::from(key[0])
        | (u32::from(key[1]) << 8)
        | (u32::from(key[2]) << 16)
        | (u32::from(key[3]) << 24);
    let v1 = u32::from(key[4])
        | (u32::from(key[5]) << 8)
        | (u32::from(key[6]) << 16)
        | (u32::from(key[7]) << 24);

    let mut c = 0u32;
    for (i, &b) in key_perm_c.iter().enumerate() {
        let bit = if b < 32 {
            (v0 >> (31 - b)) & 1
        } else {
            (v1 >> (63 - b)) & 1
        };
        c |= bit << (31 - i as u32);
    }
    let mut d = 0u32;
    for (i, &b) in key_perm_d.iter().enumerate() {
        let bit = if b < 32 {
            (v0 >> (31 - b)) & 1
        } else {
            (v1 >> (63 - b)) & 1
        };
        d |= bit << (31 - i as u32);
    }

    for i in 0..16 {
        c = ((c << key_rnd_shift[i]) | (c >> (28 - key_rnd_shift[i]))) & 0xFFFF_FFF0;
        d = ((d << key_rnd_shift[i]) | (d >> (28 - key_rnd_shift[i]))) & 0xFFFF_FFF0;

        let togen = if mode == DECRYPT { 15 - i } else { i };

        for j in 0..24 {
            let bit = (c >> (31 - key_compression[j])) & 1;
            schedule[togen][j / 8] |= (bit as u8) << (7 - (j % 8));
        }
        for j in 24..48 {
            let bit = (d >> (31 - (key_compression[j] - 27))) & 1;
            schedule[togen][j / 8] |= (bit as u8) << (7 - (j % 8));
        }
    }
    schedule
}

/// 3DES 密钥扩展（上游 `tripledes_key_setup`）。
pub(crate) fn tripledes_key_setup(key: &[u8], mode: u8) -> [[[u8; 6]; 16]; 3] {
    if mode == ENCRYPT {
        [
            key_schedule(&key[0..8], ENCRYPT),
            key_schedule(&key[8..16], DECRYPT),
            key_schedule(&key[16..24], ENCRYPT),
        ]
    } else {
        [
            key_schedule(&key[16..24], DECRYPT),
            key_schedule(&key[8..16], ENCRYPT),
            key_schedule(&key[0..8], DECRYPT),
        ]
    }
}

/// 3DES 逐块解密/加密（上游 `tripledes_crypt`）。
pub(crate) fn tripledes_crypt(data: &[u8], schedule: &[[[u8; 6]; 16]; 3]) -> [u8; 8] {
    let mut block = [0u8; 8];
    block.copy_from_slice(data);
    for i in 0..3 {
        block = crypt(&block, &schedule[i]);
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sbox_table_has_expected_first_row() {
        assert_eq!(SBOX[0][0], 14);
        assert_eq!(SBOX[0][63], 13);
        assert_eq!(SBOX[7][0], 13);
        assert_eq!(SBOX[7][63], 11);
    }

    #[test]
    fn crypt_roundtrip_encrypt_decrypt() {
        let key = b"123456781234567812345678";
        let plain = [0x01u8, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF];
        let enc_sched = tripledes_key_setup(key, ENCRYPT);
        let dec_sched = tripledes_key_setup(key, DECRYPT);
        let encrypted = tripledes_crypt(&plain, &enc_sched);
        let decrypted = tripledes_crypt(&encrypted, &dec_sched);
        assert_eq!(decrypted, plain, "3DES 加解密往返应一致");
    }
}

#[cfg(test)]
mod oracle_tests {
    use super::*;

    #[test]
    fn oracle_encrypt_vector_matches_python() {
        let key = b"123456781234567812345678";
        let plain = [0x01u8, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF];
        let sched = tripledes_key_setup(key, ENCRYPT);
        let out = tripledes_crypt(&plain, &sched);
        assert_eq!(hex(&out), "90872e7fb5660fcb");
    }

    #[test]
    fn oracle_qrc_block0_matches_python() {
        let key = b"!@#)(*$%123ZXC!@!@#)(NHL";
        let raw: Vec<u8> = {
            let path = concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/lyric/encrypted.json"
            );
            let body: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            let hex = body["req_0"]["data"]["lyric"].as_str().unwrap();
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect()
        };
        let sched = tripledes_key_setup(key, DECRYPT);
        let out = tripledes_crypt(&raw[0..8], &sched);
        assert_eq!(hex(&out), "789c8d54cb4edb40");
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}
