//! 侧栏歌单区键盘可达 + 歌单级菜单（重命名/删除）集成测试。
//!
//! 守护 M7 收口（AUDIT §29）：
//! 1. 侧栏歌单行不再是「只有行窗内的行才是 Tab 停靠点」：每组用一个 roving
//!    ListKeys 停靠点，↑/↓ 走遍全部歌单（窗口化虚拟化只影响绘制），
//!    Enter 打开歌单；
//! 2. Menu 键弹出歌单级菜单：重命名（仅 local）/ 删除（subscribed = 取消
//!    收藏、owned 走云端同步，UI 侧先按 relation 决定可用性）；删除两步确认；
//! 3. 菜单关闭后焦点回触发控件（侧栏组列表 / 歌单页「⋯」），光标停在触发
//!    行——Enter 能继续操作；
//! 4. 重命名复用 NewPlaylistForm：弹层打开带入原名并全选，提交回调发出新名。
//!
//! 到达侧栏自建组列表：根作用域 Tab 正向 9 个主导航 + 「自建歌单」分组钮 +
//! 「+」 = 12 次。歌单页「⋯」：9 导航 + 自建组头/+/组列表 + 收藏组头/组列表
//! = 14，再 + 页头返回 = 15、+「⋯」= 16（本题不播种曲目 → 无「播放全部」）。
//! 播种的侧栏模型不依赖真实媒体库。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, Model, ModelRc, SharedString, VecModel, platform::WindowEvent};

use hmp_desktop::{Data, Nav, PlaylistCover, PlaylistMenu, Route, TrackRow};

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

fn click(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    for pressed in [true, false] {
        ui.window().dispatch_event(if pressed {
            WindowEvent::PointerPressed {
                position: slint::LogicalPosition::new(x, y),
                button: slint::platform::PointerEventButton::Left,
            }
        } else {
            WindowEvent::PointerReleased {
                position: slint::LogicalPosition::new(x, y),
                button: slint::platform::PointerEventButton::Left,
            }
        });
    }
}

fn shift_tab(ui: &hmp_desktop::AppWindow) {
    let shift: SharedString = slint::platform::Key::Shift.into();
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: shift.clone() });
    tap(ui, slint::platform::Key::Tab);
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: shift });
}

fn type_text(ui: &hmp_desktop::AppWindow, text: &str) {
    for ch in text.chars() {
        let ch = ch.to_string();
        tap(ui, ch.as_str());
    }
}

fn playlist(id: &str, name: &str, relation: &str) -> PlaylistCover {
    PlaylistCover {
        id: id.into(),
        name: name.into(),
        relation: relation.into(),
        image: slint::Image::default(),
    }
}

#[test]
fn sidebar_playlist_roving_and_manage_menu() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data(
        &ui,
        &Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap()),
    );

    {
        let data = Data::get(&ui);
        data.set_sidebar_created(ModelRc::new(VecModel::from(vec![
            playlist("801", "歌单一", "local"),
            playlist("802", "歌单二", "local"),
            playlist("803", "歌单三", "local"),
        ])));
        data.set_sidebar_favorited(ModelRc::new(VecModel::from(vec![playlist(
            "901",
            "收藏的歌单",
            "subscribed",
        )])));
        // 歌单页渲染就绪所需的空曲目表（不播种 → 无「播放全部」，Tab 计数稳定）
        data.set_playlist_tracks(ModelRc::new(VecModel::from(Vec::<TrackRow>::new())));
    }
    settle(320);

    let nav = Nav::get(&ui);
    let navs: Arc<Mutex<Vec<(Route, String)>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let navs = Arc::clone(&navs);
        let ui_weak = ui.as_weak();
        nav.on_navigate(move |route, param| {
            navs.lock().unwrap().push((route, param.to_string()));
            if let Some(ui) = ui_weak.upgrade() {
                let nav = Nav::get(&ui);
                nav.set_route(route);
                nav.set_param(param);
            }
        });
    }
    let deletes: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let renames: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let deletes = Arc::clone(&deletes);
        let ui_weak = ui.as_weak();
        PlaylistMenu::get(&ui).on_delete(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let menu = PlaylistMenu::get(&ui);
                deletes
                    .lock()
                    .unwrap()
                    .push((menu.get_id().to_string(), menu.get_relation().to_string()));
            }
        });
    }
    {
        let renames = Arc::clone(&renames);
        PlaylistMenu::get(&ui).on_rename(move |name| {
            renames.lock().unwrap().push(name.to_string());
        });
    }

    // —— 1. 组级 roving：Tab 12 到自建组列表，↓×2 + Enter 打开第 3 个歌单 ——
    let menu = PlaylistMenu::get(&ui);
    for _ in 0..12 {
        tap(&ui, slint::platform::Key::Tab);
    }
    tap(&ui, slint::platform::Key::DownArrow);
    tap(&ui, slint::platform::Key::DownArrow);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        navs.lock().unwrap().last(),
        Some(&(Route::Playlist, "803".to_string())),
        "组级 roving 光标应能到达行窗外的第 3 个歌单并 Enter 打开"
    );

    // —— 2. 重命名：回到组列表 → Menu → 首项 Enter → 命名弹层（带入原名
    //    全选）→ 打字提交 ——
    // 路由切换会把焦点交还根作用域（app.slint route-focus-watch），先回到组列表
    for _ in 0..12 {
        tap(&ui, slint::platform::Key::Tab);
    }
    tap(&ui, slint::platform::Key::Menu);
    settle(64);
    assert_eq!(menu.get_id().as_str(), "803");
    assert_eq!(menu.get_source(), 0, "来自侧栏自建组");
    let rename_seq_before = menu.get_rename_seq();
    tap(&ui, slint::platform::Key::Return);
    settle(128);
    assert_eq!(
        menu.get_rename_seq(),
        rename_seq_before + 1,
        "菜单首项应发出重命名请求"
    );
    type_text(&ui, "新名字");
    tap(&ui, slint::platform::Key::Return);
    settle(64);
    {
        let renames = renames.lock().unwrap();
        assert_eq!(renames.len(), 1, "提交应发出一次 rename 回调");
        assert!(
            renames[0].ends_with("新名字"),
            "弹层带入原名并全选，打字应替换为「新名字」，实际 {}",
            renames[0]
        );
    }

    // —— 3. Menu 键 → 歌单菜单：↓ 到「删除」→ 两步确认 ——
    // 命名弹层关闭后焦点已回组列表（e2e 的落焦点逻辑），直接按 Menu
    let open_before = menu.get_open_seq();
    tap(&ui, slint::platform::Key::Menu);
    settle(64);
    assert_eq!(
        menu.get_open_seq(),
        open_before + 1,
        "Menu 键应打开歌单级菜单"
    );
    tap(&ui, slint::platform::Key::DownArrow);
    tap(&ui, slint::platform::Key::Return);
    settle(64);
    assert!(deletes.lock().unwrap().is_empty(), "首次激活只进入确认态");
    tap(&ui, slint::platform::Key::Return);
    settle(64);
    assert_eq!(
        &*deletes.lock().unwrap(),
        &[("803".to_string(), "local".to_string())],
        "再次激活才执行删除，relation 随快照下发"
    );

    // —— 4. 菜单关闭后焦点回组列表（光标停在 803）：Enter 再次打开 ——
    let nav_count = navs.lock().unwrap().len();
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        navs.lock().unwrap().len(),
        nav_count + 1,
        "关闭菜单后焦点应回组列表"
    );
    assert_eq!(
        navs.lock().unwrap().last(),
        Some(&(Route::Playlist, "803".to_string()))
    );

    // —— 5. 歌单页「⋯」：owned 歌单的重命名禁用、删除按云端语义 ——
    Data::get(&ui).set_playlist_state(2);
    Data::get(&ui).set_playlist_name("云端歌单".into());
    Data::get(&ui).set_playlist_relation("owned".into());
    nav.invoke_navigate(Route::Playlist, "801".into());
    settle(200);
    // 页头「⋯」几何：内容区 x=240、页内距 32、封面 176 + 缝 32 → 页头信息列
    // 宽 760，动作贴右缘 → 36px 钮中心 x≈1222；页头行 y=64 h=176，动作垂直
    // 居中 → 钮中心 y≈152。
    let open_before5 = menu.get_open_seq();
    click(&ui, 1222.0, 152.0);
    settle(64);
    assert_eq!(
        menu.get_open_seq(),
        open_before5 + 1,
        "点「⋯」应打开歌单级菜单"
    );
    assert_eq!(menu.get_source(), 2, "页头「⋯」应打开同一菜单");
    assert_eq!(menu.get_relation().as_str(), "owned");
    let rename_count = renames.lock().unwrap().len();
    // 重命名禁用 → 打开时光标已在「删除」；首次激活只进入确认态
    tap(&ui, slint::platform::Key::Return);
    settle(64);
    assert_eq!(
        renames.lock().unwrap().len(),
        rename_count,
        "owned 歌单首项不是重命名（云端禁用）"
    );
    assert!(
        deletes.lock().unwrap().last() != Some(&("801".to_string(), "owned".to_string())),
        "首次激活只进入删除确认态"
    );
    tap(&ui, slint::platform::Key::Return);
    settle(64);
    assert_eq!(
        deletes.lock().unwrap().last(),
        Some(&("801".to_string(), "owned".to_string())),
        "owned 删除走 daemon 云端同步语义（relation 随快照）"
    );

    // —— 6. 关闭后焦点回「⋯」：Enter 可再次打开菜单 ——
    let open_before = menu.get_open_seq();
    tap(&ui, slint::platform::Key::Return);
    settle(64);
    assert_eq!(
        menu.get_open_seq(),
        open_before + 1,
        "菜单关闭后焦点应回页头「⋯」"
    );
    tap(&ui, slint::platform::Key::Escape);
    settle(64);

    // —— 7. subscribed 删除 = 取消收藏（文案/回调按 relation 分流）——
    Data::get(&ui).set_playlist_name("收藏的歌单".into());
    Data::get(&ui).set_playlist_relation("subscribed".into());
    nav.invoke_navigate(Route::Playlist, "901".into());
    settle(200);
    click(&ui, 1222.0, 152.0);
    settle(64);
    assert_eq!(menu.get_relation().as_str(), "subscribed");
    let rename_count = renames.lock().unwrap().len();
    tap(&ui, slint::platform::Key::Return); // 首项禁用 → 光标在「取消收藏」
    settle(64);
    assert_eq!(
        renames.lock().unwrap().len(),
        rename_count,
        "subscribed 歌单不提供重命名"
    );
    tap(&ui, slint::platform::Key::Return);
    settle(64);
    assert_eq!(
        deletes.lock().unwrap().last(),
        Some(&("901".to_string(), "subscribed".to_string())),
        "subscribed 删除 = 取消收藏（daemon unfavorite 语义）"
    );
}
