//! 下载、解密、缓存命中与文件 URI 生成。

use std::path::Path;

use hmp_qqmusic_api::algorithms::qmc2::{
    Footer, decrypt_factory, detect_footer, parse_ekey, parse_ekey_decoded,
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};

use super::MediaError;
use crate::cache::{self, extension_from_magic, final_path, tmp_path};

/// 结果类型别名。
type Result<T> = std::result::Result<T, MediaError>;

/// 下载-解密-缓存的主流程（显式缓存根目录，测试用）。
///
/// - `url`：QQ 音乐加密音频流 URL
/// - `ekey`：API 返回的 ekey（`None` 或空串表示明文音质，不下载不解密）
/// - `progress`：可选的下载进度通道（`0.0..=1.0` 或 `None`）
///
/// 返回 `file://` URI（已解密缓存文件）或原 https URL。
pub async fn prepare_playable_at(
    cache_root: &Path,
    url: &str,
    ekey: Option<&str>,
    progress: Option<&tokio::sync::watch::Sender<Option<f64>>>,
) -> Result<String> {
    // ---- 1. 明文音质：直接返回原 url ----
    let ekey = ekey.filter(|e| !e.is_empty());
    let Some(ekey) = ekey else {
        return Ok(url.to_owned());
    };

    // ---- 2. 缓存命中检查 ----
    let key = cache::cache_key(url, ekey);
    let ext_guess = ext_guess_from_url(url);

    let cached = final_path(cache_root, &key, ext_guess);
    if cached.exists() {
        let head = read_first_bytes(&cached, 8)?;
        if !head.is_empty() && extension_from_magic(&head).is_some() {
            return file_uri(&cached);
        }
        let _ = std::fs::remove_file(&cached);
    }

    // ---- 3. 下载 + 尾部检测 ----
    let tmp = tmp_path(cache_root, &key);
    // 清理可能残留的临时文件
    let _ = std::fs::remove_file(&tmp);

    let strip_len = match download_and_detect_strip(url, &tmp, progress).await {
        Ok(sl) => sl,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };

    finish_download(cache_root, &key, ext_guess, &tmp, ekey, strip_len).await
}

/// 下载加密流并尝试使用文件内嵌 ekey（STag/QTag 尾部）解密。
pub async fn prepare_playable_embedded_at(
    cache_root: &Path,
    url: &str,
    progress: Option<&tokio::sync::watch::Sender<Option<f64>>>,
) -> Result<String> {
    let key = cache::cache_key(url, "");
    let ext_guess = ext_guess_from_url(url);
    let cached = final_path(cache_root, &key, ext_guess);
    if cached.exists() {
        let head = read_first_bytes(&cached, 8)?;
        if !head.is_empty() && extension_from_magic(&head).is_some() {
            return file_uri(&cached);
        }
        let _ = std::fs::remove_file(&cached);
    }
    let tmp = tmp_path(cache_root, &key);
    let _ = std::fs::remove_file(&tmp);

    let strip_len = match download_and_detect_strip(url, &tmp, progress).await {
        Ok(Some(strip_len)) => strip_len,
        Ok(None) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(MediaError::Unsupported(
                "文件不含内嵌 ekey 尾部".to_string(),
            ));
        }
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(error);
        }
    };

    let ekey = match embedded_ekey(&tmp, strip_len) {
        Ok(ekey) => ekey,
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(error);
        }
    };
    finish_download(cache_root, &key, ext_guess, &tmp, &ekey, Some(strip_len)).await
}

async fn finish_download(
    cache_root: &Path,
    key: &str,
    ext_guess: &str,
    tmp: &Path,
    ekey: &str,
    strip_len: Option<usize>,
) -> Result<String> {
    let final_base = final_path(cache_root, key, ext_guess);
    let result = decrypt_and_write(tmp, &final_base, ekey, strip_len).await;
    match result {
        Ok(actual_ext) => finish_success(cache_root, key, ext_guess, tmp, final_base, actual_ext),
        Err(DecryptError::MagicMismatch) if strip_len.is_some() => {
            debug!("QMC2 解密后魔数不匹配，重试不剥离尾部");
            let _ = std::fs::remove_file(&final_base);
            match decrypt_and_write(tmp, &final_base, ekey, None).await {
                Ok(actual_ext) => {
                    finish_success(cache_root, key, ext_guess, tmp, final_base, actual_ext)
                }
                Err(error) => finish_error(tmp, &final_base, error),
            }
        }
        Err(error) => finish_error(tmp, &final_base, error),
    }
}

fn finish_success(
    cache_root: &Path,
    key: &str,
    ext_guess: &str,
    tmp: &Path,
    final_base: std::path::PathBuf,
    actual_ext: &str,
) -> Result<String> {
    let _ = std::fs::remove_file(tmp);
    let final_path = if actual_ext != ext_guess {
        let new_path = final_path(cache_root, key, actual_ext);
        std::fs::rename(&final_base, &new_path)?;
        new_path
    } else {
        final_base
    };
    file_uri(&final_path)
}

fn finish_error(tmp: &Path, final_base: &Path, error: DecryptError) -> Result<String> {
    let head_hex = read_first_8_hex(final_base);
    let _ = std::fs::remove_file(tmp);
    let _ = std::fs::remove_file(final_base);
    match error {
        DecryptError::MagicMismatch => Err(MediaError::Unsupported(format!(
            "无法识别音频格式（前 8 字节: {head_hex}）"
        ))),
        error => Err(error.into_media_error()),
    }
}

/// 从尾部字节提取内嵌 ekey（供文件缓存与流式代理共用）。
///
/// - `tail`：从 `audio_len` 到文件末尾的全部字节（即 QTag 元数据区或 V1 密钥区）
/// - `audio_len`：加密音频部分字节数
///
/// `tail` 的 `len() + audio_len` 即原文件总长。
pub(crate) fn embedded_ekey_from_bytes(tail: &[u8], audio_len: usize) -> Result<String> {
    let total_len = audio_len + tail.len();
    let footer = detect_footer(total_len, &tail[tail.len().saturating_sub(0x40)..])
        .ok_or_else(|| MediaError::Unsupported("文件不含内嵌 ekey 尾部".to_string()))?;
    let key_bytes = match footer {
        Footer::QTag { .. } => tail[..tail.len() - 8]
            .split(|byte| *byte == b',')
            .next()
            .unwrap_or_default(),
        Footer::V1 { .. } => &tail[..tail.len() - 4],
    };
    if let Ok(text) = std::str::from_utf8(key_bytes) {
        if parse_ekey(text).is_ok() {
            return Ok(text.to_owned());
        }
    }
    let key = parse_ekey_decoded(key_bytes)
        .map_err(|_| MediaError::Unsupported("内嵌 ekey 无法解析".to_string()))?;
    Ok(hmp_qqmusic_api::algorithms::qmc2::generate_ekey(&key))
}

/// 从文件路径读取尾部并提取内嵌 ekey（委托给 [`embedded_ekey_from_bytes`]）。
fn embedded_ekey(path: &Path, audio_len: usize) -> Result<String> {
    let bytes = std::fs::read(path)?;
    embedded_ekey_from_bytes(&bytes[audio_len..], audio_len)
}

// ---------------------------------------------------------------------------
// 内部辅助
// ---------------------------------------------------------------------------

/// 根据 URL 后缀猜测音频扩展名。
fn ext_guess_from_url(url: &str) -> &'static str {
    // 取最后一个路径段（去除查询参数）
    let path_segment = url.split('?').next().unwrap_or(url);
    let file_name = path_segment.rsplit('/').next().unwrap_or(path_segment);
    let lower = file_name.to_lowercase();
    for (suffix, ext) in [
        (".mflac", "flac"),
        (".mgg", "ogg"),
        (".mmp4", "m4a"),
        (".mnac", "m4a"),
    ] {
        if lower.ends_with(suffix) {
            return ext;
        }
    }
    "bin"
}

/// 将文件路径转换为 `file://` URI。
fn file_uri(path: &Path) -> Result<String> {
    let abs = std::fs::canonicalize(path)?;
    url::Url::from_file_path(&abs)
        .map(|url| url.to_string())
        .map_err(|_| MediaError::Cache(format!("无法生成文件 URI: {}", abs.display())))
}

/// 下载文件并检测尾部，返回 strip_len。
/// 任一步失败均向上传播错误；调用者应负责删除 tmp。
async fn download_and_detect_strip(
    url: &str,
    tmp: &Path,
    progress: Option<&tokio::sync::watch::Sender<Option<f64>>>,
) -> Result<Option<usize>> {
    download_to_file(url, tmp, progress).await?;

    let total_len = std::fs::metadata(tmp)?.len() as usize;
    let tail_size = 0x40.min(total_len);
    let mut tail = vec![0u8; tail_size];
    {
        let file = std::fs::File::open(tmp)?;
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::io::BufReader::new(file);
        file.seek(SeekFrom::End(-(tail_size as i64)))?;
        file.read_exact(&mut tail)?;
    }

    Ok(detect_footer(total_len, &tail).map(|f| match f {
        Footer::QTag { audio_len } | Footer::V1 { audio_len } => audio_len,
    }))
}

/// 下载文件到临时路径，支持进度报告。
async fn download_to_file(
    url: &str,
    tmp: &Path,
    progress: Option<&tokio::sync::watch::Sender<Option<f64>>>,
) -> Result<()> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| MediaError::Network(format!("下载请求失败: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        return Err(MediaError::HttpStatus(status.as_u16()));
    }

    let content_length = response.content_length();

    let mut file = tokio::fs::File::create(tmp).await.map_err(MediaError::Io)?;

    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();

    use futures_util::StreamExt;
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| MediaError::Network(format!("下载流错误: {e}")))?;

        file.write_all(&chunk).await.map_err(MediaError::Io)?;

        downloaded += chunk.len() as u64;

        if let Some(tx) = progress {
            if let Some(total) = content_length {
                let p = downloaded as f64 / total as f64;
                let _ = tx.send(Some(p.clamp(0.0, 1.0)));
            } else {
                let _ = tx.send(None);
            }
        }
    }

    file.flush().await.map_err(MediaError::Io)?;

    // 最终进度 1.0
    if let Some(tx) = progress {
        let _ = tx.send(Some(1.0));
    }

    Ok(())
}

/// 内部解密错误：区分魔数不匹配与其他 IO 错误。
enum DecryptError {
    MagicMismatch,
    Other(MediaError),
}

impl DecryptError {
    fn into_media_error(self) -> MediaError {
        match self {
            DecryptError::MagicMismatch => MediaError::Unsupported("魔数不匹配".to_string()),
            DecryptError::Other(e) => e,
        }
    }
}

impl From<std::io::Error> for DecryptError {
    fn from(e: std::io::Error) -> Self {
        DecryptError::Other(MediaError::Io(e))
    }
}

impl From<MediaError> for DecryptError {
    fn from(e: MediaError) -> Self {
        DecryptError::Other(e)
    }
}

/// 解密临时文件到最终路径。
///
/// 返回解密后的实际扩展名，或 `DecryptError::MagicMismatch`。
async fn decrypt_and_write(
    src: &Path,
    dst: &Path,
    ekey: &str,
    strip_len: Option<usize>,
) -> std::result::Result<&'static str, DecryptError> {
    let cipher = decrypt_factory(ekey).map_err(|e| DecryptError::Other(MediaError::Key(e)))?;

    let total_len = std::fs::metadata(src)?.len() as usize;

    let mut reader = tokio::fs::File::open(src).await?;
    let mut writer = tokio::fs::File::create(dst).await?;

    let mut offset: usize = 0;
    let mut written: usize = 0;
    let mut buf = vec![0u8; 256 * 1024]; // 256 KiB

    use tokio::io::AsyncReadExt;

    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }

        let chunk = &mut buf[..n];
        cipher.decrypt(offset, chunk);

        // 尾部裁剪：若 strip_len 存在且本次写入会超出
        let to_write = if let Some(sl) = strip_len {
            if written + n > sl {
                sl.saturating_sub(written)
            } else {
                n
            }
        } else {
            n
        };

        if to_write > 0 {
            writer.write_all(&chunk[..to_write]).await?;
            written += to_write;
        }

        offset += n;

        // 已达到 strip_len → 停止读取
        if strip_len.is_some_and(|sl| written >= sl) {
            break;
        }
    }

    writer.flush().await?;
    drop(writer);

    // 魔数校验
    let head = read_first_bytes(dst, 8)?;
    match extension_from_magic(&head) {
        Some(ext) => Ok(ext),
        None => {
            warn!(
                "QMC2 解密后魔数无法识别（前 8 字节: {}），total={total_len}, strip_len={strip_len:?}",
                hex_str(&head)
            );
            Err(DecryptError::MagicMismatch)
        }
    }
}

/// 读取文件前 `n` 字节。
fn read_first_bytes(path: &Path, n: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; n];
    let actual = file.read(&mut buf)?;
    buf.truncate(actual);
    Ok(buf)
}

/// 读取文件前 8 字节的十六进制表示（用于错误信息）。
fn read_first_8_hex(path: &Path) -> String {
    match read_first_bytes(path, 8) {
        Ok(b) => hex_str(&b),
        Err(_) => "<无法读取>".to_string(),
    }
}

fn hex_str(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::testutil;
    use hmp_qqmusic_api::algorithms::qmc2::key::generate_ekey;
    use tokio::sync::watch;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_cache_root() -> PathBuf {
        testutil::test_cache_root()
    }

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn prepare_decrypts_plain_stream() {
        let root = test_cache_root().join("decrypts_plain");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let key = b"0123456789abcdefghij"; // 20 bytes → map cipher
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..2048).map(|i| (i % 256) as u8));
            v
        };
        let (encrypted, ekey) = testutil::make_encrypted(&plaintext, key, false);

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(encrypted))
            .mount(&server)
            .await;

        let url = server.uri();

        let result = prepare_playable_at(&root, &url, Some(&ekey), None)
            .await
            .unwrap();

        // 检查返回的是 file:// URI
        assert!(
            result.starts_with("file://"),
            "expected file:// URI, got {result}"
        );

        // 读取文件内容，比对明文
        let path = result.strip_prefix("file://").unwrap();
        let decoded = std::fs::read(path).unwrap();
        assert_eq!(decoded, plaintext, "decrypted content must match plaintext");

        cleanup(&root);
    }

    #[tokio::test]
    async fn prepare_strips_stag_footer() {
        let root = test_cache_root().join("strips_footer");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let key = b"0123456789ABCDEFGHIJ"; // 20 bytes → map cipher
        let plaintext = {
            let mut v = b"OggS".to_vec();
            v.extend((0..2048).map(|i| (i % 256) as u8));
            v
        };
        let (encrypted, ekey) = testutil::make_encrypted(&plaintext, key, true);

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(encrypted))
            .mount(&server)
            .await;

        let result = prepare_playable_at(&root, &server.uri(), Some(&ekey), None)
            .await
            .unwrap();

        assert!(result.starts_with("file://"));
        let path = result.strip_prefix("file://").unwrap();
        let decoded = std::fs::read(path).unwrap();

        // 尾部已被剥离，内容 == 明文
        assert_eq!(decoded, plaintext, "stripped content must match plaintext");

        // 确认文件扩展名正确
        assert!(
            path.ends_with(".ogg"),
            "expected .ogg extension, got {path}"
        );

        cleanup(&root);
    }

    #[tokio::test]
    async fn prepare_returns_url_when_no_ekey() {
        let url = "https://isure.stream.qqmusic.qq.com/something.mflac";
        let result = prepare_playable_at(Path::new("/nonexistent"), url, None, None)
            .await
            .unwrap();
        assert_eq!(result, url);

        // 空 ekey 同效
        let result = prepare_playable_at(Path::new("/nonexistent"), url, Some(""), None)
            .await
            .unwrap();
        assert_eq!(result, url);
    }

    #[tokio::test]
    async fn prepare_embedded_uses_qtag_ekey() {
        let root = test_cache_root().join("embedded_qtag");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let plaintext = b"fLaC embedded qtag";
        let key = b"0123456789abcdefghij";
        let (mut encrypted, ekey) = testutil::make_encrypted(plaintext, key, false);
        let metadata = format!("{ekey},123,2,");
        encrypted.extend_from_slice(metadata.as_bytes());
        encrypted.extend_from_slice(&(metadata.len() as u32).to_be_bytes());
        encrypted.extend_from_slice(b"QTag");
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(encrypted))
            .mount(&server)
            .await;
        let result = prepare_playable_embedded_at(&root, &server.uri(), None)
            .await
            .unwrap();
        let path = url::Url::parse(&result).unwrap().to_file_path().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), plaintext);
        cleanup(&root);
    }

    #[tokio::test]
    async fn prepare_embedded_uses_stag_ekey() {
        let root = test_cache_root().join("embedded_stag");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let plaintext = b"OggS embedded stag";
        let key = b"0123456789abcdefghij";
        let (mut encrypted, ekey) = testutil::make_encrypted(plaintext, key, false);
        encrypted.extend_from_slice(ekey.as_bytes());
        encrypted.extend_from_slice(&(ekey.len() as u32).to_le_bytes());
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(encrypted))
            .mount(&server)
            .await;
        let result = prepare_playable_embedded_at(&root, &server.uri(), None)
            .await
            .unwrap();
        let path = url::Url::parse(&result).unwrap().to_file_path().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), plaintext);
        cleanup(&root);
    }

    #[tokio::test]
    async fn prepare_embedded_fails_without_footer() {
        let root = test_cache_root().join("embedded_no_footer");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"not encrypted"))
            .mount(&server)
            .await;
        assert!(matches!(
            prepare_playable_embedded_at(&root, &server.uri(), None).await,
            Err(MediaError::Unsupported(_))
        ));
        cleanup(&root);
    }

    #[tokio::test]
    async fn prepare_replaces_stale_cache_file() {
        let root = test_cache_root().join("stale_cache");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let plaintext = b"fLaC fresh";
        let (encrypted, ekey) = testutil::make_encrypted(plaintext, b"0123456789abcdefghij", false);
        let server = MockServer::start().await;
        let url = format!("{}/test.mflac", server.uri());
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(encrypted))
            .expect(1)
            .mount(&server)
            .await;
        let key = cache::cache_key(&url, &ekey);
        std::fs::write(final_path(&root, &key, "flac"), []).unwrap();
        let result = prepare_playable_at(&root, &url, Some(&ekey), None)
            .await
            .unwrap();
        let path = url::Url::parse(&result).unwrap().to_file_path().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), plaintext);
        cleanup(&root);
    }

    #[test]
    fn file_uri_percent_encodes_paths() {
        let root = test_cache_root().join("with space").join("sub dir");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("file.flac");
        std::fs::write(&path, b"fLaC").unwrap();
        let uri = file_uri(&path).unwrap();
        assert!(uri.starts_with("file://"));
        assert!(uri.contains("%20"));
        cleanup(root.parent().unwrap());
    }

    #[tokio::test]
    async fn prepare_cache_hit_skips_download() {
        let root = test_cache_root().join("cache_hit");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let key = b"0123456789abcdefghij";
        let plaintext = b"fLaC".to_vec();
        let (encrypted, ekey) = testutil::make_encrypted(&plaintext, key, false);

        let server = MockServer::start().await;
        // 使用含后缀的 URL 以便 ext_guess 能推断为 "flac"
        let url = format!("{}/test.mflac", server.uri());
        let _mock_guard = Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(encrypted.clone()))
            .expect(1) // 仅应被命中一次
            .mount_as_scoped(&server)
            .await;

        // 首次调用：下载并缓存
        let r1 = prepare_playable_at(&root, &url, Some(&ekey), None)
            .await
            .unwrap();
        assert!(r1.starts_with("file://"));

        // 第二次调用：缓存命中（Mock expect(1) 确保不重新请求）
        let r2 = prepare_playable_at(&root, &url, Some(&ekey), None)
            .await
            .unwrap();

        assert_eq!(r1, r2, "cache hit must return same file URI");

        cleanup(&root);
    }

    #[tokio::test]
    async fn prepare_retries_without_strip_on_magic_mismatch() {
        let root = test_cache_root().join("retry_nostrip");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let key = b"0123456789abcdefghij"; // 20 bytes → map cipher

        // 构造 plaintext 使得加密后尾部 4 字节为 [64, 0, 0, 0]（V1 key_size=64），
        // 导致 detect_footer 误判 V1: audio_len = 68 - 4 - 64 = 0。
        // 首次 strip_len=0 → 解密为空 → magic None → 触发无剥离重试 → 成功。
        //
        // 加密公式: encrypted[i] = plaintext[i] XOR cipher_byte(i)
        // 因此 plaintext[i] = desired_encrypted[i] XOR cipher_byte(i)
        let ekey = generate_ekey(key);
        let cipher = decrypt_factory(&ekey).unwrap();

        // 获得位置 64-67 的密钥流
        let mut key_stream = [0u8; 4];
        cipher.decrypt(64, &mut key_stream);

        let desired_tail: [u8; 4] = 64u32.to_le_bytes(); // key_size = 64
        let mut plaintext = vec![0u8; 68];
        plaintext[0..4].copy_from_slice(b"fLaC");
        for i in 0..64 {
            plaintext[4 + i] = (i % 256) as u8;
        }
        // 调整尾部 4 字节使加密后尾部 = desired_tail
        for i in 0..4 {
            plaintext[64 + i] = desired_tail[i] ^ key_stream[i];
        }

        let (encrypted, _ekey) = testutil::make_encrypted(&plaintext, key, false);
        // 验证尾部确实为目标值
        assert_eq!(
            &encrypted[encrypted.len() - 4..],
            &desired_tail,
            "encrypted tail must match desired key_size"
        );

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(encrypted))
            .mount(&server)
            .await;

        let result = prepare_playable_at(&root, &server.uri(), Some(&ekey), None)
            .await
            .unwrap();

        // 重试成功，返回 file:// URI，内容 == 明文
        assert!(result.starts_with("file://"));
        let path = result.strip_prefix("file://").unwrap();
        let decoded = std::fs::read(path).unwrap();
        assert_eq!(
            decoded, plaintext,
            "retry without strip must recover plaintext"
        );

        cleanup(&root);
    }

    #[tokio::test]
    async fn prepare_reports_progress() {
        let root = test_cache_root().join("progress");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let key = b"0123456789abcdefghij";
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..2048).map(|i| (i % 256) as u8));
            v
        };
        let (encrypted, ekey) = testutil::make_encrypted(&plaintext, key, false);

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(encrypted))
            .mount(&server)
            .await;

        let (tx, mut rx) = watch::channel(None);

        let result = prepare_playable_at(&root, &server.uri(), Some(&ekey), Some(&tx))
            .await
            .unwrap();

        assert!(result.starts_with("file://"));

        // 收集所有进度值
        let mut values: Vec<Option<f64>> = Vec::new();
        while let Ok(()) = rx.changed().await {
            let v = *rx.borrow();
            values.push(v);
            if v == Some(1.0) {
                break;
            }
        }

        // 至少收到过 Some(p) 的值
        assert!(
            values.iter().any(|v| v.is_some()),
            "should have received at least one Some(p) progress value"
        );
        // 最终值为 Some(1.0)
        assert_eq!(values.last(), Some(&Some(1.0)), "should end with Some(1.0)");

        cleanup(&root);
    }

    #[tokio::test]
    async fn prepare_cleans_tmp_on_download_failure() {
        let root = test_cache_root().join("clean_tmp");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let url = format!("{}/test.mflac", server.uri());
        let ekey = generate_ekey(b"0123456789abcdefghij");

        let result = prepare_playable_at(&root, &url, Some(&ekey), None).await;

        match result {
            Err(MediaError::HttpStatus(500)) => {}
            other => panic!("expected Err(HttpStatus(500)), got {other:?}"),
        }

        // 检查无残留 .tmp 文件
        let has_tmp = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.path().extension().is_some_and(|ext| ext == "tmp"));
        assert!(
            !has_tmp,
            "no .tmp files should remain after download failure"
        );

        cleanup(&root);
    }
}
