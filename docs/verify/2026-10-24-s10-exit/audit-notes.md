# S10 独立交付审计笔记（四发，只读+亲跑）

审计对象：worktree s10-exit-deadlock @ c51f4e1（fix/desktop-exit-deadlock，7 提交）。
审计代理：独立交付审计（四发）。增量落盘，最终消息汇总。

## A1 cdb 栈可信度 —— 部分核实

- `verify-s10/zombie-stacks-cdb.txt` 在库，24 帧符号化栈，帧序与 tokio 0.2x
  blocking shutdown 已知语义吻合（WaitOnAddress→futex→Condvar→park→
  block_on<oneshot::Receiver>→shutdown::Receiver::wait→BlockingPool::shutdown），
  其上是 Timer::drop→CallbackVariant drop_glue→Arc<BackendRuntime>::drop_slow→
  TLS destroy（RefCell<Timer>）→destructors::list::run→windows cleanup→
  RtlFreeActivationContextStack(FLS)→LdrShutdownProcess→RtlExitUserProcess→
  ExitProcess→ucrtbase exit→__scrt_common_main_seh。自洽、与 main 真实
  返回路径吻合。
- **证据弱点（P2）**：档案把 GL（46516）与 software（73588）两栈**合并记录**
  （「两栈逐帧一致（仅地址不同），合并记录」），原始两份 cdb 输出未分别
  入库 →「同栈」论断依赖操作者转述，档案内不可独立复核。渲染器无关性
  结论仍由 base-sw-1 臂日志（ZOMBIE + 模块表无 igd*）独立支撑，故此弱点
  不动摇根因结论，但与任务书中「两组栈在档且一致」的期望有差距。
- cdb 存在性：待核（B 段）。

## A2 因果推理 —— 核实通过

- ② 闭包持 Arc 最后引用：现源码核实。bridge.rs L548-581：thread_local
  `RefCell<slint::Timer>`（L549-551），SingleShot 回调闭包 move 捕获
  `Arc::clone(runtime)`（L553/L565）。main.rs 顺序 prefs→runtime→ui→run()，
  run() 返回后 drop(ui)、drop(runtime)（main.rs L66/L69）→ TLS Timer 内
  的 Arc 成为最后引用，与栈帧 `Arc<...>::drop_slow` 位置完全吻合。✓
- ① TLS/FLS 析构时机：ExitProcess→先终其余线程→LdrShutdownProcess 跑主线程
  TLS/FLS 析构——Windows loader 既有语义；栈帧（RtlFreeActivationContextStack
  处触发 FLS 链）与之吻合。✓
- ③ BlockingPool::shutdown 语义：tokio Runtime::drop 对 blocking 池逐线程
  发 shutdown 信号并 block_on(oneshot Receiver) 等 ACK；线程已死则永久等
  ——栈帧与该实现一致。✓
- ④ TerminateProcess 无效：loader 关停期主线程不可清出——经验事实（E1+
  本轮 3 僵尸 Stop-Process -Force 后仍存活，各臂 run.log 有记录），
  非纯理论。✓
- ⑤ 运行期 drop 不死锁/退出期会：运行期线程全活、握手能完成；退出期
  应答方已被杀 → 死等。逻辑成立。✓
- blocking 池为何非空：进程内任一 spawn_blocking（DNS 解析等）即驻留；
  残余 2% 置信豁免（未指认具体来源）已在 FINDINGS 声明，诚实。

## A3 fix2 正确性 —— 核实通过

- backend.rs L101-115：`rt: &'static tokio::runtime::Runtime`，构造
  `Box::leak`。hmp-desktop 内**无其他 Runtime 构造点**（待 grep 复核
  new_current_thread 等——初步 grep 仅此一处 Box::leak）、**无 Drop impl
  于 BackendRuntime**（全 crate Drop 仅 ExitGuard）→ 无任何路径触发
  Runtime::drop。✓
- 契约单测 runtime_outlives_backend_runtime_drop（backend.rs L696-711）：
  50ms sleep 任务 + 句柄 drop 后 2s 内 recv。旧实现（持 Runtime 值）下
  drop 即 shutdown 取消在途任务 → recv 超时必红。论证成立。✓
- 副作用面：runtime 进程级泄漏一个（线程+内存），进程退出即回收；桌面
  无退出期落盘依赖 runtime；测试进程内多次 new() 泄漏多个——PLAN 风险1
  已登记。可接受。
- main.rs L62-70：watchdog arm→drop(ui)→drop(runtime)→disarm，注释与
  T7 语义一致（drop=递减引用计数，TLS Timer 持最后引用时 drop 为 no-op）。

## A4 revert 完整性 —— 初步核实

- grep 全 crate：无 `forget`、无 `KeepWindowShown` 残留。✓
- Box::leak 仅 backend.rs 一处。✓
- exit_guard.rs 语义边界注释如实（明示看门狗救不了 loader 关停期）。
- **P3 小疵**：exit_guard.rs L3 引 "FINDINGS.md §1.2"，真根因在 §1.3
  （§1.2 是 Slint 退出链细节）。交叉引用错位。
- T4 exit_paths 测试现态：待核。

## A5 九臂自洽 —— 核实中有两处发现

日志逐臂读过（repro.ps1 + 9 份 run.log 全文/尾段）：

- 判定分布与 FINDINGS §2 表格一致：base×3 ZOMBIE（1 线程 Wait/Unknown、
  Force 无效、276s 臂长）；fix×3 CLEAN 1.1s；fix2 quit-2/close-1 CLEAN
  1.1s；fix2-quit-1 存档日志 verdict=CLEAN_EXIT(0s)。
- **发现 1（P2，文档精度）**：fix2-quit-1 —— FINDINGS/PLAN 记 INCONCLUSIVE
  （弹窗被人工关闭，存活门拦截），但存档日志无任何 INCONCLUSIVE/拦截行，
  verdict 行就是 CLEAN_EXIT；该臂还是唯一 GL modules 为空的臂（其他 GL 臂
  全部列出 opengl32+igd*，software 臂 opengl32 无 igd*）→ 该臂进程状态确实
  异常、与人工干预叙述相容，INCONCLUSIVE 是**人工保守降级**（若按日志字面
  反而是 PASS）。结论方向安全（保守），但「存活门拦截」表述与日志不符，
  证据不自足。另：repro.ps1 存档版含存活门+注释（动机即此事故）→ 存档脚本
  疑为终版，部分早期臂日志可能出自旧版脚本（fix-quit-1 的 finally 行为与
  终版脚本存在时间戳矛盾佐证此点）。属 provenance 弱点，不动摇判定。
- **发现 2（P2，文档精度）**：daemon ≤15s 断言并非「全过」——quit 臂 7 次
  断言 5 True（2.1/2.1/1.1/1.6/1.1），2 False：base-quit-1（僵尸臂，daemon
  ~+60-70s 才自退）与 fix-quit-1（clean 臂，daemon >15s 未退、cleanup 强杀，
  pid 25840）。FINDINGS §0.4「本轮 2.1s/1.1s/1.6s ≤15s 断言全过」与 §2
  「daemon 每次 quit 后 ≤15s 自退」两处表述过强。所有臂 cleanup check 均
  alive=False（daemon 最终全灭，无一臂遗留 daemon）。此为 daemon 侧行为
  波动（僵尸桌面持有 IPC 连接可解释 base-quit-1；fix-quit-1 未解释），
  与桌面修复无冲突，但文档应如实。
- base-sw-1 模块表（opengl32 但无 igd*/d3d12）独立佐证 software 对照臂成立。

## A6 转向史 —— 核实通过

FINDINGS §4 三段（GL 假设 85%→software 臂证伪→cdb 实锤→撤销 T1/T3）与
git 七提交（dd0557c 文档→d8afc1b T2→e5bed6f T1+T3→ba57a91 T4→3bc41a6 T7→
2c08ce1 撤销→c51f4e1 终态文档）时序完全对应，无粉饰；§1 原始排除过程
保留「S2 结论错在哪」的自我修正记录，诚实度高。

## B 验证命令（第五发亲跑，全部退出码级核实）

- `cargo fmt --all -- --check`：exit 0，无 diff。✓
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`：exit 0。
  首跑 2.35s 全缓存命中（证据弱）→ `cargo clean -p hmp-desktop` 后
  `-p hmp-desktop` 全新编译 55.24s 复跑，0 警告 exit 0。✓（改动 crate
  实证；其余 crate 为缓存绿 + main=a5a0236 既有基线）
- `cargo test -j 1 -p hmp-desktop`：exit 0。lib 单测 **102 passed**（与
  PLAN.md L46/L68 声称精确一致）；集成 10 二进制 **10 passed +
  live_daemon 1 ignored**（设计内）+ doc 0；0 failed。✓
  **P3**：声称「8 集成」为低估（现态 10 过）——方向有利，文档该更新。
  （静态 grep `#[test]`×15，其中 ~4 个 cfg 门控未编入 Windows 目标，
  运行态 11 个与逐文件输出吻合，无隐藏失败。）
- cdb 存在：`C:\Program Files (x86)\Windows Kits\10\Debuggers\x64\cdb.exe`
  在（182,728 B）✓——A1 的「待核」销项。

## A3 补 全仓 Runtime 构造点 + T4 现态（第五发）

- grep 全 crate（显式构造 + `#[tokio::main]` 宏模式）：
  - 生产代码仅两处：hmp-desktop/src/backend.rs L108（T7 `Box::leak`
    &'static——桌面进程内唯一 Runtime）与 hmp-cli/src/main.rs L361
    `#[tokio::main]`（宏生成，main 尾正常 drop：线程全活、无「TLS 持
    Arc」模式，不属 S10 风险类）。hmp-daemon/src 零直接构造（跑在
    CLI 的 runtime 上）。examples/tests 的临时 runtime（playlist_smoke/
    spawn_probe/shot-headless/daemon tests）均为开发面，非出货面。
  - hmp-desktop 内 `impl Drop` 仅 ExitGuard → **无任何路径可触发
    Runtime::drop**，BackendRuntime 唯一性成立。✓
- T4 exit_paths 现态：ba57a91 新增 tests/exit_paths.rs（75 行，钉
  KeepWindowShown 行为面）→ 2c08ce1 撤销 T1/T3 时**整体删除**（钉的
  行为已不存在，删除自洽，grep 无残留与此一致）。残留缺口（P3 观察）：
  撤销后无自动化测试钉「X 关窗→run() 返回」现行为；真覆盖由 9 臂
  真机实验承担（T4 原文亦自认 testing backend 下死锁结构性不存在）。

## C 合并与触及面（第五发）

- `git merge-tree --write-tree a5a0236 c51f4e1` → 树 OID
  c8509f3…，无冲突清单，exit 0 = **零冲突**（main 现 HEAD 即 a5a0236）。✓
- 触及面（a5a0236..HEAD，873+/7-）：M `crates/hmp-desktop/src/{backend,
  lib,main}.rs` + A `exit_guard.rs` + A `FINDINGS.md`/`PLAN.md` +
  A `verify-s10/`×12。与预期「仅 crates/hmp-desktop + FINDINGS/PLAN +
  verify-s10」**逐文件一致**。✓
- 备注（P3）：FINDINGS/PLAN 置于仓库根，S9 先例是归位 docs/plans/
  （f37e490）——放置不合先例，合并时协调者可定夺，不违任务书字面。

## D 僵尸现状（第五发，只读 tasklist，零触碰）

- 2712：**仍在**（hmp-desktop.exe，Console/1，内存 344K——工作集已被
  削到极小）。
- 46516 / 65904 / 73588：**已消失**（无匹配任务）。前三发记录的
  Stop-Process -Force 后存活僵尸，3 个在未重启的同一会话内最终消亡
  （时点未知）。观察性软化：「TerminateProcess 后永久不可清出」宜表述
  为「审计窗口内不可清出、消亡时点不可控」——不动摇 T7 价值
  （1.1s 干净退出 vs 数十分钟不可控僵尸）。

## 尾 worktree 卫生（只报告不清理）

- `verify-dump/` 238MB 未跟踪（healthy-62204.dmp 248,878,129B + 5 个
  诊断脚本）；`verify-s10/bin-base|bin-fix|bin-fix2`（构建产物）与
  `sandbox-*/data|media|config` 运行时输出未跟踪；AUDIT-NOTES.md 自身
  未跟踪。*.log 在 .gitignore L22，库内 run.log 均 -f 强制纳入（与
  a5a0236 先例一致）。合并前无人应 `git add -A`。

## 总结论（第五发收口）

**通过——可交协调者合并。** B/C/D 全过零阻塞；A 部分既有 P2×3
（A1 合并栈、A5 fix2-quit-1 表述、A5 daemon ≤15s 过强）+ P3×4
（exit_guard §1.2→§1.3 错位、「8 集成」低估、T4 撤销后行为钉缺位、
FINDINGS/PLAN 根目录放置）均为文档精度/后续项，不触及代码正确性与
合并安全性。建议协调者合并时顺手修 3 处 P2 文档表述（≤10 行文本）。
