//! 磁盘封面统一缓存（内存治理工作③）：字节加权 LRU + 装载即降采样。
//!
//! 此前三处各自为政的 `HashMap<String, slint::Image>` 线程本地缓存
//! （library_view::local_cover_image / player_bridge::load_cover_cached /
//! online_covers::load_image）只进不出：全量解码位图（一张 3000×3000 专辑封面
//! 解码后 ~36MB）被应用层强引用钉住，Slint 内部 5MB 纹理 LRU 被废掉，RSS 随
//! 听过的封面数无限增长。合并为唯一入口 [`get_or_load`]：
//!
//! - **装载即降采样**：`image` crate 解码 → `thumbnail(max_side, max_side)` →
//!   rgba8 → `Image::from_rgba8`。缓存的与交给 UI 的都是降采样后的小图；
//!   展示位最大 336px 逻辑像素（播放页大图），512 桶余量充足；
//! - **字节加权 LRU**：key = `(路径, max_side)`，权重 = 解码后缓冲字节数，
//!   总量上限 [`CAP_BYTES`]（64MB），超限逐出最久未用条目——平滑自愈，
//!   不再依赖调用方清理；
//! - **桶约定**（MEMFIX-CONTRACT 第 3 条）：行/卡/侧栏/详情头图 = 256；
//!   当前曲播放页大图 = 512。
//!
//! slint::Image 非 Send/Sync（covers.rs 同款约束）：缓存放 thread_local，
//! 装载与 UI 消费同在主线程。600×600 → 256 桶降采样后 256×256×4 = 256KB/条，
//! 64MB ≈ 256 条；全库封面重放也不会超过上限。

use std::cell::RefCell;
use std::collections::HashMap;

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// 缓存总量上限（解码降采样后字节）。256 桶 256KB/条、512 桶 1MB/条，
/// 上限 ≈ 数百条封面，超出按最久未用逐出。
pub const CAP_BYTES: u64 = 64 * 1024 * 1024;

struct Entry {
    image: Image,
    /// 权重 = 降采样后 rgba8 缓冲字节数（width × height × 4）。
    bytes: u64,
    /// 最近使用票号（单调递增；逐出时取最小者 = 最久未用）。
    stamp: u64,
}

#[derive(Default)]
struct Cache {
    map: HashMap<(String, u32), Entry>,
    total_bytes: u64,
    next_stamp: u64,
    #[cfg(test)]
    hits: u64,
    #[cfg(test)]
    misses: u64,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

/// 磁盘图片按路径装载（带降采样）并缓存；不存在/解码失败返回 None。
///
/// `max_side` 为降采样桶：返回图最长边 ≤ max_side（`thumbnail` 保比缩放）。
pub fn get_or_load(path: &str, max_side: u32) -> Option<Image> {
    get_or_load_inner(path, max_side, CAP_BYTES)
}

fn get_or_load_inner(path: &str, max_side: u32, cap: u64) -> Option<Image> {
    CACHE.with(|cell| {
        // 先走出 RefMut 的 Deref：之后 cache.map / cache.next_stamp 等字段
        // 借用才彼此不相交（否则每次字段访问都整体重借 *cache）。
        let cache: &mut Cache = &mut cell.borrow_mut();
        let key = (path.to_owned(), max_side);
        if let Some(entry) = cache.map.get_mut(&key) {
            #[cfg(test)]
            {
                cache.hits += 1;
            }
            cache.next_stamp += 1;
            entry.stamp = cache.next_stamp;
            return Some(entry.image.clone());
        }
        #[cfg(test)]
        {
            cache.misses += 1;
        }
        let image = decode_downsampled(path, max_side)?;
        let bytes = u64::from(image.size().width) * u64::from(image.size().height) * 4;
        // 单条超上限（理论不可达：桶内最长边 ≤ max_side ≤ 512 → ≤1MB/条）
        // 防御性不缓存，只返回本次结果。
        if bytes <= cap {
            evict_until_fits(cache, bytes, cap);
            cache.next_stamp += 1;
            let stamp = cache.next_stamp;
            cache.total_bytes += bytes;
            cache.map.insert(
                key,
                Entry {
                    image: image.clone(),
                    bytes,
                    stamp,
                },
            );
        }
        Some(image)
    })
}

/// 超限逐出最久未用，直到腾出 `incoming` 字节的余量。
fn evict_until_fits(cache: &mut Cache, incoming: u64, cap: u64) {
    while cache.total_bytes + incoming > cap {
        let Some(victim) = cache
            .map
            .iter()
            .min_by_key(|(_, entry)| entry.stamp)
            .map(|(key, _)| key.clone())
        else {
            return; // 已空仍放不下（调用方已拦单条超限）→ 不可能到达
        };
        if let Some(entry) = cache.map.remove(&victim) {
            cache.total_bytes = cache.total_bytes.saturating_sub(entry.bytes);
        }
    }
}

/// 解码 + 保比降采样（最长边钳到 max_side）→ Slint rgba8 位图。
fn decode_downsampled(path: &str, max_side: u32) -> Option<Image> {
    let decoded = image::ImageReader::open(path).ok()?.decode().ok()?;
    // max_side 为 0 时 thumbnail 退化为原尺寸钳 1px，防御取 max(1)。
    let thumb = decoded.thumbnail(max_side.max(1), max_side.max(1));
    let rgba = thumb.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(rgba.as_raw());
    Some(Image::from_rgba8(buffer))
}

// ——— 测试观测（仅测试构建）：条目数/总字节/命中计数 ———
#[cfg(test)]
fn cache_stats() -> (usize, u64, u64, u64) {
    CACHE.with(|cell| {
        let cache = cell.borrow();
        (cache.map.len(), cache.total_bytes, cache.hits, cache.misses)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// 测试图目录（storage 同款：temp_dir + 进程号，不引 tempfile dev-dep）。
    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hmp-cc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 落盘一张 w×h 渐变 PNG（jpeg/png 特性即可覆盖封面产物两种格式）。
    fn write_png(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 0x40, 0xFF])
        });
        let path = dir.join(name);
        img.save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        path
    }

    #[test]
    fn loads_and_downsamples_to_bucket() {
        let dir = test_dir("downsample");
        let path = write_png(&dir, "big.png", 1200, 800);
        let image = get_or_load(path.to_str().unwrap(), 256).expect("decode ok");
        let size = image.size();
        assert!(
            size.width <= 256 && size.height <= 256,
            "最长边应钳到 256 桶，实际 {size:?}"
        );
        let ratio = f64::from(size.width) / f64::from(size.height);
        assert!(
            (ratio - 1.5).abs() < 0.05,
            "保比缩放应保持 3:2，实际 {size:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn second_call_hits_cache() {
        let dir = test_dir("hit");
        let path = write_png(&dir, "hit.png", 64, 64);
        let p = path.to_str().unwrap();
        assert!(get_or_load(p, 256).is_some());
        assert!(get_or_load(p, 256).is_some());
        let (len, _bytes, hits, misses) = cache_stats();
        assert_eq!((len, hits, misses), (1, 1, 1), "第二次调用应命中缓存");
        // 不同桶 = 不同 key：同一路径各桶独立
        assert!(get_or_load(p, 512).is_some());
        let (len, _bytes, _hits, _misses) = cache_stats();
        assert_eq!(len, 2, "(路径, 桶) 二元组为 key");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_returns_none() {
        let dir = test_dir("missing");
        let p = dir.join("nope.png").to_str().unwrap().to_owned();
        assert!(get_or_load(&p, 256).is_none());
        assert!(get_or_load("", 256).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn evicts_least_recently_used_by_weight() {
        let dir = test_dir("evict");
        // 256×256×4 = 256KB/条；cap 600KB → 第三条挤掉第一条（最久未用）
        let cap: u64 = 600 * 1024;
        let a = write_png(&dir, "a.png", 256, 256);
        let b = write_png(&dir, "b.png", 256, 256);
        let c = write_png(&dir, "c.png", 256, 256);
        let (pa, pb, pc) = (
            a.to_str().unwrap().to_owned(),
            b.to_str().unwrap().to_owned(),
            c.to_str().unwrap().to_owned(),
        );
        assert!(get_or_load_inner(&pa, 256, cap).is_some());
        assert!(get_or_load_inner(&pb, 256, cap).is_some());
        // 触碰 a：LRU 序变 b(最旧) → a → c
        assert!(get_or_load_inner(&pa, 256, cap).is_some());
        assert!(get_or_load_inner(&pc, 256, cap).is_some());
        let (len, bytes, _hits, _misses) = cache_stats();
        assert_eq!(len, 2, "cap 600KB 只容 2 条 256KB");
        assert!(bytes <= cap, "总量 {bytes} 应 ≤ cap {cap}");
        // a 被触碰过仍在，b（最久未用）应被逐出 → 命中 a、未命中 b
        let before = cache_stats();
        assert!(get_or_load_inner(&pa, 256, cap).is_some());
        let (len2, _b2, hits2, _m2) = cache_stats();
        assert_eq!(hits2, before.2 + 1, "a 应仍在缓存（被触碰过）");
        assert_eq!(len2, len, "总量不变");
        let miss_before = cache_stats().3;
        assert!(get_or_load_inner(&pb, 256, cap).is_some());
        assert_eq!(cache_stats().3, miss_before + 1, "b 应已被逐出（重新装载）");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn public_cap_constant_matches_contract() {
        assert_eq!(CAP_BYTES, 64 * 1024 * 1024);
    }
}
