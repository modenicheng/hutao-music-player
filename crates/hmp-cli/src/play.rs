//! `hmp play`：按音质回退链取流并播放。
//!
//! 流程：加载凭证 → 歌曲详情（拿 media_mid）→ 按 `AudioQuality` 回退链
//! 依次取流，首个成功（`result == 0` 且有 `purl`）的音质用于播放
//! （docs/PROJECT.md §7.3 质量回退）。

use std::time::Duration;

use hmp_core::{
    AlbumId, AlbumRef, ArtistId, ArtistRef, AudioQuality, CoverRef, HmpError, PlaybackState,
    PlaybackStatus, Track, TrackId,
};
use hmp_player_gst::{LoadRequest, PlayerCore};
use hmp_qqmusic_api::{
    QqMusicClient, SongFileType,
    song::{SongApi, SongFileInfo},
};
use hmp_storage::credential::store_from_env;
use tokio::sync::watch;

/// `AudioQuality` → 取流文件类型映射（可用的子集）。
fn quality_to_file_type(q: AudioQuality) -> Option<SongFileType> {
    match q {
        AudioQuality::Master => Some(SongFileType::MASTER),
        AudioQuality::HiRes => Some(SongFileType::MASTER),
        AudioQuality::Atmos => Some(SongFileType::ATMOS_2),
        AudioQuality::Flac => Some(SongFileType::FLAC),
        AudioQuality::Aac => Some(SongFileType::AAC_192),
        AudioQuality::Mp3_320 => Some(SongFileType::MP3_320),
        AudioQuality::Mp3_128 => Some(SongFileType::MP3_128),
        AudioQuality::Unknown(_) => None,
    }
}

/// 播放主流程。
pub async fn run(track_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let store = store_from_env();
    let credential = store.load()?.ok_or("未登录，请先运行 `hmp login`")?;
    if !credential.is_logged_in() {
        return Err("凭证无效或已过期，请重新运行 `hmp login`".into());
    }

    let client = QqMusicClient::new();
    let song_api = SongApi::new(&client);

    // 歌曲详情（媒体 mid）
    let detail = song_api.get_detail(track_id).await?;
    let media_mid = detail.track.file.media_mid.clone();
    if media_mid.is_empty() {
        return Err("歌曲缺少媒体文件信息，无法播放".into());
    }
    let title = detail.track.name.clone();
    let singers = detail
        .track
        .singer
        .iter()
        .map(|s| s.name.clone())
        .collect::<Vec<_>>()
        .join(" / ");
    println!("播放: {title} - {singers}");

    // 音质回退：优先 VIP 无损，逐步降级
    let file_info = SongFileInfo {
        mid: track_id.to_owned(),
        file_type: None,
        song_type: 0,
        media_mid: Some(media_mid),
    };
    let mut chosen: Option<(SongFileType, String)> = None;
    let mut last_error: Option<String> = None;

    'quality: for quality in AudioQuality::Master.fallback_chain() {
        let Some(file_type) = quality_to_file_type(quality.clone()) else {
            continue;
        };
        // 加密格式（.mflac/.mgg/.mmp4 等）需客户端解密，当前播放器不支持，
        // 视为不可用继续回退（docs/PROJECT.md §7.3 质量回退）。
        if file_type.is_encrypted {
            tracing::info!(quality = ?quality, "跳过加密音质（暂不支持解密）");
            continue;
        }
        let urls = song_api
            .get_song_urls(
                std::slice::from_ref(&file_info),
                file_type,
                Some(&credential),
            )
            .await;
        match urls {
            Ok(resp) => {
                for item in &resp.data {
                    if item.result == 0 && !item.purl.is_empty() {
                        chosen = Some((file_type, item.purl.clone()));
                        println!("音质: {quality:?} ({}{})", file_type.s, file_type.e);
                        break 'quality;
                    }
                    last_error = Some(format!("result={}", item.result));
                }
            }
            Err(e) => {
                last_error = Some(e.to_string());
            }
        }
    }

    let (file_type, purl) = chosen.ok_or_else(|| {
        HmpError::QualityUnavailable.to_string().replace(
            "quality is unavailable",
            &format!("所有音质均不可用 (最后错误: {:?})", last_error),
        )
    })?;
    let uri = format!("https://isure.stream.qqmusic.qq.com/{purl}",);

    // 组装领域曲目（详情含多歌手/专辑/封面，供 MPRIS metadata 使用）
    let singers = detail
        .track
        .singer
        .iter()
        .filter(|s| !s.name.is_empty())
        .map(|s| ArtistRef {
            id: ArtistId::new(if s.mid.is_empty() {
                s.id.to_string()
            } else {
                s.mid.clone()
            }),
            name: s.name.clone(),
        })
        .collect::<Vec<_>>();
    let album = (!detail.track.album.name.is_empty()).then(|| AlbumRef {
        id: AlbumId::new(detail.track.album.mid.clone()),
        name: detail.track.album.name.clone(),
    });
    let cover = (!detail.track.album.pmid.is_empty()).then(|| CoverRef {
        url: format!(
            "https://y.gtimg.cn/music/photo_new/T002R300x300M000{}.jpg",
            detail.track.album.pmid
        ),
    });
    let track = Track {
        id: TrackId::new(track_id),
        title,
        artists: singers,
        album,
        duration: detail
            .track
            .interval
            .checked_mul(1000)
            .and_then(|ms| u64::try_from(ms).ok())
            .map(Duration::from_millis),
        cover,
        url: Some(uri.clone()),
        qualities: vec![quality_from_file_type(file_type)],
    };

    // 启动播放器
    let core = PlayerCore::new().map_err(|e| e.to_string())?;
    // 注册 MPRIS（bus 名 org.mpris.MediaPlayer2.hmp，供状态栏/playerctl 显示）
    let mpris = hmp_mpris::MprisService::start(core.command_sender(), core.subscribe_state())
        .await
        .ok();
    let state_rx = core.subscribe_state();
    core.load(LoadRequest {
        track,
        uri,
        quality: quality_from_file_type(file_type),
    });
    core.play();

    println!("正在播放… 按 Ctrl+C 停止");
    print_progress(state_rx).await?;

    // 释放 MPRIS bus 名并停止播放器
    drop(mpris);
    core.shutdown();
    Ok(())
}

/// 循环打印进度（阻塞直到 Ctrl+C/出错）。
async fn print_progress(
    state_rx: watch::Receiver<PlaybackState>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut last_status = None;
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let s = state_rx.borrow().clone();
        if last_status != Some(s.status) {
            println!("状态: {:?}", s.status);
            last_status = Some(s.status);
        }
        match s.status {
            PlaybackStatus::Error => {
                return Err("播放器出错".into());
            }
            PlaybackStatus::Ended => {
                println!("播放结束");
                return Ok(());
            }
            PlaybackStatus::Stopped | PlaybackStatus::Empty => {
                return Ok(());
            }
            _ => {}
        }
    }
}

/// 反向映射（展示用）。
fn quality_from_file_type(t: SongFileType) -> AudioQuality {
    match (t.s, t.e) {
        ("AIM0", _) => AudioQuality::Master,
        ("Q0M0", _) => AudioQuality::Atmos,
        ("F0M0", _) => AudioQuality::Flac,
        ("C600", _) => AudioQuality::Aac,
        ("M800", _) => AudioQuality::Mp3_320,
        _ => AudioQuality::Mp3_128,
    }
}
