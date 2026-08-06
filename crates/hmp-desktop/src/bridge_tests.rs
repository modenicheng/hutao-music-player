//! 桥接集成测试（Slint testing backend，无真实窗口）。

use serial_test::serial;
use slint::{ComponentHandle, Model};

use crate::AppWindow;
use crate::app::{AppCommand, AppEvent, UiSongData};
use crate::bridge::{bind_callbacks, decode_png, handle_event, songs_model};

/// 初始化 testing backend（进程内仅一次）。
fn init_ui() -> AppWindow {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        i_slint_backend_testing::init_integration_test_with_system_time();
    });
    AppWindow::new().expect("create window")
}

/// 全部窗口场景（testing backend 进程内单次初始化，官方建议单一 #[test]）。
#[test]
fn ui_bridge_integration() {
    // 1) 回调 → 命令
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
    }

    // 2) 搜索结果事件 → 列表
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

    // 3) 登录二维码事件 → 显示登录面板
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

    // 4) 登录完成事件 → 更新用户
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
