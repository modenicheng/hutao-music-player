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
    use std::sync::Arc;
    use std::time::Duration;

    use hmp_core::{
        AudioQuality, LoadRequest, MediaStream, MediaStreamSource, PlaybackStatus, Track, TrackId,
    };

    use crate::{PlayerCore, source::file_path};

    /// 生成静音 WAV 字节（8kHz 单声道 16bit）。
    fn silent_wav_bytes(duration_ms: u32) -> Vec<u8> {
        let sample_rate = 8_000u32;
        let data_len = sample_rate * duration_ms / 1_000 * 2;
        let mut wav = std::io::Cursor::new(Vec::new());
        wav.write_all(b"RIFF").unwrap();
        wav.write_all(&(36 + data_len).to_le_bytes()).unwrap();
        wav.write_all(b"WAVEfmt ").unwrap();
        wav.write_all(&16u32.to_le_bytes()).unwrap();
        wav.write_all(&1u16.to_le_bytes()).unwrap();
        wav.write_all(&1u16.to_le_bytes()).unwrap();
        wav.write_all(&sample_rate.to_le_bytes()).unwrap();
        wav.write_all(&(sample_rate * 2).to_le_bytes()).unwrap();
        wav.write_all(&2u16.to_le_bytes()).unwrap();
        wav.write_all(&16u16.to_le_bytes()).unwrap();
        wav.write_all(b"data").unwrap();
        wav.write_all(&data_len.to_le_bytes()).unwrap();
        wav.write_all(&vec![0; data_len as usize]).unwrap();
        wav.into_inner()
    }

    fn write_silent_wav(path: &std::path::Path, duration_ms: u32) {
        std::fs::write(path, silent_wav_bytes(duration_ms)).unwrap();
    }

    /// 进程内内存源（[`MediaStreamSource`] 测试替身）：open 返回全量字节的
    /// `Cursor`（`Read + Seek + Send`，经 blanket impl 自动满足
    /// [`MediaStream`]）——与生产解密源同一接缝。
    struct MemSource(Arc<Vec<u8>>);

    impl std::fmt::Debug for MemSource {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            // 只打长度，不打印字节本体（生产源同理只打元信息）。
            f.debug_struct("MemSource")
                .field("len", &self.0.len())
                .finish()
        }
    }

    impl MediaStreamSource for MemSource {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }

        fn open(&self) -> std::io::Result<Box<dyn MediaStream>> {
            Ok(Box::new(std::io::Cursor::new((*self.0).clone())))
        }
    }

    /// 黑洞源（[`MediaStreamSource`] 测试替身，替代旧测试的黑洞 HTTP 服务器）：
    /// open 立即成功，但 reader 的 Read/Seek 永久阻塞——模拟后台预取永远
    /// 饥饿的远端源。
    ///
    /// 无人向 channel 发送：`recv()` 阻塞到 [`HungSource`] 被 drop（装载被
    /// 取代/Stop/Shutdown abort 后 request 释放最后一个 Arc）——senders
    /// 随之全部释放，`recv` 返回 Err、阻塞在探测上的 `spawn_blocking` 线程
    /// 以错误结束并归还线程池（tokio runtime drop 会等阻塞任务到天荒地老，
    /// 读必须随源 drop 一起终止）。
    #[derive(Debug, Default)]
    struct HungSource {
        releases: std::sync::Mutex<Vec<std::sync::mpsc::Sender<()>>>,
    }

    impl MediaStreamSource for HungSource {
        fn len(&self) -> u64 {
            1
        }

        fn open(&self) -> std::io::Result<Box<dyn MediaStream>> {
            let (release, blocked) = std::sync::mpsc::channel::<()>();
            self.releases.lock().unwrap().push(release);
            Ok(Box::new(HungReader(blocked)))
        }
    }

    /// Read/Seek 永久阻塞的 reader（阻塞在 recv 上，见 [`HungSource`]）。
    struct HungReader(std::sync::mpsc::Receiver<()>);

    impl std::io::Read for HungReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            // recv 只会因 senders 全部释放而返回 Err（无人发送）。
            let _ = self.0.recv();
            Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "hung source released",
            ))
        }
    }

    impl std::io::Seek for HungReader {
        fn seek(&mut self, _pos: std::io::SeekFrom) -> std::io::Result<u64> {
            let _ = self.0.recv();
            Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "hung source released",
            ))
        }
    }

    /// 门控源（[`MediaStreamSource`] 测试替身）：reader 的首个 `read` 阻塞
    /// 直到测试放行——精确拉长「装载在途」窗口，构造装载期间发命令的
    /// 确定性时序（[`HungSource`] 的可释放变体；阻塞发生在 spawn_blocking
    /// 的格式探测上，不占 runtime worker，drive 循环照常处理命令）。
    struct GatedSource {
        wav: Arc<Vec<u8>>,
        /// 放行端由测试持有；reader 在 `recv` 上等第一次 send（单次 open）。
        release: std::sync::Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    }

    impl std::fmt::Debug for GatedSource {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            // 只打长度，不打印字节本体（生产源同理只打元信息）。
            f.debug_struct("GatedSource")
                .field("len", &self.wav.len())
                .finish()
        }
    }

    impl MediaStreamSource for GatedSource {
        fn len(&self) -> u64 {
            self.wav.len() as u64
        }

        fn open(&self) -> std::io::Result<Box<dyn MediaStream>> {
            let release = self
                .release
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| std::io::Error::other("gated source already opened"))?;
            Ok(Box::new(GatedReader {
                wav: Arc::clone(&self.wav),
                release,
                released: false,
                pos: 0,
            }))
        }
    }

    /// 首次 `read` 阻塞等待放行的 reader；放行后服务 WAV 字节。
    struct GatedReader {
        wav: Arc<Vec<u8>>,
        release: std::sync::mpsc::Receiver<()>,
        released: bool,
        pos: usize,
    }

    impl std::io::Read for GatedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.released {
                match self.release.recv() {
                    Ok(()) => self.released = true,
                    // 放行端已丢弃（测试异常退出）→ 以 EOF 结束，不悬挂线程池。
                    Err(_) => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "gated source released",
                        ));
                    }
                }
            }
            let n = (self.wav.len() - self.pos).min(buf.len());
            buf[..n].copy_from_slice(&self.wav[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    impl std::io::Seek for GatedReader {
        fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
            let len = self.wav.len() as i64;
            let next = match pos {
                std::io::SeekFrom::Start(n) => n as i64,
                std::io::SeekFrom::End(n) => len + n,
                std::io::SeekFrom::Current(n) => self.pos as i64 + n,
            }
            .clamp(0, len);
            self.pos = next as usize;
            Ok(next as u64)
        }
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
            stream: None,
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

    /// 回归（2026-09-29 QQ 远端音频事故·驱动僵尸）：挂起的取流装载不得阻塞
    /// 命令循环——装载在独立任务执行，后续装载能立即取代它并生效。
    ///
    /// HungSource 的 Read 永久阻塞（模拟 CDN/后台预取挂起）；第二个本地 WAV
    /// 装载必须在短时间内应用（旧实现中它会排在挂起 future 之后永不执行）。
    #[tokio::test]
    async fn superseded_hung_load_does_not_block_next_load() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("next.wav");
        write_silent_wav(&wav_path, 120);

        let core = PlayerCore::new_silent_for_test();
        let mut state = core.subscribe_state();

        // gen 1：挂起的流装载（open 成功、探测 Read 永久阻塞）；
        // uri 只是元数据，播放走 stream。
        core.load(LoadRequest {
            track: Track::new(TrackId::new("hung-stream"), "Hung"),
            uri: "https://isure.stream.qqmusic.qq.com/hung.wav".into(),
            quality: AudioQuality::Mp3_128,
            load_gen: 1,
            stream: Some(Arc::new(HungSource::default())),
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
            stream: None,
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
        let core = PlayerCore::new_silent_for_test();
        let mut state = core.subscribe_state();
        core.load(LoadRequest {
            track: Track::new(TrackId::new("hung-stream"), "Hung"),
            uri: "https://isure.stream.qqmusic.qq.com/hung.wav".into(),
            quality: AudioQuality::Mp3_128,
            load_gen: 1,
            stream: Some(Arc::new(HungSource::default())),
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
        // 若装载未被取消，黑洞源永不产出本测试也不会等到完成——这里只能验证
        // Stop 生效且短窗内无复活；「Stop 后完成」的真复活路径由取代测试
        // 的取消语义（abort + drop 接收端）覆盖。
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(state.borrow().status, PlaybackStatus::Stopped);
        core.shutdown();
    }

    /// 回归（2026-09-29 QQ 远端音频事故·LIFO 死锁）：decoder 格式探测的
    /// 阻塞读必须落在 `spawn_blocking` 上——若内联在 runtime worker 上，
    /// worker 被读挂起，而源的后台预取任务落在同 worker 的 LIFO 槽（不可被
    /// 其他 worker 偷取）→ 预取任务永不调度 → 探测永远等不到字节 → 装载
    /// 死锁。旧 stream-download 实现在本测试形态下确定性死锁（超时）；现以
    /// 进程内源（内存 WAV 经 MediaStreamSource 直连 decoder）保持同一接缝，
    /// 流装载必须到达 Playing。
    #[tokio::test]
    async fn stream_source_load_reaches_playing() {
        let wav = Arc::new(silent_wav_bytes(200));

        let core = PlayerCore::new_silent_for_test();
        core.load(LoadRequest {
            track: Track::new(TrackId::new("stream-test"), "Stream"),
            uri: "https://isure.stream.qqmusic.qq.com/sample.wav".into(),
            quality: AudioQuality::Mp3_128,
            load_gen: 3,
            stream: Some(Arc::new(MemSource(wav))),
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
        .expect("in-process stream must reach Playing (probe-on-spawn_blocking regression)");
        assert_eq!(state.borrow().load_gen, 3);
        core.shutdown();
    }

    /// 回归（F2 Bug 2）：装载完成不得覆盖装载期暂停——completion 分支硬编码
    /// `sink.play()` 会让「装载期间按的暂停」过一会儿自己响（曲尾自动切歌
    /// 场景必现）。门控源精确构造时序：装载在途 → Pause（首装载无 current，
    /// Loading 态也必须记录暂停意图）→ 放行装载完成 → 必须停在 Paused 且
    /// 不自动开播；随后的 Play 命令正常恢复（解码器已 append、sink 处于
    /// paused）。
    #[tokio::test]
    async fn pause_during_load_survives_completion() {
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let core = PlayerCore::new_silent_for_test();
        let mut state = core.subscribe_state();
        core.load(LoadRequest {
            track: Track::new(TrackId::new("gated"), "Gated"),
            uri: "https://example.com/gated.wav".into(),
            quality: AudioQuality::Mp3_128,
            load_gen: 1,
            stream: Some(Arc::new(GatedSource {
                wav: Arc::new(silent_wav_bytes(400)),
                release: std::sync::Mutex::new(Some(release_rx)),
            })),
        });
        // 进入 Loading（装载阻塞在门上，completion 未到）。
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if state.borrow().status == PlaybackStatus::Loading {
                    break;
                }
                state.changed().await.unwrap();
            }
        })
        .await
        .expect("gated load should enter Loading");
        // 装载在途发暂停（首装载：current 尚为 None）。
        core.pause();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if state.borrow().status == PlaybackStatus::Paused {
                    break;
                }
                state.changed().await.unwrap();
            }
        })
        .await
        .expect("pause during load must record Paused (no current yet)");
        // 放行 → 装载完成：不得自动开播（暂停意图不丢）。
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let s = state.borrow();
                if s.load_gen == 1 && s.current.is_some() {
                    assert_eq!(
                        s.status,
                        PlaybackStatus::Paused,
                        "装载完成不得覆盖装载期暂停（F2 Bug 2）"
                    );
                    return;
                }
                drop(s);
                state.changed().await.unwrap();
            }
        })
        .await
        .expect("gated load should complete");
        // Play 命令正常恢复播放。
        core.play();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if state.borrow().status == PlaybackStatus::Playing {
                    break;
                }
                state.changed().await.unwrap();
            }
        })
        .await
        .expect("Play after load-time pause must resume playback");
        core.shutdown();
    }

    /// 对照（F2 Bug 2）：装载期间未暂停 → completion 正常 Playing（既有语义
    /// 不回归）。同一门控源，放行后完成。
    #[tokio::test]
    async fn load_without_pause_completes_to_playing() {
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let core = PlayerCore::new_silent_for_test();
        let mut state = core.subscribe_state();
        core.load(LoadRequest {
            track: Track::new(TrackId::new("gated-control"), "Gated"),
            uri: "https://example.com/gated.wav".into(),
            quality: AudioQuality::Mp3_128,
            load_gen: 5,
            stream: Some(Arc::new(GatedSource {
                wav: Arc::new(silent_wav_bytes(400)),
                release: std::sync::Mutex::new(Some(release_rx)),
            })),
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
        .expect("gated load should enter Loading");
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if state.borrow().status == PlaybackStatus::Playing {
                    break;
                }
                state.changed().await.unwrap();
            }
        })
        .await
        .expect("load without pause must reach Playing");
        assert_eq!(state.borrow().load_gen, 5);
        core.shutdown();
    }
}
