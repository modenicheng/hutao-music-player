//! 模拟播放桥（BrowserPlayerBridge 的 Rust 对应物）：队列管理 + Timer 推进
//! 播放位置 + 状态回写 Player global。M8 数据接线时换成 AppCore/daemon 命令通道。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use slint::{ComponentHandle, Global, Model, ModelRc, Weak};

use crate::{AppWindow, Player, TrackRow};

pub struct PlayerHost {
    queue: Vec<TrackRow>,
    index: Option<usize>,
    playing: bool,
    position_ms: u64,
    volume: f32,
    timer: slint::Timer,
    ui: Weak<AppWindow>,
    prefs: Arc<Mutex<crate::prefs::Prefs>>,
}

/// qualityStore.trackMaxTierId：从音质文案推曲目最高档位
fn track_max_tier(quality: &str) -> i32 {
    if quality.contains("Hi-Res") {
        3
    } else if quality.contains("FLAC") {
        2
    } else if quality.contains("320") {
        1
    } else {
        0
    }
}

impl PlayerHost {
    /// 绑定 Player global 回调并启动 250ms 快照循环。
    /// host 内含 slint::Timer（非 Send/Sync）：模拟桥全程运行在 UI 线程，刻意主线程化。
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn bind(ui: &AppWindow, prefs: Arc<Mutex<crate::prefs::Prefs>>) -> Arc<Mutex<Self>> {
        let volume = prefs.lock().expect("prefs").volume;
        let host = Arc::new(Mutex::new(Self {
            queue: Vec::new(),
            index: None,
            playing: false,
            position_ms: 0,
            volume,
            timer: slint::Timer::default(),
            ui: ui.as_weak(),
            prefs,
        }));
        host.lock().expect("player host").push_state(false);

        let player = Player::get(ui);

        {
            let host = Arc::clone(&host);
            player.on_toggle_play(move || host.lock().expect("player host").toggle_play());
        }
        {
            let host = Arc::clone(&host);
            player.on_next(move || host.lock().expect("player host").next());
        }
        {
            let host = Arc::clone(&host);
            player.on_previous(move || host.lock().expect("player host").previous());
        }
        {
            let host = Arc::clone(&host);
            player.on_play_tracks(move |tracks, start| {
                host.lock().expect("player host").play_tracks(tracks, start);
            });
        }
        {
            let host = Arc::clone(&host);
            player.on_play_at(move |index| host.lock().expect("player host").play_at(index));
        }
        {
            let host = Arc::clone(&host);
            player.on_remove_at(move |index| host.lock().expect("player host").remove_at(index));
        }
        {
            let host = Arc::clone(&host);
            player.on_clear_queue(move || host.lock().expect("player host").clear_queue());
        }
        {
            let host = Arc::clone(&host);
            player.on_seek_percent(move |percent| {
                host.lock().expect("player host").seek_percent(percent);
            });
        }
        {
            let host = Arc::clone(&host);
            player.on_set_volume(move |volume| host.lock().expect("player host").set_volume(volume));
        }

        // 纯 UI 开关（overlay / 队列抽屉）：直接回写状态
        {
            let ui_weak = ui.as_weak();
            player.on_toggle_queue(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    let player = Player::get(&ui);
                    player.set_queue_visible(!player.get_queue_visible());
                }
            });
        }
        {
            let ui_weak = ui.as_weak();
            player.on_hide_queue(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    Player::get(&ui).set_queue_visible(false);
                }
            });
        }
        {
            let ui_weak = ui.as_weak();
            player.on_show_overlay(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    Player::get(&ui).set_overlay_visible(true);
                }
            });
        }
        {
            let ui_weak = ui.as_weak();
            player.on_hide_overlay(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    Player::get(&ui).set_overlay_visible(false);
                }
            });
        }

        // 250ms 快照推进（browserPlayerBridge 同节奏）；暂停时 tick 内部门控
        {
            // 回调持有独立的 Arc 克隆，避免与 lock 借用冲突
            let host_for_timer = Arc::clone(&host);
            host.lock().expect("player host").timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(250),
                move || {
                    if let Ok(mut host) = host_for_timer.lock() {
                        host.tick();
                    }
                },
            );
        }

        host
    }

    fn toggle_play(&mut self) {
        if self.index.is_some() {
            self.playing = !self.playing;
            self.push_state(false);
        }
    }

    fn next(&mut self) {
        let Some(index) = self.index else {
            return;
        };
        self.jump_to(index + 1);
    }

    fn previous(&mut self) {
        let Some(index) = self.index else {
            return;
        };
        // 已播超过 3s 回当前曲开头，否则上一曲（通用播放器惯例）
        if self.position_ms > 3000 {
            self.position_ms = 0;
        } else if index > 0 {
            self.jump_to(index - 1);
            return;
        }
        self.push_state(false);
    }

    fn play_tracks(&mut self, tracks: ModelRc<TrackRow>, start: i32) {
        let queue: Vec<TrackRow> = (0..tracks.row_count())
            .filter_map(|index| tracks.row_data(index))
            .collect();
        if queue.is_empty() {
            return;
        }
        let start = usize::try_from(start)
            .ok()
            .filter(|i| *i < queue.len())
            .unwrap_or(0);
        self.queue = queue;
        self.index = Some(start);
        self.playing = true;
        self.position_ms = 0;
        self.push_state(true);
    }

    fn play_at(&mut self, index: i32) {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        if index < self.queue.len() {
            self.jump_to(index);
        }
    }

    fn remove_at(&mut self, index: i32) {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        if index >= self.queue.len() {
            return;
        }
        self.queue.remove(index);
        if let Some(current) = self.index {
            if index < current {
                self.index = Some(current - 1);
            } else if index == current {
                if self.queue.is_empty() {
                    self.stop();
                    return;
                }
                // 移除当前曲：落在后继曲开头（若有）
                self.index = Some(current.min(self.queue.len() - 1));
                self.position_ms = 0;
            }
        }
        self.push_state(true);
    }

    fn clear_queue(&mut self) {
        self.queue.clear();
        self.stop();
    }

    fn stop(&mut self) {
        self.index = None;
        self.playing = false;
        self.position_ms = 0;
        self.push_state(true);
    }

    fn jump_to(&mut self, index: usize) {
        if index < self.queue.len() {
            self.index = Some(index);
            self.playing = true;
            self.position_ms = 0;
            self.push_state(false);
        } else if let Some(current) = self.index {
            // 队尾：停在该曲结束位
            self.playing = false;
            self.position_ms = self.queue[current].duration_ms.max(0) as u64;
            self.push_state(false);
        }
    }

    fn seek_percent(&mut self, percent: f32) {
        let Some(index) = self.index else {
            return;
        };
        let duration = self.queue[index].duration_ms.max(0) as u64;
        self.position_ms = (duration as f32 * percent.clamp(0.0, 1.0)) as u64;
        self.push_state(false);
    }

    fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        self.prefs.lock().expect("prefs").volume = self.volume;
        let snapshot = *self.prefs.lock().expect("prefs");
        crate::prefs::store(&snapshot);
        self.push_state(false);
    }

    /// 250ms 快照：推进位置 + 自动下一曲
    fn tick(&mut self) {
        if !self.playing {
            return;
        }
        let Some(index) = self.index else {
            return;
        };
        let duration = self.queue[index].duration_ms.max(0) as u64;
        self.position_ms += 250;
        if self.position_ms >= duration {
            if index + 1 < self.queue.len() {
                self.jump_to(index + 1);
            } else {
                self.playing = false;
                self.position_ms = duration;
                self.push_state(false);
            }
            return;
        }
        self.push_state(false);
    }

    /// 状态回写 Player global（位置/进度低频快照；queue_changed 才重建队列模型）
    fn push_state(&mut self, queue_changed: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let player = Player::get(&ui);
        let current = self.index.map(|index| &self.queue[index]);
        player.set_playing(self.playing);
        player.set_volume(self.volume);
        player.set_queue_current_mid(
            current.map(|row| row.mid.to_string()).unwrap_or_default().into(),
        );
        player.set_can_previous(self.index.is_some_and(|index| index > 0));
        player.set_can_next(
            self.index
                .is_some_and(|index| index + 1 < self.queue.len()),
        );

        if let Some(row) = current {
            let duration = row.duration_ms.max(0) as u64;
            player.set_has_track(true);
            player.set_position_ms(self.position_ms as i32);
            player.set_duration_ms(duration as i32);
            player.set_progress(if duration > 0 {
                (self.position_ms as f32 / duration as f32).clamp(0.0, 1.0)
            } else {
                0.0
            });
            player.set_title(row.title.to_string().into());
            player.set_artists(row.artists.to_string().into());
            player.set_cover(row.cover.clone());
            player.set_track_quality(row.quality.to_string().into());
            player.set_track_max_tier(track_max_tier(&row.quality));
        } else {
            player.set_has_track(false);
            player.set_playing(false);
            player.set_position_ms(0);
            player.set_duration_ms(0);
            player.set_progress(0.0);
            player.set_title("".into());
            player.set_artists("".into());
            player.set_track_quality("".into());
            player.set_track_max_tier(0);
        }

        if queue_changed {
            let total_ms: u64 = self.queue.iter().map(|row| row.duration_ms.max(0) as u64).sum();
            player.set_queue_meta(
                format!(
                    "{} 首 · 总时长 {}",
                    self.queue.len(),
                    crate::format::format_long_duration(total_ms)
                )
                .into(),
            );
            player.set_queue(ModelRc::new(slint::VecModel::from(self.queue.clone())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_mapping_matches_ts() {
        assert_eq!(track_max_tier(""), 0);
        assert_eq!(track_max_tier("320kbps MP3"), 1);
        assert_eq!(track_max_tier("FLAC · 44.1kHz"), 2);
        assert_eq!(track_max_tier("Hi-Res · 96kHz/24bit"), 3);
    }
}
