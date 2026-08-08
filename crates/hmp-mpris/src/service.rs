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
    /// 用户 seek 意图（Seek/SetPosition 置位，位置变化时消费并发 Seeked）。
    pending_seek: bool,
    /// OpenUri 转发通道（上层播放 URI；None = 不支持）。
    open_uri_tx: Option<mpsc::UnboundedSender<String>>,
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
    async fn seek(&mut self, offset_us: i64) -> zbus::fdo::Result<()> {
        let new_pos = (self.position_us + offset_us).max(0);
        self.position_us = new_pos;
        self.pending_seek = true;
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
        self.pending_seek = true;
        let _ = self
            .cmd_tx
            .send(PlayerCommand::Seek(std::time::Duration::from_micros(
                pos as u64,
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
                    pending_seek: false,
                    open_uri_tx,
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
}
