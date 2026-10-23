//! 右键菜单「加入播放列表…」二级 picker 链路回归（S6 验收缺陷 #1）：
//! 弹层内完整点击不得关闭菜单（宿主 PopupWindow close-on-click-outside），
//! picker 可达、选歌单后菜单整体关闭且动作生效。
//!
//! 断言手段 = 行为断言（不扩 API 面、不动 context-menu.slint）：
//! `add-to-playlist` 只能来自 picker 内歌单行的命中 → 恰一次且 id 正确，
//! 即证明「点『加入播放列表…』后弹层存活且 picker 行可点」；选完歌单后
//! 在旧弹层区下方左击表格行，`play-tracks` 新增即证明弹层已整体关闭
//! （弹层若仍开，该点击落在弹层命中区内 → 被弹层吞掉、到不了表格——
//! 实测弹层命中区在切 picker 后仍保持主模式 248px 矩形，见 PICKER_ROW0_Y）。
//!
//! 修复前（slint 默认 close-on-click）：点「加入播放列表…」的完整点击当帧
//! 即被框架关掉弹层，随后坐标落到表格行触发 play-tracks，add-to-playlist
//! 永不触发 → 本测试必红（S6 verify/REPORT.md 缺陷 #1 证据链；本仓实施时
//! 已实测红证：adds 恒空）。
//!
//! testing backend 的 platform 是进程级单例，本文件只放一个 #[test]。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, LogicalPosition, platform::WindowEvent};

/// 曲目行内命中点（同 context_menu.rs：y=350 落在首行中部，避开行界与表头；
/// 实测本测试播种下首行顶 ≈325、行距 41）
const ROW_X: f32 = 640.0;
const ROW_Y: f32 = 350.0;

// —— 弹层面板几何（布局公式 + S6 像素标定交叉验证；面板 216px 定宽）———
// 右键释放点即弹层锚：open-x/open-y = 点击点在行内的偏移，宿主 PopupWindow
// 再 +4px 偏移 → 面板左上 = (ROW_X, ROW_Y + 4px) = (640, 354)（窗口 1280×800
// 内无 place_popup 钳制：640+216=856 ≤ 1280，354+248=602 ≤ 800）。
const PANEL_X: f32 = 640.0;
const PANEL_Y: f32 = 354.0;
/// 面板水平中心
const PANEL_CX: f32 = PANEL_X + 216.0 / 2.0;
/// 主模式面板高（context-menu.slint：16 + 6 行×36 + 2 分隔线×1 + 7 间距×2 = 248）
const PANEL_MAIN_H: f32 = 248.0;
/// picker 面板高（16 + 返回行 36 + 分隔线 1 + 4 + 2 歌单行×36 = 129；
/// 歌单行数 = min(Data.sidebar-created, 6)，本测试播种 2 条）
const PANEL_PICKER_H: f32 = 129.0;
/// 主模式「加入播放列表…」行（第 4 行）中心 y（布局：padding 8 / 行高 36 /
/// 行距 2 / 分隔线 1px 且前后各 2px spacing）：
/// 354 + 8(padding) + 36×3(前三行) + 2×3(前三行后 spacing) +
/// 1(分隔线) + 2(分隔线后 spacing) + 18(半行高) = 497
/// （交叉验证：S6 实测 lib-light 分隔线 y=466/507，与本公式在面板顶 344
/// 时推算的 466/507 逐像素吻合。）
const ADD_PLAYLIST_ROW_Y: f32 = PANEL_Y + 8.0 + 36.0 * 3.0 + 2.0 * 3.0 + 1.0 + 2.0 + 18.0;
/// picker 首个歌单行中心 y。注意：弹层内容变高（248→129）时 slint 1.17.1
/// **不重摆弹层**——命中区保持主模式 248px 矩形，可见面板在原矩形内居中
/// 重绘（软件渲染快照实测，两处锚点严格 +60：(248-129)/2=59.5，渲染顶
/// 354→414、434→494；返回行文字中心 440、分隔线 460、歌单行 0 区间
/// 463..499 均与 414 推导逐像素吻合）。行 0 中心 =
/// 414 + 8(padding) + 36(返回行) + 2 + 1(分隔线) + 2 + 18(半行高) = 481
const PICKER_ROW0_Y: f32 = PANEL_Y + 60.0 + 8.0 + 36.0 + 2.0 + 1.0 + 2.0 + 18.0;
/// 关闭探针点：弹层命中区（保持主模式矩形，底缘 602）之内、表格行之内
/// ——弹层已关则左击触达表格；未关则点击被弹层吞掉（点在可见 picker 面板
/// 之外的命中区空白处），play-tracks 不增。
const PROBE_Y: f32 = 560.0;

fn settle() {
    for _ in 0..40 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

fn click(ui: &hmp_desktop::AppWindow, x: f32, y: f32, button: slint::platform::PointerEventButton) {
    for kind in [
        WindowEvent::PointerPressed {
            position: LogicalPosition::new(x, y),
            button,
        },
        WindowEvent::PointerReleased {
            position: LogicalPosition::new(x, y),
            button,
        },
    ] {
        ui.window().dispatch_event(kind);
    }
}

/// picker 数据源播种的一条自建歌单（relation "local"：QQ 曲 source=0 全可入）。
fn playlist(id: &str, name: &str) -> hmp_desktop::PlaylistCover {
    hmp_desktop::PlaylistCover {
        id: id.into(),
        name: name.into(),
        relation: "local".into(),
        image: slint::Image::default(),
    }
}

#[test]
fn context_menu_picker_reachable_and_closes_on_selection() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data(
        &ui,
        &Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap()),
    );

    // 播种曲库（QQ 曲 source=0；行内歌手/专辑链接置空，右键路径走行宿主，
    // 测试命中点稳定不依赖链接区几何——同 context_menu.rs 播种策略）
    // + 自建歌单两条（picker 列表 = Data.sidebar-created）
    {
        let data = hmp_desktop::Data::get(&ui);
        let rows: Vec<hmp_desktop::TrackRow> = (0..30)
            .map(|i| hmp_desktop::TrackRow {
                mid: format!("picker-track-{i}").into(),
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
        data.set_liked(slint::ModelRc::new(slint::VecModel::from(rows)));
        data.set_liked_count(count);
        data.set_sidebar_created(slint::ModelRc::new(slint::VecModel::from(vec![
            playlist("701", "测试歌单一"),
            playlist("702", "测试歌单二"),
        ])));
    }
    settle();

    let menu = hmp_desktop::TrackMenu::get(&ui);
    let player = hmp_desktop::Player::get(&ui);

    let adds: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let adds = Arc::clone(&adds);
        menu.on_add_to_playlist(move |playlist_id| {
            adds.lock().unwrap().push(playlist_id.to_string());
        });
    }
    let plays: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let plays = Arc::clone(&plays);
        player.on_play_tracks(move |_tracks, start| {
            plays.lock().unwrap().push(start);
        });
    }

    // —— 0. 左键标定命中行（同 context_menu.rs 手法）+ 记 play-tracks 基线 ———
    click(&ui, ROW_X, ROW_Y, slint::platform::PointerEventButton::Left);
    settle();
    let hit_index = *plays
        .lock()
        .unwrap()
        .last()
        .expect("左键应命中曲目行触发 play-tracks");
    let baseline_plays = plays.lock().unwrap().len();

    // —— 1. 右键打开菜单（快照与标定行互为印证，菜单确已开在锚点）———
    click(
        &ui,
        ROW_X,
        ROW_Y,
        slint::platform::PointerEventButton::Right,
    );
    settle();
    assert_eq!(
        menu.get_index(),
        hit_index,
        "右键后菜单快照应指向标定行（菜单未开则后续步骤必失败于下一条断言）"
    );

    // —— 2. 点「加入播放列表…」：弹层必须存活并切 picker ——
    // 默认 close-on-click 下此完整点击即被框架关掉弹层（修复前红点）。
    click(
        &ui,
        PANEL_CX,
        ADD_PLAYLIST_ROW_Y,
        slint::platform::PointerEventButton::Left,
    );
    settle();

    // —— 3. 点 picker 首个歌单行：add-to-playlist 恰一次且 id 正确 ——
    // 弹层若在第 2 步被关，此坐标落到表格行触发 play-tracks 而非本动作。
    click(
        &ui,
        PANEL_CX,
        PICKER_ROW0_Y,
        slint::platform::PointerEventButton::Left,
    );
    settle();
    {
        let adds = adds.lock().unwrap();
        assert_eq!(
            &*adds,
            &["701".to_string()],
            "picker 歌单行命中应触发 add-to-playlist(\"701\")；弹层若在上一步被框架关闭则永不触发（S6 缺陷 #1）"
        );
    }

    // —— 4. 整体关闭探针：旧弹层区下方左击表格行 ——
    // play-tracks 恰新增一次（点击穿透到表格 ⇔ 弹层已随选择整体关闭；
    // 弹层若仍开，点击在其命中区内 → 被吞掉、play-tracks 不增），
    // 且 add-to-playlist 不再增加。
    click(
        &ui,
        PANEL_CX,
        PROBE_Y,
        slint::platform::PointerEventButton::Left,
    );
    settle();
    assert_eq!(
        plays.lock().unwrap().len(),
        baseline_plays + 1,
        "探针点击应触达表格行（弹层已整体关闭）"
    );
    assert_eq!(
        adds.lock().unwrap().len(),
        1,
        "探针不得再次触发 add-to-playlist"
    );
    // 几何常量自洽钉（漂移时首查此处）：主面板与 picker 面板高度公式。
    debug_assert_eq!(PANEL_MAIN_H, 16.0 + 6.0 * 36.0 + 2.0 * 1.0 + 7.0 * 2.0);
    debug_assert_eq!(PANEL_PICKER_H, 16.0 + 36.0 + 1.0 + 4.0 + 2.0 * 36.0);
}
