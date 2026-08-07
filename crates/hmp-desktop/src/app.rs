//! 应用核心：单一播放状态源 + 队列/会话/业务编排（docs/PROJECT.md §4.1）。
//!
//! - 消费 UI/MPRIS 命令（`AppCommand`）；
//! - 队列管理（Next/Previous/LoopMode/Shuffle）；
//! - 取流（音质回退链）→ `PlayerCore` 播放；
//! - 登录（二维码轮询）+ 凭证存储（keyring）；
//! - 发布 `PlaybackState`（UI/MPRIS 消费）。

use std::sync::Arc;
use std::time::Duration;

use hmp_core::{
    AudioQuality, LoopMode, PlaybackState, PlaybackStatus, PlayerCommand, Track, TrackId,
};
use hmp_mpris::MprisService;
use hmp_player_gst::{LoadRequest, PlayerCore};
use hmp_qqmusic_api::{
    Credential, LoginApi, LyricApi, QRLoginType, QqMusicClient, SongFileType,
    song::{SongApi, SongFileInfo},
};

use crate::lyrics::parse_lrc;
use hmp_storage::credential::{CredentialStore, store_from_env};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

/// UI 页面稳定标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiPage {
    Library,
    Recommend,
    Search,
    Queue,
    Lyrics,
    Settings,
}

impl UiPage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Recommend => "recommend",
            Self::Search => "search",
            Self::Queue => "queue",
            Self::Lyrics => "lyrics",
            Self::Settings => "settings",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "library" => Some(Self::Library),
            "recommend" => Some(Self::Recommend),
            "search" => Some(Self::Search),
            "queue" => Some(Self::Queue),
            "lyrics" => Some(Self::Lyrics),
            "settings" => Some(Self::Settings),
            _ => None,
        }
    }
}

/// UI 主题模式稳定标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    FollowSystem,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FollowSystem => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::FollowSystem),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

/// 搜索结果显示数据（标题/歌手/时长文本）。
#[derive(Clone, Debug)]
pub struct UiSongData {
    pub title: String,
    pub artist: String,
    pub duration: String,
}

/// 播放队列显示数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiQueueData {
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub duration: String,
    pub is_current: bool,
    pub is_playing: bool,
}

/// 歌词行显示数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiLyricData {
    pub timestamp_ms: u64,
    pub time: String,
    pub text: String,
    pub translation: String,
}

/// 功能状态显示数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiFeatureData {
    pub name: String,
    pub status: String,
    pub detail: String,
}

/// 应用事件（AppCore → UI）。
#[derive(Clone, Debug)]
pub enum AppEvent {
    /// 搜索完成（结果列表）。
    SearchDone(Vec<UiSongData>),
    /// 搜索失败。
    SearchFailed(String),
    /// 播放队列状态更新。
    QueueUpdated(Vec<UiQueueData>),
    /// 开始加载指定歌曲的歌词。
    LyricsLoading(String),
    /// 指定歌曲的歌词加载完成。
    LyricsLoaded {
        mid: String,
        lines: Vec<UiLyricData>,
    },
    /// 指定歌曲的歌词加载失败。
    LyricsFailed { mid: String, message: String },
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
    /// 播放队列第 idx 首（真实播放队列）。
    PlayQueueIndex(usize),
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
    /// 重新加载当前歌曲歌词。
    ReloadLyrics,
    /// 退出。
    Quit,
}

/// 队列条目（解析后曲目）。
#[derive(Clone)]
pub struct QueueItem {
    pub track: Track,
    pub mid: String,
    pub media_mid: String,
    pub song_type: i64,
}

struct ResolvedSongDetail {
    media_mid: String,
    duration: Option<u64>,
    song_type: i64,
}

#[derive(Default)]
struct LoginSessionState {
    generation: u64,
    cancel: Option<CancellationToken>,
}

impl LoginSessionState {
    fn begin(&mut self) -> (u64, CancellationToken) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.generation = self.generation.wrapping_add(1);
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        (self.generation, cancel)
    }

    fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
    }

    fn accepts(&self, generation: u64) -> bool {
        self.generation == generation
            && self
                .cancel
                .as_ref()
                .is_some_and(|cancel| !cancel.is_cancelled())
    }
}

struct LoginResult {
    generation: u64,
    result: Result<Credential, String>,
}

struct LyricResult {
    generation: u64,
    mid: String,
    result: Result<Vec<UiLyricData>, String>,
}

#[derive(Default)]
struct LyricRequestState {
    generation: u64,
    mid: Option<String>,
    cancel: Option<CancellationToken>,
}

impl LyricRequestState {
    fn begin(&mut self, mid: String) -> (u64, CancellationToken) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.generation = self.generation.wrapping_add(1);
        let cancel = CancellationToken::new();
        self.mid = Some(mid);
        self.cancel = Some(cancel.clone());
        (self.generation, cancel)
    }

    fn accepts(&self, generation: u64, mid: &str) -> bool {
        self.generation == generation
            && self.mid.as_deref() == Some(mid)
            && self
                .cancel
                .as_ref()
                .is_some_and(|cancel| !cancel.is_cancelled())
    }
}

#[derive(Debug)]
enum LoginUpdatePayload {
    Qr(Vec<u8>),
    Status(String),
}

#[derive(Debug)]
struct LoginUpdate {
    generation: u64,
    payload: LoginUpdatePayload,
}

fn forward_login_update(
    events_tx: &mpsc::UnboundedSender<AppEvent>,
    session: &LoginSessionState,
    update: LoginUpdate,
) {
    if !session.accepts(update.generation) {
        return;
    }
    let event = match update.payload {
        LoginUpdatePayload::Qr(png) => AppEvent::LoginQr(png),
        LoginUpdatePayload::Status(status) => AppEvent::LoginStatus(status),
    };
    let _ = events_tx.send(event);
}

/// 应用核心。
pub struct AppCore {
    pub client: QqMusicClient,
    pub player: Arc<PlayerCore>,
    state_rx: watch::Receiver<PlaybackState>,
    cmd_rx: mpsc::UnboundedReceiver<AppCommand>,
    events_tx: mpsc::UnboundedSender<AppEvent>,
    store: Box<dyn CredentialStore>,
    credential: Option<Credential>,
    songs: Vec<hmp_qqmusic_api::protocol::search::QuickSong>,
    queue: Vec<QueueItem>,
    queue_index: usize,
    loop_mode: LoopMode,
    shuffle: bool,
    last_queue_state: Option<(String, bool)>,
    current_lyrics: Option<(String, i64)>,
    lyric_requests: LyricRequestState,
    lyric_results_tx: mpsc::UnboundedSender<LyricResult>,
    lyric_results_rx: mpsc::UnboundedReceiver<LyricResult>,
    login_results_tx: mpsc::UnboundedSender<LoginResult>,
    login_results_rx: mpsc::UnboundedReceiver<LoginResult>,
    login_updates_tx: mpsc::UnboundedSender<LoginUpdate>,
    login_updates_rx: mpsc::UnboundedReceiver<LoginUpdate>,
    login_generation: u64,
    login_cancel: Option<CancellationToken>,
    _mpris: Option<MprisService>,
}

impl AppCore {
    /// 构造应用核心（启动播放器 + MPRIS + 加载凭证）。
    pub fn new(
        cmd_rx: mpsc::UnboundedReceiver<AppCommand>,
        events_tx: mpsc::UnboundedSender<AppEvent>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let player = Arc::new(PlayerCore::new()?);
        let state_rx = player.subscribe_state();
        let initial_queue_state = queue_state_key(&state_rx.borrow());
        let store = store_from_env();
        // 密钥环不可用不阻塞启动：降级为未登录，凭据保存时再报错
        let credential = match store.load() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("credential load failed (not logged in): {e}");
                None
            }
        };
        let (login_results_tx, login_results_rx) = mpsc::unbounded_channel();
        let (login_updates_tx, login_updates_rx) = mpsc::unbounded_channel();
        let (lyric_results_tx, lyric_results_rx) = mpsc::unbounded_channel();
        let mpris = tokio::runtime::Handle::current()
            .block_on(MprisService::start(
                player.command_sender(),
                player.subscribe_state(),
            ))
            .ok();
        Ok(Self {
            client: QqMusicClient::new(),
            player,
            state_rx,
            cmd_rx,
            events_tx,
            store,
            credential,
            songs: Vec::new(),
            queue: Vec::new(),
            queue_index: 0,
            loop_mode: LoopMode::None,
            shuffle: false,
            last_queue_state: Some(initial_queue_state),
            current_lyrics: None,
            lyric_requests: LyricRequestState::default(),
            lyric_results_tx,
            lyric_results_rx,
            login_results_tx,
            login_results_rx,
            login_updates_tx,
            login_updates_rx,
            login_generation: 0,
            login_cancel: None,
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

    /// 当前播放队列的 UI 快照。
    pub fn queue_snapshot(&self) -> Vec<UiQueueData> {
        let state = self.state_rx.borrow();
        let current_track_id = state.current.as_ref().map(|track| track.id.as_ref());

        self.queue
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let is_current = index == self.queue_index;
                let matches_playback = current_track_id == Some(item.track.id.as_ref());
                UiQueueData {
                    track_id: item.track.id.to_string(),
                    title: item.track.title.clone(),
                    artist: item.track.artist_names(),
                    duration: item
                        .track
                        .duration
                        .map(|duration| format_secs(duration.as_secs_f32()))
                        .unwrap_or_else(|| "--:--".into()),
                    is_current,
                    is_playing: is_current
                        && matches_playback
                        && state.status == PlaybackStatus::Playing,
                }
            })
            .collect()
    }

    /// 事件循环（消费命令及后台登录结果）。
    pub async fn run(&mut self) {
        loop {
            tokio::select! {
                command = self.cmd_rx.recv() => {
                    let Some(command) = command else { break };
                    match command {
                        AppCommand::Search(keyword) => self.search(&keyword).await,
                        AppCommand::PlayIndex(idx) => self.play_index(idx).await,
                        AppCommand::PlayQueueIndex(idx) => self.play_queue_index(idx).await,
                        AppCommand::TogglePlay => self.toggle_play(),
                        AppCommand::Next => self.play_relative(1).await,
                        AppCommand::Previous => self.play_relative(-1).await,
                        AppCommand::Seek(secs) => {
                            self.player.seek(Duration::from_secs_f32(secs.max(0.0)));
                        }
                        AppCommand::SetVolume(v) => self.player.set_volume(v.clamp(0.0, 1.0) as f64),
                        AppCommand::LoginStart => self.start_login(),
                        AppCommand::LoginCancel => self.cancel_login(),
                        AppCommand::ReloadLyrics => self.reload_lyrics(),
                        AppCommand::Quit => break,
                    }
                }
                result = self.login_results_rx.recv() => {
                    if let Some(result) = result {
                        self.finish_login(result);
                    }
                }
                update = self.login_updates_rx.recv() => {
                    if let Some(update) = update {
                        self.forward_login_update(update);
                    }
                }
                lyric_result = self.lyric_results_rx.recv() => {
                    if let Some(result) = lyric_result {
                        self.finish_lyric_result(result);
                    }
                }
                changed = self.state_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let key = queue_state_key(&self.state_rx.borrow());
                    self.publish_queue_if_changed(key);
                }
            }
        }
        self.cancel_login_session();
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
            Err(e) => {
                let _ = self.events_tx.send(AppEvent::SearchFailed(e.to_string()));
                tracing::error!("search failed: {e}");
            }
        }
    }

    async fn play_index(&mut self, idx: usize) {
        let Some(song) = self.songs.get(idx) else {
            tracing::warn!("play index out of range: {idx}");
            return;
        };
        let mid = song.mid.clone();
        let title = song.name.clone();

        let detail = match self.client_music_detail(&mid).await {
            Ok(detail) => detail,
            Err(error) => {
                tracing::error!("detail failed: {error}");
                return;
            }
        };
        if detail.media_mid.is_empty() {
            tracing::error!("no media_mid for {mid}");
            return;
        }
        let (file_type, uri) = match self
            .resolve_stream(&mid, &detail.media_mid, detail.song_type)
            .await
        {
            Some(value) => value,
            None => {
                tracing::error!("all qualities unavailable for {mid}");
                return;
            }
        };

        self.queue = self
            .songs
            .iter()
            .map(|song| QueueItem {
                track: Track {
                    id: TrackId::new(song.mid.clone()),
                    title: song.name.clone(),
                    artists: vec![hmp_core::ArtistRef {
                        id: hmp_core::ArtistId::new(song.mid.clone()),
                        name: song.singer.clone(),
                    }],
                    album: None,
                    duration: None,
                    cover: None,
                    qualities: Vec::new(),
                },
                mid: song.mid.clone(),
                media_mid: String::new(),
                song_type: 0,
            })
            .collect();
        self.queue_index = idx;
        let item = &mut self.queue[idx];
        item.media_mid = detail.media_mid;
        item.song_type = detail.song_type;
        item.track.duration = detail.duration.map(Duration::from_secs);
        item.track.qualities = vec![quality_from_file_type(file_type)];

        self.player.load(LoadRequest {
            uri,
            track: item.track.clone(),
            quality: quality_from_file_type(file_type),
        });
        self.current_lyrics = Some((mid.clone(), detail.song_type));
        self.start_lyrics_load(mid.clone(), detail.song_type);
        self.publish_queue_snapshot();
        tracing::info!(mid, title, "playing");
    }

    async fn play_queue_index(&mut self, idx: usize) {
        let Some(item) = queue_item_at(&self.queue, idx) else {
            tracing::warn!("queue index out of range: {idx}");
            return;
        };
        self.play_queue_item(idx, item).await;
    }

    async fn play_relative(&mut self, delta: isize) {
        if self.queue.is_empty() {
            return;
        }
        let next = if self.loop_mode == LoopMode::Track {
            self.queue_index
        } else if self.shuffle && delta > 0 && self.queue.len() > 1 {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos() as usize)
                .unwrap_or(0);
            let mut index = (self.queue_index * 31 + seed + self.queue.len()) % self.queue.len();
            if index == self.queue_index {
                index = (index + 1) % self.queue.len();
            }
            index
        } else {
            (self.queue_index as isize + delta).rem_euclid(self.queue.len() as isize) as usize
        };
        let item = self.queue[next].clone();
        self.play_queue_item(next, item).await;
    }

    async fn play_queue_item(&mut self, next: usize, mut item: QueueItem) {
        let current_index = self.queue_index;
        let was_unresolved = item.media_mid.is_empty();
        if was_unresolved {
            let detail = match self.client_music_detail(&item.mid).await {
                Ok(detail) => detail,
                Err(error) => {
                    tracing::error!("detail failed for queue: {error}");
                    return;
                }
            };
            item.media_mid = detail.media_mid;
            item.song_type = detail.song_type;
            item.track.duration = detail.duration.map(Duration::from_secs);
        }
        let (file_type, uri) = match self
            .resolve_stream(&item.mid, &item.media_mid, item.song_type)
            .await
        {
            Some(value) => value,
            None => return,
        };
        item.track.qualities = vec![quality_from_file_type(file_type)];
        self.queue_index = next;
        self.queue[next] = item.clone();
        self.player.load(LoadRequest {
            uri,
            track: item.track,
            quality: quality_from_file_type(file_type),
        });
        self.current_lyrics = Some((item.mid.clone(), item.song_type));
        self.start_lyrics_load(item.mid, item.song_type);
        let key = queue_state_key(&self.state_rx.borrow());
        self.last_queue_state = Some(key.clone());
        if queue_direct_publication_needed(current_index, next, was_unresolved) {
            let _ = self
                .events_tx
                .send(AppEvent::QueueUpdated(self.queue_snapshot()));
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
    ) -> Result<ResolvedSongDetail, hmp_qqmusic_api::QqMusicError> {
        let song_api = SongApi::new(&self.client);
        let detail = song_api.get_detail(mid).await?;
        Ok(ResolvedSongDetail {
            media_mid: detail.track.file.media_mid.clone(),
            duration: u64::try_from(detail.track.interval).ok(),
            song_type: detail.track.type_,
        })
    }

    /// 音质回退取流：Master → HiRes → Atmos → FLAC → AAC → 320 → 128。
    async fn resolve_stream(
        &self,
        mid: &str,
        media_mid: &str,
        song_type: i64,
    ) -> Option<(SongFileType, String)> {
        let song_api = SongApi::new(&self.client);
        let info = SongFileInfo {
            mid: mid.to_owned(),
            file_type: None,
            song_type,
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

    fn reload_lyrics(&mut self) {
        if let Some((mid, song_type)) = &self.current_lyrics {
            self.start_lyrics_load(mid.clone(), *song_type);
        }
    }

    fn start_lyrics_load(&mut self, mid: String, song_type: i64) {
        if mid.is_empty() {
            return;
        }
        let (generation, cancel) = self.lyric_requests.begin(mid.clone());
        let _ = self.events_tx.send(AppEvent::LyricsLoading(mid.clone()));
        let client = QqMusicClient::with_config(self.client.config());
        let results_tx = self.lyric_results_tx.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                _ = cancel.cancelled() => return,
                result = async {
                    LyricApi::new(&client)
                        .get_lyric(&mid, song_type, false, true, false, false)
                        .await
                        .map(|response| parse_lrc(&response.lyric, &response.trans))
                        .map_err(|error| error.to_string())
                } => result,
            };
            let _ = results_tx.send(LyricResult {
                generation,
                mid,
                result,
            });
        });
    }

    fn finish_lyric_result(&self, result: LyricResult) {
        forward_lyric_result(&self.events_tx, &self.lyric_requests, result);
    }

    fn publish_queue_if_changed(&mut self, key: (String, bool)) {
        if !queue_publication_changed(&mut self.last_queue_state, key) {
            return;
        }
        let _ = self
            .events_tx
            .send(AppEvent::QueueUpdated(self.queue_snapshot()));
    }

    fn publish_queue_snapshot(&mut self) {
        let key = queue_state_key(&self.state_rx.borrow());
        self.last_queue_state = Some(key);
        let _ = self
            .events_tx
            .send(AppEvent::QueueUpdated(self.queue_snapshot()));
    }

    // -----------------------------------------------------------------
    // 登录
    // -----------------------------------------------------------------

    fn start_login(&mut self) {
        let (generation, cancel) = self.begin_login_session();
        let client = QqMusicClient::with_config(self.client.config());
        let updates_tx = self.login_updates_tx.clone();
        let results_tx = self.login_results_tx.clone();

        tokio::spawn(async move {
            let login = LoginApi::new(&client);
            let result = async {
                let qr = tokio::select! {
                    _ = cancel.cancelled() => return Err("登录已取消".into()),
                    result = login.get_qrcode(QRLoginType::Qq) => {
                        result.map_err(|error| format!("获取二维码失败: {error}"))?
                    }
                };
                let _ = updates_tx.send(LoginUpdate {
                    generation,
                    payload: LoginUpdatePayload::Qr(qr.data.clone()),
                });
                let _ = updates_tx.send(LoginUpdate {
                    generation,
                    payload: LoginUpdatePayload::Status("请用 QQ 手机版扫码并确认".into()),
                });
                login
                    .wait_qrcode_login(
                        &qr,
                        Default::default(),
                        Duration::from_secs(180),
                        Some(&cancel),
                    )
                    .await
                    .map_err(|error| error.to_string())
            }
            .await;
            let _ = results_tx.send(LoginResult { generation, result });
        });
    }

    fn begin_login_session(&mut self) -> (u64, CancellationToken) {
        let mut session = LoginSessionState {
            generation: self.login_generation,
            cancel: self.login_cancel.take(),
        };
        let result = session.begin();
        self.login_generation = session.generation;
        self.login_cancel = session.cancel;
        result
    }

    fn cancel_login_session(&mut self) {
        let mut session = LoginSessionState {
            generation: self.login_generation,
            cancel: self.login_cancel.take(),
        };
        session.cancel();
        self.login_generation = session.generation;
        self.login_cancel = session.cancel;
    }

    fn login_session(&self) -> LoginSessionState {
        LoginSessionState {
            generation: self.login_generation,
            cancel: self.login_cancel.clone(),
        }
    }

    fn accepts_login_result(&self, generation: u64) -> bool {
        self.login_session().accepts(generation)
    }

    fn forward_login_update(&self, update: LoginUpdate) {
        forward_login_update(&self.events_tx, &self.login_session(), update);
    }

    fn cancel_login(&mut self) {
        self.cancel_login_session();
        let _ = self
            .events_tx
            .send(AppEvent::LoginStatus("登录已取消".into()));
    }

    fn finish_login(&mut self, login: LoginResult) {
        if !self.accepts_login_result(login.generation) {
            return;
        }
        match login.result {
            Ok(credential) => {
                if let Err(error) = self.store.save(&credential) {
                    let message = format!("保存登录凭证失败: {error}");
                    let _ = self.events_tx.send(AppEvent::LoginStatus(message));
                    tracing::error!("save credential failed: {error}");
                    return;
                }
                let name = credential.uin.clone();
                self.credential = Some(credential);
                self.cancel_login_session();
                let _ = self.events_tx.send(AppEvent::LoginDone(name));
                tracing::info!("login ok");
            }
            Err(message) => {
                let _ = self.events_tx.send(AppEvent::LoginStatus(message.clone()));
                tracing::warn!("login failed: {message}");
            }
        }
    }
}

fn queue_item_at(queue: &[QueueItem], index: usize) -> Option<QueueItem> {
    queue.get(index).cloned()
}

fn queue_direct_publication_needed(
    current_index: usize,
    next_index: usize,
    was_unresolved: bool,
) -> bool {
    current_index != next_index || was_unresolved
}

fn queue_publication_changed(
    last_queue_state: &mut Option<(String, bool)>,
    key: (String, bool),
) -> bool {
    if last_queue_state.as_ref() == Some(&key) {
        return false;
    }
    *last_queue_state = Some(key);
    true
}

fn forward_lyric_result(
    events_tx: &mpsc::UnboundedSender<AppEvent>,
    requests: &LyricRequestState,
    result: LyricResult,
) {
    if !requests.accepts(result.generation, &result.mid) {
        return;
    }
    let event = match result.result {
        Ok(lines) => AppEvent::LyricsLoaded {
            mid: result.mid,
            lines,
        },
        Err(message) => AppEvent::LyricsFailed {
            mid: result.mid,
            message,
        },
    };
    let _ = events_tx.send(event);
}

fn queue_state_key(state: &PlaybackState) -> (String, bool) {
    let current_id = state
        .current
        .as_ref()
        .map(|track| track.id.to_string())
        .unwrap_or_default();
    (current_id, state.status == PlaybackStatus::Playing)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starting_or_cancelling_login_invalidates_the_previous_session() {
        let mut state = LoginSessionState::default();
        let (first_generation, first_token) = state.begin();
        let (second_generation, _) = state.begin();
        assert!(second_generation > first_generation);
        assert!(first_token.is_cancelled());
        state.cancel();
        assert!(!state.accepts(second_generation));
    }

    #[test]
    fn queue_selection_uses_queue_items_when_search_results_diverge() {
        let search_mids = ["search-mid-a", "search-mid-b"];
        let queue = ["queue-mid-a", "queue-mid-b"]
            .into_iter()
            .map(|mid| QueueItem {
                track: Track::new(TrackId::new(mid.to_owned()), mid),
                mid: mid.to_owned(),
                media_mid: format!("media-{mid}"),
                song_type: 0,
            })
            .collect::<Vec<_>>();

        let selected = queue_item_at(&queue, 1).expect("queue index should resolve");
        assert_eq!(selected.mid, "queue-mid-b");
        assert_ne!(selected.mid, search_mids[1]);
        assert!(queue_item_at(&queue, queue.len()).is_none());
    }

    #[test]
    fn queue_direct_publication_needed_for_index_or_resolution_changes() {
        assert!(queue_direct_publication_needed(0, 1, false));
        assert!(queue_direct_publication_needed(0, 0, true));
        assert!(!queue_direct_publication_needed(0, 0, false));
    }

    #[test]
    fn queue_publication_baseline_and_direct_update_suppress_duplicates() {
        let baseline = (String::new(), false);
        let mut last_queue_state = Some(baseline.clone());
        assert!(!queue_publication_changed(&mut last_queue_state, baseline));

        let direct_update = ("queue-mid-b".to_owned(), true);
        last_queue_state = Some(direct_update.clone());
        assert!(!queue_publication_changed(
            &mut last_queue_state,
            direct_update
        ));
    }

    #[test]
    fn queue_state_changes_only_for_track_or_playing_status() {
        let mut state = PlaybackState {
            status: PlaybackStatus::Playing,
            current: Some(Track::new(TrackId::new("mid-1"), "First")),
            ..PlaybackState::default()
        };
        let playing_key = queue_state_key(&state);

        state.position = Duration::from_secs(30);
        assert_eq!(queue_state_key(&state), playing_key);

        state.status = PlaybackStatus::Paused;
        assert_ne!(queue_state_key(&state), playing_key);

        let paused_key = queue_state_key(&state);
        state.current = Some(Track::new(TrackId::new("mid-2"), "Second"));
        assert_ne!(queue_state_key(&state), paused_key);
    }

    #[test]
    fn login_updates_drop_stale_generations_and_forward_current_updates() {
        let mut state = LoginSessionState::default();
        let (first_generation, _) = state.begin();
        let (second_generation, _) = state.begin();
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();

        forward_login_update(
            &events_tx,
            &state,
            LoginUpdate {
                generation: first_generation,
                payload: LoginUpdatePayload::Qr(vec![1, 2, 3]),
            },
        );
        assert!(events_rx.try_recv().is_err());

        forward_login_update(
            &events_tx,
            &state,
            LoginUpdate {
                generation: second_generation,
                payload: LoginUpdatePayload::Status("扫码".into()),
            },
        );
        assert!(matches!(
            events_rx.try_recv(),
            Ok(AppEvent::LoginStatus(status)) if status == "扫码"
        ));

        state.cancel();
        forward_login_update(
            &events_tx,
            &state,
            LoginUpdate {
                generation: second_generation,
                payload: LoginUpdatePayload::Qr(vec![4, 5, 6]),
            },
        );
        assert!(events_rx.try_recv().is_err());
    }

    #[test]
    fn lyric_generations_drop_same_mid_reload_and_a_b_a_results() {
        let mut state = LyricRequestState::default();
        let (first, first_token) = state.begin("mid-a".into());
        let (reload, _) = state.begin("mid-a".into());
        assert!(first_token.is_cancelled());
        assert!(!state.accepts(first, "mid-a"));
        assert!(state.accepts(reload, "mid-a"));

        let (mid_b, mid_b_token) = state.begin("mid-b".into());
        let (mid_a_again, _) = state.begin("mid-a".into());
        assert!(mid_b_token.is_cancelled());
        assert!(!state.accepts(mid_b, "mid-b"));
        assert!(state.accepts(mid_a_again, "mid-a"));
        assert!(!state.accepts(reload, "mid-a"));
    }
}
