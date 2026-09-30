//! 本地扫描共享逻辑（里程碑 E2 下沉自 hmp-cli/src/scan.rs）。
//!
//! CLI `hmp scan` 与 daemon watcher 复用：文件指纹（移动/改名检测候选）、
//! 封面提取（`<data_dir>/covers/<hash>.jpg`）。

use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::path::Path;

/// 文件指纹：内容 hash（前 1MB）+ size。
/// 不含路径与 mtime：移动/改名后内容不变 → 指纹不变（供行复用候选）；
/// 内容相同但 mtime 不同的文件由 `record_scan_file` 内的 mtime 校验排除。
pub fn file_fingerprint(path: &Path, size: u64) -> std::io::Result<String> {
    use std::io::Read;
    let mut hasher = DefaultHasher::new();
    let mut f = std::fs::File::open(path)?;
    let mut buf = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.write(&buf[..n]);
        total += n as u64;
        if total >= 1_048_576 {
            break; // 1MB 前缀足以区分
        }
    }
    hasher.write_u64(size);
    Ok(format!("{:016x}", hasher.finish()))
}

/// 提取封面到 `<data_dir>/covers/<hash>.jpg`；返回 cover_uri（`file://…`）。
/// 同 hash 已存在则跳过写（去重）。
pub fn persist_cover(cover: &[u8]) -> std::io::Result<String> {
    let covers = crate::data_dir().join("covers");
    std::fs::create_dir_all(&covers)?;
    let mut hasher = DefaultHasher::new();
    hasher.write(cover);
    let name = format!("{:016x}.jpg", hasher.finish());
    let cpath = covers.join(&name);
    if !cpath.exists() {
        // tmp + rename：并发写同一 hash（watcher 与播放解析同时命中新封面）
        // 交错写会因 exists() 去重而永久缓存半张封面。
        let tmp = covers.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, cover)?;
        std::fs::rename(&tmp, &cpath)?;
    }
    Ok(format!("file://{}", cpath.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_stable_across_moves() {
        let dir = std::env::temp_dir().join(format!("hmp-fp2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.mp3");
        let b = dir.join("b.mp3");
        std::fs::write(&a, b"content-x").unwrap();
        std::fs::write(&b, b"content-x").unwrap();
        let fa = file_fingerprint(&a, std::fs::metadata(&a).unwrap().len()).unwrap();
        let fb = file_fingerprint(&b, std::fs::metadata(&b).unwrap().len()).unwrap();
        assert_eq!(fa, fb, "内容相同则指纹相同（移动检测候选）");
        let diff = dir.join("diff.mp3");
        std::fs::write(&diff, b"other").unwrap();
        let fd = file_fingerprint(&diff, std::fs::metadata(&diff).unwrap().len()).unwrap();
        assert_ne!(fa, fd);
    }

    #[test]
    fn persist_cover_writes_deduplicated_file() {
        // 隔离数据目录：persist_cover 写 `<data_dir>/covers`（全局解析路径）。
        // 不隔离有两个后果：与同二进制内持 TEST_ENV_LOCK 改 XDG_DATA_HOME 的
        // 测试（xdg.rs dirs_*）竞态——数据目录被指到随后删除的 TempDir，
        // exists() 断言偶发红（2026-09-29 全量跑复现）；且测试封面会落进
        // 开发机真实媒体库目录。
        let _lock = crate::TEST_ENV_LOCK.lock().unwrap();
        let dir = tempfile::TempDir::new().unwrap();
        // SAFETY: 持 TEST_ENV_LOCK，改 env 的测试互斥串行。
        unsafe {
            std::env::set_var("XDG_DATA_HOME", dir.path());
        }
        let cover = vec![1u8, 2, 3, 4];
        let uri1 = persist_cover(&cover).unwrap();
        let uri2 = persist_cover(&cover).unwrap();
        assert_eq!(uri1, uri2, "同封面去重（同 hash 文件名）");
        assert!(uri1.starts_with("file://"), "{uri1}");
        let p = uri1.strip_prefix("file://").unwrap();
        assert!(std::path::Path::new(p).exists());
        // SAFETY: 同上（先恢复 env 再让 TempDir 落盘内容随目录销毁）。
        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
    }
}
