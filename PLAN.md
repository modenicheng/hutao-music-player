# S10 PLAN：桌面退出死锁修复（fix/desktop-exit-deadlock 实施计划）

- 输入：FINDINGS.md（本 worktree 根）。根因：退出路径 femtovg GL context/surface
  销毁的驱动内核态调用不返回（置信 85%）；**已发生的内核态卡死无法被任何进程内
  手段终止**（TerminateProcess/ExitProcess 都要等线程出内核）⇒ 修复必须让销毁
  调用根本不发生。
- 方案（FINDINGS §3c）：**leak-on-exit 为主 + X 关窗路径统一 + 次级用户态看门狗**。
- 所有权：仅 `crates/hmp-desktop/**`（+ 两文档）。禁碰 daemon 侧、hmp-core、
  docs/**。无协议/数据格式/依赖变化。

## 非目标

- 不修 GL/驱动本身、不换默认 renderer、不 patch slint（`SLINT_BACKEND=
  winit-software` 仅作为文档提及的用户应急逃生门）。
- 不处理「僵尸周期拉起隔离 daemon」的观察（FINDINGS §1-S5：非根因，是伴生危害；
  僵尸不再产生即消失）。
- 不做 AUDIT.md 登记（docs/ 禁碰；§10 附建议文案，由用户/协调者落账）。

## 任务分解

### T1 退出编排重构（main.rs）

`ui.run()` 返回后改为：

```rust
ui.run()?;
// S10：ui 的 drop 链会销毁 femtovg GL context/surface（wglDeleteContext 等
// 驱动内核态调用，E1 两臂实锤卡死成不可杀僵尸）。run() 尾部 hide() 已解除
// 组件自持环（strong_component_ref.take()），main 的 ui 是最后强引用——
// forget 它即无人触发 adapter/renderer drop，GL 销毁调用整链不发生；GPU
// 资源交给内核进程 rundown（与 GL 进程崩溃同路径）。
// 注：run() 尾部 hide 在 Windows 只走 winit SW_HIDE、不含 GL；forget 后
// 退出段再无任何 GL 调用（FINDINGS §1.2-3）。
tracing::info!("event loop exited; skipping GL teardown (S10 leak-on-exit)");
let guard = exit_guard::arm(EXIT_WATCHDOG, || std::process::exit(0));
std::mem::forget(ui);
drop(runtime);          // 只减 main 的引用计数：回调闭包（已随 ui 泄漏）持
                        // Arc clone，runtime 有意不 shutdown，worker 线程由
                        // ExitProcess 回收（FINDINGS §3c-1，更少一段用户态等待）
guard.disarm();         // 正常路径；drop 段用户态卡死时看门狗兜底 exit(0)
Ok(())
```

- `EXIT_WATCHDOG = Duration::from_secs(2)`（任务给定值；arm→disarm 正常路径
  本身 <1ms，2s 覆盖极端调度余量 20 倍以上）。
- 注释里写明「为什么泄漏是正确的」与 FINDINGS 引用，防后人「修复」掉。

验收：代码评审 + clippy（mem::forget 是常规 API，无需 allow）。

### T2 exit_guard 模块（新 `src/exit_guard.rs`）

```rust
/// 退出段看门狗：arm 起线程 sleep(timeout)，醒来时 done 未置位则调 exit_fn。
/// disarm 置位 done。exit_fn 可注入（生产 = process::exit(0)，测试 = channel）。
pub fn arm(timeout: Duration, exit_fn: impl FnOnce() + Send + 'static) -> ExitGuard;
pub struct ExitGuard { done: Arc<AtomicBool> }   // disarm(self) 置位
```

- 线程 detach：正常路径进程先退，线程随进程死；不 join（join 反而把清理变串行）。
- 语义边界注释：**只兜用户态卡死**；内核态卡死（S10 根因形态）任何 exit 都无效
  （FINDINGS §1.2 关键推论），主防线是 T1 的 leak。
- 覆盖边界注释（O4）：看门狗在 main 尾部 disarm，**静态析构段（main 返回后的
  CRT atexit/静态 drop）不在覆盖内**——该段无已知挂起面（tracing 无缓冲 sink），
  如未来出现再前移 disarm 时机。
- `lib.rs` 注册 `pub mod exit_guard;`。

### T3 X 关窗路径统一（main.rs，T1 前段）

```rust
ui.window().on_close_requested(|| {
    // S10：默认行为是 hide → 最后窗口关闭退出 → Exit 处理时 hidden 窗口被
    // suspend（GL 销毁在事件循环内执行，与 quit 路径同族风险）。改为保持
    // Visible 直接退出：两条退出路径合流到「visible + leak-on-exit」，
    // suspend_all_hidden_windows 对 visible 窗口跳过（slint 1.17.1
    // event_loop.rs L118-132/L556）。run() 自己的尾部 hide 在 Exit 之后执行，
    // Windows 上只走 winit SW_HIDE、不含 GL——无漏网 GL 调用（FINDINGS §1.2-3）。
    let _ = slint::quit_event_loop();
    slint::CloseRequestResponse::KeepWindowShown
});
```

- 放在 `bridge::bind` 之前、`AppWindow::new()` 之后（main.rs L17 后）。
- UX 不变：X 仍然退出整个应用（此前默认 hide→quit 也是退出）；窗口随进程消亡。
- quit_event_loop 失败分支（winit 后端有 proxy，仅循环已拆时失败=进程已在退出，
  X 无从点击）：`let _ =` 静默 + KeepWindowShown，注释说明。

### T4 单元/集成测试（testing backend，软件渲染——结构性无法复现死锁本身，
只钉行为逻辑）

新 `tests/exit_paths.rs`（testing backend 单例限制 → 本文件一个 #[test]，
模式抄 live_daemon.rs L60-66/L96/L252/L255）：

1. `close_requested_quits_without_hiding`：
   - AppWindow::new → `ui.show()`（**M5：is_visible() 实现是
     strong_component_ref.is_some()（corelib window.rs L2030），从未 show 恒
     false**；show() 存入自持强引用后 is_visible 才为 true）→ 装 T3 同款
     handler（抽成 `hmp_desktop` 内可复用函数 `install_exit_on_close(ui:
     &AppWindow)`，T3 调用、测试复用——避免测试钉的是「复制粘贴的旧行为」）；
   - driver 线程：**`invoke_from_event_loop` + ui_weak 派发**（M5：`ui.window()`
     借用非 Send 的 Window，跨线程直持不可行；live_daemon.rs L60-66 同款
     on_ui 中转）→ `ui.window().dispatch_event(slint::platform::WindowEvent::
     CloseRequested)`（corelib 层回调派发，backend 无关：api.rs L746-752
     request_close → KeepWindowShown 则不 hide）；
   - 主线程 `slint::run_event_loop()` 返回后断言：返回成功 + `ui.window().
     is_visible()` 仍 true（未走 hide/suspend 路径）。
2. `exit_guard.rs` 内嵌单元测试（纯逻辑、注入 exit_fn）：
   - `fires_when_not_disarmed`：arm(50ms, tx.send) → 1s 内收到；
   - `silent_after_disarm`：arm(50ms, tx.send) → 立即 disarm → 300ms 无信号。
   （时序测试取宽裕界，防 CI 抖动。）

既有回归保护：`cargo test -j 1 -p hmp-desktop` 全绿（context_menu/drag/
eq_clock/live_daemon 等不涉退出编排，预期零改动）。

### T5 真机验收实验（实施阶段执行；流程=FINDINGS §2，此处只列判据）

- 脚本：`verify-s10/repro.ps1`（worktree 根、不入库——verify-dump/ 同先例），
  复用 verify-dump/ 三个取证脚本作僵尸现场记录。
- 构建纪律：`cargo build --release -p hmp -p hmp-desktop` 同批；`hmp quit`
  （默认端点）杀常驻旧 daemon 后再起隔离环境（AUDIT §18/§22）。
- 臂与判据（顺序执行，总预算 ≤1h）：
  1. arm-base-quit ×2：预期复现僵尸（存活 >5s 且 Stop-Process -Force 后仍存活）。
     若 0/2 → 追加 20min E1 等长轮 ×1；仍 0 → 记录「短周期不复现」，改判据 4
     的对照（基线 23.5min 轮 vs 修复 23.5min 轮，预算另报）。
  2. arm-fix-quit ×2：**2/2 在 quit 后 ≤5s 进程消失**（无僵尸、无需 Force）。
  3. arm-fix-close ×1：CloseMainWindow 后 ≤5s 消失。
  4. arm-software ×1（可选强化证据）：基线二进制 + `SLINT_BACKEND=
     winit-software`，quit 后 ≤5s 消失 ⇒ GL 依赖性运行时对照。
- **GL 后端实测记录（O3）**：基线/GL 臂每轮在播放期采样进程加载模块
  （`(Get-Process -Id $pid).Modules`，过滤 opengl32/libEGL/nvoglv64/atig/dl
  等），把实测后端形态（WGL vs EGL、驱动 DLL）写进 log——M1 修正后生产路径
  预期 WGL 优先，实测留档直接支撑/修正该论断。
- 安全：隔离 XDG + HMP_IPC_ENDPOINT；音量 0；每轮 finally PID 精确杀；成僵尸
  只取证不反复杀，报告用户重启。

### T6 收尾

- `cargo fmt --all -- --check`（改动文件 fmt 达标）/ `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` / `cargo test -j 1 -p hmp-desktop`
  （审核实施要求口径；全仓逐 crate 测试不在本任务范围，留给独立审计/合流前）。
- 不 push、不动主工作区、禁 git stash（纪律）。FINDINGS/PLAN 实施期修订随
  相应任务提交。

## 真机验收判据（汇总，T5 详情）

| 判据 | 标准 |
|---|---|
| 修复有效性 | arm-fix-quit 2/2：`hmp quit` 后 ≤5s hmp-desktop.exe 消失 |
| X 关窗 | arm-fix-close 1/1：≤5s 消失 |
| 基线对照 | arm-base-quit 复现僵尸（或短周期不复现时按 T5-1 降级路径） |
| 无回归 | daemon 侧每次 quit 后自身 ≤15s 退出；库护栏文件零变化 |
| 全仓验证 | fmt/clippy(-D warnings)/test -j 1 逐 crate 全绿 |

## 风险与回滚

1. **残余假设风险**（FINDINGS 15%）：真实卡点在 ExitProcess→驱动 DLL detach →
   leak-on-exit 无效。T5 arm-software 臂提供证伪通道；若真发生：fallback =
   常态 `SLINT_BACKEND=winit-software`（产品决策，另行立项）。
2. **last-present 残余窗口**：Exit 事件处理前可能有一次 pending redraw 走
   swap_buffers——正常渲染路径，E1 无证据指向它，且无 API 可抑制（需 patch
   slint）。记录不处理。
3. `KeepWindowShown` 副作用：quit_event_loop 失败时 X 失效——实践不可达
   （失败=循环已拆=进程退出中），注释钉死。
4. 泄漏跳过静态析构：tracing 无缓冲 sink、rusqlite 连接即开即弃（FINDINGS §3
   正当性核对）——无数据丢失面。
5. **上游修复解除判据（O2）**：本绕过的解除条件 = slint 上游对退出路径 GL
   context 销毁挂死给出修复（追踪 slint-ui/slint 相关 issue；或 glutin/winit
   侧 context destroy 卡死的驱动侧 workaround 落地）。届时：升级依赖 → 删
   forget + 还原默认 close 行为 → 重跑 T5 复现实验（base 臂直接复现僵尸才
   有资格谈解除——若 base 已不复现，说明驱动/上游已修，可解除）。在 AUDIT
   登记解除条件防遗忘。
6. **回滚**：单 crate 纯 hmp-desktop 提交，`git revert` 即回滚；无 schema/协议/
   daemon 变更；回滚后行为回到现状（可复现僵尸）。若修复本身引入新问题：
   SLINT_BACKEND=winit-software 环境变量即时逃生（无需回滚构建）。

## 实施顺序与依赖

T2（exit_guard，独立）→ T3+T1（main.rs，一次提交）→ T4（测试）→ T5（真机）
→ T6。T1/T3 可与 T2 并行；T5 依赖 T1-T4 完成（fix 臂需要完整修复）。
预计代码量：main.rs ~+40 行、exit_guard.rs ~80 行、测试 ~80 行。

## 需裁决问题（不阻塞开工）

1. 看门狗超时取 2s（任务给定）——若希望更保守（4s）请指示，默认 2s。
2. AUDIT 落账文案见下节（docs/ 禁碰，由用户执行）。

## AUDIT 落账建议文案（docs/AUDIT.md §25-① 的闭环条目，供用户粘贴）

> **S10 桌面退出死锁闭环（2026-10-24 调查 + 修复）**：`hmp quit` 后 hmp-desktop
> 不可杀僵尸的根因 = 退出路径 femtovg GL context/surface 销毁（glutin
> ApiPreference::FallbackEgl = **WGL 优先、EGL 兜底**，生产路径大概率 WGL
> desktop GL；ensure_current + 缓存 glDelete\* + wglDeleteContext/
> eglDestroy\*）在驱动内核态不返回；TerminateProcess/ExitProcess 均需等线程
> 出内核，故不可杀。修复：leak-on-exit（run() 返回后 forget(ui)——run 尾部
> hide 已解除组件自持环，forget 后无人触发 adapter/renderer drop；GPU 资源交
> 内核 rundown）+ X 关窗 on_close_requested(KeepWindowShown) 统一「visible
> 退出」（避免 Exit 时 hidden 窗口在循环内 suspend）+ 2s 用户态看门狗兜底
> （对内核态无效，仅纵深）。真机验收：修复臂 quit/close 2/2+1/1 ≤5s 退出，
> 基线臂复现僵尸；解除判据见 worktree PLAN.md 风险节 5。详证 worktree
> FINDINGS.md（S10）。
