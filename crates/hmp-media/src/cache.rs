//! 缓存键、文件命名与容量驱逐。

use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};

/// 根据 url 与 ekey 生成稳定的缓存键（SHA-1 前 16 位十六进制）。
pub fn cache_key(url: &str, ekey: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(url.as_bytes());
    hasher.update(b"|");
    hasher.update(ekey.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..8])
}

/// 根据文件头魔数推断音频扩展名。
///
/// 返回 `None` 表示无法识别。
pub fn extension_from_magic(head: &[u8]) -> Option<&'static str> {
    match head {
        [b'f', b'L', b'a', b'C', ..] => Some("flac"),
        [b'O', b'g', b'g', b'S', ..] => Some("ogg"),
        [b'f', b't', b'y', b'p', ..] => Some("m4a"),
        [b'I', b'D', b'3', ..] => Some("mp3"),
        [0xff, 0xfb, ..] => Some("mp3"),
        _ => None,
    }
}

/// 最终缓存文件路径：`<root>/<key>.<ext>`。
pub fn final_path(root: &Path, key: &str, ext: &str) -> PathBuf {
    root.join(format!("{key}.{ext}"))
}

/// 下载中临时文件路径：`<root>/.<key>.<pid>.tmp`。
pub fn tmp_path(root: &Path, key: &str) -> PathBuf {
    root.join(format!(".{key}.{}.tmp", std::process::id()))
}

/// 容量驱逐：若根目录内（不含 `*.tmp`）总大小超过上限则按 mtime 升序删除最旧文件直至达标。
///
/// 上限由环境变量 `HMP_DECRYPT_CACHE_MIB` 控制（默认 2048 MiB）。
pub fn evict_if_needed(root: &Path) -> Result<(), std::io::Error> {
    let cap_bytes = cache_cap_bytes();

    // 收集非 .tmp 文件及元数据
    let mut files: Vec<(PathBuf, std::fs::Metadata)> = Vec::new();
    let mut total_size: u64 = 0;

    let dir = match std::fs::read_dir(root) {
        Ok(d) => d,
        Err(_) => return Ok(()),
    };

    for entry in dir {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        // 跳过 .tmp 文件与目录
        if path.extension().is_some_and(|e| e == "tmp") {
            continue;
        }
        if path.is_dir() {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        total_size += meta.len();
        files.push((path, meta));
    }

    if total_size <= cap_bytes || files.is_empty() {
        return Ok(());
    }

    // 按 mtime 升序（最旧在前）
    files.sort_by_key(|(_, m)| m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH));

    for (path, meta) in &files {
        if total_size <= cap_bytes {
            break;
        }
        let _ = std::fs::remove_file(path);
        total_size = total_size.saturating_sub(meta.len());
    }

    Ok(())
}

/// 解析 `HMP_DECRYPT_CACHE_MIB` 环境变量，默认 2048。
fn cache_cap_bytes() -> u64 {
    const DEFAULT_MIB: u64 = 2048;
    let mib = std::env::var("HMP_DECRYPT_CACHE_MIB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MIB);
    mib * 1024 * 1024
}

// ---------------------------------------------------------------------------
// 内部 hex 编码（仅需 encode，不引入额外依赖）
// ---------------------------------------------------------------------------
mod hex {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

    pub fn encode(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            s.push(HEX_CHARS[(b >> 4) as usize] as char);
            s.push(HEX_CHARS[(b & 0x0f) as usize] as char);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_stable_and_distinct() {
        let k1 = cache_key("https://a/1.mflac", "ekey1");
        let k2 = cache_key("https://a/1.mflac", "ekey1");
        assert_eq!(k1, k2, "两次相同输入应一致");

        let k3 = cache_key("https://a/2.mflac", "ekey1");
        assert_ne!(k1, k3, "不同 url 应不同");

        let k4 = cache_key("https://a/1.mflac", "ekey2");
        assert_ne!(k1, k4, "不同 ekey 应不同");

        // 长度 == 16（hex 前缀）
        assert_eq!(k1.len(), 16);
    }

    #[test]
    fn extension_from_magic_known() {
        assert_eq!(extension_from_magic(b"fLaC"), Some("flac"));
        assert_eq!(extension_from_magic(b"fLaC\0\0\0\0"), Some("flac"));
        assert_eq!(extension_from_magic(b"OggS"), Some("ogg"));
        assert_eq!(extension_from_magic(b"ftyp"), Some("m4a"));
        assert_eq!(extension_from_magic(b"ftypisom"), Some("m4a"));
        assert_eq!(extension_from_magic(b"ID3"), Some("mp3"));
        assert_eq!(extension_from_magic(b"ID3\0"), Some("mp3"));
        assert_eq!(extension_from_magic(&[0xff, 0xfb]), Some("mp3"));
        assert_eq!(extension_from_magic(&[0xff, 0xfb, 0x90, 0x00]), Some("mp3"));
    }

    #[test]
    fn extension_from_magic_unknown() {
        assert_eq!(extension_from_magic(b"\0\0\0\0"), None);
        assert_eq!(extension_from_magic(b"RIFF"), None);
        assert_eq!(extension_from_magic(&[]), None);
        assert_eq!(extension_from_magic(&[0x80]), None);
    }

    #[test]
    fn evict_keeps_under_cap() {
        let root = std::env::temp_dir().join(format!("hmp-media-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        // 创建 3 个文件（各约 400 KiB），mtime 错开
        // 3×400 KiB = 1200 KiB > 1 MiB cap → 最旧的一个被驱逐，剩余 800 KiB ≤ 1 MiB
        let f1 = root.join("a.bin");
        let f2 = root.join("b.bin");
        let f3 = root.join("c.bin");

        let chunk = vec![0u8; 400 * 1024]; // 400 KiB
        std::fs::write(&f1, &chunk).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&f2, &chunk).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&f3, &chunk).unwrap();

        unsafe {
            std::env::set_var("HMP_DECRYPT_CACHE_MIB", "1");
        }

        evict_if_needed(&root).unwrap();

        // 验证目录总大小 <= 1024 * 1024
        let mut remaining: u64 = 0;
        let mut names: Vec<String> = Vec::new();
        for e in std::fs::read_dir(&root).unwrap() {
            let e = e.unwrap();
            remaining += e.metadata().unwrap().len();
            names.push(e.file_name().to_string_lossy().to_string());
        }

        assert!(
            remaining <= 1024 * 1024,
            "total {remaining} should be <= 1 MiB"
        );
        // a.bin 应被删（最旧），b 和 c 留下
        assert!(
            !names.contains(&"a.bin".to_string()),
            "oldest file should be removed"
        );
        assert!(names.contains(&"b.bin".to_string()));
        assert!(names.contains(&"c.bin".to_string()));

        // cleanup
        unsafe {
            std::env::remove_var("HMP_DECRYPT_CACHE_MIB");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
