//! HMP cross-platform audio player.
//!
//! The crate owns URI preparation, decoding, device output, transport commands and
//! playback-state publication. Queue and source-resolution policy stay in the daemon.

mod core;
pub mod source;

pub use core::PlayerCore;
pub use core::open_default_output;

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::Duration;

    use hmp_core::{AudioQuality, LoadRequest, PlaybackStatus, Track, TrackId};

    use crate::{PlayerCore, source::file_path};

    fn write_silent_wav(path: &std::path::Path, duration_ms: u32) {
        let sample_rate = 8_000u32;
        let data_len = sample_rate * duration_ms / 1_000 * 2;
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + data_len).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&sample_rate.to_le_bytes()).unwrap();
        file.write_all(&(sample_rate * 2).to_le_bytes()).unwrap();
        file.write_all(&2u16.to_le_bytes()).unwrap();
        file.write_all(&16u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data_len.to_le_bytes()).unwrap();
        file.write_all(&vec![0; data_len as usize]).unwrap();
    }

    #[test]
    fn file_uri_roundtrips_windows_paths() {
        let path = std::env::temp_dir().join("hmp player test.wav");
        let uri = url::Url::from_file_path(&path).unwrap();
        assert_eq!(file_path(&uri).unwrap(), path);
    }

    #[tokio::test]
    async fn generated_wav_loads_seeks_and_ends_without_device() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.wav");
        write_silent_wav(&path, 120);
        let uri = url::Url::from_file_path(&path).unwrap().to_string();
        let core = PlayerCore::new_silent_for_test();
        core.load(LoadRequest {
            track: Track::new(TrackId::new("local-test"), "Silent"),
            uri,
            quality: AudioQuality::Flac,
            load_gen: 7,
        });

        let mut state = core.subscribe_state();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if matches!(
                    state.borrow().status,
                    PlaybackStatus::Playing | PlaybackStatus::Ended
                ) {
                    break;
                }
                state.changed().await.unwrap();
            }
        })
        .await
        .expect("WAV should load");
        assert_eq!(state.borrow().load_gen, 7);

        core.seek(Duration::from_millis(20));
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if state.borrow().status == PlaybackStatus::Ended {
                    break;
                }
                state.changed().await.unwrap();
            }
        })
        .await
        .expect("WAV should end");
        core.shutdown();
    }

    /// 回归（2026-09-29 QQ 远端音频事故·驱动僵尸）：挂起的网络装载不得阻塞
    /// 命令循环——装载在独立任务执行，后续装载能立即取代它并生效。
    ///
    /// 黑洞服务器（接受连接但不响应）模拟 CDN/代理挂起；第二个本地 WAV
    /// 装载必须在短时间内应用（旧实现中它会排在挂起 future 之后永不执行）。
    #[tokio::test]
    async fn superseded_hung_load_does_not_block_next_load() {
        // 黑洞 HTTP 服务器：accept 后保持沉默
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let blackhole_addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let _ = stream; // 收下连接，永不响应
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("next.wav");
        write_silent_wav(&wav_path, 120);

        let core = PlayerCore::new_silent_for_test();
        let mut state = core.subscribe_state();

        // gen 1：挂起的网络装载（回环地址 → no_proxy 客户端 → 黑洞无响应头）
        core.load(LoadRequest {
            track: Track::new(TrackId::new("hung-stream"), "Hung"),
            uri: format!("http://{blackhole_addr}/stream"),
            quality: AudioQuality::Mp3_128,
            load_gen: 1,
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if state.borrow().status == PlaybackStatus::Loading {
                    break;
                }
                state.changed().await.unwrap();
            }
        })
        .await
        .expect("hung load should enter Loading");

        // gen 2：立即换曲——挂起装载被取代，新装载必须应用
        core.load(LoadRequest {
            track: Track::new(TrackId::new("local-next"), "Next"),
            uri: url::Url::from_file_path(&wav_path).unwrap().to_string(),
            quality: AudioQuality::Flac,
            load_gen: 2,
        });
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let cur = state.borrow();
                if cur.load_gen == 2 && cur.status == PlaybackStatus::Playing {
                    break;
                }
                drop(cur);
                state.changed().await.unwrap();
            }
        })
        .await
        .expect("subsequent load must apply while first load hangs (drive loop responsive)");
        assert_eq!(
            state.borrow().current.as_ref().map(|t| t.id.as_ref()),
            Some("local-next")
        );
        core.shutdown();
    }

    /// 回归（同上事故·复活防护）：Stop 取消未完成装载——晚到的装载完成
    /// 不得在 Stop 之后把状态「复活」为 Playing。
    #[tokio::test]
    async fn stop_cancels_inflight_load_no_late_resurrection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let blackhole_addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let _ = stream;
            }
        });

        let core = PlayerCore::new_silent_for_test();
        let mut state = core.subscribe_state();
        core.load(LoadRequest {
            track: Track::new(TrackId::new("hung-stream"), "Hung"),
            uri: format!("http://{blackhole_addr}/stream"),
            quality: AudioQuality::Mp3_128,
            load_gen: 1,
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if state.borrow().status == PlaybackStatus::Loading {
                    break;
                }
                state.changed().await.unwrap();
            }
        })
        .await
        .expect("hung load should enter Loading");

        core.stop();
        // 若装载未被取消，黑洞永不响应本测试也不会等到完成——这里只能验证
        // Stop 生效且短窗内无复活；「Stop 后完成」的真复活路径由取代测试
        // 的取消语义（abort + drop 接收端）覆盖。
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(state.borrow().status, PlaybackStatus::Stopped);
        core.shutdown();
    }

    /// 回归（2026-09-29 QQ 远端音频事故·LIFO 死锁）：decoder 格式探测的
    /// 阻塞读若内联在 runtime worker 上，worker 被读挂起，而 stream_download
    /// 的下载任务落在同 worker 的 LIFO 槽（不可偷取）→ 下载任务永不调度 →
    /// 探测永远等不到字节 → 装载死锁（修复：探测在 `spawn_blocking` 上）。
    /// 本地 HTTP 服务真流式供 WAV——旧代码在本测试下确定性死锁（超时）。
    #[tokio::test]
    async fn http_stream_load_reaches_playing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("http.wav");
        write_silent_wav(&path, 200);
        let body = std::fs::read(&path).unwrap();

        // 极简 HTTP/1.1 服务：忽略 Range，一律 200 + 全量 body
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let body = body.clone();
                std::thread::spawn(move || {
                    use std::io::{BufRead, BufReader, Write};
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request_line = String::new();
                    while reader.read_line(&mut request_line).unwrap_or(0) > 0 {
                        if request_line.trim().is_empty() {
                            break;
                        }
                        request_line.clear();
                    }
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: audio/wav\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(&body);
                    let _ = stream.flush();
                });
            }
        });

        let core = PlayerCore::new_silent_for_test();
        core.load(LoadRequest {
            track: Track::new(TrackId::new("http-test"), "Http"),
            uri: format!("http://{addr}/sample.wav"),
            quality: AudioQuality::Mp3_128,
            load_gen: 3,
        });
        let mut state = core.subscribe_state();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if state.borrow().status == PlaybackStatus::Playing {
                    break;
                }
                state.changed().await.unwrap();
            }
        })
        .await
        .expect("HTTP stream must reach Playing (LIFO deadlock regression)");
        assert_eq!(state.borrow().load_gen, 3);
        core.shutdown();
    }

    /// 回环主机判定：no_proxy 客户端选择依据。
    #[test]
    fn loopback_host_classification() {
        assert!(crate::core::is_loopback_host("127.0.0.1"));
        assert!(crate::core::is_loopback_host("::1"));
        assert!(crate::core::is_loopback_host("localhost"));
        assert!(!crate::core::is_loopback_host("isure.stream.qqmusic.qq.com"));
        assert!(!crate::core::is_loopback_host("192.168.1.10"));
        assert!(!crate::core::is_loopback_host(""));
    }
}
