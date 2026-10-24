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

    // S10 T3：X 关窗统一到「Visible 退出」——默认 hide 会让窗口在事件循环
    // Exit 处理时被 suspend（GL 销毁在循环内执行）；保持 Visible 直接退出后，
    // 与 quit 路径合流到下方 leak-on-exit 收口（详见 exit_guard 模块 doc）。
    exit_guard::install_exit_on_close(&ui);

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

    // S10 T1：ui 的 drop 链会销毁 femtovg GL context/surface（wglDeleteContext
    // 等驱动内核态调用，E1 两臂实锤卡死成不可杀僵尸）。run() 尾部 hide() 已
    // 解除组件自持环（strong_component_ref.take()），main 的 ui 是最后强引用
    // ——forget 它即无人触发 adapter/renderer drop，GL 销毁调用整链不发生；
    // GPU 资源交给内核进程 rundown 回收（与任何 GL 进程崩溃同路径）。
    // 注：run() 尾部 hide 在 Windows 只走 winit SW_HIDE、不含 GL；forget 后
    // 退出段再无任何 GL 调用（worktree FINDINGS.md §1.2-3）。
    tracing::info!("event loop exited; skipping GL teardown (S10 leak-on-exit)");
    let guard = exit_guard::arm(exit_guard::EXIT_WATCHDOG, || std::process::exit(0));
    std::mem::forget(ui);
    // 只减 main 的引用计数：bridge/player_bridge 的回调闭包（已随 ui 泄漏）
    // 持 Arc clone，runtime 有意不 shutdown，worker 线程由 ExitProcess 回收
    // （FINDINGS §3c-1——比显式 shutdown 更少一段用户态等待）。
    drop(runtime);
    // 清理完成：解除看门狗。arm→disarm 之间若用户态卡死（锁/阻塞 IO），看门狗
    // 线程 exit(0) 兜底；内核态卡死任何 exit 都无效（S10 根因形态），主防线
    // 是上面的 forget——见 exit_guard 模块 doc 的语义边界。
    guard.disarm();
    Ok(())
}
