# S4「IPC 健壮性」计划（PLAN.md）

> 工作流 S4 · 分支 `feat/ipc-robustness` · 基线 `cd4a75e`。
> 使命：修复 docs/AUDIT.md §16「开放项」中的 IPC 三项。文档依据：§16（含 #4 wire 兼容约定、#5 quit 幂等先科）、§19（优雅退出全链路/不复活语义）、§22（StaleBackend/离线重连，勿破坏）。
> 上次尝试中断未留下任何残片（worktree 干净、无笔记），本计划为从头编写。
> **审核修订（裁决已折入）**：必改 M1（§2.2 池化 FIFO 假设与守护 + §⑤/T1 Windows 本机测试前置）、M2（§3.1 退避公式去内层 min）；可选项①（§2.1 僵尸第二形态）、②（§⑥ 实例回收）、③（T4 测试构造）、④（§⑤ hmp-desktop 编译面）全部采纳。

---

## ① 目标与范围

| # | 问题（AUDIT §16 原文口径） | 本计划目标 |
|---|---|---|
| 1 | accept 循环一错即停摆无自愈（server.rs 的 break） | daemon 不因单连接级/瞬时 accept 错误失去服务能力：accept 错误进入「分类 + 退避重试」循环；仅两类情形终止——持续失败达上限、Windows 管道名被他进程夺走——且终止走**优雅退出**（引擎 Quit → §19 Shutdown 广播 → 进程退出），不产生「活着但不可达」的僵尸 daemon |
| 2 | 单实例管道 + 1s BUSY 重试预算在连接风暴下可能耗尽（实测裸连 97% BUSY） | 服务端预创建多个待命管道实例（池化），并发连接直接命中待命实例；客户端 BUSY 重试预算 1s→5s 并加指数退避。双端配合，连接风暴下不再耗尽 |
| 3 | 客户端读响应无超时 | CLI `DaemonClient::request` 全程（写请求 + 读响应 + 解码）加超时；超时语义 = 报错退出、绝不挂死。不触碰 Subscribe 事件流（推送无超时语义，长连接正确行为） |

**范围外（明确不做）**：
- 不改任何 wire 协议（Request/Response/Event/DaemonState 零变化）→ §16 #4 的 serde default 约定本流不触发。
- 不改桌面端（hmp-desktop 禁碰；其 `backend.rs` 自有的请求读路径无超时，见 ⑧ 待裁决）。
- 不改引擎命令循环内联 await 解析的排队问题（§16 另一开放项，非本流）。
- Unix 侧 accept 错误处理一并纳入项 1（双平台照顾），但 Unix 无 BUSY 概念（domain socket 有 backlog），项 2 仅 Windows。

---

## ② 现状与根因（带证据）

### 2.1 项 1：accept 一错即停摆

**accept 循环结构**（`crates/hmp-daemon/src/server.rs:50-67`）：

```rust
pub async fn serve(mut listener: IpcListener, handle: EngineHandle) {
    loop {
        match listener.accept().await {
            Ok(stream) => { /* spawn handle_connection */ }
            Err(e) => {
                tracing::error!(%e, "accept failed");
                break;                       // ← server.rs:63：一错即退出循环
            }
        }
    }
}
```

`serve` 由 `serve.rs:182` `tokio::spawn` 持有，仅在退出编排时 `server_handle.abort()`（serve.rs:216）。**break 之后 serve 任务结束，但 daemon 进程继续运行**（引擎、tray、MPRIS/SMTC 均存活）——daemon 成为「活着但永久不可达」的僵尸：桌面端每 2s 重连永远失败且因 `ever_connected` 永不重拉（§19 语义），CLI 下一次命令 `connect_or_spawn` 会拉起**第二个** daemon，与僵尸进程并发访问同一 SQLite 库。

**僵尸的第二种形态（审核可选项①）**：serve 死后桌面订阅连接仍在——每连接 handler 任务是独立 tokio 任务，serve 退出不触碰它们，其持有的已连接管道实例让管道名继续存活（无 listening 实例、新连接全数 BUSY）。CLI `connect_or_spawn` 拉起的第二 daemon 探测失败后走 bind，`first_pipe_instance` 撞上存活实例的管道名 →「already running; exiting」让位退出；CLI 在 3s 就绪窗口内连不上任何 listening 实例 → 报「daemon startup timed out」。该形态下僵尸既不能服务新连接、又挡住替换实例，直到存活连接全部断开。自愈修复（serve 不再 break）使两种形态一并消失。

**错误可达路径**（`crates/hmp-daemon/src/transport.rs`）：
- Windows：`accept()` 内两处 `?` —— `server.connect().await?`（transport.rs:113，ConnectNamedPipe 系统调用失败）与补建下一实例 `ServerOptions::new().create(name)?`（transport.rs:116；资源耗尽 / 管道名被夺等）。任一失败 → `accept` 返回 Err → server.rs:63 break。
- Unix：`listener.accept().await?`（transport.rs:103）——ECONNABORTED / EMFILE / ENFILE 等瞬时错误同样一击致命。UnixListener 在 accept 错误后本身仍可用，停摆纯属自伤。
- 另有 panic 路径 transport.rs:111-112 `.expect("pipe listener instance consumed twice")`：仅当 accept 被连调两次而中间未补建时触发，现行流程不可达，池化改造后该路径自然消失（见 ③3.1）。

### 2.2 项 2：1s BUSY 预算在风暴下耗尽

**客户端预算**（`crates/hmp-daemon/src/transport.rs:54-69`）：

```rust
const BUSY: i32 = 231; // ERROR_PIPE_BUSY
const BUSY_RETRIES: u32 = 40; // 40 × 25ms ≈ 1s
for attempt in 0..BUSY_RETRIES {
    match ClientOptions::new().open(&name) { … sleep(25ms) … }
}
```

**服务端根因——单待命实例**（transport.rs:37-42、109-119）：`PipeListener` 只持有 `current: Option<NamedPipeServer>` 一个待连接实例，且仅在 accept 内、上一实例 ConnectNamedPipe **完成后**才补建下一个（transport.rs:114-116）。任意时刻至多 1 个实例处于 listening 态；并发客户端 CreateFile 竞争同一实例，输家拿 ERROR_PIPE_BUSY，只能靠客户端 1s 预算内盲重试。裸连风暴实测 97% BUSY（§16 记载）即此机制。

**Windows 语义关键事实**：管道实例自 `CreateNamedPipe` 起即处于 listening 态，客户端可在服务端**尚未调用 ConnectNamedPipe 时**直接连上（MSDN ConnectNamedPipe；tokio `NamedPipeServer::connect` 把已连实例返回为 Ok）。因此**预创建 N 个待命实例 = N 个并发连接坑位**，无需服务端并发 await 多个 connect，也无需依赖 connect 的取消安全性（tokio 1.53.1 文档载明 cancel-safe，`tokio-1.53.1/src/net/windows/named_pipe.rs:169-173`，但本设计不需要利用它）。

**池化正确性的隐含假设与守护（必改 M1，裁决记录）**：accept「`pop_front` 取最老实例 + `connect()`」只有在客户端 CreateFile 命中**最老** listening 实例（FIFO 语义）时才与客户端连接对号；该 FIFO 语义**无官方文档佐证**（Win32 仅承诺「an available listening instance」）。守护方式：改造后 bind 即建满池，现有 `roundtrip_between_listener_and_stream`、`listener_accepts_multiple_connections` 与 server.rs 全部测试的「首连」恰是**满池空闲态单客户端**场景——若内核实际按 LIFO 分配，首连会落在新实例上而 accept 停在最老实例上等待，上述测试当场挂死（不是静默错连）。故「Windows 本机执行 T1–T4 全部测试并全绿」即是对该假设的实证守护（见 §⑤ 完成前置）。tokio cancel-safe 与 mio ERROR_PIPE_CONNECTED→Ok 映射已经独立核实，不在假设之列。

### 2.3 项 3：客户端读响应无超时

**CLI 客户端读路径**（`crates/hmp-cli/src/client.rs:65-80`）：

```rust
pub async fn request(&mut self, req: &Request) -> Result<Response, CliError> {
    let frame = encode_frame(req)...;
    self.stream.write_all(&frame).await?;      // 无超时
    self.stream.read_exact(&mut len_buf).await?;  // 无超时：daemon 挂死则 CLI 永久挂死
    self.stream.read_exact(&mut payload).await?;  // 同上
    ...
}
```

CLI 全部远端命令（`main.rs` 的 `run_remote` / `run_quit` / 各 `cmd_*`）都经 `DaemonClient::request`，无一有超时。daemon 若死锁（如 §16 已知的命令循环内联解析阻塞）、或半途崩溃不落 EOF（极端情形），CLI 永久挂起。

**任务书口径勘误**：使命描述称「hmp-core 客户端与 hmp-cli 调用点加超时」。实查 `crates/hmp-core/src/ipc.rs`（1087 行）为**纯协议层**（类型 + 帧编解码），不存在客户端；现行唯一进程内客户端实现即 `hmp-cli/src/client.rs`（桌面端另有一份私有实现在 hmp-desktop/backend.rs，禁碰）。故超时落在 hmp-cli/client.rs，见 ⑧ 待裁决 1。

**不得破坏的既有语义**（证据）：
- `hmp quit` 幂等（§16 #5）：`main.rs:560-575 run_quit` 走 `connect_existing`（client.rs:55-57 纯连接），`endpoint_absent`（main.rs:581-588）把 NotFound/ConnectionRefused 判为「daemon 未运行 = 成功」。超时改动不触碰连接路径，不受影响。
- 桌面离线重连 / StaleBackend churn 记账（§22）：全在 hmp-desktop，禁碰；本流对其唯一渗透是 transport.rs 的 `IpcStream::connect` 行为改进（预算加长），对桌面是纯增强（§22 的 `STALE_BINARY_SEEN` 快速失败逻辑在更高层，不受影响）。
- §19 订阅广播：`Event::Shutdown`（ipc.rs:327-331）、server.rs:158-165 terminated 臂、serve.rs:22 `SUBSCRIBER_SHUTDOWN_GRACE`、serve.rs:204-216 term_wait→grace→abort 编排——本流不改这些路径，仅在其下游新增一条「fatal → Quit」入口（见 ③3.4）。

---

## 3 设计方案

### 3.1 项 1：accept 自愈（server.rs + transport.rs）

**推荐方案：错误分类 + 指数退避重试 + 有界放弃（放弃走优雅退出）**

`server.rs::serve()` 循环改为：

```
loop {
    match listener.accept().await {
        Ok(stream)  => { consecutive = 0; spawn(handle_connection); }
        Err(e) => {
            if accept_failure_is_fatal(&e) || consecutive >= MAX_CONSECUTIVE_ACCEPT_FAILURES {
                tracing::error!(%e, consecutive, "accept unrecoverable; requesting graceful shutdown");
                let _ = handle.command_tx.send(Request::Quit);   // 与 SIGINT/托盘退出/hmp quit 同一入口
                return;                                          // serve 任务收场；后续编排由 serve.rs 既有链路完成
            }
            let delay = accept_backoff(consecutive);             // 25ms × 2^n，封顶 1s
            tracing::warn!(%e, consecutive, delay:?, "accept failed; retrying");
            tokio::time::sleep(delay).await;
            consecutive += 1;
        }
    }
}
```

配套纯决策函数（可单测，先例：§22 `restart_worthwhile`「纯决策逻辑单测钉死」）：
- `fn accept_failure_is_fatal(e: &io::Error) -> bool` —— `#[cfg(windows)]`：`e.raw_os_error() == Some(5)`（ERROR_ACCESS_DENIED：我们不再持有任何实例、管道名已被另一 daemon 以 first_pipe_instance 占走——续命只会把自己挂到别人的管道上，正确行为是让位退出）；`#[cfg(unix)]`：恒 `false`（持 flock 期间 UnixListener 没有不可恢复的 accept 错误类；EMFILE 等瞬时错误等待即可）。
- `fn accept_backoff(consecutive: u32) -> Duration` —— `min(25ms << n, 1s)`：序列 **25/50/100/200/400/800/1000ms** 后钉在 1s（§2 proxy accept 先例同思路：退避重试）。**必改 M2（裁决）**：原式 `min(25ms << min(n,5), 1s)` 的内层 min 使外层永不生效（实际 800ms 封顶）——去掉内层 min 让封顶真正为 1s，与「64 次 ≈60s」估算自洽；实现以 shift 夹 6 仅防 `<<` 溢出（25ms<<6=1.6s 已超封顶，不改变输出序列）。
- `const MAX_CONSECUTIVE_ACCEPT_FAILURES: u32 = 64` —— 连续 64 次（退避总和 25+50+100+200+400+800 + 58×1000ms ≈ 59.6s ≈ 60s 持续失败）后放弃；成功一次即清零。

**transport.rs 配合（Windows）**：`PipeListener` 改池化（与项 2 合并实施，见 3.2）。池化后 accept() 的错误面收敛为「补建实例失败」与「connect 失败」两类，且**监听器对象在错误后保持可重用**（下一轮 accept 继续从 standby 取/补建），serve 的重试天然生效。Unix 侧 accept 错误不销毁 listener，无需 transport 改动。

**备选与取舍**：
- **备选 A：错误后整监听器重建（drop + re-bind）**。Windows 上重绑需先放手头实例，引入「无实例窗口」放大与第二 daemon 的名字竞争；Unix 上 flock 持有期间 bind 残留清理逻辑（serve.rs:124-127）在运行期复用会引入 TOCTOU。否决：错误分类 + 原监听器重试更小更安全。
- **备选 B：永不放弃、无限重试**（hmp-media proxy 先例是「连续 64 次失败才放弃」；若无限重试，极端 EBADF/失效 listener 下 daemon 变成「无限刷日志的活僵尸」，与现状 break 同级糟糕）。采用有界放弃 + 优雅退出：退出后 CLI 侧 `connect_or_spawn` 自然拉起全新 daemon（NotFound 路径），比任何带病续命都干净。**给协调者的裁决点见 ⑧-2。**
- **备选 C：serve() 返回 Result 让 serve.rs 编排决定退出**。需要改 serve.rs 的任务等待结构（server_handle.abort 改 select），侵入更大。否决：serve() 自持 `handle.command_tx` 发 Quit 即可汇入既有优雅退出链（serve.rs 信号任务同款做法，serve.rs:162-180）。

### 3.2 项 2：待命实例池 + 客户端预算加宽（transport.rs）

**服务端（主修复）——`PipeListener` 池化**：

```rust
#[cfg(windows)]
struct PipeListener {
    name: String,
    /// 待命实例池（CreateNamedPipe 后即处 listening 态：客户端 CreateFile
    /// 可直接占坑，无需服务端并发 await connect）。
    standby: VecDeque<NamedPipeServer>,
}
const PIPE_STANDBY: usize = 4;
```

- `bind()`：首个实例仍 `first_pipe_instance(true)`（单实例裁决语义不变，serve.rs:128-136 依赖的 ERROR_ACCESS_DENIED 判定原样保留）；随后**尽力**预建 `PIPE_STANDBY - 1` 个普通实例（失败仅降级池深、不使 bind 失败——单实例语义只系于首个实例）。
- `accept()`：先尽力补池至 `PIPE_STANDBY`（失败 warn、缩池续跑——这正是自愈：资源紧张时缩容，缓解后自动回满）；`pop_front` 取一个；**若池空且补建也失败**，返回 Err（serve 退避后重试即再次尝试补建）；对取出的实例 `connect().await`（客户端已连上则立即返回，正常负载路径零等待）；成功则作为 `IpcStream::PipeServer` 返回。原 transport.rs:111-112 的 `expect("consumed twice")` panic 路径随结构消失。
- 效果：4 个并发连接在不依赖服务端 accept 速度的情况下全部落坑；BUSY 仅在 >4 并发同瞬到达时出现，且被客户端预算吸收。

**客户端（兜底）——`IpcStream::connect` 预算 1s→5s + 指数退避**：

```
总预算 5s；退避 10ms 起 ×2 封顶 250ms（10/20/40/80/160/250/250/…）。
仅对 ERROR_PIPE_BUSY 生效，其余错误即时返回（语义不变）。
```

**serve.rs 单例探测短预算**：serve.rs:120 的「connect 探测活 daemon」改用 `connect_with_budget(&path, 250ms)`（新增内部 API，`connect` = `connect_with_budget(path, 5s)` 薄包装）。理由：探测期间若他 daemon 的管道全部实例忙（池深 4 下罕见但存在），BUSY 重试 5s 才失败属无谓拖慢第二实例启动；短预算后探测失败 → 走 bind → `first_pipe_instance` 撞 ACCESS_DENIED → 既有「already running; exiting」路径（serve.rs:130-135）正确收场，结果不变、路径更快。

**备选与取舍**：
- **备选 A：每实例一任务（tokio 官方 named_pipe 示例模式）**。K 个 worker 各自 `connect().await` 后经 channel 交还。功能等价但引入「bind 同步构造器内/首 accept 时 lazy spawn 任务」的生命周期问题（bind 在无 runtime 上下文会 panic 的 API 回归、Drop 语义、测试残留任务），复杂度不成比例。否决——待命池以「实例天生 listening」的 Win32 语义达到同等效果，单 accept 循环结构不变。
- **备选 B：客户端 `WaitNamedPipe` 阻塞等待**（Win32 惯用法）。需要 `spawn_blocking` 包裹且语义上是「等任意实例空闲」的全局等待，与预算重试相比无增益、多一层阻塞线程管理。否决。
- **备选 C：只加客户端预算、不动服务端**。97% BUSY 的根因在服务端单坑位；只放宽客户端等于把风暴转成 5s 内的排队踩踏，且每失败一次都是一次完整 CreateFile 系统调用。否决——服务端池化是主修复，客户端是纵深。
- `PIPE_STANDBY = 4` 的量纲依据：桌面端峰值并发短连接 ≈ 队列行封面预取信号量 8（§20）+ 主订阅连接 + CLI 偶发命令；池 4 + 客户端 5s 退避可吸收远超此数的瞬时风暴。见 ⑧-3。

### 3.3 项 3：CLI 请求超时（hmp-cli/client.rs）

**推荐方案：`request()` 整体包一层 `tokio::time::timeout`**

```rust
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

impl DaemonClient {
    pub async fn request(&mut self, req: &Request) -> Result<Response, CliError> {
        self.request_with_timeout(req, REQUEST_TIMEOUT).await
    }
    pub(crate) async fn request_with_timeout(&mut self, req: &Request, timeout: Duration)
        -> Result<Response, CliError>   // 测试可注短超时
    { /* tokio::time::timeout(timeout, async { 原写+读+解码体 }) */ }
}
```

- 新增 `CliError::Timeout(Duration)` 变体（thiserror 文案英文，与仓库用户可见文案约定一致）：`"daemon did not respond within {0:?}; it may be wedged — try `hmp quit` and retry"`。
- **30s 量纲**：最慢合法路径是内容读类请求（Search/Lyric/CoverGet/Discover/Top/Comment，daemon 出网代理；CoverGet 含整图下载）。10–15s 在弱网下会误杀合法请求；30s 覆盖上游（hmp-qqmusic-api 客户端）自身超时之外的余量。见 ⑧-4。
- 超时后连接即弃（`DaemonClient` 随之 drop，管道实例归还）；daemon 侧的 handler 任务最多再等一次 `read_frame` EOF 收场，无泄漏（server.rs:104-173 既有 reader/abort 收尾）。
- **Subscribe 长连接不受影响**：CLI 不使用 Subscribe；桌面端自有读循环在 hmp-desktop（禁碰），本改动不涉事件流。
- `connect` 侧已有界（Windows BUSY 预算 5s；Unix connect 即时），无需另加。
- main.rs 无需改动：全部远端命令收口于 `DaemonClient::request`，超时在收口点生效即覆盖所有调用点（含 `run_quit`——daemon 挂死时 `hmp quit` 30s 报错而非挂死，符合「报错不挂死」）。

### 3.4 与 §19「不复活用户主动退出的 daemon」的相容性论证（单独小节）

**结论：自愈机制完全内嵌于存活 daemon 的 serve 任务内，不创建进程、不触发拉起，与 §19 的三条边界全部相容。逐条论证：**

1. **用户主动退出（tray/`hmp quit`/SIGINT）**：退出链是 引擎 Quit → terminated 置位 → serve.rs:214 1s 广播宽限 → `server_handle.abort()`（serve.rs:216）→ 进程退出。serve 任务被 abort 时，无论它正在 accept、退避 sleep 还是刚要重试，都随任务取消而终止——**自愈重试循环没有任何路径能在 abort 后继续执行**（tokio 任务 abort 语义）。进程退出后管道实例随句柄消亡，不复活。
2. **本流新增的「fatal → Quit」出口只会加速退出、不会延迟或绕过**：它发送的 `Request::Quit` 与 SIGINT 处理器（serve.rs:162-180）、`hmp quit`、StaleBackend 重启流（§18）汇入**同一引擎入口**，走的是 §19 已验证的全链路（terminated → Shutdown 帧 → 宽限 → 清理）。订阅中的桌面端会收到 `Event::Shutdown` 优雅关窗，不落离线僵尸。
3. **EOF 竞态与拉起资格（§19.4 `ever_connected`）**：该逻辑在 hmp-desktop（禁碰），本流零改动。自愈改变的只是「daemon 活着时的可达性」：现状 break 后桌面 2s 重连永远失败（且永不重拉，正确但无意义地空转）；自愈后连接恢复，重连自然成功。**不存在任何「daemon 已退出但仍被连上/拉起」的新窗口**——探测/连接语义未变（serve.rs:120 探测短预算只影响第二 daemon 启动速度，见 3.2）。
4. **崩溃（kill -9）语义不变**：无 Shutdown 事件、客户端走离线重连（§19.5）。本流不触碰 EOF 语义。
5. **§22 StaleBackend 不破坏**：指纹核对（`backend_build`）逻辑与数据面不动；`STALE_BINARY_SEEN` 快速失败在桌面侧。唯一交集是 transport 层 connect 预算加宽——对 §22 场景（连不上 → 立即 NotFound 而非 BUSY）行为不变。

---

## ④ 任务分解

> 所有任务在 worktree `D:\coding\hutao-music-player\.worktrees\s4-ipc-robust` 内进行。逐 crate `cargo check -j 4` / 测试 `-j 1`。

### T1 transport.rs：Windows 待命实例池（项 2 服务端半 + 项 1 的 Windows 错误面收敛）

**触及文件**：`crates/hmp-daemon/src/transport.rs`（结构体 `PipeListener`；`IpcListener::bind` Windows 臂；`IpcListener::accept` Windows 臂；模块文档头；文件内 `mod tests` 新增用例）。

改动要点：
- `PipeListener { name, standby: VecDeque<NamedPipeServer> }` + `const PIPE_STANDBY: usize = 4`（crate 可见，供测试断言）。
- `bind`：首实例 `first_pipe_instance(true)`（语义不变）；随后尽力预建至池深，失败仅 warn 降级。
- `accept`：补池（尽力）→ `pop_front`（空且补建失败 → `Err`，监听器保持可重用）→ `connect().await` → 返回 `PipeServer` 流。`connect()` 失败时**丢弃该实例**（不回池）并返回 Err——serve 的重试会取下一待命实例，实现实例级自愈。
- 删除 `expect("pipe listener instance consumed twice")` 路径。

**验收标准**：
- 既有 `roundtrip_between_listener_and_stream`、`listener_accepts_multiple_connections` 原样通过（池化不破坏基本收发；**这组测试同时是 §2.2 FIFO 假设的守护**——满池空闲态单客户端场景，非 FIFO 会当场挂死）。
- 新增 `standby_pool_absorbs_concurrent_connects_without_accept`：bind 后**不调用 accept**，并发 `PIPE_STANDBY` 个 `IpcStream::connect` 全部在 2s 内成功；随后顺序 `accept` 收齐全部。（旧代码在 Windows 上必失败：单实例 → 其余连接耗尽 1s 预算；Unix 上经 backlog 天然通过。）
- 新增 `bind_denies_second_daemon_on_windows`（cfg(windows)）：对同名管道二次 `IpcListener::bind` 返回 `raw_os_error()==5`（钉死单实例裁决语义不被池化破坏）。
- **完成前置（必改 M1）**：T1–T4 全部测试在 **Windows 本机**执行且全绿——CI 无 Windows 面，池化/busy/单实例语义只有 Windows 能实证（本流实施机即 Windows，照做）。

### T2 transport.rs + serve.rs：客户端 BUSY 预算/退避 + 探测短预算（项 2 客户端半）

**触及文件**：`crates/hmp-daemon/src/transport.rs`（`IpcStream::connect` Windows 臂改为 `connect_with_budget` 内部函数 + 常量 `CONNECT_BUDGET: Duration = 5s`、`BUSY_BACKOFF` 序列；tests 新增退避序列单测）；`crates/hmp-daemon/src/serve.rs` **仅 120 行一处**（探测改 `connect_with_budget(&path, Duration::from_millis(250))`）。

**验收标准**：
- 单测 `busy_backoff_progression`：首延 10ms、逐次 ×2、封顶 250ms、累计达 5s 停（纯函数钉死策略）。
- 非 BUSY 错误（如 ERROR_FILE_NOT_FOUND）仍然首次即返回——`quit_no_spawn.rs`（hmp-cli/tests）依赖的 NotFound 语义不变。
- `cargo test -j 1 -p hmp-cli --test quit_no_spawn` 通过（现成回归）。

### T3 server.rs：accept 自愈循环 + fatal 优雅退出（项 1）

**触及文件**：`crates/hmp-daemon/src/server.rs`——**精确改动区域清单（供 S1 并行流对齐）**：
1. `pub async fn serve(mut listener, handle)` 函数体（现 server.rs:50-67）：循环骨架重写，**仅此一处**。
2. 顶部 `use` 区（现 server.rs:6-15）：新增 `std::time::Duration`。
3. 新增模块级私有项（放在 `serve` 与 `requires_credential` 之间）：`accept_failure_is_fatal` / `accept_backoff` / `MAX_CONSECUTIVE_ACCEPT_FAILURES`。
4. 文件尾部 `mod tests`：新增决策函数单测。
   **不触碰**：`handle_connection`、`handle_frame`（S1 加 dispatch 分支的区域）、`requires_credential`、`quality_alias_valid`、`read_frame`/`write_frame`、`socket_path` 及既有全部测试。

**验收标准**：
- 单测：`accept_backoff` 退避序列与封顶；`accept_failure_is_fatal`——cfg(windows) 下 raw 5 → true、其他 → false（unix 版恒 false 用 cfg 编译分支各自断言）；连续计数上限常量存在且被 `serve` 引用（编译期保证）。
- 既有 server.rs 测试全部通过（`quit_broadcasts_shutdown_to_subscribers` 等走新 accept 路径）。
- `cargo check -j 4 -p hmp-daemon` 零警告（clippy `-D warnings` 基线）。

### T4 hmp-cli/client.rs：请求超时（项 3）

**触及文件**：`crates/hmp-cli/src/client.rs`（`CliError` 增 `Timeout` 变体；`request` 拆 `request_with_timeout`；`mod tests` 新增回环用例）。**hmp-cli/src/main.rs 预计零改动**（超时收口在 `request`，全部调用点自动覆盖；若评审要求 main.rs 显式注释则仅加注释，不改逻辑）。

**验收标准**：
- 新增 `request_times_out_when_daemon_silent`：**临时端点** bind listener → accept 一条连接 → 读走请求帧后**不应答**；`request_with_timeout(Request::Status, 150ms)` 在 ~1s 内返回 `Err(CliError::Timeout(..))`（绝对不挂死；tempfile 已是 dev-dep）。**构造方式（可选项③，写死在验收标准）**：用模块内私有 `DaemonClient::try_connect(&temp_path)` 构造客户端——`connect_existing`/`connect_or_spawn` 会碰生产端点 `\\.\pipe\hmp`，禁用。
- 既有 `protocol_error_*` 三测与 quit 幂等测试（`quit_on_dead_endpoint_succeeds_without_spawning`）不变绿转红。

### T5 全量回归与验收

**触及文件**：无新增改动；按 ⑤ 执行。

---

## ⑤ 测试计划

**完成前置（必改 M1）**：T1–T4 的全部测试（含既有与新増）在 **Windows 本机**执行且全绿为完成前置——CI 无 Windows 面，池化/busy/单实例语义只有 Windows 能实证（§2.2 FIFO 假设即由此守护）。本流实施机即 Windows 本机。

**纪律**（依任务书）：逐 crate、`-j 1`、不动 `CARGO_TARGET_DIR`、不 `cargo clean`、不跑全仓并发测试；wiremock 相关集成测试（daemon tests 里的 reconcile 系）如需跑，单独逐个 `-j 1`。共享 target 目录可能出现 §21 记载的「幽灵编译错误」——遇冷跑复验，不清目录。

| 步骤 | 命令（worktree 内） | 预期 |
|---|---|---|
| 1 | `cargo check -j 4 -p hmp-daemon -p hmp-cli` | 零错误零警告 |
| 2 | `cargo clippy -j 4 -p hmp-daemon -p hmp-cli --all-targets -- -D warnings` | 干净 |
| 3 | `cargo test -j 1 -p hmp-daemon --lib`（transport + server 单测，Windows 本机原生覆盖管道双路径） | 全绿（含新增池化/退避/决策单测） |
| 4 | `cargo test -j 1 -p hmp-cli`（lib + `quit_no_spawn` 集成） | 全绿 |
| 5 | `cargo test -j 1 -p hmp-core --lib` | 全绿（未改动，协议回归锚） |
| 5b | `cargo check -j 4 -p hmp-desktop`（可选项④：transport 公共 API 变更的**零改动编译面**确认——桌面是 `IpcStream::connect` 调用方，预算加宽对其是纯增强；不越 S7 界、不改其代码） | 零错误 |
| 6 | `cargo fmt --all -- --check` | 干净 |
| 7（真机冒烟，可选） | 手动：起 `hmp serve`，脚本并发 32 裸连接 + 正常 `hmp status` 混跑 | 全部成功或可控报错，daemon 存活、日志可见补池行为 |

**测试设计说明**：
- 无法从外部确定性制造真实 accept 瞬时错误（EMFILE 等），故项 1 的核心逻辑以**纯决策函数单测**钉死（`restart_worthwhile` 先例），循环接线由代码审阅 + 既有集成测试间接覆盖。
- 连接风暴端到端（97% BUSY 复现 → 修复后清零）依赖时序，不适合 CI 断言；以 `standby_pool_absorbs_concurrent_connects_without_accept` 的确定性版本（不 accept 的静态坑位测试）替代，真机风暴留冒烟。
- 进程级 e2e 先例（`#[cfg(unix)]` 门控 daemon_cli.rs、`#[ignore]` 真机项）本流不新增此类用例——改动均可 crate 内验证，避免 Windows 上进程级测试的三雷。

---

## ⑥ 风险与回滚

| 风险 | 评估 | 缓解 |
|---|---|---|
| 池化改变了「任意时刻至多 1 待命实例」的隐含假设 | 全仓唯一消费方是 server.rs `serve` 与 transport 自测（已 grep 证实：IpcListener 仅 serve.rs:128 与测试使用）；桌面/CLI 只用 `IpcStream::connect`（走池的客户端侧，只受益） | T1 既有用例原样通过即守护 |
| Windows 管道名被夺（全实例瞬时消失 + 第二 daemon 抢绑）后，我们的普通 create 可能挂到对方管道 | 触发窗口极窄（要求补建连续失败期间所有连接实例也恰好全部结束）；行为不劣于现状（现状 break 后必然僵尸 + 第二 daemon） | fatal 分类捕获 ACCESS_DENIED → 优雅退出让位；PLAN 记录该边界 |
| fatal 误判：daemon 健康但持续 60s accept 失败 → 退出 | 60s 持续失败意味着事实上不可服务；退出后 CLI 自动拉新实例优于带病续命 | 阈值 64 次可调；日志带 consecutive 计数便于事后归因 |
| 30s CLI 超时误杀弱网慢请求 | 低概率（上游 API 有自身超时）；误杀表现为明确报错可重试 | 常量单点可调；见 ⑧-4 |
| serve 短预算探测在极端繁忙下误判「无 daemon」→ 第二实例 bind → 撞 first_pipe_instance | 不会误启：bind 失败即走「already running; exiting」（既有路径），只是多花一次 bind | 已由 serve.rs:130-135 语义覆盖，无需新代码 |
| 与 S1 合并冲突 | server.rs 触碰区域已限定 `serve()` 函数体 + use 区 + 新增私有项 + tests 尾部，与 dispatch（handle_frame）不相交 | 见 ⑦ |
| 回滚 | 改动集中于 transport.rs / server.rs `serve` / client.rs / serve.rs 一行，无协议/存储/持久化变化 | 单 commit revert 即完全回滚；无数据迁移 |
| 待命实例泄漏？（审核可选项②的回答） | 管道实例是内核对象句柄（`NamedPipeServer` 持 HANDLE）：listener drop（serve 任务结束）、connect 失败丢弃、进程退出三种路径都全量回收，无用户态资源可泄漏；池化常态成本仅多 3 个句柄 | 无需显式清理逻辑；Win32 句柄语义保证 |

---

## ⑦ 与并行流的边界确认

- **S1（在线内容，另一 worktree 改 server.rs dispatch）**：本流 server.rs 改动 = `serve()` 函数体（50-67 行区域）+ 顶部 use + `serve` 与 `requires_credential` 之间新增三个私有项 + tests 尾部追加。**不进入** `handle_frame`/`handle_connection` 及任何 Request 分支——S1 新增 dispatch 分支与本流零文本重叠；合并顺序任意。
- **S1 同文件交集文件**：仅 server.rs（上述）。transport.rs / serve.rs / client.rs 无并行流认领（按任务书所有权）。
- **禁碰清单遵守**：engine.rs、engine_tests.rs、content.rs、hmp-desktop/**、hmp-player/**、hmp-media/**、hmp-qqmusic-api/**、.github/**、README.md、docs/**（本 PLAN.md 除外）——本流只读引用了 AUDIT.md 与 tokio registry 源码，未改动。
- **对 hmp-desktop 的传导**：transport.rs 公共 API（`IpcStream::connect` 预算加宽、`IpcListener` 内部结构变化不外露）——桌面零改动自动受益；桌面自身请求无超时的缺口**不在本流**（禁碰），已在 ⑧ 列为待裁决。
- **主工作区**：未触碰（只读）；未操作其他 worktree；未 push。

---

## ⑧ 待协调者裁决的问题清单

1. **「hmp-core 客户端」勘误**：hmp-core/ipc.rs 实为纯协议层、无客户端代码；超时落在 hmp-cli/client.rs 的 `DaemonClient::request`。桌面端（hmp-desktop/backend.rs）自有请求读路径同样无超时，但属禁碰——是否派生独立小流补齐，请裁决。
2. **放弃阈值与 fatal 语义**：连续 64 次 accept 失败（≈60s）即发 Quit 优雅退出（备选为无限重试）；Windows ACCESS_DENIED 即刻让位退出。若倾向「永不自杀、无限退避」，改动仅一行常量/分类函数，请裁决。
3. **池深 `PIPE_STANDBY = 4`**：基于桌面峰值并发（预取信号量 8 + 订阅 + CLI）与「池 + 5s 客户端预算」的纵深估算；如需更大余量可到 8（成本仅 4 个额外句柄），请裁决。
4. **CLI `REQUEST_TIMEOUT = 30s`**：为弱网 CoverGet/Discover 留余量；若产品更倾向交互敏感（10–15s 快速失败），单常量调整，请裁决。
