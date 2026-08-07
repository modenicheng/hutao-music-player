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
    AudioQuality, LoopMode, PlaybackCapabilities, PlaybackState, PlaybackStatus, PlayerCommand,
    Track, TrackId,
};
use hmp_media;
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

/// MPRIS 媒体命令路由结果。
#[derive(Debug, PartialEq, Eq)]
enum MediaCommandRoute {
    /// 队列相对移动（Next=1 / Previous=-1）。
    QueueRelative(isize),
    /// 转发 `PlayerCore`。
    Forward,
}

struct ResolvedSongDetail {
    media_mid: String,
    duration: Option<u64>,
    song_type: i64,
    /// (歌手 mid, 歌手名)
    singers: Vec<(String, String)>,
    /// (专辑 mid, 专辑名)
    album: Option<(String, String)>,
    /// 专辑封面媒体 ID（用于构造 CDN 封面 URL）
    cover_pmid: Option<String>,
}

#[derive(Default)]
struct NetworkRequestState {
    generation: u64,
    cancel: Option<CancellationToken>,
}

impl NetworkRequestState {
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

struct SearchResult {
    generation: u64,
    keyword: String,
    result: Result<Vec<hmp_qqmusic_api::protocol::search::QuickSong>, String>,
}

enum PlayRequest {
    Search {
        index: usize,
        songs: Vec<hmp_qqmusic_api::protocol::search::QuickSong>,
    },
    Queue {
        index: usize,
        item: QueueItem,
    },
}

struct ResolvedPlayback {
    index: usize,
    songs: Option<Vec<hmp_qqmusic_api::protocol::search::QuickSong>>,
    item: QueueItem,
    file_type: SongFileType,
    uri: String,
}

struct PlayResult {
    generation: u64,
    result: Result<ResolvedPlayback, String>,
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
    last_queue_state: Option<(String, bool)>,
    current_lyrics: Option<(String, i64)>,
    // MPRIS → 播放器/队列命令（由 MPRIS 服务发出，AppCore 路由：
    // Next/Previous 走队列，其余转发 PlayerCore）
    media_cmd_rx: mpsc::UnboundedReceiver<PlayerCommand>,
    // 队列能力（CanGoNext/CanGoPrevious）→ MPRIS
    capabilities_tx: watch::Sender<PlaybackCapabilities>,
    search_requests: NetworkRequestState,
    search_results_tx: mpsc::UnboundedSender<SearchResult>,
    search_results_rx: mpsc::UnboundedReceiver<SearchResult>,
    play_requests: NetworkRequestState,
    pending_queue_index: Option<usize>,
    play_results_tx: mpsc::UnboundedSender<PlayResult>,
    play_results_rx: mpsc::UnboundedReceiver<PlayResult>,
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
        let (search_results_tx, search_results_rx) = mpsc::unbounded_channel();
        let (play_results_tx, play_results_rx) = mpsc::unbounded_channel();
        // MPRIS 媒体命令通道：Next/Previous 等由 AppCore 路由到队列逻辑，
        // 播放级命令转发 PlayerCore（PlayerCore 自身忽略 Next/Previous/Shuffle）。
        let (media_cmd_tx, media_cmd_rx) = mpsc::unbounded_channel();
        // 队列能力（CanGoNext/CanGoPrevious）→ MPRIS
        let (capabilities_tx, capabilities_rx) = watch::channel(PlaybackCapabilities::default());
        let mpris = tokio::runtime::Handle::current()
            .block_on(MprisService::start_with_capabilities(
                media_cmd_tx,
                player.subscribe_state(),
                Some(capabilities_rx),
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
            last_queue_state: Some(initial_queue_state),
            current_lyrics: None,
            media_cmd_rx,
            capabilities_tx,
            search_requests: NetworkRequestState::default(),
            search_results_tx,
            search_results_rx,
            play_requests: NetworkRequestState::default(),
            pending_queue_index: None,
            play_results_tx,
            play_results_rx,
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
        let state = self.state_rx.borrow().clone();
        self.queue_snapshot_for_state(&state)
    }

    fn queue_snapshot_for_state(&self, state: &PlaybackState) -> Vec<UiQueueData> {
        queue_snapshot_for_state(&self.queue, self.queue_index, state)
    }

    /// 事件循环（消费命令及后台登录结果）。
    pub async fn run(&mut self) {
        loop {
            tokio::select! {
                command = self.cmd_rx.recv() => {
                    let Some(command) = command else { break };
                    match command {
                        AppCommand::Search(keyword) => self.start_search(keyword),
                        AppCommand::PlayIndex(idx) => self.start_play_index(idx),
                        AppCommand::PlayQueueIndex(idx) => self.start_play_queue_index(idx),
                        AppCommand::TogglePlay => self.toggle_play(),
                        AppCommand::Next => self.start_play_relative(1),
                        AppCommand::Previous => self.start_play_relative(-1),
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
                media_cmd = self.media_cmd_rx.recv() => {
                    let Some(cmd) = media_cmd else { continue };
                    self.handle_media_command(cmd);
                }
                result = self.search_results_rx.recv() => {
                    if let Some(result) = result {
                        self.finish_search(result);
                    }
                }
                result = self.play_results_rx.recv() => {
                    if let Some(result) = result {
                        self.finish_play(result);
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
                    let state = self.state_rx.borrow().clone();
                    self.publish_queue_if_changed(state);
                }
            }
        }
        self.search_requests.cancel();
        self.play_requests.cancel();
        self.cancel_login_session();
    }

    // -----------------------------------------------------------------
    // MPRIS 媒体命令路由
    // -----------------------------------------------------------------

    /// 路由 MPRIS 命令：队列级（Next/Previous）走队列逻辑；
    /// 其余（播放/暂停/跳转/音量/循环/随机）转发 `PlayerCore`，
    /// 循环与随机写入单一状态源 `PlaybackState`。
    fn handle_media_command(&mut self, cmd: PlayerCommand) {
        match Self::route_media_command(&cmd) {
            MediaCommandRoute::QueueRelative(delta) => self.start_play_relative(delta),
            MediaCommandRoute::Forward => {
                let _ = self.player.command_sender().send(cmd).ok();
            }
        }
    }

    /// MPRIS 命令路由决策（纯函数，便于测试）。
    fn route_media_command(cmd: &PlayerCommand) -> MediaCommandRoute {
        match cmd {
            PlayerCommand::Next => MediaCommandRoute::QueueRelative(1),
            PlayerCommand::Previous => MediaCommandRoute::QueueRelative(-1),
            _ => MediaCommandRoute::Forward,
        }
    }

    /// 发布队列能力（CanGoNext/CanGoPrevious）。
    fn sync_capabilities(&self) {
        let caps = PlaybackCapabilities {
            can_go_next: !self.queue.is_empty(),
            can_go_previous: !self.queue.is_empty(),
        };
        let _ = self.capabilities_tx.send(caps);
    }

    // -----------------------------------------------------------------
    // 搜索 / 播放
    // -----------------------------------------------------------------

    fn start_search(&mut self, keyword: String) {
        let (generation, cancel) = self.search_requests.begin();
        let client = QqMusicClient::with_config(self.client.config());
        let results_tx = self.search_results_tx.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                _ = cancel.cancelled() => return,
                result = client.quick_search(&keyword) => {
                    result.map(|response| response.songs).map_err(|error| error.to_string())
                }
            };
            let _ = results_tx.send(SearchResult {
                generation,
                keyword,
                result,
            });
        });
    }

    fn finish_search(&mut self, result: SearchResult) {
        if !self.search_requests.accepts(result.generation) {
            return;
        }
        match result.result {
            Ok(songs) => {
                let data = songs
                    .iter()
                    .map(|song| UiSongData {
                        title: song.name.clone(),
                        artist: song.singer.clone(),
                        duration: "—".into(),
                    })
                    .collect();
                self.songs = songs;
                let _ = self.events_tx.send(AppEvent::SearchDone(data));
                tracing::info!(
                    count = self.songs.len(),
                    keyword = result.keyword,
                    "search done"
                );
            }
            Err(message) => {
                let _ = self.events_tx.send(AppEvent::SearchFailed(message.clone()));
                tracing::error!(keyword = result.keyword, "search failed: {message}");
            }
        }
    }

    fn start_play_index(&mut self, index: usize) {
        if index >= self.songs.len() {
            tracing::warn!("play index out of range: {index}");
            return;
        }
        self.pending_queue_index = None;
        self.start_play_request(PlayRequest::Search {
            index,
            songs: self.songs.clone(),
        });
    }

    fn start_play_queue_index(&mut self, index: usize) {
        let Some(item) = queue_item_at(&self.queue, index) else {
            tracing::warn!("queue index out of range: {index}");
            return;
        };
        self.pending_queue_index = Some(index);
        self.start_play_request(PlayRequest::Queue { index, item });
    }

    fn start_play_relative(&mut self, delta: isize) {
        if self.queue.is_empty() {
            return;
        }
        // 循环/随机单一状态源：由 PlayerCore 状态承载（MPRIS 写入同一处）
        let state = self.state_rx.borrow().clone();
        let loop_mode = state.loop_mode;
        let shuffle = state.shuffle;
        let current = self.pending_queue_index.unwrap_or(self.queue_index);
        let next = if loop_mode == LoopMode::Track {
            current
        } else if shuffle && delta > 0 && self.queue.len() > 1 {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos() as usize)
                .unwrap_or(0);
            let mut index = (current * 31 + seed + self.queue.len()) % self.queue.len();
            if index == current {
                index = (index + 1) % self.queue.len();
            }
            index
        } else {
            (current as isize + delta).rem_euclid(self.queue.len() as isize) as usize
        };
        self.pending_queue_index = Some(next);
        self.start_play_request(PlayRequest::Queue {
            index: next,
            item: self.queue[next].clone(),
        });
    }

    fn start_play_request(&mut self, request: PlayRequest) {
        let (generation, cancel) = self.play_requests.begin();
        let client = QqMusicClient::with_config(self.client.config());
        let credential = self.credential.clone();
        let results_tx = self.play_results_tx.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                _ = cancel.cancelled() => return,
                result = resolve_play_request(&client, credential.as_ref(), request) => result,
            };
            let _ = results_tx.send(PlayResult { generation, result });
        });
    }

    fn finish_play(&mut self, result: PlayResult) {
        if !self.play_requests.accepts(result.generation) {
            return;
        }
        self.pending_queue_index = None;
        let resolved = match result.result {
            Ok(resolved) => resolved,
            Err(message) => {
                tracing::error!("playback resolution failed: {message}");
                return;
            }
        };
        if let Some(songs) = resolved.songs {
            self.queue = songs.iter().map(queue_item_from_search_song).collect();
        }
        let Some(queue_item) = self.queue.get_mut(resolved.index) else {
            tracing::warn!("resolved queue index out of range: {}", resolved.index);
            return;
        };
        self.queue_index = resolved.index;
        *queue_item = resolved.item;
        let mut item = queue_item.clone();
        // 供 MPRIS `xesam:url` 使用
        item.track.url = Some(resolved.uri.clone());
        let quality = quality_from_file_type(resolved.file_type);
        self.player.load(LoadRequest {
            uri: resolved.uri,
            track: item.track.clone(),
            quality,
        });
        self.current_lyrics = Some((item.mid.clone(), item.song_type));
        self.start_lyrics_load(item.mid.clone(), item.song_type);
        self.sync_capabilities();
        self.publish_queue_snapshot();
        tracing::info!(mid = item.mid, title = item.track.title, "playing");
    }

    fn toggle_play(&self) {
        self.player
            .command_sender()
            .send(PlayerCommand::TogglePlay)
            .ok();
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

    fn publish_queue_if_changed(&mut self, state: PlaybackState) {
        let key = queue_state_key(&state);
        if !queue_publication_changed(&mut self.last_queue_state, key) {
            return;
        }
        let snapshot = self.queue_snapshot_for_state(&state);
        let _ = self.events_tx.send(AppEvent::QueueUpdated(snapshot));
    }

    fn publish_queue_snapshot(&self) {
        let state = self.state_rx.borrow().clone();
        let snapshot = self.queue_snapshot_for_state(&state);
        let _ = self.events_tx.send(AppEvent::QueueUpdated(snapshot));
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

async fn resolve_play_request(
    client: &QqMusicClient,
    credential: Option<&Credential>,
    request: PlayRequest,
) -> Result<ResolvedPlayback, String> {
    match request {
        PlayRequest::Search { index, songs } => {
            let song = songs
                .get(index)
                .ok_or_else(|| format!("search index out of range: {index}"))?;
            let mut item = queue_item_from_search_song(song);
            resolve_queue_item(client, &mut item).await?;
            let (file_type, uri, ekey) = resolve_stream(
                client,
                credential,
                &item.mid,
                &item.media_mid,
                item.song_type,
            )
            .await
            .ok_or_else(|| format!("all qualities unavailable for {}", item.mid))?;
            let uri = match &ekey {
                Some(key) => hmp_media::prepare_playable(&uri, Some(key), None)
                    .await
                    .map_err(|e| format!("QMC2 decrypt failed for {}: {e}", item.mid))?,
                None => uri,
            };
            item.track.qualities = vec![quality_from_file_type(file_type)];
            Ok(ResolvedPlayback {
                index,
                songs: Some(songs),
                item,
                file_type,
                uri,
            })
        }
        PlayRequest::Queue { index, mut item } => {
            resolve_queue_item(client, &mut item).await?;
            let (file_type, uri, ekey) = resolve_stream(
                client,
                credential,
                &item.mid,
                &item.media_mid,
                item.song_type,
            )
            .await
            .ok_or_else(|| format!("all qualities unavailable for {}", item.mid))?;
            let uri = match &ekey {
                Some(key) => hmp_media::prepare_playable(&uri, Some(key), None)
                    .await
                    .map_err(|e| format!("QMC2 decrypt failed for {}: {e}", item.mid))?,
                None => uri,
            };
            item.track.qualities = vec![quality_from_file_type(file_type)];
            Ok(ResolvedPlayback {
                index,
                songs: None,
                item,
                file_type,
                uri,
            })
        }
    }
}

fn queue_item_from_search_song(song: &hmp_qqmusic_api::protocol::search::QuickSong) -> QueueItem {
    QueueItem {
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
            url: None,
            qualities: Vec::new(),
        },
        mid: song.mid.clone(),
        media_mid: String::new(),
        song_type: 0,
    }
}

async fn resolve_queue_item(client: &QqMusicClient, item: &mut QueueItem) -> Result<(), String> {
    if item.media_mid.is_empty() {
        let detail = client_music_detail(client, &item.mid)
            .await
            .map_err(|error| format!("detail failed for {}: {error}", item.mid))?;
        if detail.media_mid.is_empty() {
            return Err(format!("no media_mid for {}", item.mid));
        }
        item.media_mid = detail.media_mid;
        item.song_type = detail.song_type;
        item.track.duration = detail.duration.map(Duration::from_secs);
        // 用详情丰富元数据（MPRIS：xesam:artist/album、mpris:artUrl）
        if !detail.singers.is_empty() {
            item.track.artists = detail
                .singers
                .iter()
                .map(|(id, name)| hmp_core::ArtistRef {
                    id: hmp_core::ArtistId::new(id.clone()),
                    name: name.clone(),
                })
                .collect();
        }
        if let Some((album_id, album_name)) = detail.album {
            item.track.album = Some(hmp_core::AlbumRef {
                id: hmp_core::AlbumId::new(album_id),
                name: album_name,
            });
        }
        if let Some(pmid) = detail.cover_pmid {
            item.track.cover = Some(hmp_core::CoverRef {
                url: format!("https://y.gtimg.cn/music/photo_new/T002R300x300M000{pmid}.jpg"),
            });
        }
    }
    Ok(())
}

async fn client_music_detail(
    client: &QqMusicClient,
    mid: &str,
) -> Result<ResolvedSongDetail, hmp_qqmusic_api::QqMusicError> {
    let song_api = SongApi::new(client);
    let detail = song_api.get_detail(mid).await?;
    let singers = detail
        .track
        .singer
        .iter()
        .filter(|s| !s.name.is_empty())
        .map(|s| {
            let id = if s.mid.is_empty() {
                s.id.to_string()
            } else {
                s.mid.clone()
            };
            (id, s.name.clone())
        })
        .collect::<Vec<_>>();
    let album = (!detail.track.album.name.is_empty()).then(|| {
        (
            detail.track.album.mid.clone(),
            detail.track.album.name.clone(),
        )
    });
    let cover_pmid = (!detail.track.album.pmid.is_empty()).then(|| detail.track.album.pmid.clone());
    Ok(ResolvedSongDetail {
        media_mid: detail.track.file.media_mid.clone(),
        duration: u64::try_from(detail.track.interval).ok(),
        song_type: detail.track.type_,
        singers,
        album,
        cover_pmid,
    })
}

/// 音质回退取流：Master → HiRes → Atmos → FLAC → AAC → 320 → 128。
/// 返回 `(file_type, https_uri, optional_ekey)`；加密格式携带 ekey 供解密。
async fn resolve_stream(
    client: &QqMusicClient,
    credential: Option<&Credential>,
    mid: &str,
    media_mid: &str,
    song_type: i64,
) -> Option<(SongFileType, String, Option<String>)> {
    let song_api = SongApi::new(client);
    let info = SongFileInfo {
        mid: mid.to_owned(),
        file_type: None,
        song_type,
        media_mid: Some(media_mid.to_owned()),
    };
    for quality in AudioQuality::Master.fallback_chain() {
        let Some(file_type) = quality_to_file_type(quality.clone()) else {
            continue;
        };
        match song_api
            .get_song_urls(std::slice::from_ref(&info), file_type, credential)
            .await
        {
            Ok(response) => {
                for item in &response.data {
                    if item.result == 0 && !item.purl.is_empty() {
                        let ekey = file_type
                            .is_encrypted
                            .then(|| item.ekey.clone())
                            .filter(|k| !k.is_empty());
                        if file_type.is_encrypted && ekey.as_deref().map_or(true, str::is_empty) {
                            tracing::debug!(quality = ?quality, "encrypted stream without ekey");
                            continue;
                        }
                        let uri = format!("https://isure.stream.qqmusic.qq.com/{}", item.purl);
                        tracing::info!(quality = ?quality, "stream resolved");
                        return Some((file_type, uri, ekey));
                    }
                }
            }
            Err(error) => tracing::debug!("quality {quality:?} failed: {error}"),
        }
    }
    None
}

fn queue_item_at(queue: &[QueueItem], index: usize) -> Option<QueueItem> {
    queue.get(index).cloned()
}

fn queue_snapshot_for_state(
    queue: &[QueueItem],
    queue_index: usize,
    state: &PlaybackState,
) -> Vec<UiQueueData> {
    let current_track_id = state.current.as_ref().map(|track| track.id.as_ref());
    queue
        .iter()
        .enumerate()
        .map(|(index, item)| UiQueueData {
            track_id: item.track.id.to_string(),
            title: item.track.title.clone(),
            artist: item.track.artist_names(),
            duration: item
                .track
                .duration
                .map(|duration| format_secs(duration.as_secs_f32()))
                .unwrap_or_else(|| "--:--".into()),
            is_current: index == queue_index,
            is_playing: index == queue_index
                && current_track_id == Some(item.track.id.as_ref())
                && state.status == PlaybackStatus::Playing,
        })
        .collect()
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
    fn newer_network_requests_cancel_and_invalidate_older_results() {
        let mut state = NetworkRequestState::default();
        let (first_generation, first_token) = state.begin();
        let (second_generation, second_token) = state.begin();

        assert!(first_token.is_cancelled());
        assert!(!state.accepts(first_generation));
        assert!(state.accepts(second_generation));

        state.cancel();
        assert!(second_token.is_cancelled());
        assert!(!state.accepts(second_generation));
    }

    #[test]
    fn media_command_routing_routes_queue_and_forwards_player_commands() {
        use hmp_core::PlayerCommand;
        // Next/Previous → 队列相对移动
        assert_eq!(
            AppCore::route_media_command(&PlayerCommand::Next),
            MediaCommandRoute::QueueRelative(1)
        );
        assert_eq!(
            AppCore::route_media_command(&PlayerCommand::Previous),
            MediaCommandRoute::QueueRelative(-1)
        );
        // 其余全部转发 PlayerCore
        for cmd in [
            PlayerCommand::Play,
            PlayerCommand::Pause,
            PlayerCommand::TogglePlay,
            PlayerCommand::Stop,
            PlayerCommand::Seek(std::time::Duration::from_secs(30)),
            PlayerCommand::SetVolume(0.5),
            PlayerCommand::SetLoopMode(hmp_core::LoopMode::Track),
            PlayerCommand::SetShuffle(true),
        ] {
            assert_eq!(
                AppCore::route_media_command(&cmd),
                MediaCommandRoute::Forward,
                "{cmd:?} should be forwarded to PlayerCore"
            );
        }
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
    fn direct_snapshot_preserves_playing_state_when_watcher_key_is_unchanged() {
        let queue = vec![QueueItem {
            track: Track::new(TrackId::new("mid-1"), "First"),
            mid: "mid-1".into(),
            media_mid: "media-mid-1".into(),
            song_type: 0,
        }];
        let state = PlaybackState {
            status: PlaybackStatus::Playing,
            current: Some(queue[0].track.clone()),
            ..PlaybackState::default()
        };
        let key = queue_state_key(&state);
        let mut last_queue_state = Some(key.clone());

        assert!(!queue_publication_changed(&mut last_queue_state, key));
        let snapshot = queue_snapshot_for_state(&queue, 0, &state);
        assert!(snapshot[0].is_playing);
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
