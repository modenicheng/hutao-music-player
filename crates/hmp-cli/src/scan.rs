//! `hmp scan <目录>`：递归扫描本地音乐入库（标签元数据 + 文件名回退）。
//!
//! 幂等：同路径重扫只更新元数据；输出新增/更新计数。

use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

use hmp_storage::{LibraryDb, read_meta};

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

/// 扫描目录入库，返回 (新增, 更新)。
/// 目录入口先 canonicalize：`hmp scan ./Music` 不再因 daemon 后续 cwd 不同而失配。
pub fn scan_dir(dir: &Path, db: &mut LibraryDb) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    let dir = dir
        .canonicalize()
        .map_err(|_| format!("不是目录: {}", dir.display()))?;
    let mut files = Vec::new();
    let mut visited = HashSet::new();
    collect_audio(&dir, &mut visited, &mut files)?;
    let (mut added, mut updated) = (0u32, 0u32);
    for path in files {
        let key = format!("local:{}", path.display());
        let existed = db.track_id("local", &key)?.is_some();
        db.add_local_file(&path, read_meta(&path).as_ref())?;
        if existed {
            updated += 1;
        } else {
            added += 1;
        }
    }
    Ok((added, updated))
}

/// 运行入口。
pub async fn run(dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = hmp_storage::data_dir().join("library.sqlite3");
    let mut db = LibraryDb::open(&path)?;
    let (added, updated) = scan_dir(Path::new(dir), &mut db)?;
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "扫描完成：新增 {added} 首，更新 {updated} 首（库: {}）",
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
        std::fs::write(dir.path().join("a.mp3"), b"not audio").unwrap();
        std::fs::write(dir.path().join("b.flac"), b"not audio").unwrap();
        std::fs::write(dir.path().join("sub").join("c.ogg"), b"not audio").unwrap();
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
        let (added, _) = scan_dir(&scan_dir_input, &mut db).unwrap();
        assert_eq!(added, 3);
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
    fn scan_is_idempotent_and_counts() {
        let dir = sample_dir();
        let mut db = LibraryDb::open_in_memory().unwrap();
        let (added, updated) = scan_dir(dir.path(), &mut db).unwrap();
        assert_eq!(added, 3);
        assert_eq!(updated, 0);
        // 重扫 → 全部更新，无新增。
        let (added, updated) = scan_dir(dir.path(), &mut db).unwrap();
        assert_eq!(added, 0);
        assert_eq!(updated, 3);
        // 无标签 → 文件名回退入库（title = 文件名主干，经 add_local_file 回退）。
    }
}
