//! HMP cross-platform audio player.
//!
//! The crate owns URI preparation, decoding, device output, transport commands and
//! playback-state publication. Queue and source-resolution policy stay in the daemon.

mod core;
pub mod source;

pub use core::PlayerCore;

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
}
