//! `hmp scan <目录>`：递归扫描本地音乐入库（标签元数据 + 文件名回退）。
//!
//! 幂等：同路径重扫只更新元数据；输出新增/更新/跳过/缺失计数。
//! 里程碑 E：扫描根注册（scan_roots + generation）、增量跳过（mtime_ns+size）、
//! missing 标记、移动/改名指纹复用、封面提取。

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::io::Write;
use std::path::Path;

use hmp_storage::{LibraryDb, ScanOutcome, read_meta};

/// 扫描报告。
#[derive(Clone, Copy, Debug, Default)]
pub struct ScanReport {
    /// 新曲目入库。
    pub added: u32,
    /// 元数据/路径更新（含移动指纹复用）。
    pub updated: u32,
    /// mtime+size 未变，跳过。
    pub skipped: u32,
    /// 本轮标记为缺失的文件数。
    pub missing: u32,
}

/// 递归收集音频文件（按扩展名过滤）。
/// 目录与文件均 canonicalize：相对路径/symlink → 绝对真实路径，
/// 保证 `local:<path>` 身份稳定、可去重（P1）；visited 集合防 symlink 环。
fn collect_audio(
    dir: &Path,
    visited: &mut HashSet<std::path::PathBuf>,
    out: &mut Vec<std::path::PathBuf>,
) -> std::io::Result<()> {
    let real = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !visited.insert(real.clone()) {
        return Ok(()); // 已访问（symlink 环/重复目录）
    }
    for entry in std::fs::read_dir(&real)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_audio(&path, visited, out)?;
        } else if hmp_storage::is_audio_ext(&path) {
            out.push(path.canonicalize().unwrap_or(path));
        }
    }
    Ok(())
}

/// 文件指纹：内容 hash（前 1MB）+ size。
/// 不含路径与 mtime：移动/改名后内容不变 → 指纹不变（供行复用候选）；
/// 内容相同但 mtime 不同的文件由 record_scan_file 内的 mtime 校验排除。
fn file_fingerprint(path: &Path, size: u64) -> std::io::Result<String> {
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
fn persist_cover(cover: &[u8]) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let covers = hmp_storage::data_dir().join("covers");
    std::fs::create_dir_all(&covers)?;
    let mut hasher = DefaultHasher::new();
    hasher.write(cover);
    let name = format!("{:016x}.jpg", hasher.finish());
    let cpath = covers.join(&name);
    if !cpath.exists() {
        std::fs::write(&cpath, cover)?;
    }
    Ok(Some(format!("file://{}", cpath.display())))
}

/// 扫描目录入库，返回报告。
/// 目录入口先 canonicalize：`hmp scan ./Music` 不再因 daemon 后续 cwd 不同而失配。
pub fn scan_dir(root: &Path, db: &mut LibraryDb) -> Result<ScanReport, Box<dyn std::error::Error>> {
    let dir = root
        .canonicalize()
        .map_err(|_| format!("不是目录: {}", root.display()))?;
    let (root_id, generation) = db.begin_scan(&dir)?;
    let mut files = Vec::new();
    let mut visited = HashSet::new();
    collect_audio(&dir, &mut visited, &mut files)?;
    let mut report = ScanReport::default();
    for path in files {
        let md = std::fs::metadata(&path)?;
        let size = md.len();
        let fp = file_fingerprint(&path, size)?;
        let local_meta = read_meta(&path);
        // 封面先落盘（新增/更新时；record_scan_file 前完成，cover_uri 经 set_track_cover）。
        let cover_uri = match &local_meta {
            Some(m) => m.cover.as_deref().map(persist_cover).transpose()?.flatten(),
            None => None,
        };
        let outcome = db.record_scan_file(root_id, generation, &path, local_meta.as_ref(), &fp)?;
        if let Some(uri) = &cover_uri {
            db.set_track_cover(&format!("local:{}", path.display()), uri)?;
        }
        match outcome {
            ScanOutcome::Added => report.added += 1,
            ScanOutcome::Updated => report.updated += 1,
            ScanOutcome::Skipped => report.skipped += 1,
            ScanOutcome::MissingReset => report.skipped += 1, // 复位不算更新
        }
    }
    report.missing = db.finish_scan(root_id, generation)?;
    Ok(report)
}

/// 运行入口。
pub async fn run(dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = hmp_storage::data_dir().join("library.sqlite3");
    let mut db = LibraryDb::open(&path)?;
    let report = scan_dir(Path::new(dir), &mut db)?;
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "扫描完成：新增 {} 首，更新 {} 首，跳过 {} 首，缺失 {} 首（库: {}）",
        report.added,
        report.updated,
        report.skipped,
        report.missing,
        path.display()
    )?;
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("a.mp3"), b"aaa").unwrap();
        std::fs::write(dir.path().join("b.flac"), b"bbbb").unwrap();
        std::fs::write(dir.path().join("sub").join("c.ogg"), b"ccccc").unwrap();
        std::fs::write(dir.path().join("note.txt"), b"skip me").unwrap();
        dir
    }

    #[test]
    fn collect_recurses_and_filters() {
        let dir = sample_dir();
        let mut files = Vec::new();
        let mut visited = HashSet::new();
        collect_audio(dir.path(), &mut visited, &mut files).unwrap();
        assert_eq!(files.len(), 3, "3 个音频，1 个非音频被过滤: {files:?}");
        // 规范化：路径应绝对化（collect 内部 canonicalize）。
        assert!(files.iter().all(|p| p.is_absolute()));
    }

    #[test]
    fn scan_canonicalizes_relative_dir() {
        // 相对目录扫描 → 入库 key 为绝对真实路径（daemon 后续 cwd 不同也能找到）。
        let dir = sample_dir();
        let mut db = LibraryDb::open_in_memory().unwrap();
        let rel = dir
            .path()
            .strip_prefix(std::env::current_dir().unwrap())
            .map(|p| p.to_path_buf());
        let scan_dir_input = match rel {
            Ok(r) if r.components().count() > 0 => r,
            _ => dir.path().to_path_buf(), // tempdir 不在 cwd 下：退化为绝对路径
        };
        let report = scan_dir(&scan_dir_input, &mut db).unwrap();
        assert_eq!(report.added, 3);
        // 每个文件都能以 canonical 路径查到（相对 key 会失配）。
        for name in ["a.mp3", "b.flac"] {
            let canonical = std::fs::canonicalize(dir.path().join(name)).unwrap();
            let key = format!("local:{}", canonical.display());
            assert!(
                db.track_id("local", &key).unwrap().is_some(),
                "入库 key 应为 canonical 绝对路径: {key}"
            );
        }
    }

    #[test]
    fn scan_is_incremental_and_counts() {
        let dir = sample_dir();
        let mut db = LibraryDb::open_in_memory().unwrap();
        let r1 = scan_dir(dir.path(), &mut db).unwrap();
        assert_eq!(r1.added, 3);
        assert_eq!(r1.updated, 0);
        assert_eq!(r1.missing, 0);
        // 重扫（mtime+size 未变）→ 全部跳过，无新增/更新。
        let r2 = scan_dir(dir.path(), &mut db).unwrap();
        assert_eq!(r2.added, 0);
        assert_eq!(r2.updated, 0);
        assert_eq!(r2.skipped, 3, "增量：未变文件应跳过");
        // 删除一个文件 → 缺失标记。
        std::fs::remove_file(dir.path().join("a.mp3")).unwrap();
        let r3 = scan_dir(dir.path(), &mut db).unwrap();
        assert_eq!(r3.added, 0);
        assert_eq!(r3.missing, 1, "删除的文件应标 missing");
    }

    #[test]
    fn fingerprint_matches_path_change() {
        let dir = sample_dir();
        let mut db = LibraryDb::open_in_memory().unwrap();
        scan_dir(dir.path(), &mut db).unwrap();
        // 移动 a.mp3 → a2.mp3：指纹命中（mtime+size 不变），复用行不产生孤儿。
        let old = dir.path().join("a.mp3");
        let new = dir.path().join("a2.mp3");
        std::fs::rename(&old, &new).unwrap();
        let r = scan_dir(dir.path(), &mut db).unwrap();
        assert_eq!(r.added, 0, "移动后不应新增");
        assert_eq!(r.updated, 1, "指纹命中复用行");
        let key_new = format!("local:{}", new.canonicalize().unwrap().display());
        let key_old = format!(
            "local:{}",
            old.canonicalize().unwrap_or(old.clone()).display()
        );
        assert!(db.track_id("local", &key_new).unwrap().is_some());
        assert!(
            db.track_id("local", &key_old).unwrap().is_none(),
            "旧路径行应迁移"
        );
    }
}
