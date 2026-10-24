//! HMP 桌面应用入口：Slint UI（通透胡桃木设计语言）+ daemon 播放桥（M8）。
//! UI 是 daemon 的又一个适配器（docs/PROJECT.md §8.6）：IPC 传输在 backend.rs，
//! 状态映射在 player_bridge.rs；库页数据经 library_view 直读媒体库（离线降级空态）。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global};

use hmp_desktop::{backend, bridge, exit_guard, menu_bridge, player_bridge, prefs};

fn main() -> Result<(), slint::PlatformError> {
    let prefs = Arc::new(Mutex::new(prefs::load()));

    // IPC runtime（bootstrap 第一步）：订阅任务挂上后随 main 存活到 run() 退出。
    let runtime = Arc::new(backend::BackendRuntime::new().expect("tokio runtime"));

    let ui = hmp_desktop::AppWindow::new()?;

    {
        let loaded = *prefs.lock().expect("prefs");
        hmp_desktop::Theme::get(&ui).set_mode(loaded.theme_mode);
        hmp_desktop::Quality::get(&ui).set_selected(loaded.quality);
        hmp_desktop::TrackMenu::get(&ui).set_enabled(loaded.context_menu);
    }

    bridge::load_data(&ui, &runtime);
    bridge::bind(&ui, Arc::clone(&prefs), Arc::clone(&runtime));
    // 播放桥：连接/拉起 daemon → 订阅状态推送；彻底失败降级离线
    // （Player 全空、命令 no-op），UI 照常打开。
    player_bridge::bind(&ui, Arc::clone(&runtime), Arc::clone(&prefs));
    // 右键菜单动作桥（启停持久化 + PlayNext/QueueAppend/PlaylistWrite 派发）
    menu_bridge::bind(&ui, Arc::clone(&prefs), Arc::clone(&runtime));

    // 顶栏拖动桥（title-bar.slint）：Slint 语言层没有窗口移动原语，经 winit
    // 句柄发起系统级移动循环（拖到屏缘的贴靠/半屏分列由系统原生处理）。
    // testing/非 winit 后端拿不到 winit 句柄 → with_winit_window 返回 None，
    // 回调退化为空操作。SC_MOVE 模态循环会吞掉 pointer release，返回后补发
    // 合成抬起复位 TouchArea 抓取态（落点 (0,0) 在顶栏命中区内，无副作用）。
    {
        use slint::winit_030::WinitWindowAccessor;
        let ui_weak = ui.as_weak();
        hmp_desktop::WinChrome::get(&ui).on_drag(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let _ = ui.window().with_winit_window(|w| w.drag_window());
                ui.window()
                    .dispatch_event(slint::platform::WindowEvent::PointerReleased {
                        position: slint::LogicalPosition::new(0.0, 0.0),
                        button: slint::platform::PointerEventButton::Left,
                    });
            }
        });
    }

    ui.run()?;

    // S10 退出段编排（真根因修复在 backend::BackendRuntime T7：runtime 与
    // 进程同寿，ExitProcess 期 TLS 析构不再触发 Runtime::drop 的
    // BlockingPool::shutdown 死锁——见 backend.rs 模块 doc 与 worktree
    // FINDINGS.md）。这里只留看门狗做**纯纵深**：武装期间完成重量级清理，
    // 用户态挂起（假想的锁/阻塞 IO 卡 drop）由 exit(0) 兜底；内核态/
    // loader 关停期卡死任何 exit 都无效（历史上正是后者，已由 T7 消灭）。
    let guard = exit_guard::arm(exit_guard::EXIT_WATCHDOG, || std::process::exit(0));
    // 含 femtovg GL context 销毁——基线臂 3/3 实证该段正常完成（真卡点在
    // 更晚的 ExitProcess 期 TLS/tokio 段，与渲染器无关：GL 与 software
    // 僵尸同栈，verify-s10 僵尸栈存档）。
    drop(ui);
    // T7 下只递减引用计数（schedule_refresh 的 TLS Timer 持有最后引用，
    // 其析构 drop 为 no-op——这正是死锁被消灭的机制）。
    drop(runtime);
    guard.disarm();
    Ok(())
}
