//! MPRIS 服务实现（zbus）。

use std::borrow::Cow;
use std::collections::HashMap;

use hmp_core::{LoopMode, PlaybackState, PlaybackStatus, PlayerCommand};
use tokio::sync::{mpsc, watch};
use zbus::Connection;
use zbus::fdo::Properties;
use zbus::names::InterfaceName;
use zbus::object_server::{InterfaceRef, SignalEmitter};
use zbus::zvariant::{OwnedValue, Value};

use crate::metadata;

/// MPRIS 对象路径。
pub const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
/// MPRIS bus 名。
pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.hmp";
/// Player 接口名。
pub const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
/// 根接口名。
pub const ROOT_IFACE: &str = "org.mpris.MediaPlayer2";

/// MPRIS 服务错误。
#[derive(Debug, thiserror::Error)]
pub enum MprisError {
    /// zbus 错误。
    #[error("zbus error: {0}")]
    Zbus(#[from] zbus::Error),
}

/// 根接口（`org.mpris.MediaPlayer2`）。
pub struct MprisRoot {
    identity: String,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl MprisRoot {
    #[zbus(property)]
    fn identity(&self) -> &str {
        &self.identity
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        "hmp"
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<&str> {
        vec!["https", "http", "file"]
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<&str> {
        vec![]
    }

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
}

/// Player 接口（`org.mpris.MediaPlayer2.Player`）。
///
/// 状态字段由同步任务更新；方法把控制意图转换为 [`PlayerCommand`] 发出
/// （队列/下一首等由上层应用核心消费）。
pub struct MprisPlayer {
    cmd_tx: mpsc::UnboundedSender<PlayerCommand>,
    playback_status: String,
    loop_status: String,
    shuffle: bool,
    volume: f64,
    position_us: i64,
    metadata: HashMap<String, OwnedValue>,
    rate: f64,
    minimum_rate: f64,
    maximum_rate: f64,
    can_go_next: bool,
    can_go_previous: bool,
    can_play: bool,
    can_pause: bool,
    can_seek: bool,
    can_control: bool,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl MprisPlayer {
    /// 下一首（上层队列核心处理）。
    fn next(&self) {
        let _ = self.cmd_tx.send(PlayerCommand::Next);
    }

    /// 上一首。
    fn previous(&self) {
        let _ = self.cmd_tx.send(PlayerCommand::Previous);
    }

    /// 暂停。
    fn pause(&self) {
        let _ = self.cmd_tx.send(PlayerCommand::Pause);
    }

    /// 播放/暂停切换。
    fn play_pause(&self) {
        let cmd = if self.playback_status == "Playing" {
            PlayerCommand::Pause
        } else {
            PlayerCommand::Play
        };
        let _ = self.cmd_tx.send(cmd);
    }

    /// 停止。
    fn stop(&self) {
        let _ = self.cmd_tx.send(PlayerCommand::Stop);
    }

    /// 播放。
    fn play(&self) {
        let _ = self.cmd_tx.send(PlayerCommand::Play);
    }

    /// 相对跳转（偏移微秒，MPRIS spec）。
    async fn seek(&mut self, offset_us: i64) -> zbus::fdo::Result<()> {
        let new_pos = (self.position_us + offset_us).max(0);
        self.position_us = new_pos;
        let _ = self
            .cmd_tx
            .send(PlayerCommand::Seek(std::time::Duration::from_micros(
                new_pos as u64,
            )));
        Ok(())
    }

    /// 绝对定位（轨道 ID 校验由上层完成）。
    async fn set_position(
        &mut self,
        _track_id: zbus::zvariant::ObjectPath<'_>,
        position_us: i64,
    ) -> zbus::fdo::Result<()> {
        let pos = position_us.max(0);
        self.position_us = pos;
        let _ = self
            .cmd_tx
            .send(PlayerCommand::Seek(std::time::Duration::from_micros(
                pos as u64,
            )));
        Ok(())
    }

    /// 打开 URI（当前不支持，返回错误）。
    fn open_uri(&self, _uri: &str) -> zbus::fdo::Result<()> {
        Err(zbus::fdo::Error::NotSupported("URI 播放暂不支持".into()))
    }

    /// Seeked 信号（播放器核心 seek 后由同步任务发出）。
    #[zbus(signal)]
    async fn seeked(
        &self,
        _signal_emitter: &SignalEmitter<'_>,
        position_us: i64,
    ) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> &str {
        &self.playback_status
    }

    /// LoopStatus 可写（客户端 Set → 播放器循环模式）。
    #[zbus(property)]
    fn set_loop_status(&mut self, val: &str) -> zbus::fdo::Result<()> {
        let mode = match val {
            "Track" => LoopMode::Track,
            "Playlist" => LoopMode::List,
            _ => LoopMode::None,
        };
        self.loop_status = val.to_owned();
        let _ = self.cmd_tx.send(PlayerCommand::SetLoopMode(mode));
        Ok(())
    }

    #[zbus(property)]
    fn loop_status(&self) -> &str {
        &self.loop_status
    }

    /// Shuffle 可写（上层队列核心处理）。
    #[zbus(property)]
    fn set_shuffle(&mut self, val: bool) -> zbus::fdo::Result<()> {
        self.shuffle = val;
        let _ = self.cmd_tx.send(PlayerCommand::SetShuffle(val));
        Ok(())
    }

    #[zbus(property)]
    fn shuffle(&self) -> bool {
        self.shuffle
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        self.rate
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        self.minimum_rate
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        self.maximum_rate
    }

    /// Volume 可写（客户端 Set → 播放器音量）。
    #[zbus(property)]
    fn set_volume(&mut self, val: f64) -> zbus::fdo::Result<()> {
        let clamped = val.clamp(0.0, 1.0);
        self.volume = clamped;
        let _ = self.cmd_tx.send(PlayerCommand::SetVolume(clamped));
        Ok(())
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.volume
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        self.position_us
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        self.metadata.clone()
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.can_go_next
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.can_go_previous
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.can_play
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.can_pause
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.can_seek
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        self.can_control
    }
}

/// MPRIS 服务句柄。
pub struct MprisService {
    connection: Connection,
    _sync_task: tokio::task::JoinHandle<()>,
}

impl MprisService {
    /// 启动 MPRIS 服务：注册 bus 名 + 接口，并同步 `PlaybackState`。
    ///
    /// `cmd_tx` 为播放器命令发送端（`PlayerCore` 暴露）；
    /// `state_rx` 为播放状态订阅。
    pub async fn start(
        cmd_tx: mpsc::UnboundedSender<PlayerCommand>,
        state_rx: watch::Receiver<PlaybackState>,
    ) -> Result<Self, MprisError> {
        let connection = zbus::connection::Builder::session()?
            .name(BUS_NAME.to_owned())?
            .serve_at(
                OBJECT_PATH,
                MprisRoot {
                    identity: "胡桃音乐播放器".into(),
                },
            )?
            .serve_at(
                OBJECT_PATH,
                MprisPlayer {
                    cmd_tx: cmd_tx.clone(),
                    playback_status: "Stopped".into(),
                    loop_status: "None".into(),
                    shuffle: false,
                    volume: 1.0,
                    position_us: 0,
                    metadata: HashMap::new(),
                    rate: 1.0,
                    minimum_rate: 1.0,
                    maximum_rate: 1.0,
                    can_go_next: false,
                    can_go_previous: false,
                    can_play: false,
                    can_pause: false,
                    can_seek: false,
                    can_control: true,
                },
            )?
            .build()
            .await?;

        let sync_task = tokio::spawn(sync_loop(connection.clone(), state_rx));

        Ok(Self {
            connection,
            _sync_task: sync_task,
        })
    }

    /// 返回底层连接（测试/调试用）。
    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

/// 状态同步循环：`PlaybackState` → MPRIS 属性 + PropertiesChanged/Seeked。
async fn sync_loop(connection: Connection, mut state_rx: watch::Receiver<PlaybackState>) {
    // 首次立即同步
    {
        let state = state_rx.borrow().clone();
        publish_state(&connection, &state).await;
    }
    while state_rx.changed().await.is_ok() {
        let state = state_rx.borrow().clone();
        publish_state(&connection, &state).await;
    }
}

/// 把一份状态发布到 MPRIS 接口。
async fn publish_state(connection: &Connection, state: &PlaybackState) {
    let server = connection.object_server();
    let Ok(player_ref) = server.interface::<_, MprisPlayer>(OBJECT_PATH).await else {
        return;
    };

    // 1) Seeked 信号（position 变化且状态为播放/暂停时）
    if state.position.as_micros() as i64 != player_ref.get().await.position_us {
        let position_us = state.position.as_micros().min(i64::MAX as u128) as i64;
        player_ref.get_mut().await.update_position(position_us);
        if matches!(
            state.status,
            PlaybackStatus::Playing | PlaybackStatus::Paused
        ) {
            let _ = emit_seeked(&player_ref, position_us).await;
        }
    }

    // 2) 可写属性 → PropertiesChanged
    let mut changed: HashMap<&str, Value> = HashMap::new();
    {
        let mut iface = player_ref.get_mut().await;
        iface.update_props(state, &mut changed);
    }
    let _ = emit_properties_changed(player_ref, changed).await;
}

/// 更新 Player 接口状态字段，收集变更属性。
impl MprisPlayer {
    fn update_position(&mut self, position_us: i64) {
        self.position_us = position_us;
    }

    fn update_props(&mut self, state: &PlaybackState, changed: &mut HashMap<&str, Value>) {
        let status = metadata::playback_status(state.status);
        if self.playback_status != status {
            self.playback_status = status.to_owned();
            changed.insert("PlaybackStatus", Value::from(status));
        }
        let loop_status = metadata::loop_status(state.loop_mode);
        if self.loop_status != loop_status {
            self.loop_status = loop_status.to_owned();
            changed.insert("LoopStatus", Value::from(loop_status));
        }
        if (self.volume - state.volume).abs() > 1e-9 {
            self.volume = state.volume;
            changed.insert("Volume", Value::F64(state.volume));
        }
        let can_seek = state.can_seek;
        if self.can_seek != can_seek {
            self.can_seek = can_seek;
            changed.insert("CanSeek", Value::Bool(can_seek));
        }
        let can_play = !matches!(
            state.status,
            PlaybackStatus::Empty | PlaybackStatus::Stopped
        );
        if self.can_play != can_play {
            self.can_play = can_play;
            changed.insert("CanPlay", Value::Bool(can_play));
        }
        let can_pause = matches!(
            state.status,
            PlaybackStatus::Playing | PlaybackStatus::Paused
        );
        if self.can_pause != can_pause {
            self.can_pause = can_pause;
            changed.insert("CanPause", Value::Bool(can_pause));
        }
        // Metadata 变更
        if let Some(track) = &state.current {
            let new_meta: HashMap<String, OwnedValue> = metadata::metadata_from_track(track)
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect();
            if self.metadata != new_meta {
                self.metadata = new_meta;
                changed.insert("Metadata", Value::from(self.metadata.clone()));
            }
        } else if !self.metadata.is_empty() {
            self.metadata = HashMap::new();
            changed.insert(
                "Metadata",
                Value::from(HashMap::<String, OwnedValue>::new()),
            );
        }
    }
}

async fn emit_properties_changed(
    player_ref: InterfaceRef<MprisPlayer>,
    changed: HashMap<&str, Value<'_>>,
) {
    if changed.is_empty() {
        return;
    }
    let emitter = player_ref.signal_emitter();
    let iface = InterfaceName::try_from(PLAYER_IFACE).expect("static iface name");
    let _ = Properties::properties_changed(emitter, iface, changed, Cow::Borrowed(&[])).await;
}

async fn emit_seeked(player_ref: &InterfaceRef<MprisPlayer>, position_us: i64) {
    // Position 属性失效 + 发 Seeked 信号（spec 要求）
    let emitter = player_ref.signal_emitter();
    let _ = Properties::properties_changed(
        emitter,
        InterfaceName::try_from(PLAYER_IFACE).expect("static"),
        HashMap::new(),
        Cow::Borrowed(&["Position"]),
    )
    .await;
    let _ = player_ref.seeked(emitter, position_us).await;
    // 同步任务需要看到新位置（避免重复发 Seeked）——字段已在 publish 中更新
}
