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
///
/// S9 R2：落原图之外 best-effort 增写派生缩略件 `<hash>.512.jpg`（≤512px、
/// JPEG q80，桌面读侧 miss 优先消费——解码触摸 ~6.5MB → ~1.2MB）；失败只
/// 日志不影响返回值。五个调用点（daemon content/local/watcher/reconcile +
/// CLI scan）经此自动全覆盖。
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
    write_derived_cover(&covers, &name, cover);
    Ok(format!("file://{}", cpath.display()))
}

/// 派生缩略件写（S9 R2，best-effort）：拼名契约 [`hmp_core::media::derived_cover_name`]
/// （互锚：读侧 hmp-desktop cover_cache 同源拼名）。
///
/// 跳过条件：拼名失败 / 派生件已存在（同 hash 去重）/ 原图最长边 ≤512
/// （header 探尺寸不整解——QQ CDN 小封面天然免写免解码）。任何失败只
/// debug 日志、绝不向调用方报错：派生件缺失时读侧自动回退原图，写侧
/// 签名与返回值保持不变。
fn write_derived_cover(covers: &Path, original_name: &str, cover: &[u8]) {
    if let Err(err) = try_write_derived_cover(covers, original_name, cover) {
        tracing::debug!(%err, original = original_name, "derived cover write skipped (best-effort)");
    }
}

fn try_write_derived_cover(
    covers: &Path,
    original_name: &str,
    cover: &[u8],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use image::ImageReader;
    let side = hmp_core::media::COVER_DERIVED_SIDE;
    let derived_name =
        hmp_core::media::derived_cover_name(original_name).ok_or("empty cover stem")?;
    let dpath = covers.join(&derived_name);
    if dpath.exists() {
        return Ok(()); // 同 hash 去重（与原图同款 exists 短路）
    }
    // header 探尺寸（into_dimensions 只读头，不整解）：≤512 原图读侧直用。
    let (w, h) = ImageReader::new(std::io::Cursor::new(cover))
        .with_guessed_format()?
        .into_dimensions()?;
    if w.max(h) <= side {
        return Ok(());
    }
    // 整解一次（daemon 侧瞬态有界）→ 保比缩到 ≤side → JPEG q80 → tmp+rename
    //（与原图同款原子性：CLI scan 与 daemon 并发同 hash 时 rename 幂等覆盖，
    // 内容相同）。
    let img = image::load_from_memory(cover)?;
    let thumb = img.thumbnail(side, side);
    let mut encoded = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 80);
    thumb.write_with_encoder(encoder)?;
    let tmp = covers.join(format!(".{derived_name}.tmp"));
    std::fs::write(&tmp, encoded)?;
    std::fs::rename(&tmp, &dpath)?;
    Ok(())
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

    // —— S9 R2 派生缩略件写侧（互锚契约：hmp-core::media::derived_cover_name）——

    /// 隔离数据目录（同上方先例：持锁改 XDG_DATA_HOME——锁随返回值存活到
    /// 测试结束）；data_dir() 在 XDG 覆盖时 = `$XDG_DATA_HOME/hmp`，
    /// covers 即 `$XDG_DATA_HOME/hmp/covers`。
    fn isolated_covers() -> (
        std::sync::MutexGuard<'static, ()>,
        tempfile::TempDir,
        std::path::PathBuf,
    ) {
        let lock = crate::TEST_ENV_LOCK.lock().unwrap();
        let dir = tempfile::TempDir::new().unwrap();
        // SAFETY: 持 TEST_ENV_LOCK。
        unsafe {
            std::env::set_var("XDG_DATA_HOME", dir.path());
        }
        let covers = dir.path().join("hmp").join("covers");
        std::fs::create_dir_all(&covers).unwrap();
        (lock, dir, covers)
    }

    fn drop_isolation() {
        // SAFETY: 调用方仍持 TEST_ENV_LOCK（isolated_covers 返回的 guard）。
        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
    }

    /// w×h JPEG 封面字节（S6 素材同量级：>512 需出派生件）。
    fn jpeg_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 0x40, 0xFF])
        });
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut buf),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        buf
    }

    /// covers 目录内全部文件名（无点文件过滤——tmp 残留也要可见）。
    fn cover_names(covers: &Path) -> Vec<String> {
        std::fs::read_dir(covers)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    /// 互锚：写侧产名 = hmp-core 拼名（`.512.jpg` 钉死）。
    fn derived_name_of(original: &str) -> String {
        hmp_core::media::derived_cover_name(original).unwrap()
    }

    #[test]
    fn persist_cover_writes_derived_thumbnail_for_large_covers() {
        let (_lock, dir, covers) = isolated_covers();
        let uri = persist_cover(&jpeg_bytes(1200, 900)).unwrap();
        let original = std::path::Path::new(uri.strip_prefix("file://").unwrap())
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let derived = derived_name_of(&original);
        assert_eq!(
            derived,
            format!("{}.512.jpg", original.strip_suffix(".jpg").unwrap())
        );
        let names = cover_names(&covers);
        assert!(names.contains(&original), "原图落盘");
        assert!(names.contains(&derived), "派生件落盘：{names:?}");
        // 派生件解码 ≤512 且保比（1200×900 = 4:3 → 512×384）
        let decoded = image::ImageReader::open(covers.join(&derived))
            .unwrap()
            .decode()
            .unwrap();
        let (w, h) = (decoded.width(), decoded.height());
        assert!(w.max(h) <= 512, "派生件最长边 ≤512，实际 {w}×{h}");
        let ratio = f64::from(w) / f64::from(h);
        assert!((ratio - 4.0 / 3.0).abs() < 0.05, "保比 4:3，实际 {w}×{h}");
        drop_isolation();
        drop(dir);
    }

    #[test]
    fn persist_cover_second_call_rewrites_nothing() {
        let (_lock, dir, covers) = isolated_covers();
        let bytes = jpeg_bytes(1200, 900);
        let uri = persist_cover(&bytes).unwrap();
        let original = std::path::Path::new(uri.strip_prefix("file://").unwrap())
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let derived = derived_name_of(&original);
        let stat = |name: &str| {
            std::fs::metadata(covers.join(name))
                .unwrap()
                .modified()
                .unwrap()
        };
        let (o1, d1) = (stat(&original), stat(&derived));
        std::thread::sleep(std::time::Duration::from_millis(20));
        let uri2 = persist_cover(&bytes).unwrap();
        assert_eq!(uri, uri2);
        assert_eq!(stat(&original), o1, "二次调用原图不重写（mtime 不变）");
        assert_eq!(stat(&derived), d1, "二次调用派生件不重写");
        drop_isolation();
        drop(dir);
    }

    #[test]
    fn persist_cover_skips_derived_for_small_covers() {
        let (_lock, dir, covers) = isolated_covers();
        persist_cover(&jpeg_bytes(300, 300)).unwrap();
        let names = cover_names(&covers);
        assert!(
            names.iter().all(|n| !n.ends_with(".512.jpg")),
            "≤512 原图不出派生件（header 短路）：{names:?}"
        );
        assert_eq!(names.len(), 1, "只有原图一幅");
        drop_isolation();
        drop(dir);
    }

    #[test]
    fn persist_cover_garbage_bytes_still_ok_without_derived() {
        let (_lock, dir, covers) = isolated_covers();
        let uri = persist_cover(b"not an image at all").unwrap();
        assert!(uri.starts_with("file://"), "垃圾字节原图照写、不 Err");
        let names = cover_names(&covers);
        assert_eq!(
            names.iter().filter(|n| !n.starts_with('.')).count(),
            1,
            "只有原图，无派生件：{names:?}"
        );
        drop_isolation();
        drop(dir);
    }

    #[test]
    fn persist_cover_survives_derived_tmp_leftover() {
        let (_lock, dir, covers) = isolated_covers();
        let bytes = jpeg_bytes(1200, 900);
        // 预置同名 tmp 残留（上次写中途崩溃的形态）：tmp+rename 照常覆盖成功。
        let mut hasher = DefaultHasher::new();
        hasher.write(&bytes);
        let original = format!("{:016x}.jpg", hasher.finish());
        let tmp = covers.join(format!(
            ".{}.512.jpg.tmp",
            original.strip_suffix(".jpg").unwrap()
        ));
        std::fs::write(&tmp, b"stale garbage").unwrap();
        persist_cover(&bytes).unwrap();
        let names = cover_names(&covers);
        assert!(
            names.contains(&derived_name_of(&original)),
            "tmp 残留不阻碍派生件成功：{names:?}"
        );
        assert!(
            !names.iter().any(|n| n.ends_with(".tmp")),
            "tmp 已被 rename 消费"
        );
        drop_isolation();
        drop(dir);
    }
}
