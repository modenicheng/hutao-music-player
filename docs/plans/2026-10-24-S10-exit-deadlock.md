# S10 PLAN：桌面退出死锁修复（fix/desktop-exit-deadlock 实施记录）

- 输入/证据：FINDINGS.md（本 worktree 根）+ verify-s10/（实验日志与 cdb 栈）。
- **终态方案（T7）**：BackendRuntime 持 `&'static` tokio Runtime（进程同寿），
  根除「ExitProcess 期 TLS 析构 → Runtime::drop → BlockingPool::shutdown 死等
  已死线程」死锁；退出段看门狗为纯纵深。GL/femtovg 销毁被证伪，相关
  leak-on-exit 件（T1/T3）已撤销（转向史见 FINDINGS §4）。
- 所有权：仅 `crates/hmp-desktop/**`（+ 两文档 + verify-s10 证据）。禁碰
  daemon 侧、hmp-core、docs/**。

## 非目标

- 不改 tokio/slint/winit 上游（上游报告另议，见 FINDINGS §5）。
- 不处理「僵尸周期拉起 daemon」（伴生危害，僵尸不再产生即消失）。
- 不做 AUDIT 登记（docs/ 禁碰；文案见尾部供用户粘贴）。

## 任务与提交（已全部执行）

| 任务 | 内容 | 提交 |
|---|---|---|
| 文档修订 | 采纳审核 M1-M5/O1-O4（WGL 优先级、Ghostty 撤引、run 尾 hide、runtime 泄漏语义、测试补口） | dd0557c |
| T2 | exit_guard 看门狗（可注入 exit_fn + 时序单测×2；语义边界注释） | d8afc1b |
| T1+T3 | （中途形态）forget(ui) + KeepWindowShown + 看门狗 | e5bed6f |
| T4 | （中途形态）exit_paths 行为测试 | ba57a91 |
| T5 | 真机实验：base 3/3 ZOMBIE（含 software 臂证伪 GL）→ cdb 取栈 → 真根因 | 无代码 |
| T7 | **BackendRuntime `&'static` Runtime + 契约单测**（runtime_outlives_backend_runtime_drop） | 3bc41a6 |
| 复盘 | 撤销 T1/T3（前提被证伪），恢复自然 drop + 默认 X 关窗；exit_guard 文档重写 | 2c08ce1 |
| 证据 | verify-s10（repro.ps1 + 各臂 run.log + cdb 栈存档；二进制/沙箱不入库） | （本次） |

## 真机验收（T5 实际执行结果，FINDINGS §2 全表）

- **判据全过**：fix2（终态）quit 2/2 + X close 1/1 均 ≤5s 干净退出（1.1s）、
  无僵尸；基线 3/3 复现僵尸（GL 与 software 同签名同栈）；daemon 每次
  quit 后 1.1-2.1s 自退；隔离沙箱零外泄（默认端点/真库未触碰，
  live-hmp 快照 + 每臂独立管道）。
- GL 后端实测：opengl32.dll + Intel igd*，无 libEGL → WGL（记入各臂日志）。
- fix2-quit-1 因弹窗被人工关闭判 INCONCLUSIVE（脚本存活门拦截，非缺陷）。

## 单测面（为何死锁本身不可单测、测什么）

- 死锁发生在 ExitProcess 期 TLS 析构 + 真 tokio blocking 池 + 真进程退出——
  testing backend（软件渲染）与测试进程模型下结构性不存在。
- 行为面钉死：`runtime_outlives_backend_runtime_drop`（backend.rs）——句柄
  消亡后在途任务继续完成；旧实现（Runtime 值语义）下该测试失败（任务被
  shutdown 取消）。看门狗时序 ×2（exit_guard.rs）。
- 回归：`cargo test -j 1 -p hmp-desktop` 102 单测 + 8 集成全绿。

## 风险与回滚

1. runtime 泄漏一个/进程：设计使然（进程同寿），进程退出即回收；测试进程
   每实例化 BackendRuntime 泄漏一个 runtime——单测试进程量级 1-2 个，可忽略
   （若未来测试大量构造再议显式 shutdown API）。
2. 未来有人给 BackendRuntime 加回「值语义/显式 shutdown」：契约单测会失败
   （shutdown 取消在途任务），且 exit_guard/backend 模块 doc 写明死锁史。
3. 其他 TLS 捕获 `Arc<BackendRuntime>`：T7 下 drop 为 no-op，无害（这正是
   T7 优于 forget 版的原因——不依赖 Arc 存活布局）。
4. **回滚**：revert 3bc41a6+2c08ce1 即回到 base 行为（可复现僵尸）；
   无 schema/协议/daemon 变更。
5. 解除判据（本修复为根因修复，无上游依赖）：若未来希望恢复 runtime 正常
   生命周期，前提是 tokio 的 ExitProcess 期 TLS 析构场景有官方结论/修复，
   且重跑 verify-s10 base 臂确认不再复现。

## 验证命令（已执行）

- `cargo fmt --all -- --check` ✅
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` ✅
  （hmp-desktop 全目标；workspace 其余 crate 未触碰）
- `cargo test -j 1 -p hmp-desktop` ✅（102 + 8 全绿，live_daemon 1 ignored 设计内）

## AUDIT 落账建议文案（docs/AUDIT.md §25-① 闭环，供用户粘贴）

> **S10 桌面退出死锁闭环（2026-10-24，调查→实施→实验改写根因）**：
> `hmp quit` 后 hmp-desktop 不可杀僵尸的真根因 = ExitProcess 期 TLS 析构：
> bridge schedule_refresh 的 thread_local slint::Timer 回调闭包捕获
> Arc<BackendRuntime>（退出时最后引用）→ Runtime::drop → BlockingPool::
> shutdown 在 oneshot 上死等已被 ExitProcess 杀死的 blocking 线程 →
> WaitOnAddress 永久挂起、困在 loader 关停期故不可杀（cdb 符号化栈实锤，
> GL/software 渲染器同栈；最初的 femtovg GL 销毁假设被 software 对照臂
> 证伪）。修复：BackendRuntime 持 &'static Runtime（T7，任何路径不触发
> shutdown 握手，契约单测钉死）+ 退出段 2s 看门狗（纯纵深）。真机验收：
> 修复臂 quit 2/2 + X close 1/1 ≤1.1s 干净退出；基线 3/3 复现（3min 短周期
> 即复现，E1 的 23.5min 非必要）。详证 worktree FINDINGS.md + verify-s10/
> （S10）。
