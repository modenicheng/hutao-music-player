//! 弹层键盘导航（右键菜单 / 音质选择，M7 收口 AUDIT §29）+ 焦点回触发器。
//!
//! 守护：
//! 1. 右键菜单打开即聚焦第一项（「播放」），↑/↓ 移动、Home/End 到首尾、
//!    Enter 激活；禁用项（无远端 mid 的查看歌手/专辑）被跳过；
//! 2. 「加入播放列表…」二级 picker 沿用同一焦点作用域，↓ + Enter 选歌单；
//! 3. 菜单关闭（执行动作 / Esc）后焦点回触发它的曲目表光标行；
//! 4. 音质选择弹层：打开即聚焦、↓/End 移动、Enter 选定、Esc 关闭后焦点回
//!    音质徽章（再回车能重新打开）。
//!
//! 焦点断言一律走行为面：弹层内 Enter → TrackMenu/Quality 回调；回到列表后
//! Enter → Player.play-tracks（光标行）。弹层焦点丢失时这些断言会得到 0 次
//! 或错误下标。
//!
//! 几何沿用 context_menu.rs：y=350 落在「我喜欢」表首行中部（TestingBackend
//! 无 Viewport 绑定 → 全量渲染，行下标 = 模型下标）。音质徽章到达方式：
//! 从窗口根作用域 Shift+Tab 6 次（关闭/最大化/最小化 → 音量 → 播放 → 徽章；
//! 队列为空、上一曲/下一曲禁用）。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, ModelRc, SharedString, VecModel, platform::WindowEvent};

use hmp_desktop::{Data, Player, PlaylistCover, Quality, QueueRow, TrackMenu, TrackRow};

const ROW_X: f32 = 640.0;
const ROW_Y: f32 = 350.0;

fn settle(ms: u32) {
    for _ in 0..(ms / 16).max(1) {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

fn tap(ui: &hmp_desktop::AppWindow, key: impl Into<SharedString>) {
    let key: SharedString = key.into();
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: key });
}

fn shift_tab(ui: &hmp_desktop::AppWindow) {
    let shift: SharedString = slint::platform::Key::Shift.into();
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: shift.clone(),
    });
    tap(ui, slint::platform::Key::Tab);
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: shift });
}

fn click(ui: &hmp_desktop::AppWindow, x: f32, y: f32, button: slint::platform::PointerEventButton) {
    for pressed in [true, false] {
        ui.window().dispatch_event(if pressed {
            WindowEvent::PointerPressed {
                position: slint::LogicalPosition::new(x, y),
                button,
            }
        } else {
            WindowEvent::PointerReleased {
                position: slint::LogicalPosition::new(x, y),
                button,
            }
        });
    }
}

fn playlist(id: &str, name: &str) -> PlaylistCover {
    PlaylistCover {
        id: id.into(),
        name: name.into(),
        relation: "local".into(),
        image: slint::Image::default(),
    }
}

#[test]
fn popup_menus_navigate_by_keyboard_and_restore_focus() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data(
        &ui,
        &Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap()),
    );

    // 播种：30 行「我喜欢」（无歌手/专辑 mid → 菜单最后两项禁用）+ 两个自建歌单
    {
        let data = Data::get(&ui);
        let rows: Vec<TrackRow> = (0..30)
            .map(|i| TrackRow {
                mid: format!("menu-key-{i}").into(),
                source: 0,
                title: format!("曲目 {i}").into(),
                artists: "测试歌手".into(),
                artist_mid: "".into(),
                album: "测试专辑".into(),
                album_mid: "".into(),
                duration_ms: 200_000,
                quality: "".into(),
            })
            .collect();
        let count = rows.len() as i32;
        data.set_liked(ModelRc::new(VecModel::from(rows)));
        data.set_liked_count(count);
        data.set_sidebar_created(ModelRc::new(VecModel::from(vec![
            playlist("701", "测试歌单一"),
            playlist("702", "测试歌单二"),
        ])));
    }
    let player = Player::get(&ui);
    // 队列清空：音质徽章前方的 Tab 站数量稳定（空队列无队列键）
    player.set_queue(ModelRc::new(VecModel::from(Vec::<QueueRow>::new())));
    settle(320);

    let menu = TrackMenu::get(&ui);
    let plays: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(Vec::new()));
    let play_nexts = Arc::new(Mutex::new(0usize));
    let appends = Arc::new(Mutex::new(0usize));
    let adds: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let plays = Arc::clone(&plays);
        player.on_play_tracks(move |_tracks, start| plays.lock().unwrap().push(start));
    }
    {
        let play_nexts = Arc::clone(&play_nexts);
        menu.on_play_next(move || *play_nexts.lock().unwrap() += 1);
    }
    {
        let appends = Arc::clone(&appends);
        menu.on_append_queue(move || *appends.lock().unwrap() += 1);
    }
    {
        let adds = Arc::clone(&adds);
        menu.on_add_to_playlist(move |id| adds.lock().unwrap().push(id.to_string()));
    }

    // 左键标定命中行（同 context_menu.rs：先标定再断言，几何漂移免疫）
    click(&ui, ROW_X, ROW_Y, slint::platform::PointerEventButton::Left);
    settle(64);
    let hit = *plays.lock().unwrap().last().expect("左键应命中的行");
    let baseline_plays = plays.lock().unwrap().len();

    // —— 1. 打开即聚焦第一项：Enter = 播放；关闭后焦点回触发行 ——
    click(
        &ui,
        ROW_X,
        ROW_Y,
        slint::platform::PointerEventButton::Right,
    );
    settle(64);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        plays.lock().unwrap().last(),
        Some(&hit),
        "菜单打开即聚焦「播放」，Enter 应从命中行开播"
    );
    settle(64);
    let after_menu = plays.lock().unwrap().len();
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        plays.lock().unwrap().len(),
        after_menu + 1,
        "菜单关闭后焦点应回曲目表（回车 = 播放光标行）"
    );
    assert_eq!(
        plays.lock().unwrap().last(),
        Some(&hit),
        "回落的列表光标应停在右键触发行"
    );

    // —— 2. ↓ 移动 + Enter（下一首播放）——
    click(
        &ui,
        ROW_X,
        ROW_Y,
        slint::platform::PointerEventButton::Right,
    );
    settle(64);
    tap(&ui, slint::platform::Key::DownArrow);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        *play_nexts.lock().unwrap(),
        1,
        "↓ 后 Enter 应触发下一首播放"
    );

    // —— 3. End 跳过禁用项 → 末个可用 =「加入播放列表…」→ picker → 选歌单 ——
    click(
        &ui,
        ROW_X,
        ROW_Y,
        slint::platform::PointerEventButton::Right,
    );
    settle(64);
    tap(&ui, slint::platform::Key::End);
    tap(&ui, slint::platform::Key::Return);
    settle(64);
    assert!(
        adds.lock().unwrap().is_empty(),
        "End 应落在「加入播放列表…」而非禁用导航项"
    );
    tap(&ui, slint::platform::Key::DownArrow);
    tap(&ui, slint::platform::Key::Return);
    settle(64);
    assert_eq!(
        &*adds.lock().unwrap(),
        &["701".to_string()],
        "picker 首项 ↓ + Enter 应加入第一个歌单"
    );

    // —— 4. Home 回第一项 + 关闭后回落 ——
    click(
        &ui,
        ROW_X,
        ROW_Y,
        slint::platform::PointerEventButton::Right,
    );
    settle(64);
    tap(&ui, slint::platform::Key::End);
    tap(&ui, slint::platform::Key::Home);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        plays.lock().unwrap().last(),
        Some(&hit),
        "End→Home 后 Enter 应回到「播放」"
    );
    settle(64);

    // —— 5. Esc 关闭 + 焦点回列表 + 光标落在触发行 ——
    click(
        &ui,
        ROW_X,
        ROW_Y,
        slint::platform::PointerEventButton::Right,
    );
    settle(64);
    tap(&ui, slint::platform::Key::Escape);
    settle(64);
    tap(&ui, slint::platform::Key::DownArrow);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        plays.lock().unwrap().last(),
        Some(&(hit + 1)),
        "Esc 关闭菜单后焦点应回列表，↓+Enter 播下一行"
    );
    // 队列/歌单动作未被偷跑
    assert_eq!(*appends.lock().unwrap(), 0, "未点「加入队列」不应触发");
    assert_eq!(*play_nexts.lock().unwrap(), 1);
    assert_eq!(adds.lock().unwrap().len(), 1);

    // —— 6. 音质弹层：从根作用域 Shift+Tab 6 次落到徽章，方向键 + Enter ——
    // 指针活动 = 列表让出焦点并交还根作用域（keynav 焦点模型的两段式）
    hmp_desktop::keynav::pointer_activity(&ui);
    settle(64);
    // route 变化把焦点交还根作用域：Shift+Tab 倒着走 3 个窗口钮 + 播放条
    // （音量 → 播放 → 音质徽章；上一曲/下一曲禁用、队列为空）
    for _ in 0..6 {
        shift_tab(&ui);
    }
    let selects: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let selects = Arc::clone(&selects);
        Quality::get(&ui).on_select(move |tier| selects.lock().unwrap().push(tier));
    }
    player.set_track_max_tier(3);
    tap(&ui, slint::platform::Key::Return); // 打开音质弹层
    settle(64);
    tap(&ui, slint::platform::Key::DownArrow);
    tap(&ui, slint::platform::Key::DownArrow);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        *selects.lock().unwrap(),
        vec![2],
        "↓×2 + Enter 应选择第 3 档（无损）"
    );
    settle(64);

    // 关闭后焦点回徽章：Enter 重新打开；End 到最高档并选定
    tap(&ui, slint::platform::Key::Return);
    settle(64);
    tap(&ui, slint::platform::Key::End);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        *selects.lock().unwrap(),
        vec![2, 3],
        "再开弹层 End + Enter 应选最高档"
    );
    settle(64);

    // 最高档降级：End 落到本曲最高档（不越界）
    player.set_track_max_tier(1);
    tap(&ui, slint::platform::Key::Return);
    settle(64);
    tap(&ui, slint::platform::Key::End);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        *selects.lock().unwrap(),
        vec![2, 3, 1],
        "End 应钳到本曲最高档（高清），不得选到未提供档位"
    );
    settle(64);

    // Esc 关闭后焦点回徽章：回车重开 + ↓ + Enter = 第 2 档
    tap(&ui, slint::platform::Key::Return);
    settle(64);
    tap(&ui, slint::platform::Key::Escape);
    settle(64);
    tap(&ui, slint::platform::Key::Return);
    settle(64);
    tap(&ui, slint::platform::Key::DownArrow);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        *selects.lock().unwrap(),
        vec![2, 3, 1, 1],
        "Esc 后焦点回徽章，回车应能重开弹层并选定下一档"
    );
    assert!(baseline_plays < plays.lock().unwrap().len());
}
