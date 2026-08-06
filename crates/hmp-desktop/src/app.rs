//! 应用核心：单一播放状态源 + 队列/会话/业务编排（docs/PROJECT.md §4.1）。
//!
//! - 消费 UI/MPRIS 命令（`AppCommand`）；
//! - 队列管理（Next/Previous/LoopMode/Shuffle）；
//! - 取流（音质回退链）→ `PlayerCore` 播放；
//! - 登录（二维码轮询）+ 凭证存储（keyring）；
//! - 发布 `PlaybackState`（UI/MPRIS 消费）。

use std::sync::Arc;
use std::time::Duration;

use hmp_core::{AudioQuality, LoopMode, PlaybackState, PlayerCommand, Track, TrackId};
use hmp_mpris::MprisService;
use hmp_player_gst::{LoadRequest, PlayerCore};
use hmp_qqmusic_api::{
    Credential, LoginApi, QRLoginType, QqMusicClient, SongFileType,
    song::{SongApi, SongFileInfo},
};
use hmp_storage::credential::{CredentialStore, store_from_env};
use tokio::sync::mpsc;

/// 搜索结果显示数据（标题/歌手/时长文本）。
#[derive(Clone, Debug)]
pub struct UiSongData {
    pub title: String,
    pub artist: String,
    pub duration: String,
}

/// 应用事件（AppCore → UI）。
#[derive(Clone, Debug)]
pub enum AppEvent {
    /// 搜索完成（结果列表）。
    SearchDone(Vec<UiSongData>),
    /// 登录二维码（PNG 字节）。
    LoginQr(Vec<u8>),
    /// 登录状态文本。
    LoginStatus(String),
    /// 登录完成（用户昵称/UID）。
    LoginDone(String),
}

/// 应用命令（UI/MPRIS 统一入口）。
#[derive(Clone, Debug)]
pub enum AppCommand {
    /// 搜索。
    Search(String),
    /// 播放队列第 idx 首（搜索结果）。
    PlayIndex(usize),
    /// 播放/暂停。
    TogglePlay,
    /// 下一首。
    Next,
    /// 上一首。
    Previous,
    /// 跳转（秒）。
    Seek(f32),
    /// 音量（0..1）。
    SetVolume(f32),
    /// 开始登录。
    LoginStart,
    /// 取消登录。
    LoginCancel,
    /// 退出。
    Quit,
}

/// 队列条目（解析后曲目）。
#[derive(Clone)]
pub struct QueueItem {
    pub track: Track,
    pub mid: String,
    pub media_mid: String,
}

/// 应用核心。
pub struct AppCore {
    pub client: QqMusicClient,
    pub player: Arc<PlayerCore>,
    cmd_rx: mpsc::UnboundedReceiver<AppCommand>,
    events_tx: mpsc::UnboundedSender<AppEvent>,
    store: Box<dyn CredentialStore>,
    credential: Option<Credential>,
    songs: Vec<hmp_qqmusic_api::protocol::search::QuickSong>,
    queue: Vec<QueueItem>,
    queue_index: usize,
    loop_mode: LoopMode,
    shuffle: bool,
    _mpris: Option<MprisService>,
}

impl AppCore {
    /// 构造应用核心（启动播放器 + MPRIS + 加载凭证）。
    pub fn new(
        cmd_rx: mpsc::UnboundedReceiver<AppCommand>,
        events_tx: mpsc::UnboundedSender<AppEvent>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let player = Arc::new(PlayerCore::new()?);
        let store = store_from_env();
        // 密钥环不可用不阻塞启动：降级为未登录，凭据保存时再报错
        let credential = match store.load() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("credential load failed (not logged in): {e}");
                None
            }
        };
        let mpris = tokio::runtime::Handle::current()
            .block_on(MprisService::start(
                player.command_sender(),
                player.subscribe_state(),
            ))
            .ok();
        Ok(Self {
            client: QqMusicClient::new(),
            player,
            cmd_rx,
            events_tx,
            store,
            credential,
            songs: Vec::new(),
            queue: Vec::new(),
            queue_index: 0,
            loop_mode: LoopMode::None,
            shuffle: false,
            _mpris: mpris,
        })
    }

    /// 是否有有效登录凭证。
    pub fn logged_in(&self) -> bool {
        self.credential.as_ref().is_some_and(|c| c.is_logged_in())
    }

    /// 登录用户展示名。
    pub fn user_name(&self) -> String {
        self.credential
            .as_ref()
            .map(|c| c.uin.clone())
            .unwrap_or_default()
    }

    /// 当前搜索结果（UI 拉取）。
    pub fn songs(&self) -> &[hmp_qqmusic_api::protocol::search::QuickSong] {
        &self.songs
    }

    /// 事件循环（消费命令）。
    pub async fn run(&mut self) {
        while let Some(cmd) = self.cmd_rx.recv().await {
            match cmd {
                AppCommand::Search(keyword) => self.search(&keyword).await,
                AppCommand::PlayIndex(idx) => self.play_index(idx).await,
                AppCommand::TogglePlay => self.toggle_play(),
                AppCommand::Next => self.play_relative(1).await,
                AppCommand::Previous => self.play_relative(-1).await,
                AppCommand::Seek(secs) => {
                    self.player.seek(Duration::from_secs_f32(secs.max(0.0)));
                }
                AppCommand::SetVolume(v) => self.player.set_volume(v.clamp(0.0, 1.0) as f64),
                AppCommand::LoginStart => self.login_start().await,
                AppCommand::LoginCancel => self.login_cancel().await,
                AppCommand::Quit => break,
            }
        }
    }

    // -----------------------------------------------------------------
    // 搜索 / 播放
    // -----------------------------------------------------------------

    async fn search(&mut self, keyword: &str) {
        match self.client.quick_search(keyword).await {
            Ok(result) => {
                let songs = result.songs;
                let data = songs
                    .iter()
                    .map(|s| UiSongData {
                        title: s.name.clone(),
                        artist: s.singer.clone(),
                        duration: "—".into(),
                    })
                    .collect();
                self.songs = songs;
                let _ = self.events_tx.send(AppEvent::SearchDone(data));
                tracing::info!(count = self.songs.len(), keyword, "search done");
            }
            Err(e) => tracing::error!("search failed: {e}"),
        }
    }

    async fn play_index(&mut self, idx: usize) {
        let Some(song) = self.songs.get(idx) else {
            tracing::warn!("play index out of range: {idx}");
            return;
        };
        let mid = song.mid.clone();
        let title = song.name.clone();
        let artist = song.singer.clone();

        // 歌曲详情（media_mid）
        let (media_mid, interval) = match self.client_music_detail(&mid).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("detail failed: {e}");
                return;
            }
        };
        if media_mid.is_empty() {
            tracing::error!("no media_mid for {mid}");
            return;
        }

        // 音质回退取流
        let (file_type, uri) = match self.resolve_stream(&mid, &media_mid).await {
            Some(v) => v,
            None => {
                tracing::error!("all qualities unavailable for {mid}");
                return;
            }
        };

        let track = Track {
            id: TrackId::new(mid.clone()),
            title: title.clone(),
            artists: vec![hmp_core::ArtistRef {
                id: hmp_core::ArtistId::new(mid.clone()),
                name: artist.clone(),
            }],
            album: None,
            duration: interval.map(Duration::from_secs),
            cover: None,
            qualities: vec![quality_from_file_type(file_type)],
        };

        // 组装队列（搜索结果 → 队列）
        self.queue.clear();
        for s in &self.songs {
            self.queue.push(QueueItem {
                track: Track {
                    id: TrackId::new(s.mid.clone()),
                    title: s.name.clone(),
                    artists: vec![hmp_core::ArtistRef {
                        id: hmp_core::ArtistId::new(s.mid.clone()),
                        name: s.singer.clone(),
                    }],
                    album: None,
                    duration: None,
                    cover: None,
                    qualities: vec![],
                },
                mid: s.mid.clone(),
                media_mid: String::new(),
            });
        }
        self.queue_index = idx;
        if let Some(item) = self.queue.get_mut(idx) {
            item.media_mid = media_mid.clone();
            item.track.duration = interval.map(Duration::from_secs);
            item.track.qualities = vec![quality_from_file_type(file_type)];
            item.track.cover = None;
        }
        let _ = track;

        self.player.load(LoadRequest {
            uri,
            track: self.queue[self.queue_index].track.clone(),
            quality: quality_from_file_type(file_type),
        });
        tracing::info!(mid, title, "playing");
    }

    async fn play_relative(&mut self, delta: isize) {
        if self.queue.is_empty() {
            return;
        }
        // 单曲循环：next/prev 重播当前曲目
        if self.loop_mode == LoopMode::Track {
            let item = self.queue[self.queue_index].clone();
            if !item.media_mid.is_empty() {
                let mid = item.mid.clone();
                let media_mid = item.media_mid.clone();
                if let Some((file_type, uri)) = self.resolve_stream(&mid, &media_mid).await {
                    self.player.load(LoadRequest {
                        uri,
                        track: item.track,
                        quality: quality_from_file_type(file_type),
                    });
                }
            }
            return;
        }
        // 随机播放：next 随机选一首（避免与当前相同）
        let next = if self.shuffle && delta > 0 && self.queue.len() > 1 {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as usize)
                .unwrap_or(0);
            let mut idx = (self.queue_index * 31 + seed + self.queue.len()) % self.queue.len();
            if idx == self.queue_index {
                idx = (idx + 1) % self.queue.len();
            }
            idx
        } else {
            // 顺序/列表循环：列表末尾回绕
            (self.queue_index as isize + delta).rem_euclid(self.queue.len() as isize) as usize
        };
        self.queue_index = next;
        let item = self.queue[next].clone();
        if item.media_mid.is_empty() {
            // 队列里未解析的项需重新取流
            let mid = item.mid.clone();
            let (media_mid, _) = match self.client_music_detail(&mid).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("detail failed for queue: {e}");
                    return;
                }
            };
            let (file_type, uri) = match self.resolve_stream(&mid, &media_mid).await {
                Some(v) => v,
                None => return,
            };
            let mut item = item;
            item.media_mid = media_mid;
            self.queue[next] = item.clone();
            self.player.load(LoadRequest {
                uri,
                track: item.track,
                quality: quality_from_file_type(file_type),
            });
        }
    }

    fn toggle_play(&self) {
        self.player
            .command_sender()
            .send(PlayerCommand::TogglePlay)
            .ok();
    }

    // -----------------------------------------------------------------
    // 取流
    // -----------------------------------------------------------------

    async fn client_music_detail(
        &self,
        mid: &str,
    ) -> Result<(String, Option<u64>), hmp_qqmusic_api::QqMusicError> {
        let song_api = SongApi::new(&self.client);
        let detail = song_api.get_detail(mid).await?;
        Ok((
            detail.track.file.media_mid.clone(),
            detail
                .track
                .interval
                .checked_mul(1000)
                .and_then(|v| u64::try_from(v).ok())
                .map(Duration::from_millis)
                .map(|d| d.as_secs()),
        ))
    }

    /// 音质回退取流：Master → HiRes → Atmos → FLAC → AAC → 320 → 128。
    async fn resolve_stream(&self, mid: &str, media_mid: &str) -> Option<(SongFileType, String)> {
        let song_api = SongApi::new(&self.client);
        let info = SongFileInfo {
            mid: mid.to_owned(),
            file_type: None,
            song_type: 0,
            media_mid: Some(media_mid.to_owned()),
        };
        for quality in AudioQuality::Master.fallback_chain() {
            let Some(ft) = quality_to_file_type(quality.clone()) else {
                continue;
            };
            let resp = song_api
                .get_song_urls(std::slice::from_ref(&info), ft, self.credential.as_ref())
                .await;
            match resp {
                Ok(resp) => {
                    for item in &resp.data {
                        if item.result == 0 && !item.purl.is_empty() {
                            let uri = format!("https://isure.stream.qqmusic.qq.com/{}", item.purl);
                            tracing::info!(quality = ?quality, "stream resolved");
                            return Some((ft, uri));
                        }
                    }
                }
                Err(e) => tracing::debug!("quality {quality:?} failed: {e}"),
            }
        }
        None
    }

    // -----------------------------------------------------------------
    // 登录
    // -----------------------------------------------------------------

    async fn login_start(&mut self) {
        let login = LoginApi::new(&self.client);
        let qr = match login.get_qrcode(QRLoginType::Qq).await {
            Ok(qr) => qr,
            Err(e) => {
                let _ = self
                    .events_tx
                    .send(AppEvent::LoginStatus(format!("获取二维码失败: {e}")));
                tracing::error!("get qrcode failed: {e}");
                return;
            }
        };
        let _ = self.events_tx.send(AppEvent::LoginQr(qr.data.clone()));
        let _ = self
            .events_tx
            .send(AppEvent::LoginStatus("请用 QQ 手机版扫码并确认".into()));
        // 二维码回调由 UI 侧处理（QRImage 属性）；此处轮询等待
        match login
            .wait_qrcode_login(&qr, Default::default(), Duration::from_secs(180), None)
            .await
        {
            Ok(credential) => {
                if let Err(e) = self.store.save(&credential) {
                    tracing::error!("save credential failed: {e}");
                } else {
                    let name = credential.uin.clone();
                    self.credential = Some(credential);
                    let _ = self.events_tx.send(AppEvent::LoginDone(name));
                    tracing::info!("login ok");
                }
            }
            Err(e) => tracing::warn!("login cancelled/failed: {e}"),
        }
    }

    async fn login_cancel(&mut self) {
        // 当前实现通过 wait_qrcode_login 的超时/取消令牌控制；
        // 简化：无取消令牌，UI 层关闭登录面板即可。
    }
}

/// `AudioQuality` → 取流文件类型。
pub fn quality_to_file_type(q: AudioQuality) -> Option<SongFileType> {
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

/// 文件类型 → 音质标签。
pub fn quality_from_file_type(t: SongFileType) -> AudioQuality {
    match (t.s, t.e) {
        ("AIM0", _) => AudioQuality::Master,
        ("Q0M0", _) => AudioQuality::Atmos,
        ("F0M0", _) => AudioQuality::Flac,
        ("C600", _) => AudioQuality::Aac,
        ("M800", _) => AudioQuality::Mp3_320,
        _ => AudioQuality::Mp3_128,
    }
}

/// 当前播放状态（供 UI 同步任务消费）。
pub fn playback_snapshot(
    state: &PlaybackState,
) -> (String, String, String, f32, f32, String, String) {
    let title = state
        .current
        .as_ref()
        .map(|t| t.title.clone())
        .unwrap_or_default();
    let artist = state
        .current
        .as_ref()
        .map(|t| t.artist_names())
        .unwrap_or_default();
    let status = match state.status {
        hmp_core::PlaybackStatus::Playing => "playing",
        hmp_core::PlaybackStatus::Paused => "paused",
        _ => "stopped",
    }
    .to_owned();
    let pos = state.position.as_secs_f32();
    let dur = state.duration.map(|d| d.as_secs_f32()).unwrap_or(0.0);
    (
        title,
        artist,
        status,
        pos,
        dur,
        format_secs(pos),
        format_secs(dur),
    )
}

/// 秒 → mm:ss。
pub fn format_secs(s: f32) -> String {
    let s = s.max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}
