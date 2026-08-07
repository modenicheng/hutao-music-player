//! 桥接集成测试（Slint testing backend，无真实窗口）。

use serial_test::serial;
use slint::{ComponentHandle, Model};

use crate::AppWindow;
use crate::app::{AppCommand, AppEvent, ThemeMode, UiLyricData, UiPage, UiQueueData, UiSongData};
use crate::bridge::{
    bind_callbacks, bind_ui_state_callbacks, decode_png, handle_event, songs_model,
};

/// 初始化 testing backend（进程内仅一次）。
fn init_ui() -> AppWindow {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        i_slint_backend_testing::init_integration_test_with_system_time();
    });
    AppWindow::new().expect("create window")
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

/// 全部窗口场景（testing backend 进程内单次初始化，官方建议单一 #[test]）。
#[test]
#[serial]
fn app_starts_in_library_and_accepts_theme_modes() {
    // 0) 初始路由和主题模式可由生成的 UI 属性读取和修改。
    {
        let ui = init_ui();
        assert_eq!(ui.get_current_page(), "library");
        assert_eq!(ui.get_theme_mode(), "system");
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
        ui.invoke_search_requested("开始懂了".into());
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::Search(k) if k == "开始懂了"));
        ui.invoke_play_requested(2);
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::PlayIndex(2)));
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
    }

    // 3) 搜索结果事件 → 列表
    {
        let ui = init_ui();
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
    }

    // 4) Task 1 contract-only events are accepted until later UI mappings land.
    {
        let ui = init_ui();
        let weak = ui.as_weak();
        let events = [
            AppEvent::SearchFailed("network error".into()),
            AppEvent::QueueUpdated(Vec::new()),
            AppEvent::LyricsLoading("mid-1".into()),
            AppEvent::LyricsLoaded {
                mid: "mid-1".into(),
                lines: vec![UiLyricData {
                    timestamp_ms: 0,
                    time: "00:00".into(),
                    text: "line".into(),
                    translation: String::new(),
                }],
            },
            AppEvent::LyricsFailed {
                mid: "mid-1".into(),
                message: "network error".into(),
            },
        ];
        for event in events {
            assert!(handle_event(&weak, event));
        }
    }

    // 5) 登录二维码事件 → 显示登录面板
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
    }

    // 6) 登录完成事件 → 更新用户
    {
        let ui = init_ui();
        let weak = ui.as_weak();
        handle_event(&weak, AppEvent::LoginDone("10001".into()));
        assert!(ui.get_logged_in());
        assert_eq!(ui.get_user_name(), "10001");
        assert!(!ui.get_show_login());
    }
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
