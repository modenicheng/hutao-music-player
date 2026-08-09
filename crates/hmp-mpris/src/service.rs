//! MPRIS 服务实现（zbus）。

use std::borrow::Cow;
use std::collections::HashMap;

use hmp_core::{LoopMode, PlaybackCapabilities, PlaybackState, PlaybackStatus, PlayerCommand};
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
    /// Quit 转发通道（上层 daemon 退出；None = 不支持）。
    quit_tx: Option<mpsc::UnboundedSender<()>>,
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
        // 与 daemon 实际能力一致：仅本地文件（MPRIS OpenUri 只接受 file://）。
        vec!["file"]
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<&str> {
        // 与本地扫描器 is_audio_ext 对齐（GStreamer/lofty 实际支持的容器）。
        vec![
            "audio/mpeg",
            "audio/flac",
            "audio/ogg",
            "audio/mp4",
            "audio/wav",
            "audio/aac",
            "audio/x-ape",
            "audio/x-aiff",
        ]
    }

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        // daemon 提供 `hmp quit`（Request::Quit）：有转发通道即可退出。
        self.quit_tx.is_some()
    }

    /// 退出播放器（转发到上层 `Request::Quit`）。
    fn quit(&self) -> zbus::fdo::Result<()> {
        match &self.quit_tx {
            Some(tx) => tx
                .send(())
                .map_err(|_| zbus::fdo::Error::Failed("退出转发通道关闭".into())),
            None => Err(zbus::fdo::Error::NotSupported("退出不可用".into())),
        }
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
    /// 用户 seek 意图（Seek/SetPosition 置位，位置变化时消费并发 Seeked）。
    pending_seek: bool,
    /// OpenUri 转发通道（上层播放 URI；None = 不支持）。
    open_uri_tx: Option<mpsc::UnboundedSender<String>>,
    /// 当前曲目领域 id（SetPosition 的 stale TrackId 校验）。
    current_track_id: Option<String>,
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
        let _ = self.cmd_tx.send(PlayerCommand::TogglePlay);
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
    /// 越过曲尾 → 等价于下一首（spec）；否则 clamp 到 [0, 曲长] 后 Seek。
    async fn seek(&mut self, offset_us: i64) -> zbus::fdo::Result<()> {
        let new_pos = self.position_us + offset_us;
        if let Some(len) = self.length_us() {
            if new_pos > len {
                self.pending_seek = false;
                let _ = self.cmd_tx.send(PlayerCommand::Next);
                return Ok(());
            }
        }
        let new_pos = new_pos.max(0);
        self.position_us = new_pos;
        self.pending_seek = true;
        let _ = self
            .cmd_tx
            .send(PlayerCommand::Seek(std::time::Duration::from_micros(
                new_pos as u64,
            )));
        Ok(())
    }

    /// 绝对定位。MPRIS spec：stale TrackId 忽略；位置 <0 或超过曲长不处理；
    /// CanSeek=false 时返回 NotSupported。
    async fn set_position(
        &mut self,
        track_id: zbus::zvariant::ObjectPath<'_>,
        position_us: i64,
    ) -> zbus::fdo::Result<()> {
        // stale TrackId：与当前曲目对象路径不一致 → 忽略。
        if let Some(cur) = &self.current_track_id {
            let expected = metadata::track_id_object_path(cur);
            if expected.as_str() != track_id.as_str() {
                return Ok(());
            }
        }
        if !self.can_seek {
            return Err(zbus::fdo::Error::NotSupported("当前曲目不支持跳转".into()));
        }
        if position_us < 0 {
            return Ok(());
        }
        if let Some(len) = self.length_us() {
            if position_us > len {
                return Ok(());
            }
        }
        self.position_us = position_us;
        self.pending_seek = true;
        let _ = self
            .cmd_tx
            .send(PlayerCommand::Seek(std::time::Duration::from_micros(
                position_us as u64,
            )));
        Ok(())
    }

    /// 打开 URI：转发到上层（`open_uri_tx` 存在时）；无转发通道 → NotSupported。
    fn open_uri(&self, uri: &str) -> zbus::fdo::Result<()> {
        match &self.open_uri_tx {
            Some(tx) => {
                tx.send(uri.to_string())
                    .map_err(|_| zbus::fdo::Error::Failed("转发通道关闭".into()))?;
                Ok(())
            }
            None => Err(zbus::fdo::Error::NotSupported("URI 播放暂不支持".into())),
        }
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

    /// Rate 可写（MPRIS 定义为 Read/Write）；播放器仅支持 1.0x，
    /// 其余取值按规范返回 NotSupported。
    #[zbus(property)]
    fn set_rate(&mut self, rate: f64) -> zbus::fdo::Result<()> {
        if (rate - 1.0).abs() > f64::EPSILON {
            return Err(zbus::fdo::Error::NotSupported(
                "仅支持 1.0x 播放速率".into(),
            ));
        }
        self.rate = rate;
        Ok(())
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
        Self::start_with_capabilities(cmd_tx, state_rx, None).await
    }

    /// 启动 MPRIS 服务，并同步队列能力（`CanGoNext`/`CanGoPrevious`）。
    ///
    /// `capabilities_rx` 由上层队列核心发布（`None` 时能力恒为 false，
    /// 适用于无队列语义的调用方，如 CLI）。
    pub async fn start_with_capabilities(
        cmd_tx: mpsc::UnboundedSender<PlayerCommand>,
        state_rx: watch::Receiver<PlaybackState>,
        capabilities_rx: Option<watch::Receiver<PlaybackCapabilities>>,
    ) -> Result<Self, MprisError> {
        Self::start_with_capabilities_and_uri(cmd_tx, state_rx, capabilities_rx, None).await
    }

    /// 启动 MPRIS 服务并挂载 OpenUri 转发通道（C4）。
    pub async fn start_with_capabilities_and_uri(
        cmd_tx: mpsc::UnboundedSender<PlayerCommand>,
        state_rx: watch::Receiver<PlaybackState>,
        capabilities_rx: Option<watch::Receiver<PlaybackCapabilities>>,
        open_uri_tx: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<Self, MprisError> {
        Self::start_with_capabilities_uri_and_quit(
            cmd_tx,
            state_rx,
            capabilities_rx,
            open_uri_tx,
            None,
        )
        .await
    }

    /// 启动 MPRIS 服务并挂载 OpenUri 与 Quit 转发通道。
    /// `quit_tx` 存在时 CanQuit=true 且根接口 `Quit` 方法可用（上层转发
    /// `Request::Quit`）；None 时 CanQuit=false。
    pub async fn start_with_capabilities_uri_and_quit(
        cmd_tx: mpsc::UnboundedSender<PlayerCommand>,
        state_rx: watch::Receiver<PlaybackState>,
        capabilities_rx: Option<watch::Receiver<PlaybackCapabilities>>,
        open_uri_tx: Option<mpsc::UnboundedSender<String>>,
        quit_tx: Option<mpsc::UnboundedSender<()>>,
    ) -> Result<Self, MprisError> {
        let connection = zbus::connection::Builder::session()?
            .name(BUS_NAME.to_owned())?
            .serve_at(
                OBJECT_PATH,
                MprisRoot {
                    identity: "胡桃音乐播放器".into(),
                    quit_tx: quit_tx.clone(),
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
                    pending_seek: false,
                    open_uri_tx,
                    current_track_id: None,
                },
            )?
            .build()
            .await?;

        let sync_task = tokio::spawn(sync_loop(connection.clone(), state_rx, capabilities_rx));

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

/// 状态同步循环：`PlaybackState` → MPRIS 属性 + PropertiesChanged/Seeked；
/// `PlaybackCapabilities` → CanGoNext/CanGoPrevious。
async fn sync_loop(
    connection: Connection,
    mut state_rx: watch::Receiver<PlaybackState>,
    capabilities_rx: Option<watch::Receiver<PlaybackCapabilities>>,
) {
    // 首次立即同步
    {
        let state = state_rx.borrow().clone();
        publish_state(&connection, &state).await;
    }
    let Some(mut capabilities_rx) = capabilities_rx else {
        while state_rx.changed().await.is_ok() {
            let state = state_rx.borrow().clone();
            publish_state(&connection, &state).await;
        }
        return;
    };
    {
        let caps = *capabilities_rx.borrow();
        publish_capabilities(&connection, &caps).await;
    }
    loop {
        tokio::select! {
            changed = state_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let state = state_rx.borrow().clone();
                publish_state(&connection, &state).await;
            }
            changed = capabilities_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let caps = *capabilities_rx.borrow();
                publish_capabilities(&connection, &caps).await;
            }
        }
    }
}

/// 把队列能力发布到 MPRIS `CanGoNext`/`CanGoPrevious`。
async fn publish_capabilities(connection: &Connection, caps: &PlaybackCapabilities) {
    let server = connection.object_server();
    let Ok(player_ref) = server.interface::<_, MprisPlayer>(OBJECT_PATH).await else {
        return;
    };
    let mut changed: HashMap<&str, Value> = HashMap::new();
    {
        let mut iface = player_ref.get_mut().await;
        if iface.can_go_next != caps.can_go_next {
            iface.can_go_next = caps.can_go_next;
            changed.insert("CanGoNext", Value::Bool(caps.can_go_next));
        }
        if iface.can_go_previous != caps.can_go_previous {
            iface.can_go_previous = caps.can_go_previous;
            changed.insert("CanGoPrevious", Value::Bool(caps.can_go_previous));
        }
    }
    emit_properties_changed(player_ref, changed).await;
}

/// 把一份状态发布到 MPRIS 接口。
async fn publish_state(connection: &Connection, state: &PlaybackState) {
    let server = connection.object_server();
    let Ok(player_ref) = server.interface::<_, MprisPlayer>(OBJECT_PATH).await else {
        return;
    };

    // 1) Position 更新。Seeked 信号仅在**用户 seek 后**发出，
    //    不随普通进度 tick 连续发送——MPRIS spec：Seeked 表示不连续位置跳变。
    let position_us = state.position.as_micros().min(i64::MAX as u128) as i64;
    let emit_seek = {
        let mut iface = player_ref.get_mut().await;
        iface.apply_position(position_us)
    };
    if emit_seek {
        let _ = emit_seeked(&player_ref, position_us).await;
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

    /// 当前曲长（微秒；无 metadata 时 None）。
    fn length_us(&self) -> Option<i64> {
        self.metadata
            .get("mpris:length")
            .and_then(|v| v.downcast_ref::<i64>().ok())
    }

    /// 处理一次位置同步：更新 position；返回是否应发 Seeked。
    /// 规则：仅当位置**变化**且存在用户 seek 意图（pending_seek）时才发——
    /// 普通播放进度 tick 不发送 Seeked（MPRIS spec：Seeked = 不连续跳变）。
    /// 位置未变化时保留 pending（seek 尚未被驱动应用，等下一次同步）。
    fn apply_position(&mut self, position_us: i64) -> bool {
        if self.position_us == position_us {
            return false;
        }
        self.update_position(position_us);
        let emit = self.pending_seek;
        self.pending_seek = false;
        emit
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
        if self.shuffle != state.shuffle {
            self.shuffle = state.shuffle;
            changed.insert("Shuffle", Value::Bool(state.shuffle));
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
        let can_play = !matches!(state.status, PlaybackStatus::Empty);
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
            // SetPosition 的 stale TrackId 校验基准（领域 id）。
            if self.current_track_id.as_deref() != Some(track.id.as_ref()) {
                self.current_track_id = Some(track.id.0.clone());
            }
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
            self.current_track_id = None;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn iface() -> MprisPlayer {
        let (tx, _rx) = mpsc::unbounded_channel();
        MprisPlayer {
            cmd_tx: tx,
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
            pending_seek: false,
            open_uri_tx: None,
            current_track_id: None,
        }
    }

    fn state(shuffle: bool) -> PlaybackState {
        PlaybackState {
            shuffle,
            ..Default::default()
        }
    }

    /// Shuffle 属性从 PlaybackState 回同步（旧代码漏同步：hmp shuffle on 后
    /// 桌面 Shell 仍显示 false）。
    #[test]
    fn update_props_syncs_shuffle_on() {
        let mut iface = iface();
        let mut changed: HashMap<&str, Value> = HashMap::new();
        iface.update_props(&state(true), &mut changed);
        assert!(iface.shuffle);
        assert_eq!(changed.get("Shuffle"), Some(&Value::Bool(true)));
    }

    #[test]
    fn update_props_syncs_shuffle_off() {
        let mut iface = iface();
        iface.shuffle = true;
        let mut changed: HashMap<&str, Value> = HashMap::new();
        iface.update_props(&state(false), &mut changed);
        assert!(!iface.shuffle);
        assert_eq!(changed.get("Shuffle"), Some(&Value::Bool(false)));
    }

    /// 无变化时不产生属性变更。
    #[test]
    fn update_props_noop_when_shuffle_unchanged() {
        let mut iface = iface();
        let mut changed: HashMap<&str, Value> = HashMap::new();
        iface.update_props(&state(false), &mut changed);
        assert!(!changed.contains_key("Shuffle"));
    }

    /// OpenUri：有转发通道 → 送达；无 → NotSupported（与 SupportedUriSchemes 一致）。
    #[test]
    fn open_uri_forwards_when_channel_present() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut iface = iface();
        iface.open_uri_tx = Some(tx);
        iface.open_uri("file:///tmp/x.mp3").unwrap();
        assert_eq!(rx.try_recv().unwrap(), "file:///tmp/x.mp3");
        // 通道仍通，后续 URI 不报错。
        iface.open_uri("https://x/1.mp3").unwrap();
        assert_eq!(rx.try_recv().unwrap(), "https://x/1.mp3");
    }

    #[test]
    fn open_uri_not_supported_without_channel() {
        let iface = iface();
        assert!(iface.open_uri("file:///tmp/x.mp3").is_err());
    }

    /// Seeked 语义：普通进度 tick 不发 Seeked；用户 seek 后位置变化才发一次。
    #[test]
    fn seeked_only_after_user_seek() {
        let mut iface = iface();
        // 普通 tick：位置前进 → 不发 Seeked。
        assert!(!iface.apply_position(1_000_000));
        assert_eq!(iface.position_us, 1_000_000);
        assert!(!iface.apply_position(2_000_000), "进度 tick 不得发 Seeked");

        // 用户 seek：置 pending → 位置变化 → 发一次并消费。
        iface.pending_seek = true;
        assert!(iface.apply_position(60_000_000), "seek 后应发 Seeked");
        assert!(!iface.apply_position(61_000_000), "Seeked 只发一次");
    }

    /// Seek 尚未被驱动应用（位置未变）→ pending 保留，位置变化后再发。
    #[test]
    fn pending_seek_survives_until_position_moves() {
        let mut iface = iface();
        iface.pending_seek = true;
        assert!(!iface.apply_position(0), "位置未变不发");
        assert!(iface.pending_seek, "pending 应保留至位置变化");
        assert!(iface.apply_position(5_000_000));
        assert!(!iface.pending_seek);
    }

    /// MPRIS spec：SetPosition 传入 stale TrackId → 忽略（不 Seek、不置 pending）。
    #[tokio::test]
    async fn set_position_ignores_stale_track_id() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut iface = iface();
        iface.cmd_tx = tx;
        iface.can_seek = true;
        iface.current_track_id = Some("qq:mid-a".into());
        iface.metadata.insert(
            "mpris:length".into(),
            OwnedValue::try_from(Value::from(100_000_000i64)).unwrap(),
        );
        let stale = metadata::track_id_object_path("qq:mid-other");
        iface.set_position(stale, 50_000_000).await.unwrap();
        assert!(!iface.pending_seek, "stale TrackId 不得触发 Seek");
        assert!(rx.try_recv().is_err(), "不得向驱动发 Seek");
    }

    /// MPRIS spec：SetPosition 位置 <0 或超过曲长 → 不处理。
    #[tokio::test]
    async fn set_position_rejects_negative_and_beyond_length() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut iface = iface();
        iface.cmd_tx = tx;
        iface.can_seek = true;
        iface.current_track_id = Some("qq:mid-a".into());
        iface.metadata.insert(
            "mpris:length".into(),
            OwnedValue::try_from(Value::from(100_000_000i64)).unwrap(),
        );
        let path = metadata::track_id_object_path("qq:mid-a");
        iface.set_position(path.clone(), -1).await.unwrap();
        assert!(!iface.pending_seek, "负位置不处理");
        iface.set_position(path, 100_000_001).await.unwrap();
        assert!(!iface.pending_seek, "超过曲长不处理");
        assert!(rx.try_recv().is_err());
    }

    /// SetPosition 合法路径：匹配 TrackId + 范围内 + CanSeek → 发 Seek。
    #[tokio::test]
    async fn set_position_seeks_when_valid() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut iface = iface();
        iface.cmd_tx = tx;
        iface.can_seek = true;
        iface.current_track_id = Some("qq:mid-a".into());
        iface.metadata.insert(
            "mpris:length".into(),
            OwnedValue::try_from(Value::from(100_000_000i64)).unwrap(),
        );
        let path = metadata::track_id_object_path("qq:mid-a");
        iface.set_position(path, 30_000_000).await.unwrap();
        assert!(iface.pending_seek);
        assert_eq!(iface.position_us, 30_000_000);
        assert!(matches!(
            rx.try_recv().unwrap(),
            PlayerCommand::Seek(d) if d.as_micros() == 30_000_000
        ));
    }

    /// SetPosition：CanSeek=false → NotSupported（spec：不可跳转时拒绝）。
    #[tokio::test]
    async fn set_position_rejects_when_cannot_seek() {
        let mut iface = iface();
        iface.current_track_id = Some("qq:mid-a".into());
        let path = metadata::track_id_object_path("qq:mid-a");
        assert!(iface.set_position(path, 1_000).await.is_err());
    }

    /// MPRIS spec：Seek 越过曲尾 → 等价于 Next（不再发 Seek）。
    #[tokio::test]
    async fn seek_beyond_length_sends_next() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut iface = iface();
        iface.cmd_tx = tx;
        iface.position_us = 90_000_000;
        iface.metadata.insert(
            "mpris:length".into(),
            OwnedValue::try_from(Value::from(100_000_000i64)).unwrap(),
        );
        iface.seek(20_000_000).await.unwrap(); // 90s + 20s > 100s
        assert!(matches!(rx.try_recv().unwrap(), PlayerCommand::Next));
        assert!(!iface.pending_seek);
    }

    /// Rate setter：仅接受 1.0；其余取值 NotSupported。
    #[tokio::test]
    async fn set_rate_accepts_only_1x() {
        let mut iface = iface();
        iface.set_rate(1.0).unwrap();
        assert_eq!(iface.rate, 1.0);
        assert!(iface.set_rate(2.0).is_err());
        assert!(iface.set_rate(0.5).is_err());
        assert_eq!(iface.rate, 1.0, "拒绝时不得改 rate");
    }

    /// CanPlay：Stopped（Stop 保留当前曲）仍可 Play；仅 Empty 为 false。
    #[test]
    fn can_play_true_when_stopped_with_track() {
        let mut iface = iface();
        let mut changed: HashMap<&str, Value> = HashMap::new();
        let st = PlaybackState {
            status: PlaybackStatus::Stopped,
            current: Some(hmp_core::Track::new(
                hmp_core::TrackId::new("qq:mid-a"),
                "a",
            )),
            ..Default::default()
        };
        iface.update_props(&st, &mut changed);
        assert!(iface.can_play, "Stopped 有曲时应可 Play");
        // Empty（无曲）→ false。
        let mut changed: HashMap<&str, Value> = HashMap::new();
        let st2 = PlaybackState {
            status: PlaybackStatus::Empty,
            current: None,
            ..Default::default()
        };
        iface.update_props(&st2, &mut changed);
        assert!(!iface.can_play, "Empty 不可 Play");
    }

    /// 根接口 Quit：有转发通道 → 送达；无 → NotSupported。
    #[test]
    fn root_quit_forwards_when_channel_present() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let root = MprisRoot {
            identity: "t".into(),
            quit_tx: Some(tx),
        };
        assert!(root.can_quit());
        root.quit().unwrap();
        assert!(rx.try_recv().is_ok());
        let root = MprisRoot {
            identity: "t".into(),
            quit_tx: None,
        };
        assert!(!root.can_quit());
        assert!(root.quit().is_err());
    }

    /// MIME 列表与本地扫描器支持对齐（is_audio_ext 同集合）。
    #[test]
    fn mime_types_cover_local_formats() {
        let root = MprisRoot {
            identity: "t".into(),
            quit_tx: None,
        };
        let mimes = root.supported_mime_types();
        for m in [
            "audio/mpeg",
            "audio/flac",
            "audio/ogg",
            "audio/wav",
            "audio/x-ape",
            "audio/x-aiff",
        ] {
            assert!(mimes.contains(&m), "缺少 {m}");
        }
    }
}
