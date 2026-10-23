//! 远端详情导航集成测试（TestingBackend；tests/context_menu.rs 基建）。
//!
//! - O2：on_navigate A→B + A 迟到回包按代数丢弃（R4 竞态防护）；
//! - O1：qq: 参数导航后 state=1 在 UI 线程同步置位（不套 invoke）；
//! - D-5：远端曲目行歌手/专辑链接带 qq: 前缀。
//!
//! 出网隔离：导航会真实 spawn 请求——HMP_IPC_ENDPOINT 指向不存在管道令
//! 其快速失败；no_event_loop 下其迟到回包的 invoke 闭包不会被泵执行，
//! 断言只依赖手工注入的回包（确定性）。

use slint::{ComponentHandle, Global, Model};

#[test]
fn remote_playlist_navigation_drops_late_response() {
    // SAFETY: 单线程测试进程内串行执行（本文件仅一个测试）；
    // 值唯一，不影响其他测试二进制。
    unsafe {
        std::env::set_var("HMP_IPC_ENDPOINT", r"\\.\pipe\hmp-test-remote-nav-bogus");
    }
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    let runtime = std::sync::Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap());
    let prefs = std::sync::Arc::new(std::sync::Mutex::new(hmp_desktop::prefs::Prefs::default()));
    hmp_desktop::bridge::bind(&ui, prefs, std::sync::Arc::clone(&runtime));

    let nav = hmp_desktop::Nav::get(&ui);
    let data = hmp_desktop::Data::get(&ui);

    // —— A：远端歌单 qq:111 → state=1 同步置位（O1，导航栈内直接 set）———
    nav.invoke_navigate(hmp_desktop::Route::Playlist, "qq:111".into());
    assert_eq!(data.get_playlist_state(), 1, "导航远端参数即同步进入加载态");
    let gen_a = hmp_desktop::remote_details::current_playlist_generation();

    // —— B：快速切到 qq:222 → A 代数过期 ——
    nav.invoke_navigate(hmp_desktop::Route::Playlist, "qq:222".into());
    let gen_b = hmp_desktop::remote_details::current_playlist_generation();
    assert!(gen_b > gen_a, "重新装载必须 bump 代数");
    assert_eq!(data.get_playlist_state(), 1);

    // —— A 的迟到回包（携带有效数据）→ 按代数丢弃，不得污染状态 ——
    let page_a = hmp_core::RemotePlaylistDetail {
        id: 111,
        name: "A 歌单（迟到）".into(),
        ..Default::default()
    };
    hmp_desktop::remote_details::apply_playlist_response(
        &ui.as_weak(),
        &runtime,
        gen_a,
        Ok(hmp_core::Response::PlaylistDetail(page_a)),
    );
    assert_eq!(data.get_playlist_state(), 1, "迟到回包不得改变状态");
    assert_eq!(
        data.get_playlist_name().as_str(),
        "",
        "迟到回包不得落地数据"
    );

    // —— B 的回包 → 落地（就绪 + 投影 + D-5 行内链接）———
    let page_b = hmp_core::RemotePlaylistDetail {
        id: 222,
        name: "B 歌单".into(),
        creator: "胡桃".into(),
        tracks: vec![hmp_core::DiscoverNewSong {
            mid: "m-1".into(),
            name: "歌一".into(),
            singer: "歌手甲".into(),
            album: "专辑甲".into(),
            interval: 100,
            picurl: String::new(),
            album_mid: "am-1".into(),
            artist_mid: "sm-1".into(),
        }],
        total: 1,
        ..Default::default()
    };
    hmp_desktop::remote_details::apply_playlist_response(
        &ui.as_weak(),
        &runtime,
        gen_b,
        Ok(hmp_core::Response::PlaylistDetail(page_b)),
    );
    assert_eq!(data.get_playlist_state(), 2, "当前代回包落地就绪态");
    assert_eq!(data.get_playlist_name().as_str(), "B 歌单");
    let rows = data.get_playlist_tracks();
    let first = rows.iter().next().expect("曲目行已落地");
    assert_eq!(first.artist_mid.as_str(), "qq:sm-1", "D-5 行内歌手链接");
    assert_eq!(first.album_mid.as_str(), "qq:am-1", "D-5 行内专辑链接");
}
