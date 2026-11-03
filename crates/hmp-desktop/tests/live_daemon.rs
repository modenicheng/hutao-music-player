//! 真机 daemon 的播放操作 E2E（#[ignore]：需要本机正在运行 `hmp serve`，
//! CI 无 daemon 故跳过）。验证审计重构后的完整链路：订阅状态落地
//! （标题/能力/队列投影）→ 按钮回调 → PlayList IPC → daemon 生效 → 推送回写。
//!
//! 运行：`cargo test -p hmp-desktop --test live_daemon -- --ignored`
//! 前置：`./target/debug/hmp serve &`（跑过 playlist_smoke 建立多曲队列更佳）。
//!
//! 线程模型：测试线程跑 slint 事件循环（订阅经 invoke_from_event_loop 投递，
//! 须有运行中的循环）；driver 线程编排时序、经 invoke_from_event_loop 读写
//! Player global、直接走 Status IPC 交叉验证 daemon；断言失败记入 failures，
//! quit_event_loop 收尾后统一判。testing backend 单例 → 本文件仅一个 #[test]。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Global, Model, Weak};

use hmp_desktop::{AppWindow, Player, backend, bridge, player_bridge, prefs};

type Failures = Arc<Mutex<Vec<String>>>;

fn fail(failures: &Failures, msg: String) {
    failures.lock().unwrap().push(msg);
}

/// driver 线程内等待条件（真实时间；订阅推送 ~10Hz，秒级条件足够）。
fn wait_for(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("等待超时: {what}");
    false
}

/// daemon 复合状态（Status IPC——与 UI 订阅独立的通道，交叉验证用）。
fn daemon_status() -> hmp_core::ipc::DaemonState {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async {
        match backend::request(hmp_core::ipc::Request::Status).await {
            Ok(hmp_core::ipc::Response::Status(s)) => s,
            other => panic!("daemon 不可达（先 `hmp serve`）: {other:?}"),
        }
    })
}

/// 在 UI 线程执行闭包并取回结果（invoke_from_event_loop 无返回值 → channel 中转）。
fn on_ui<T: Send + 'static>(
    ui: &Weak<AppWindow>,
    f: impl FnOnce(&AppWindow) -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ui = ui.clone();
    slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui.upgrade() {
            let _ = tx.send(f(&ui));
        }
    })
    .ok()?;
    rx.recv_timeout(Duration::from_secs(2)).ok()
}

#[test]
#[ignore = "需要本机 daemon（hmp serve）与本地媒体库"]
fn live_daemon_playback_operations() {
    i_slint_backend_testing::init_integration_test_with_system_time();

    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    let runtime = Arc::new(backend::BackendRuntime::new().unwrap());
    bridge::load_data(&ui, &runtime);
    let prefs = Arc::new(Mutex::new(prefs::load()));
    player_bridge::bind(&ui, Arc::clone(&runtime), prefs);

    let ui_weak = ui.as_weak();
    let failures: Failures = Arc::new(Mutex::new(Vec::new()));

    // —— driver：编排时序与断言，结束退出事件循环 ——
    let driver_failures = Arc::clone(&failures);
    std::thread::spawn(move || {
        let failures = driver_failures;

        // 1. 订阅状态落地：标题/能力/队列与 daemon 一致。
        let st = daemon_status();
        if st.queue.len <= 1 {
            fail(
                &failures,
                "前置失败：队列应有多首（先跑 playlist_smoke）".into(),
            );
            slint::quit_event_loop().unwrap();
            return;
        }
        let daemon_title = st.playback.current.as_ref().unwrap().title.clone();
        let ok = wait_for(
            "订阅首帧（标题落地）",
            Duration::from_secs(8),
            || {
                on_ui(&ui_weak, |ui| Player::get(ui).get_title().to_string())
                    .map(|t| t == daemon_title)
                    .unwrap_or(false)
            },
        );
        if !ok {
            fail(
                &failures,
                format!("订阅标题未落地（期望 {daemon_title:?}）"),
            );
        }
        let (has_track, can_prev, can_next, queue_len) = on_ui(&ui_weak, |ui| {
            let p = Player::get(ui);
            (
                p.get_has_track(),
                p.get_can_previous(),
                p.get_can_next(),
                p.get_queue().row_count(),
            )
        })
        .expect("读取 Player 状态");
        if !has_track {
            fail(&failures, "有当前曲：has_track 应为 true".into());
        }
        if !can_prev || !can_next {
            fail(&failures, "多曲队列：上一曲/下一曲能力应为 true".into());
        }
        if queue_len != st.queue.len {
            fail(
                &failures,
                format!("队列投影 {queue_len} != daemon {}", st.queue.len),
            );
        }

        // 2. toggle-play → daemon 翻转 → Player.playing 回写。
        on_ui(&ui_weak, |ui| Player::get(ui).invoke_toggle_play()).unwrap();
        let ok = wait_for("toggle-play 生效", Duration::from_secs(5), || {
            daemon_status().playback.status != hmp_core::PlaybackStatus::Playing
        });
        if !ok {
            fail(&failures, "toggle-play 未生效（daemon 态未翻转）".into());
        } else {
            let now_playing = daemon_status().playback.status == hmp_core::PlaybackStatus::Playing;
            let ok = wait_for(
                "推送回写 Player.playing",
                Duration::from_secs(5),
                || {
                    on_ui(&ui_weak, |ui| Player::get(ui).get_playing())
                        .map(|p| p == now_playing)
                        .unwrap_or(false)
                },
            );
            if !ok {
                fail(&failures, "Player.playing 未随推送回写".into());
            }
        }
        // 复位为播放态。
        on_ui(&ui_weak, |ui| Player::get(ui).invoke_toggle_play()).unwrap();
        wait_for("toggle-play 复位", Duration::from_secs(5), || {
            daemon_status().playback.status == hmp_core::PlaybackStatus::Playing
        });

        // 3. next → 换曲 → 标题推送更新（与 daemon 交叉验证）。
        let before_title = on_ui(&ui_weak, |ui| Player::get(ui).get_title().to_string()).unwrap();
        on_ui(&ui_weak, |ui| Player::get(ui).invoke_next()).unwrap();
        let ok = wait_for("next 换曲推送", Duration::from_secs(6), || {
            on_ui(&ui_weak, |ui| Player::get(ui).get_title().to_string())
                .map(|t| !t.is_empty() && t != before_title)
                .unwrap_or(false)
        });
        if ok {
            let ui_title = on_ui(&ui_weak, |ui| Player::get(ui).get_title().to_string()).unwrap();
            let daemon_title = daemon_status()
                .playback
                .current
                .as_ref()
                .unwrap()
                .title
                .clone();
            if ui_title != daemon_title {
                fail(
                    &failures,
                    format!("标题不一致 UI={ui_title:?} daemon={daemon_title:?}"),
                );
            }
        } else {
            fail(&failures, "next 后标题未推送更新".into());
        }

        // 4. 进度在走（音频管线活着）。
        let p1 = daemon_status().playback.position;
        std::thread::sleep(Duration::from_secs(2));
        let p2 = daemon_status().playback.position;
        if p2 <= p1 {
            fail(&failures, format!("播放中进度未前进: {p1:?} → {p2:?}"));
        }

        // 5. play-tracks（本地曲目整表 + 中间下标）→ PlayList 整表替换队列。
        let planned = on_ui(&ui_weak, |ui| {
            let tracks = hmp_desktop::Data::get(ui).get_local_tracks();
            let count = tracks.row_count();
            if count > 1 {
                let start = (count / 2) as i32;
                Player::get(ui).invoke_play_tracks(tracks, start);
            }
            count
        })
        .expect("触发 play-tracks");
        if planned <= 1 {
            fail(&failures, "前置失败：本地媒体库应有多首曲目".into());
        } else {
            let count = planned;
            // 注意不能等 len==count：上一队列也是同曲数（瞬时相等）；直接等
            // 游标落到起播下标——PlayList 生效的确定性标志。
            let ok = wait_for(
                "PlayList 整表入队（游标落位）",
                Duration::from_secs(8),
                || daemon_status().queue.current == Some(count / 2),
            );
            if !ok {
                fail(
                    &failures,
                    format!("PlayList 后游标未落到起播下标 {}", count / 2),
                );
            } else {
                let ok = wait_for("队列投影落地", Duration::from_secs(6), || {
                    on_ui(&ui_weak, |ui| Player::get(ui).get_queue().row_count())
                        .map(|n| n == count)
                        .unwrap_or(false)
                });
                if ok {
                    let bad = on_ui(&ui_weak, |ui| {
                        let q = Player::get(ui).get_queue();
                        (0..q.row_count())
                            .filter_map(|i| q.row_data(i))
                            .find(|row| row.title == row.mid)
                            .map(|row| row.mid.to_string())
                    })
                    .flatten();
                    if let Some(mid) = bad {
                        fail(&failures, format!("队列行标题回退成 id: {mid}"));
                    }
                } else {
                    fail(&failures, "队列投影未落地".into());
                }
            }
        }

        slint::quit_event_loop().unwrap();
    });

    slint::run_event_loop().expect("事件循环");

    let collected = failures.lock().unwrap();
    assert!(
        collected.is_empty(),
        "真机 E2E 断言失败:\n- {}",
        collected.join("\n- ")
    );
}
