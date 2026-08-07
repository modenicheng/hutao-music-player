//! 桥接集成测试（Slint testing backend，无真实窗口）。

use serial_test::serial;
use slint::{ComponentHandle, Model};

use crate::AppWindow;
use crate::app::{AppCommand, AppEvent, ThemeMode, UiLyricData, UiPage, UiQueueData, UiSongData};
use crate::bridge::{
    bind_callbacks, bind_ui_state_callbacks, decode_png, handle_event, lyric_mid_matches,
    lyrics_model, lyrics_model_at_position, queue_model, songs_model, valid_model_index,
};
use crate::demo::{demo_recommendations, feature_matrix};

/// 初始化 testing backend（进程内仅一次）。
fn init_ui() -> AppWindow {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        i_slint_backend_testing::init_integration_test_with_system_time();
    });
    let ui = AppWindow::new().expect("create window");
    ui.set_feature_statuses(crate::bridge::feature_model(feature_matrix()));
    ui
}

#[test]
fn page_and_theme_values_use_stable_wire_names() {
    assert_eq!(UiPage::Library.as_str(), "library");
    assert_eq!(UiPage::parse("queue"), Some(UiPage::Queue));
    assert_eq!(UiPage::parse("unknown"), None);
    assert_eq!(ThemeMode::FollowSystem.as_str(), "system");
    assert_eq!(ThemeMode::parse("light"), Some(ThemeMode::Light));
}

#[test]
fn queue_event_contains_current_playing_flags() {
    let event = AppEvent::QueueUpdated(vec![UiQueueData {
        track_id: "mid-1".into(),
        title: "晴天".into(),
        artist: "周杰伦".into(),
        duration: "04:29".into(),
        is_current: true,
        is_playing: true,
    }]);
    assert!(matches!(event, AppEvent::QueueUpdated(items) if items[0].is_current));
}

#[test]
fn reload_lyrics_command_is_distinct_from_playback_commands() {
    let command = AppCommand::ReloadLyrics;
    assert!(matches!(command, AppCommand::ReloadLyrics));

    for playback_command in [
        AppCommand::TogglePlay,
        AppCommand::Next,
        AppCommand::Previous,
    ] {
        assert!(!matches!(playback_command, AppCommand::ReloadLyrics));
    }
}

#[test]
fn generated_model_indices_are_validated_before_conversion() {
    assert_eq!(valid_model_index(-1, 3), None);
    assert_eq!(valid_model_index(3, 3), None);
    assert_eq!(valid_model_index(2, 3), Some(2));
}

/// 全部窗口场景（testing backend 进程内单次初始化，官方建议单一 #[test]）。
#[test]
#[serial]
fn app_starts_in_library_and_accepts_theme_modes() {
    // 0) 初始路由和主题模式可由生成的 UI 属性读取和修改。
    {
        let ui = init_ui();
        assert_eq!(ui.get_current_page(), "library");
        assert_eq!(ui.get_theme_mode(), "system");
        assert!(!ui.get_search_completed());
        ui.set_theme_mode("light".into());
        assert_eq!(ui.get_theme_mode(), "light");
        ui.set_theme_mode("dark".into());
        assert_eq!(ui.get_theme_mode(), "dark");

        bind_ui_state_callbacks(&ui);
        ui.invoke_navigate_requested("queue".into());
        assert_eq!(ui.get_current_page(), "queue");
        ui.invoke_navigate_requested("bad-page".into());
        assert_eq!(ui.get_current_page(), "queue");
        ui.invoke_theme_requested("light".into());
        assert_eq!(ui.get_theme_mode(), "light");
        ui.invoke_theme_requested("bad-theme".into());
        assert_eq!(ui.get_theme_mode(), "light");

        ui.set_current_page("settings".into());
        assert_eq!(ui.get_feature_statuses().row_count(), 7);
        assert_eq!(
            ui.get_feature_statuses().row_data(5).unwrap().status,
            "开发中 / 演示数据"
        );
        ui.invoke_theme_requested("light".into());
        assert_eq!(ui.get_theme_mode(), "light");
        ui.invoke_theme_requested("invalid".into());
        assert_eq!(ui.get_theme_mode(), "light");
    }

    // 1) 登录按钮可点击（覆盖"登录无法点击"反馈）：扫描侧边栏底部注入点击
    {
        let ui = init_ui();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        bind_callbacks(&ui, tx);
        assert!(!ui.get_logged_in());
        assert!(!ui.get_show_login());
        let mut hit = None;
        'scan: for y in (0..=720).step_by(10) {
            for x in (10..=220).step_by(10) {
                let pos = slint::LogicalPosition::new(x as f32, y as f32);
                ui.window()
                    .dispatch_event(slint::platform::WindowEvent::PointerPressed {
                        position: pos,
                        button: slint::platform::PointerEventButton::Left,
                    });
                ui.window()
                    .dispatch_event(slint::platform::WindowEvent::PointerReleased {
                        position: pos,
                        button: slint::platform::PointerEventButton::Left,
                    });
                if let Ok(cmd) = rx.try_recv() {
                    assert!(matches!(cmd, AppCommand::LoginStart));
                    hit = Some((x, y));
                    break 'scan;
                }
            }
        }
        assert!(
            hit.is_some(),
            "login button not clickable at any scanned position"
        );
    }
    // 2) 回调 → 命令
    {
        let ui = init_ui();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        bind_callbacks(&ui, tx);
        ui.invoke_search_query_edited("  开始懂了  ".into());
        assert!(ui.get_search_query_valid());
        ui.set_search_completed(true);
        ui.invoke_search_requested("  开始懂了  ".into());
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::Search(k) if k == "开始懂了"));
        assert!(ui.get_search_loading());
        assert!(!ui.get_search_completed());
        assert_eq!(ui.get_search_error_text(), "");
        ui.invoke_search_query_edited(" \t\n ".into());
        assert!(!ui.get_search_query_valid());
        ui.invoke_search_requested(" \t\n ".into());
        assert!(rx.try_recv().is_err(), "blank search must be ignored");
        ui.set_songs(songs_model(vec![
            UiSongData {
                title: "one".into(),
                artist: "artist".into(),
                duration: "01:00".into(),
            },
            UiSongData {
                title: "two".into(),
                artist: "artist".into(),
                duration: "02:00".into(),
            },
            UiSongData {
                title: "three".into(),
                artist: "artist".into(),
                duration: "03:00".into(),
            },
        ]));
        ui.invoke_play_requested(2);
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::PlayIndex(2)));
        ui.invoke_play_requested(-1);
        ui.invoke_play_requested(3);
        assert!(
            rx.try_recv().is_err(),
            "invalid song indices must be ignored"
        );

        ui.set_queue(queue_model(vec![
            UiQueueData {
                track_id: "queue-0".into(),
                title: "one".into(),
                artist: "artist".into(),
                duration: "01:00".into(),
                is_current: true,
                is_playing: false,
            },
            UiQueueData {
                track_id: "queue-1".into(),
                title: "two".into(),
                artist: "artist".into(),
                duration: "02:00".into(),
                is_current: false,
                is_playing: false,
            },
        ]));
        ui.invoke_play_queue_requested(1);
        assert!(matches!(
            rx.try_recv().unwrap(),
            AppCommand::PlayQueueIndex(1)
        ));
        ui.invoke_play_queue_requested(-1);
        ui.invoke_play_queue_requested(2);
        assert!(
            rx.try_recv().is_err(),
            "invalid queue indices must be ignored"
        );
        ui.invoke_play_pause();
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::TogglePlay));
        ui.invoke_next_requested();
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::Next));
        ui.invoke_prev_requested();
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::Previous));
        ui.invoke_seek_requested(42.5);
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::Seek(v) if (v - 42.5).abs() < 1e-6));
        ui.invoke_volume_requested(0.3);
        assert!(
            matches!(rx.try_recv().unwrap(), AppCommand::SetVolume(v) if (v - 0.3).abs() < 1e-6)
        );
        ui.invoke_login_start();
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::LoginStart));
        ui.invoke_login_cancel();
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::LoginCancel));
        ui.invoke_load_lyrics_requested();
        assert!(
            rx.try_recv().is_err(),
            "empty lyric context must be ignored"
        );
        ui.set_current_track_id("queue-0".into());
        ui.invoke_load_lyrics_requested();
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::ReloadLyrics));
    }

    // 3) 加载期间，旧搜索结果行的指针操作不能发出播放命令。
    {
        let ui = init_ui();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        bind_callbacks(&ui, tx);
        ui.window().set_size(slint::PhysicalSize::new(820, 560));
        ui.set_current_page("search".into());
        ui.set_songs(songs_model(vec![UiSongData {
            title: "开始懂了".into(),
            artist: "孙燕姿".into(),
            duration: "04:30".into(),
        }]));

        let mut row_position = None;
        'scan: for y in (120..=500).step_by(10) {
            for x in (240..=800).step_by(10) {
                let position = slint::LogicalPosition::new(x as f32, y as f32);
                ui.window()
                    .dispatch_event(slint::platform::WindowEvent::PointerPressed {
                        position,
                        button: slint::platform::PointerEventButton::Left,
                    });
                ui.window()
                    .dispatch_event(slint::platform::WindowEvent::PointerReleased {
                        position,
                        button: slint::platform::PointerEventButton::Left,
                    });
                if matches!(rx.try_recv(), Ok(AppCommand::PlayIndex(0))) {
                    row_position = Some(position);
                    break 'scan;
                }
            }
        }
        let position = row_position.expect("rendered search result row is clickable");

        ui.set_search_loading(true);
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerPressed {
                position,
                button: slint::platform::PointerEventButton::Left,
            });
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerReleased {
                position,
                button: slint::platform::PointerEventButton::Left,
            });
        assert!(rx.try_recv().is_err(), "loading row emitted play callback");
    }

    // 4) 搜索结果事件 → 列表和完成状态
    {
        let ui = init_ui();
        ui.set_logged_in(true);
        ui.set_user_name("account sentinel".into());
        ui.set_queue(queue_model(vec![UiQueueData {
            track_id: "queue sentinel".into(),
            title: "queue title".into(),
            artist: "queue artist".into(),
            duration: "01:00".into(),
            is_current: true,
            is_playing: false,
        }]));
        ui.set_lyrics(lyrics_model(
            vec![UiLyricData {
                timestamp_ms: 1_000,
                time: "00:01".into(),
                text: "lyrics sentinel".into(),
                translation: String::new(),
            }],
            0.0,
        ));
        ui.set_lyrics_state("error".into());
        ui.set_lyrics_error_text("lyrics sentinel".into());
        ui.set_search_loading(true);
        ui.set_search_error_text("old error".into());
        let weak = ui.as_weak();
        handle_event(
            &weak,
            AppEvent::SearchDone(vec![
                UiSongData {
                    title: "开始懂了".into(),
                    artist: "孙燕姿".into(),
                    duration: "—".into(),
                },
                UiSongData {
                    title: "晴天".into(),
                    artist: "周杰伦".into(),
                    duration: "—".into(),
                },
            ]),
        );
        assert_eq!(ui.get_songs().row_count(), 2);
        assert_eq!(ui.get_songs().row_data(0).unwrap().title, "开始懂了");
        assert_eq!(ui.get_songs().row_data(1).unwrap().artist, "周杰伦");
        assert!(ui.get_logged_in());
        assert_eq!(ui.get_user_name(), "account sentinel");
        assert_eq!(
            ui.get_queue().row_data(0).unwrap().track_id,
            "queue sentinel"
        );
        assert_eq!(ui.get_lyrics().row_data(0).unwrap().text, "lyrics sentinel");
        assert_eq!(ui.get_lyrics_state(), "error");
        assert_eq!(ui.get_lyrics_error_text(), "lyrics sentinel");
        assert!(!ui.get_search_loading());
        assert!(ui.get_search_completed());
        assert_eq!(ui.get_search_error_text(), "");

        ui.set_search_completed(false);
        handle_event(&weak, AppEvent::SearchFailed("network error".into()));
        assert_eq!(ui.get_songs().row_count(), 2, "failure preserves results");
        assert!(!ui.get_search_loading());
        assert!(ui.get_search_completed());
        assert_eq!(ui.get_search_error_text(), "network error");
    }

    // 5) Queue and lyric events map all identity/state fields; stale lyrics are ignored.
    {
        let ui = init_ui();
        let weak = ui.as_weak();
        ui.set_logged_in(true);
        ui.set_user_name("account sentinel".into());
        ui.set_songs(songs_model(vec![UiSongData {
            title: "search sentinel".into(),
            artist: "artist".into(),
            duration: "01:00".into(),
        }]));
        ui.set_lyrics_state("idle".into());
        ui.set_current_page("library".into());
        assert_eq!(ui.get_current_page(), "library");
        handle_event(
            &weak,
            AppEvent::QueueUpdated(vec![
                UiQueueData {
                    track_id: "mid-current".into(),
                    title: "晴天".into(),
                    artist: "周杰伦".into(),
                    duration: "04:29".into(),
                    is_current: true,
                    is_playing: true,
                },
                UiQueueData {
                    track_id: "mid-next".into(),
                    title: "开始懂了".into(),
                    artist: "孙燕姿".into(),
                    duration: "04:30".into(),
                    is_current: false,
                    is_playing: false,
                },
            ]),
        );
        assert_eq!(ui.get_queue().row_count(), 2);
        assert!(ui.get_logged_in());
        assert_eq!(ui.get_user_name(), "account sentinel");
        assert_eq!(ui.get_songs().row_data(0).unwrap().title, "search sentinel");
        assert_eq!(ui.get_lyrics_state(), "idle");
        let current = ui.get_queue().row_data(0).unwrap();
        assert_eq!(current.track_id, "mid-current");
        assert!(current.is_current);
        assert!(current.is_playing);
        let next = ui.get_queue().row_data(1).unwrap();
        assert!(!next.is_current);
        assert!(!next.is_playing);

        ui.set_search_error_text("search error remains isolated".into());
        handle_event(&weak, AppEvent::LyricsLoading("".into()));
        assert_eq!(ui.get_lyrics_state(), "idle");
        handle_event(&weak, AppEvent::LyricsLoading("mid-current".into()));
        assert_eq!(ui.get_lyrics_request_mid(), "mid-current");
        assert_eq!(ui.get_lyrics_state(), "loading");
        handle_event(
            &weak,
            AppEvent::LyricsLoaded {
                mid: "stale-mid".into(),
                lines: vec![UiLyricData {
                    timestamp_ms: 500,
                    time: "00:00".into(),
                    text: "stale".into(),
                    translation: String::new(),
                }],
            },
        );
        assert_eq!(ui.get_lyrics().row_count(), 0, "stale MID must be ignored");
        assert_eq!(ui.get_lyrics_state(), "loading");

        handle_event(
            &weak,
            AppEvent::LyricsLoaded {
                mid: "mid-current".into(),
                lines: vec![
                    UiLyricData {
                        timestamp_ms: 1_000,
                        time: "00:01".into(),
                        text: "first".into(),
                        translation: "第一句".into(),
                    },
                    UiLyricData {
                        timestamp_ms: 3_000,
                        time: "00:03".into(),
                        text: "second".into(),
                        translation: String::new(),
                    },
                ],
            },
        );
        assert_eq!(ui.get_lyrics_state(), "ready");
        assert_eq!(ui.get_lyrics().row_count(), 2);
        assert_eq!(ui.get_lyrics().row_data(0).unwrap().translation, "第一句");
        let updated = lyrics_model_at_position(&ui.get_lyrics(), 3_500.0);
        assert!(!updated.row_data(0).unwrap().is_active);
        assert!(updated.row_data(1).unwrap().is_active);

        handle_event(
            &weak,
            AppEvent::LyricsFailed {
                mid: "stale-mid".into(),
                message: "stale failure".into(),
            },
        );
        assert_eq!(ui.get_lyrics_state(), "ready");

        handle_event(&weak, AppEvent::LyricsLoading("mid-current".into()));
        handle_event(
            &weak,
            AppEvent::LyricsFailed {
                mid: "mid-current".into(),
                message: "lyric failure".into(),
            },
        );
        assert_eq!(ui.get_lyrics_state(), "error");
        assert_eq!(ui.get_lyrics_error_text(), "lyric failure");
        assert_eq!(ui.get_search_error_text(), "search error remains isolated");

        handle_event(&weak, AppEvent::LyricsLoading("mid-current".into()));
        handle_event(
            &weak,
            AppEvent::LyricsLoaded {
                mid: "mid-current".into(),
                lines: Vec::new(),
            },
        );
        assert_eq!(ui.get_lyrics_state(), "empty");
        assert_eq!(ui.get_lyrics().row_count(), 0, "no fallback lyrics allowed");
        assert_eq!(ui.get_lyrics_error_text(), "");
        assert!(ui.get_logged_in());
        assert_eq!(ui.get_user_name(), "account sentinel");
        assert_eq!(ui.get_songs().row_data(0).unwrap().title, "search sentinel");
        assert_eq!(ui.get_queue().row_data(0).unwrap().track_id, "mid-current");
    }

    // 6) 登录二维码事件 → 显示登录面板
    {
        let ui = init_ui();
        let weak = ui.as_weak();
        // 用 image crate 生成合法 PNG（encode → decode roundtrip）
        let png = {
            let mut buf = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                8,
                8,
                image::Rgb([255, 0, 0]),
            ))
            .write_to(&mut buf, image::ImageFormat::Png)
            .expect("encode png");
            buf.into_inner()
        };
        let img = decode_png(&png).expect("decode png");
        assert!(img.size().width > 0 && img.size().height > 0);
        handle_event(&weak, AppEvent::LoginQr(png));
        assert!(ui.get_show_login(), "login panel should show");

        // The modal backdrop absorbs outside clicks without cancelling the session.
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerPressed {
                position: slint::LogicalPosition::new(10.0, 10.0),
                button: slint::platform::PointerEventButton::Left,
            });
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerReleased {
                position: slint::LogicalPosition::new(10.0, 10.0),
                button: slint::platform::PointerEventButton::Left,
            });
        assert!(ui.get_show_login(), "backdrop click must not cancel login");
    }

    // 7) 登录完成事件 → 更新用户
    {
        let ui = init_ui();
        ui.set_songs(songs_model(vec![UiSongData {
            title: "search sentinel".into(),
            artist: "artist".into(),
            duration: "01:00".into(),
        }]));
        ui.set_search_error_text("search sentinel".into());
        ui.set_queue(queue_model(vec![UiQueueData {
            track_id: "queue sentinel".into(),
            title: "queue".into(),
            artist: "artist".into(),
            duration: "01:00".into(),
            is_current: true,
            is_playing: false,
        }]));
        ui.set_lyrics(lyrics_model(
            vec![UiLyricData {
                timestamp_ms: 1_000,
                time: "00:01".into(),
                text: "lyric sentinel".into(),
                translation: String::new(),
            }],
            0.0,
        ));
        ui.set_lyrics_state("loading".into());
        ui.set_lyrics_request_mid("lyric sentinel".into());
        let weak = ui.as_weak();
        handle_event(&weak, AppEvent::LoginDone("10001".into()));
        assert!(ui.get_logged_in());
        assert_eq!(ui.get_user_name(), "10001");
        assert!(!ui.get_show_login());
        assert_eq!(ui.get_songs().row_data(0).unwrap().title, "search sentinel");
        assert_eq!(ui.get_search_error_text(), "search sentinel");
        assert_eq!(
            ui.get_queue().row_data(0).unwrap().track_id,
            "queue sentinel"
        );
        assert_eq!(ui.get_lyrics().row_data(0).unwrap().text, "lyric sentinel");
        assert_eq!(ui.get_lyrics_state(), "loading");
        assert_eq!(ui.get_lyrics_request_mid(), "lyric sentinel");
    }

    // 8) AppCore events stop cleanly once the UI weak handle is stale.
    {
        let weak = {
            let ui = init_ui();
            ui.as_weak()
        };
        assert!(!handle_event(&weak, AppEvent::QueueUpdated(Vec::new())));
    }
}

#[test]
fn queue_event_updates_slint_model() {
    let model = queue_model(vec![UiQueueData {
        track_id: "mid-current".into(),
        title: "晴天".into(),
        artist: "周杰伦".into(),
        duration: "04:29".into(),
        is_current: true,
        is_playing: true,
    }]);

    assert_eq!(model.row_count(), 1);
    let current = model.row_data(0).unwrap();
    assert_eq!(current.track_id, "mid-current");
    assert!(current.is_current);
    assert!(current.is_playing);
}

#[test]
fn lyric_event_updates_slint_model() {
    assert!(lyric_mid_matches("mid-current", "mid-current"));
    assert!(!lyric_mid_matches("mid-current", "stale-mid"));
    assert!(!lyric_mid_matches("", ""));

    let model = lyrics_model(
        vec![UiLyricData {
            timestamp_ms: 1_000,
            time: "00:01".into(),
            text: "first".into(),
            translation: "第一句".into(),
        }],
        1_000.0,
    );
    assert_eq!(model.row_count(), 1);
    let line = model.row_data(0).unwrap();
    assert_eq!(line.translation, "第一句");
    assert!(line.is_active);

    assert_eq!(lyrics_model(Vec::new(), 0.0).row_count(), 0);
}

#[test]
fn demo_recommendations_are_local_and_marked_as_demo() {
    let items = demo_recommendations();
    assert!(!items.is_empty());
    assert!(items.iter().all(|item| item.status == "demo"));
    assert!(
        items
            .iter()
            .all(|item| item.cover.size().width == 320 && item.cover.size().height == 320)
    );
}

#[test]
fn feature_matrix_uses_approved_statuses() {
    let matrix = feature_matrix()
        .into_iter()
        .map(|item| (item.name, item.status, item.detail))
        .collect::<Vec<_>>();
    assert_eq!(matrix.len(), 7);
    assert_eq!(
        matrix,
        [
            ("登录", "已接入", "QQ 音乐扫码登录与凭据状态"),
            ("搜索", "已接入", "使用 QQ Music Rust API"),
            (
                "播放控制",
                "已接入",
                "播放、暂停、上一首、下一首、Seek、音量"
            ),
            ("队列展示", "已接入", "展示 AppCore 当前真实队列"),
            ("歌词展示", "部分接入", "已接入接口与空状态，按真实返回展示"),
            ("推荐内容", "开发中 / 演示数据", "当前使用本地演示数据"),
            ("收藏与资料库同步", "开发中", "尚未接入账号云端同步"),
        ]
        .map(|(name, status, detail)| (name.into(), status.into(), detail.into()))
    );
}

#[test]
#[serial]
fn songs_model_maps_fields() {
    let model = songs_model(vec![UiSongData {
        title: "晴天".into(),
        artist: "周杰伦".into(),
        duration: "04:29".into(),
    }]);
    assert_eq!(model.row_count(), 1);
    let s = model.row_data(0).unwrap();
    assert_eq!(s.title, "晴天");
    assert_eq!(s.artist, "周杰伦");
    assert_eq!(s.duration, "04:29");
}
