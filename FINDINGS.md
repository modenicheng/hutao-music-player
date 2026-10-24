# S10 FINDINGS：桌面退出死锁（`hmp quit` 后 hmp-desktop.exe 不可杀僵尸）

- 分支：`fix/desktop-exit-deadlock`。调查 → 审核 → 实施期间由实验+cdb 取栈**改写根因**（转向史见 §4）。
- 证据：docs/verify/2026-10-24-mem-e1/（E1 原始复现）+ 本 worktree `verify-s10/`
  （复现/验收实验、cdb 符号化栈 `zombie-stacks-cdb.txt`）。
- 结论速览：**真根因 = 退出期 TLS 析构触发 tokio Runtime::drop 的
  BlockingPool::shutdown 死等已被 ExitProcess 杀死的 blocking 线程（栈实锤，
  置信 ~98%）；修复 = BackendRuntime 持 `&'static` Runtime（T7）+ 退出段
  看门狗（纵深）。GL/femtovg 销毁被证伪为卡死点（GL 与 software 僵尸同栈）。**

## 0. 现象与事实

1. E1 两臂（fix 23940 / base 2712）`hmp quit` 后 hmp-desktop.exe 挂死：
   `Stop-Process -Force`（TerminateProcess）后仍存活 → 不可杀僵尸；重启才清。
2. 僵尸只剩 1 线程（State=Wait，WaitReason=Unknown）——其余线程全部退出。
3. MiniDump（comsvcs）对僵尸失败——与「困在 loader 关停期」相容。
4. daemon 侧正常退出（E1：alive=False；本轮 2.1s/1.1s/1.6s ≤15s 断言全过）。
5. **本轮复现（verify-s10，3min 播放短周期即复现，无需 E1 的 23.5min）**：
   - base（a5a0236）+ GL：**2/2 ZOMBIE**（同 E1 签名）；
   - base + `SLINT_BACKEND=winit-software`：**1/1 ZOMBIE** → 卡死与渲染器无关
     （此臂最初是 GL 假设的对照实验，结果反成证伪证据）；
   - GL 后端实测：opengl32.dll + Intel igd*（无 libEGL）→ 生产路径 **WGL**
     （M1 修正的预测成立）。
6. **cdb 存在**（`C:\Program Files (x86)\Windows Kits\10\Debuggers\x64\cdb.exe`，
   前任"缺席"误判）→ 非侵入附加成功，符号化栈见 §1.3。
7. PID 2712 = E1 base 臂僵尸（01:40 起，s9-mem-base 二进制）——机器未重启时
   楔了 12+ 小时（此前"已消失"的 tasklist 检查是误报）。

## 1. 退出路径代码审计（排除过程，均有效）

main.rs（crates/hmp-desktop）：`prefs` → `runtime`(Arc<BackendRuntime>) →
`ui`(AppWindow) → 各 bind → `ui.run()`。run() = show + 事件循环 + **尾部
hide**（api.rs L1076-1078；hide=take 自持 strong_component_ref + SW_HIDE，
Windows 上不含 GL，window.rs L1655-1662 / winitwindowadapter.rs L1161-1175）。
run() 返回后 reverse 声明序：`ui` drop（→ 组件/adapter/renderer drop：femtovg
= ensure_current+glDelete\*+wglDeleteContext/eglDestroy\*；winit Window drop =
PostMessageW 异步、无阻塞）→ `runtime` drop → main 返回 → exit()/ExitProcess。

嫌疑排除/确证（原始调查，结论被 §1.3 部分改写）：
- S2 tokio Runtime drop「无阻塞池任务」——**错**（见 §1.3：blocking 池存在且
  shutdown 会死等；错在以为 drop 只发生在 main 栈且线程都活着）。
- S3 订阅循环 Shutdown 分支干净（backend.rs L487-494，无嫌疑）。
- S4 事件循环确实正常返回（僵尸栈位于 exit() 之后，实锤）。
- S5「僵尸周期拉起 daemon」非根因（spawn 源在 Shutdown 后均 return）。
- S1/S6 GL 销毁、daemon 侧——GL 被证伪（§1.3），daemon 无嫌疑。

## 1.2 Slint 退出链细节（源码级，i-slint-*-1.17.1 / winit 0.30.13 / glutin 0.32.3）

- quit：Event::Shutdown → `slint::quit_event_loop()`（跨线程 PostMessage）→
  CustomEvent::Exit → `event_loop.exit()` → winit `run_on_demand` 循环 break；
  退出时对 **Hidden** 窗口跑 suspend_all_hidden_windows（#8795 修复），
  quit 路径主窗口 Visible → 跳过；X 关窗路径 hide 在先、Exit 处理时已 Hidden
  → 会在循环内跑 clear_graphics_context——**两路径销毁调用时机不同但内容
  相同，且都被证伪为卡死点**（基线臂 3/3 均完成了该段后才挂在 TLS/tokio）。
- GL 装载：glcontext.rs L77 `ApiPreference::FallbackEgl` = glutin-winit 0.5.0
  语义「系统 profile（WGL）优先、EGL 兜底」；默认请求 GLES2.0、WGL 下失败
  回落 desktop GL。实测模块表（opengl32+igd*、无 libEGL）证实 WGL。
- ui drop 链的 GL 销毁（clear_graphics_context 三步）与 winit 窗口销毁
  （PostMessage 异步）均为**非阻塞形态**——与「3/3 基线臂完成该段」一致。

## 1.3 真根因（cdb 僵尸栈实锤，verify-s10/zombie-stacks-cdb.txt）

GL 僵尸（46516）与 software 僵尸（73588）主线程栈**逐帧一致**：

```
ntdll!ZwWaitForAlertByThreadId / RtlWaitOnAddress / KERNELBASE!WaitOnAddress
std futex → Condvar::wait
tokio::runtime::park::Inner::park
tokio::park::CachedParkThread::block_on<oneshot::Receiver<()>>
tokio::runtime::blocking::shutdown::Receiver::wait
tokio::runtime::blocking::pool::BlockingPool::shutdown        ← 卡点
core::ptr::drop_glue<…Image…>（Timer 回调闭包 drop，内联）
alloc::sync::Arc<hmp_desktop::backend::BackendRuntime>::drop_slow  ← 最后引用
core::ptr::drop_glue<i_slint_core::timers::CallbackVariant>
i_slint_core::timers::Timer::Drop::drop
std::sys::thread_local::native::lazy::destroy<RefCell<Timer>>   ← TLS 析构
std::sys::thread_local::destructors::list::run
std::sys::thread_local::guard::windows::cleanup
ntdll!RtlFreeActivationContextStack（FLS 回调）
ntdll!LdrShutdownProcess ← ntdll!RtlExitUserProcess ← KERNEL32!ExitProcess ← exit()
```

**机制链**：
1. `bridge::schedule_refresh`（bridge.rs L549）的 thread_local
   `RefCell<slint::Timer>`，其 SingleShot 回调闭包捕获 `Arc<BackendRuntime>`。
2. main 正常返回 → exit()/ExitProcess：**先终止其余线程**（tokio worker +
   blocking 池），再 LdrShutdownProcess 跑主线程 FLS/TLS 析构。
3. TLS Timer drop → 闭包 drop → 此刻 Arc 为最后引用（ui/runtime 已在 main
   尾部 drop 完）→ `BackendRuntime::drop` → tokio `Runtime::drop` →
   `BlockingPool::shutdown` 逐线程等 oneshot ACK——**应答线程已被杀死** →
   `WaitOnAddress` 永久挂起。
4. 主线程困在 loader 关停期 → TerminateProcess 亦无法清出 → 不可杀僵尸；
   其余线程已死 → 只剩 1 线程、Wait/Unknown；comsvcs 挂起失败同因。

为什么 blocking 池非空：进程生命周期内任一 `spawn_blocking`（如 reqwest/
hyper DNS 解析等）即驻留 blocking 线程池，与播放状态无关。

**置信度：~98%**（两渲染器同栈 + 时间线闭环 + 修复实验 3/3 通过）。
残余 2%：未逐一指认是哪个 spawn_blocking 创建了池线程（对修复无影响）。

### 关键推论（修复约束）

- ExitProcess 期的 TLS/FLS 析构里**任何**可能阻塞的 drop 都是死锁面
  （此刻进程内只有主线程，任何等待跨线程事件的代码必死）。
- Runtime::drop 的 shutdown 握手是为活进程设计的——**退出路径永远不该执行
  它**。修复 = runtime 与进程同寿（T7），而非「换时机销毁」或「事后补救」
  （loader 关停期卡死后看门狗 exit(0) 同样会卡）。
- GL/femtovg 销毁与 winit 窗口销毁被证据豁免：无需 leak、无需改 X 关窗路径。

## 2. 复现/验收实验（已执行，verify-s10/repro.ps1 + 各 sandbox-*/log/run.log）

- 隔离三件套（XDG 沙箱 + 每臂独立 HMP_IPC_ENDPOINT 管道）+ 音量 0 门 +
  近静音 WAV×10 + 3min 播放/churn + 退出触发 + 60s 轮询 + Force 再 10s 判定。
- 二进制：bin-base = a5a0236；bin-fix = ba57a91（forget 版，中途形态）；
  bin-fix2 = 3bc41a6+2c08ce1（**终态：T7+看门狗，自然 drop**）。

| 臂 | 二进制 | 退出 | 结果 |
|---|---|---|---|
| base-quit-1 | base | quit | **ZOMBIE**（1 线程 Wait/Unknown，Force 无效） |
| base-quit-2 | base | quit | **ZOMBIE**（同签名；daemon 2.1s 退出） |
| base-sw-1 | base+software | quit | **ZOMBIE**（渲染器无关实锤） |
| fix-quit-1 | fix | quit | CLEAN_EXIT 1.1s |
| fix-quit-2 | fix | quit | CLEAN_EXIT 1.1s（daemon 1.1s） |
| fix-close-1 | fix | X close | CLEAN_EXIT 1.1s |
| fix2-quit-1 | fix2 | quit | INCONCLUSIVE（弹窗被人工关闭，存活门拦截） |
| fix2-quit-2 | fix2 | quit | **CLEAN_EXIT 1.1s** |
| fix2-close-1 | fix2 | X close | **CLEAN_EXIT 1.1s** |

判据全过：修复臂 quit/close 2/2+1/1 ≤5s 退出、无僵尸；基线臂 3/3 复现；
daemon 每次 quit 后 ≤15s 自退；库护栏零变化（隔离沙箱）。
测试后断言单测不可覆盖死锁本身（testing backend 软件渲染 + 无 ExitProcess
竞态路径），行为面由 `runtime_outlives_backend_runtime_drop` 契约测试钉死。

## 3. 修复终态（T7 + 纵深）

1. **T7（根因修复，backend.rs）**：`BackendRuntime` 持 `&'static
   tokio::runtime::Runtime`（构造时 `Box::leak`）——**任何路径**（含 ExitProcess
   期 TLS 析构、未来任何 TLS 捕获 Arc 的组合）都不再触发 Runtime::drop 的
   shutdown 握手；worker/blocking 线程由 ExitProcess 常规回收。契约单测：
   句柄消亡后在途任务继续完成（旧实现下该测试失败）。
   `schedule_refresh` 的 TLS Timer 捕获 Arc 从此无害（drop=no-op），不改。
2. **看门狗（exit_guard.rs，纯纵深）**：main 在 `ui.run()` 返回后武装 2s
   计时线程，显式 `drop(ui)/drop(runtime)` 完成后 disarm；只兜用户态挂起，
   对内核态/loader 关停期卡死无效（注释写明，防预期漂移）。
3. **撤销项（转向记录见 §4）**：T1 forget(ui)、T3 KeepWindowShown 已 revert
   ——其前提（GL 销毁卡死）被证伪；恢复自然 drop 与默认 X 关窗行为。

**正当性**：AppWindow 内无 exit-time 落盘（prefs 变更即存）；daemon 独立；
runtime 泄漏一个（进程级，~几 MB + 线程，进程退出即回收）。

## 4. 调查转向史（为什么最初结论是 GL、后来改写）

1. 初始证据（E1 + 1 线程 + TerminateProcess 无效 + S8 GL/WDDM 签名）下，
   「femtovg GL context 销毁在驱动内核态不返回」是与全部证据相容的最优假设
   （85% 置信），外部同族案例（Slint #8795、Khronos wglDeleteContext）支持。
   审核通过该架构（leak-on-exit + X 关窗统一 + 看门狗）。
2. 实施期 software 对照臂（原设计为佐证）**复现僵尸** → GL 必要性被证伪；
   cdb 取栈（前任误报缺席，实际存在）拿到符号化栈 → 真根因（§1.3）。
3. 保留已验证无害的看门狗，撤销 forget/KeepWindowShown（前提不成立），
   落 T7 根因修复，补验通过（§2 fix2 行）。fix（forget 版）与 fix2（T7 版）
   都有效，但只有 T7 版因果正确且不依赖偶然的 Arc 存活布局。

## 5. 遗留与建议

- **用户需重启**清理 4 个僵尸（2712/46516/65904/73588）。
- tokio 层面：`BlockingPool::shutdown` 无超时 + ExitProcess 期 TLS 析构组合
   是通用陷阱（任何「TLS 捕获 runtime 句柄」的 tokio+slint 应用同病），
   可考虑上游报告（非本任务范围）。
- AUDIT 落账文案见 PLAN.md 尾部（docs/ 禁碰，由用户执行）。
