# S10 FINDINGS：桌面退出死锁（`hmp quit` 后 hmp-desktop.exe 不可杀僵尸）

- 分支：`fix/desktop-exit-deadlock`（基于 main a5a0236）。调查与计划阶段，禁实现。
- 证据基线：docs/verify/2026-10-24-mem-e1/{arm-fix,arm-base}/mem_run.log + AUDIT §25 ①。
- 增量落盘：每排除/确证一个嫌疑即 append。
- 结论速览：**根因 = 退出路径 femtovg GL context/surface 销毁（驱动内核态不返回），
  置信度 85%；推荐修复 = leak-on-exit（不让销毁调用发生）+ X 关窗路径统一 +
  次级用户态看门狗**。详见 §1.2 / §3。

## 0. 现象与已确证事实（调查输入，直接引用）

1. E1 两臂（fix 23940 / base 2712）`hmp quit`（隔离端点）后 hmp-desktop.exe 挂死：
   - fix 臂 log：`[01:38:25] cleanup: hmp quit` → 5s 后 `Stop-Process pid=23940 (still alive)` → 又 3s 后 `alive=True`。
   - **`Stop-Process -Force` = TerminateProcess 也杀不掉** → 进程有线程卡在不可中断内核调用（Windows 语义：TerminateProcess 需等所有线程从内核态返回才能真正终止进程对象）。
2. 僵尸只剩 1 线程（State=Wait，WaitReason=Unknown）——其余线程全部退出，主线程卡在最后清理段（zombie-thread-detail.ps1 取证）。
3. MiniDump 对僵尸失败、对健康进程成功（healthy-62204.dmp 248MB）——用户态转储器无法完成挂起/读内存，内核态卡死旁证。
4. daemon 侧正常退出（fix 臂 pid=62032 alive=False）——死锁纯在桌面进程。
5. 与 S8 GL/WDDM commit 放大签名相容（docs/verify/2026-10-23/mem-rca-FINDINGS.md P5）。
6. 僵尸周期性拉起隔离 daemon（AUDIT §25 ①原文："僵尸周期拉起隔离 daemon"）——待代码核对：桌面侧有 daemon 守护/自动重启逻辑（见 §2.x 嫌疑表）。
7. 2026-10-24 本日核查：PID 2712 与 23940 均已不在（重启清掉/自行消亡），无法补 dump。

## 1. 退出路径代码审计（完成）

main.rs（55 行）入口结构：
```rust
let prefs = Arc::new(Mutex::new(prefs::load()));          // L12
let runtime = Arc::new(backend::BackendRuntime::new())    // L15  tokio runtime
let ui = hmp_desktop::AppWindow::new()?;                  // L17
bridge::load_data(&ui, &runtime); bridge::bind(...);      // L26-27
player_bridge::bind(&ui, &runtime, prefs);                // L30  连接/拉起 daemon + 订阅
menu_bridge::bind(...);                                   // L32
ui.run()                                                  // L54  slint 事件循环，阻塞
```
`ui.run()` 返回后（reverse 声明序 drop）：**① `ui` drop（AppWindow → Window → winit 窗口销毁 → femtovg GL context 销毁，头号嫌疑）→ ② `runtime` drop（tokio Runtime drop：默认 shutdown 等待阻塞任务完成——第二嫌疑）→ ③ prefs（trivial）→ ④ 进程 main 返回退出。**

### 嫌疑清单（逐个排除/确证，append）

- [已确证·根因部位] S1: AppWindow drop → femtovg GL context 销毁（详见 §1.2 slint 审计）
- [已排除] S2: tokio Runtime drop 等待阻塞任务——IpcStream 是 tokio 异步命名管道
  （transport.rs L22-24，reactor 驱动，无阻塞池）；query_library_meta 直接在 worker
  线程跑阻塞 rusqlite（backend.rs L558-561，毫秒级、非 spawn_blocking）。
  Runtime::drop 无「等待阻塞任务」的长尾。且若死锁在此，GL 销毁早已完成、
  TerminateProcess 应有效（用户态等待可被线程终止打断）——与不可杀事实矛盾。
  **顺序上 runtime drop 也根本到不了：main 线程先卡在 ui drop。**
- [已排除] S3: backend.rs 订阅循环 Event::Shutdown 分支自身死锁——代码干净：
  `tracing::info!` + `slint::quit_event_loop()` + `return`（backend.rs L487-494），
  无锁、无通道对撞。§19 修复链（AUDIT §19）已钉「Shutdown 帧先于 EOF」
  （daemon server.rs L222 + 回归测试 quit_broadcasts_shutdown_to_subscribers）。
- [已排除] S4: quit_event_loop 未送达导致事件循环未返回——若循环未返回，主线程
  在用户态消息循环里等待，TerminateProcess 必杀（事实是不可杀）→ 事件循环确实
  返回了、卡点在其后的 teardown。
- [已澄清] S5: 「僵尸周期拉起隔离 daemon」——desktop lib 内唯一 spawn 源是订阅循环
  首连 `connect_or_spawn()`（backend.rs L422-425，`ever_connected=false` 时）与
  陈旧构建 `restart_backend()`（L251-262，仅首连核对路径）。E1 场景 23 分钟在播，
  `ever_connected=true`，收到 Shutdown 后订阅任务 return——僵尸进程内**没有**周期
  spawn 代码路径在跑。合理解释：①观察到的「拉起」是**后续 E1 臂/脚本再次起隔离
  daemon**（另一臂脚本 `hmp serve`），僵尸不退出所以每次清理都残留；②或 zombie
  观察窗口在 Stop-Process 之前的 5s 内 tokio runtime 仍在跑重连循环（EOF 竞态
  版本），connect() 失败≠拉起。**S5 不构成根因，是僵尸的伴生危害**（僵尸占着
  用户视线/资源，重启才清）。置信：中高。
- [已排除] S6: daemon 侧（daemon 正常退出，事实 4）

## 1.2 退出路径完整链（Slint 1.17.1 源码级审计，crates.io registry 源）

hmp-desktop 用 slint 默认特性（+unstable-winit-030）→ **winit 0.30.13 后端 +
femtovg GLUTIN OpenGL 渲染器**（i-slint-backend-winit-1.17.1 / i-slint-renderer-femtovg-1.17.1 /
glutin 0.32.3 / glutin-winit 0.5.0）。逐段核实：

1. **订阅线程**（tokio）：`Event::Shutdown` → `slint::quit_event_loop()` → 任务 return
   （backend.rs L487-494）。quit 经 EventLoopProxy::send_event → PostMessage 唤醒
   （i-slint-backend-winit lib.rs L841-849）。
2. **事件循环退出**：CustomEvent::Exit(generation) → `event_loop.exit()` =
   winit ControlFlow::Exit（event_loop.rs L549-558）→ winit Windows `run_on_demand`
   消息循环 break（winit-0.30.13 platform_impl/windows/event_loop.rs L269-285）。
   **winit 退出时不销毁窗口**——窗口由 `Arc<Window>` 引计数管理。
   退出前唯一 GL 相关动作：`suspend_all_hidden_windows()`（#8795 的修复，只对
   **Hidden** 窗口调 `renderer.suspend()`=clear_graphics_context）；`hmp quit` 时
   主窗口仍是 **Visible** → 被跳过（event_loop.rs L118-132, L556）。
3. **drop 触发机制与 `ui.run()` 尾部 hide**（M3 修正后如实描述）：
   `WindowInner::show()` 在窗口显示时把组件实例强引用存进
   `strong_component_ref`（window.rs L1624-1629，组件↔窗口自持环——显示期间
   main 里的 `ui` 句柄不是最后强引用）；`ComponentHandle::run()` = `show()` +
   事件循环 + **返回后 `hide()`**（api.rs L1076-1078）——`hide()`
   （window.rs L1655-1662）`take()` 掉自持强引用 + `set_visible(false)` +
   `release_keepalive()`。**因此 run() 返回后，main 的 `ui` drop 才成为最后
   强引用 → 触发组件 drop → `window_adapter` Rc drop（generator/rust.rs
   L675/687）→ `WinitWindowAdapter` drop：字段序 `renderer`（L331）先于
   `winit_window_or_none`（L355）→ renderer drop 时窗口仍存活（GL 销毁→
   DestroyWindow 顺序正确，非误序 bug）。这也正是 forget(ui) 有效的机理**：
   run() 尾部 hide 已解除自持环，forget 掉 main 的最后强引用即无人再触发
   adapter/renderer drop。Windows 上该尾部 hide 只走 winit `set_visible(false)`
   （SW_HIDE），**不含任何 GL 调用**（winitwindowadapter.rs L1161-1175：仅
   Wayland 或 SLINT_DESTROY_WINDOW_ON_HIDE 才走 suspend）。
4. **GL 销毁本体**（FemtoVGRenderer::drop → clear_graphics_context，
   i-slint-renderer-femtovg lib.rs L492-494/L527-570 + opengl.rs L180/L205）：
   a. `ensure_current()` → `context.make_current(&surface)`（glutin →
      **wglMakeCurrent / eglMakeCurrent**；impl 在 i-slint-backend-winit
      `renderer/femtovg/glcontext.rs` L24-31，经 opengl.rs L205
      `with_graphics_api` 调用）；
   b. 三缓存 clear_all（graphics_cache/layer_cache/texture_cache → femtovg
      Canvas drop → **glDelete\* 批量删除**）；
   c. `graphics_backend.clear_graphics_context()` → drop boxed OpenGLContext
      （renderer/femtovg/glcontext.rs L17-21 字段序：context→surface→
      winit_window）→ **wglDeleteContext / eglDestroyContext+eglDestroySurface**
      （glutin Drop）。
5. **Windows GL 装载形态**（renderer/femtovg/glcontext.rs L76-77 + glutin-winit
   0.5.0 lib.rs L221-231）：`ApiPreference::FallbackEgl` 文档语义 = "Fallback to
   `EGL` when failed to create the system profile"——**WGL 优先、EGL 兜底**；
   请求版本默认 OpenGLES 2.0（L115-116），WGL 下 ES profile 创建失败则回落
   无版本属性的 desktop GL（L135-141 双 fallback）。**生产路径大概率 WGL
   （desktop GL 或 WGL-ES2 profile），EGL 仅 WGL 不可用时**——[Khronos 论坛
   wglDeleteContext 挂死](https://community.khronos.org/t/hang-in-wgldeletecontext-in-dtor/75731)
   正是 WGL 侧同族证据（4c 的直接同款调用）。
6. X 关窗路径同样危险：CloseRequested → corelib 默认 hide → 最后窗口关闭退出
   循环 → **此时窗口已 Hidden**，suspend_all_hidden_windows 在事件循环内对它跑
   同一套 clear_graphics_context（第 2 点）——同族调用，仅时机不同且更早
   （卡死会发生在 run_event_loop 内部而非 main 的 drop 段）。E1 只复现了
   quit 路径。

### S1 结论（根因判定）

**死锁点 = `ui` drop 段的 GL context/surface 销毁链（上述 4a/4b/4c），驱动
内核态调用不返回**。证据闭环：
- TerminateProcess 无效 ⇒ 唯一存活线程（主线程）处于**不可中断内核调用**——
  用户态等待（锁/消息循环/管道读）都会被线程终止打断，不成立；
- 该窗口期内主线程唯一会进的内核调用族 = GL/驱动（dxgkrnl/WDDM）；
- 与 S8 实锤的 GL/WDDM commit 放大（PM 2.27GB、PM:WS=13.6:1）同系统态——
  巨量 GPU allocation 使销毁路径重且更易命中驱动同步缺陷；
- MiniDump 失败：comsvcs 需挂起全部线程，内核态卡死线程挂不起来；
- 外部同族案例（M2 修正：Ghostty #3369 实为 Linux/GTK 现象且可 Ctrl+C 终止，
  与本缺陷「内核态不可杀」不同族，撤回不引）：[Slint #8795](https://github.com/slint-ui/slint/issues/8795)
  （Windows 退出时 GL 资源延迟清理挂死，1.13 修复=退出时主动 suspend 剩余
  surface——但只覆盖 Hidden 窗口）、[Khronos 论坛 wglDeleteContext 挂死](https://community.khronos.org/t/hang-in-wgldeletecontext-in-dtor/75731)
  （WGL dtor 挂死 = 4c 直接同款；M1 修正后生产路径大概率 WGL，此证据反而更贴）、
  [不可杀进程机制](https://superuser.com/questions/136272/how-can-i-kill-an-unkillable-process)
  （线程困在内核态驱动调用 ⇒ TerminateProcess 等线程退出 ⇒ 杀不掉）。

精确到 4a/4b/4c 哪一步需要 cdb（本机缺席）或僵尸现场（2712 已消失）——三者
同为「驱动销毁窗口」，不影响修复方案选型。**置信度：高（85%）**；残余 15%：
退出末段 ExitProcess→驱动 DLL detach 挂死（同 GL 族、同修复面）与最后一次
swap_buffers 呈现时 GPU 已 wedge（quit 前的渲染路径）。

### 关键推论（约束修复方案）

**一旦主线程已进入内核态卡死，任何进程内手段都救不回**：ExitProcess/
TerminateProcess 都要等全部线程从内核返回才真正终止（Windows Internals；
Super User 不可杀机制条目）。⇒ 「看门狗 + std::process::exit(0)」对**已发生**
的内核卡死无效；**必须不让销毁调用发生**（预防），而非卡死后补救。

## 2. 复现实验设计（供实施阶段；每轮 ≤10 分钟、隔离端点、音量 0）

**目的**：①短周期基线复现（E1 是 23.5min/臂——需确认更短周期也能复现，否则
验收标准退化为「与 E1 同长」）；②A/B 修复；③GL 依赖性对照；④X 关窗路径采样。

环境与安全（沿用 S9 E1 既定纪律，docs/verify/2026-10-24-mem-e1/env-s9.ps1 模式）：
- 隔离：临时 XDG 数据目录 + `HMP_IPC_ENDPOINT` 独立管道；开工前
  `verify-dump/live-hmp-procs.ps1` 快照在跑 hmp 进程，只允许碰隔离 PID。
- 音量 0（daemon 侧 user_volume=0 门 + 近静音 WAV 种子）。
- release 构建，`hmp`/`hmp-desktop` 同批（mtime + backend_build 记录进 log）。
- 每轮看门狗：脚本 finally 段 PID 精确杀（桌面+隔离 daemon），**绝不 taskkill /IM**。
- **若成僵尸：只记录不反复杀**（thread-check/zombie-window-check 取证留现场，
  供后续 cdb 补装时分析；报告用户重启）。

流程（单轮 ~6 分钟）：
1. 清场（隔离端点 `hmp quit`）→ 起隔离 daemon → 种子 ~10 首近静音 WAV + 少量封面。
2. 起 hmp-desktop → CLI 驱动：play → 库页浏览（触发封面装载/GL 纹理增长）→
   next×3 → 静置 ~3 min（模拟 E1 的 GL commit 积累）。
3. 退出触发（分臂数组）：
   - quit 臂：隔离端点 `hmp quit`；
   - close 臂：`(Get-Process -Id $desktop).CloseMainWindow()`（走 X 关窗路径）。
4. 轮询 60s×1s：desktop PID 消失 = 正常退出（记录耗时）；存活 >5s → `Stop-Process
   -Force` → 再轮询 10s → 仍存活 = **僵尸复现**（记录 thread-check 输出）。
5. daemon 侧断言：quit 后 daemon 自身在 ≤15s 退出（隔离臂每次运行复核）。

臂数（总预算 ≤10min/臂×4 + 构建）：
- arm-base-quit ×2：基线 + quit（预期 2/2 僵尸——若 0/2，升格为 20min E1 同长
  轮 ×1 再判；仍不复现则记录「短周期不复现」，验收改为 E1 等长）。
- arm-fix-quit ×2：修复 + quit（判据：2/2 ≤5s 退出、无僵尸）。
- arm-fix-close ×1：修复 + X 关窗（同判据；基线 close 臂不做——僵尸留现场代价高）。
- arm-software ×1（可选强证据）：**基线二进制** + `SLINT_BACKEND=winit-software`
  （slint default 特性已编入 renderer-software，backend-selector lib.rs L160
  解析 "software"→winit+sw）同流程 quit——预期干净退出 ⇒ GL 依赖性的运行时
  对照（同一代码路径、仅无 GL），把「根因在 GL 销毁」从推断升格为实验事实。

无头/单测不可复现性说明：tests/* 走 i-slint-backend-testing（软件渲染、无 GL
context 销毁链）——**死锁在测试后端结构性不存在**，故修复正确性只能真机验收，
单测只覆盖逻辑层（见 PLAN）。

## 3. 修复方向评估

### a) 退出路径排序（订阅先停 / GL context 提前释放）——否决为主修

- 「订阅先停」：无事可做——订阅任务在 Shutdown 分支已干净 return（§1 嫌疑 S3）。
- 「GL context 提前释放」（在事件循环内先 suspend）：clear_graphics_context 本身
  就是危险调用链（§1.2-4），提前只是把卡死点从 main 挪进事件循环——同一族调用、
  同一内核风险，X 关窗路径（Exit 时 hidden 窗口 suspend）恰是该形态，与 quit 路径
  同样危险。**换时机不消除风险**。

### b) 事件循环返回后限时清理 + ~2s 超时 std::process::exit(0) 兜底——只能做次级防线

- 对**已发生**的内核态卡死无效（§1.2 关键推论：ExitProcess/TerminateProcess 都要
  等全部线程从内核返回；exit(0) 从看门狗线程调用同样卡在 rundown）。
- 对**用户态**挂起（假想的锁死锁/阻塞 IO 卡 drop）有效且廉价：保留为纵深防御，
  正当性与任务给定一致（daemon 独立、桌面无盘状态、退出段无并发依赖）。

### c) 组合（推荐）——leak-on-exit 为主 + 两翼补口

**主修复：不让 GL 销毁调用发生（leak-on-exit）**
1. main.rs：`ui.run()` 返回后 `std::mem::forget(ui)`——run() 尾部 hide 已解除
   组件自持环（§1.2-3），forget 掉 main 的最后强引用后**无人再触发 adapter/
   renderer drop**，§1.2-4 全部销毁调用（4a/4b/4c）不发生；进程 main 返回 →
   ExitProcess，GPU 资源由内核进程 rundown 回收（与任何 GL 进程崩溃同路径，
   系统设计上不依赖用户态配合）。`drop(runtime)` 在此只减引用计数——bridge/
   player_bridge 的回调闭包（随 ui 一起 forget）仍持 `Arc<BackendRuntime>`
   clone，**tokio runtime 同样有意泄漏、不执行 shutdown，worker 线程由
   ExitProcess 回收**（比显式 shutdown 更少一段用户态等待，更安全）。
2. X 关窗路径统一：`ui.window().on_close_requested(|| { quit_event_loop();
   KeepWindowShown })`——窗口保持 **Visible**，使 Exit 处理时
   `suspend_all_hidden_windows` 跳过（hidden 才 suspend），两条退出路径合流到
   「visible + forget」。否则 forget 只保 quit 路径，X 关窗在循环内就跑了 GL
   销毁。run() 自己的尾部 hide 在 Exit 处理**之后**执行，Windows 上只走 winit
   SW_HIDE、不含 GL（§1.2-3）——forget 后无任何漏网 GL 调用。
3. 次级看门狗（b 方案）：`ui.run()` 返回后先武装 ~2s 计时线程，清理完成则
   disarm；超时（用户态卡死兜底）`std::process::exit(0)`。

**正当性核对**：
- AppWindow 内无必须落盘状态：prefs 变更即存（player_bridge store_timer），无
  exit-time save；daemon 独立进程自持状态（PROJECT §4.2/§8.6）；库连接全部
  即开即弃（backend.rs L608 / player_bridge.rs L246，无持久句柄泄漏面）。
- 泄漏量 ~250MB（E1 实测 WS 量级）只在进程死前存活几毫秒~秒，无观察者。
- UX：窗口随进程消亡消失，与正常销毁视觉等价（顺序颠倒：先进程死后窗口销毁）。
- 修复后事件循环内不再有任何 GL destroy/suspend 调用（quit 与 close 两路径均验）。

**残余风险**（写进 PLAN 风险节）：
- 若真实卡点在 ExitProcess→驱动 DLL detach（15% 残余假设），leak-on-exit 无效
  ——但该路径与「任意 GL 进程被强杀」相同，业界非普遍不可杀；arm-software/
  arm-fix 真机验收直接证伪或证实。
- `KeepWindowShown` 使 X 按钮在 quit_event_loop 失败时无响应（winit 后端有
  proxy，仅循环已拆时失败——彼时进程已在退出，无用户可感窗口期）。
- 忘 drop 跳过 tracing flush 等静态析构：tracing_subscriber 无缓冲文件 sink，
  无损。

**不建议的替代**：换 renderer（software 常态化=牺牲 GPU 渲染质量，skia/wgpu=
新依赖面）、patch slint 强制 WGL（维护成本高）——但 `SLINT_BACKEND=winit-software`
作为**用户侧应急逃生门**在文档/CHANGELOG 提及即可（无需代码）。
