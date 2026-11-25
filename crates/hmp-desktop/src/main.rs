//! HMP 桌面应用入口：Slint UI（通透胡桃木设计语言）+ daemon 播放桥（M8）。
//! UI 是 daemon 的又一个适配器（docs/PROJECT.md §8.6）：IPC 传输在 backend.rs，
//! 状态映射在 player_bridge.rs；库页数据经 library_view 直读媒体库（离线降级空态）。

use std::path::Path;
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global};

use hmp_desktop::{backend, bridge, exit_guard, menu_bridge, player_bridge, prefs};

/// 日志轮转阈值：启动时 `desktop.log` 超过此大小 → 改名 `desktop.log.1`
/// （只留一份历史，覆盖上一份）。
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

fn main() -> Result<(), slint::PlatformError> {
    init_logging();

    let prefs = Arc::new(Mutex::new(prefs::load()));

    // IPC runtime（bootstrap 第一步）：订阅任务挂上后随 main 存活到 run() 退出。
    let runtime = Arc::new(backend::BackendRuntime::new().expect("tokio runtime"));

    let ui = hmp_desktop::AppWindow::new()?;

    {
        let loaded = *prefs.lock().expect("prefs");
        hmp_desktop::Theme::get(&ui).set_mode(loaded.theme_mode);
        hmp_desktop::Quality::get(&ui).set_selected(loaded.quality);
        hmp_desktop::TrackMenu::get(&ui).set_enabled(loaded.context_menu);
        hmp_desktop::Theme::get(&ui).set_reduce_motion(loaded.reduce_motion);
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

    // 键盘导航（M7 焦点可达）：任意指针按下即退出键盘模式——聚焦中的控件
    // 让出焦点，空格/ESC 回到全局收口（ui/keynav.slint 焦点模型）。Slint
    // 语言层没有全局指针监听原语（事件被命中的 TouchArea accept 后不再
    // 冒泡到祖先），经 winit 事件钩子登记：钩子先于 Slint 派发执行、在 UI
    // 线程上，直接写全局属性。testing/非 winit 后端无此钩子（集成测试显式
    // 调用 keynav::pointer_activity）。
    {
        use slint::winit_030::WinitWindowAccessor;
        use slint::winit_030::winit::event::{ElementState, WindowEvent};
        let ui_weak = ui.as_weak();
        ui.window().on_winit_window_event(move |_window, event| {
            match event {
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    ..
                }
                | WindowEvent::Touch(..) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        hmp_desktop::keynav::pointer_activity(&ui);
                    }
                }
                _ => {}
            }
            slint::winit_030::EventResult::Propagate
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

/// 日志初始化（U3）：此前桌面端不装 subscriber，全仓的 `tracing::warn!` /
/// `error!` 运行时是空操作（封面、歌词、连接失败不留痕迹）。级别取
/// `RUST_LOG`，取不到用 warn；追加写 `<cache_dir>/logs/desktop.log`，文件
/// 打不开退回 stderr；`try_init` 避免重复初始化 panic。
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false);
    match open_log_file() {
        // `Arc<File>` 即 MakeWriter（`&File` 实现 io::Write）
        Some(file) => {
            let _ = builder.with_writer(Arc::new(file)).try_init();
        }
        None => {
            let _ = builder.try_init();
        }
    }
}

/// 打开追加写的日志文件：建 `logs/` 目录 → 启动期轮转 → 以 append 打开；
/// 任一步失败 → None（调用方退回 stderr）。
fn open_log_file() -> Option<std::fs::File> {
    let dir = hmp_storage::cache_dir().join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("desktop.log");
    rotate_oversized_log(&path);
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

/// 启动期轮转：日志已超过 [`LOG_MAX_BYTES`] → 改名 `desktop.log.1`（覆盖
/// 上一份）并返回 true；文件不存在 / 读不到大小 / 改名失败 → false（轮转
/// 只是保洁，失败则照常追加）。
fn rotate_oversized_log(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > LOG_MAX_BYTES => {}
        _ => return false,
    }
    let previous = path.with_extension("log.1");
    // Windows 上 rename 不覆盖已存在的目标：先移除上一份
    let _ = std::fs::remove_file(&previous);
    std::fs::rename(path, previous).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 临时日志路径（temp_dir + 进程号，不碰真实缓存目录）。
    fn temp_log(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hmp-desktop-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("desktop.log")
    }

    /// U3 轮转判断：未超限原样保留；超过 5 MiB 改名 desktop.log.1（覆盖
    /// 上一份）；文件不存在不轮转。
    #[test]
    fn log_rotates_only_when_over_limit() {
        let path = temp_log("rotate");
        let rotated = path.with_extension("log.1");

        // 文件不存在 → 不轮转
        assert!(!rotate_oversized_log(&path));

        // 恰好 5 MiB：不算「超过」，不轮转
        std::fs::write(&path, vec![b'x'; LOG_MAX_BYTES as usize]).unwrap();
        assert!(!rotate_oversized_log(&path));
        assert!(path.exists() && !rotated.exists());

        // 超过 1 字节 → 轮转：原文件移走，.1 承接内容
        std::fs::write(&path, vec![b'x'; LOG_MAX_BYTES as usize + 1]).unwrap();
        assert!(rotate_oversized_log(&path));
        assert!(!path.exists());
        assert_eq!(
            std::fs::metadata(&rotated).unwrap().len(),
            LOG_MAX_BYTES + 1
        );

        // 旧 .1 被新一份覆盖
        std::fs::write(&path, vec![b'y'; LOG_MAX_BYTES as usize + 1]).unwrap();
        assert!(rotate_oversized_log(&path));
        assert_eq!(std::fs::read(&rotated).unwrap()[0], b'y');

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
