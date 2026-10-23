# S3 计划：引擎命令循环去阻塞 + 播放边界修复

> 基线：`cd4a75e`（worktree `.worktrees/s3-engine-deblock`，分支 `feat/engine-deblock`）。
> 对应 docs/AUDIT.md §16「开放项」播放链路四项。行号引用以基线为准。
>
> **已按评审修订（v2）**：落实必改 M1-M6 与可选 O1-O5 裁决——M1 守卫 (c)
> 改按代际 `error_handled_gen`；M2 弃置登记按条数推进 seq 并写明 Stop 覆盖
> 入口；M3 Stop/Shutdown 失效在途 seek；M4 LIST_RESOLVE_TIMEOUT 经
> `start_with_options` 注入并补两个回归；M5 不改 player.rs（复用
> `EngineError::Internal`）；M6 数字更正（引擎测试 67 个、
> start_with_options 调用点 9 处）；O1 Play 命令顺带清闩；O2 worker 每作业
> panic 隔离；O3 测试 11 措辞；O4 QueueAppend 满拒绝不推 seq；O5 音频线程
> 停摆界措辞。下文相应段落已就地更新并标注（M#）/（O#）。

---

## ① 目标与范围

按 AUDIT §16「播放」开放项逐条：

1. **引擎命令循环内联 await 解析去阻塞**：`Play(歌单/专辑)` / `PlayList` /
   `PlayNext` / `QueueAppend` 的 `resolve_source_ids`（分页拉取，秒级）目前
   在引擎命令循环内联 await，期间所有播放控制命令（Pause/Seek/Volume/Next）
   排队，UI 感知「点了没反应几秒」。目标：解析移后台 worker（FIFO + 有界
   通道 + 每作业总超时），命令循环只投递、提交与查询。
2. **中途读错误不再被当成 EOS 静默跳歌**：DecryptReader 中途拉流失败 →
   rodio 解码器把 IO 错误当源终止 → `sink.empty()` 被判 `Ended` → 引擎
   `on_ended` 自动跳下一首，全程无 Error 态。目标：播放器侧以「错误闩」
   区分中途错误与真实曲尾，发显式 `PlayerEvent::Error` + `status=Error`；
   引擎侧置 `last_error`、闭合播放会话、置 `Failed` 阶段，**不自动跳歌**。
3. **`try_seek` 阻塞驱动命令循环最长 ~30s 去阻塞**：rodio `Sink::try_seek`
   同步等待音频线程执行 seek，远端未缓冲区间 seek 的探测读受 CDN 超时
   （connect 10s + read 30s）钳制，期间 hmp-player `drive` 循环冻结、
   全部播放命令排队。目标：seek 移 `spawn_blocking` + 代际守卫 + ACK 超时，
   `drive` 循环不再共享该阻塞。
4. **预解析期总超时**：`schedule_preload` 的后台 `resolve_track` 无任何
   总超时（含无 Range CDN 回退全量下载，分钟级），任务与网络资源无限期
   挂起。目标：预解析包裹有界超时（默认 15s），超时静默丢弃（预解析本就
   是尽力而为）。

**不在范围内（明确记录，见 §6/§7）**：
- `load_and_play` 内联 `resolve_track`（单曲网络解析）与 `wait_current_applied`
  （≤`load_timeout`=5s/次）仍内联——完整装载状态机化列为后续项（§6 详述理由）。
- 协议/IPC 零变更（hmp-core 不动，全部复用既有字段与事件变体）。
- server.rs / transport.rs / serve.rs / content.rs / sync.rs 等其他 daemon 文件
  与 hmp-desktop / hmp-core / hmp-cli / hmp-qqmusic-api 不触碰。

**不可破坏语义（AUDIT §6 判定保留项 + §16/§17/§19）**：事务式换曲
（save_state/restore_state、装载成功才提交）、PreloadSlot 双键防乱序、
SessionMirror 脏检查节流、QueueCore 双数组、`wait_current_applied` 的
(gen, id) 双条件 ACK、§17 降档循环语义（**Timeout 不降档为有意边界**、
失败档不重复装载、降档链耗尽必回滚）、§19 优雅退出时序（Quit →
persist → sticky term_tx）。

---

## ② 现状与根因（证据）

### #1 命令循环内联 await 解析

引擎是单任务 actor：`run()`（engine.rs:346-505）以 `tokio::select!` 三臂
循环（cmd_rx / state_rx / events_rx），任何命令臂内 `.await` 未完成前，
其余命令全部滞留通道：

| 内联点 | 位置 | 内容 |
|---|---|---|
| `QueueAppend` | engine.rs:363-377 | `self.resolver.resolve_source_ids(&src).await` 内联 |
| `play_source` | engine.rs:724-737 | 同上（Play / PlayNext / OpenUri 共用，engine.rs:360/362/462） |
| `play_list` | engine.rs:806-827 | `resolve_id_stubs(&ids).await`；未命中 id 逐个走解析器（engine.rs:903-906） |
| `load_and_play` | engine.rs:1027, 1029-1032 | `resolve_track` / `resolve_track_excluding` 内联（单曲，本计划保留） |

列表解析的耗时源：`resolve_source_ids_impl`（player.rs:509-565）对歌单/
专辑分页拉取（100 首/页、上限 100 页，player.rs:633），每页一次 HTTP；
QQ 网络下单页 0.2-1s，大歌单（用户实库 1591 首 ≈ 16 页）即「秒级」。
期间 Pause/Volume/Seek/Next 排队 —— UI「点了没反应几秒」。
注意现有缓解只覆盖一半：`play_list` 的库内 id 走批量投影快路径
（engine.rs:862-896，§14.1），但 `Play(Playlist/Album)`、`QueueAppend`
与库外 id 仍全程内联。

### #2 中途读错误被 rodio 视为 EOS → 静默跳歌

链路逐跳：

1. **中途拉流失败以 `Err` 暴露**：`DecryptReader::read` 命中粘滞错误即
   返回 `Err`（hmp-media/stream/reader.rs:272-277；错误语义注释 reader.rs:24-26
   「中途失败 = 解码线程 Read Err」）。
2. **rodio/symphonia 把 IO 错误当源终止**：rodio 0.21.1
   `SymphoniaDecoder::next`（decoder/symphonia.rs:317-334）：
   `format.next_packet().ok()?`（读失败→None）、`Err(_) => return None`
   （非解码类错误→None）。源迭代器静默结束，错误被吞。
3. **hmp-player 把「源结束」判为曲尾**：drive 循环 ticker
   （hmp-player/core.rs:562-575）`Playing && sink.empty()` →
   `status=Ended` + `PlayerEvent::PlaybackEnded`。`sink.empty()` 只数
   队列长度（sink.rs:317-321），无法区分自然播完与错误终止。
4. **引擎据此跳歌**：engine.rs:483-491 同代 `PlaybackEnded` → `on_ended`
   （engine.rs:912-929）→ `advance_on_eos` + `load_and_play` 下一首。
   全程无 `last_error`、无 Error 态 —— 网络断流表现为「歌自己跳了」。
5. 引擎对同代 `PlayerEvent::Error` 仅 `publish()`（engine.rs:492-499），
   无收口（该事件现状唯一来源是装载期失败与 seek 失败，前者由
   `wait_current_applied` 消化，后者刻意不宣告故障，core.rs:454-469）。

### #3 `try_seek` 阻塞驱动命令循环最长 ~30s

- hmp-player drive 循环（core.rs:413-577）`PlayerCommand::Seek` 臂**内联同步**
  调 `sink.try_seek(position)`（core.rs:454-469）。
- rodio 0.21.1 `Sink::try_seek`（sink.rs:236-260）：写入 `SeekOrder` 后
  **`feedback.recv()` 同步阻塞**，直到音频线程在 `periodic_access`
  （5ms 周期，sink.rs:148-156 append 内闭包）里执行 `seek.attempt` →
  rodio `Decoder::try_seek` → symphonia `FormatReader::seek`。文档
  "blocks between 0 and ~5 milliseconds" 假设快源。
- 窗口外 seek 到未缓冲区间的探测读经 `SyncStream`（core.rs:26-43）进入
  `DecryptReader::read` 的 condvar 等待（reader.rs:287-299），由生产者
  网络（`cdn_client` connect 10s + read 30s，stream/source.rs:365，
  AUDIT §2 修复）钳制 → 最长 ~30-40s。
- 后果二连：① 音频回调线程停摆（underrun，rodio 内部，外部无法根治）；
  ② **drive 循环 `feedback.recv()` 同等阻塞** → Play/Pause/Volume/Stop/
  Load/新 Seek 全部在 cmd_rx/load_rx 排队（§16 原文「try_seek 阻塞命令
  循环最长 30s」即此；引擎命令循环本身不阻塞——engine 经通道投递，
  但命令到不了驱动，用户观感相同）。
- 前科：AUDIT §16 修复 1（DecryptReader 窗口内 seek 双向永久挂起）即经
  `try_seek` 阻塞反馈瘫痪驱动的事故路径。

### #4 预解析期无总超时

- `schedule_preload`（engine.rs:1171-1203）`tokio::spawn` 后直接
  `resolver.resolve_track(&next).await`，**无任何超时包裹**。
- `resolve_track` → `resolve_track_impl`（player.rs:292-488）：get_detail +
  get_song_urls + `prepare_media`（stream/source.rs:384-…）；CDN 不支持
  Range 时回退 `decrypt` **全量下载-解密-缓存**（分钟级，source.rs:378-379、
  player.rs:404-410 注释）。单请求虽被 cdn_client 超时钳制（§2），但链路
  总时长无界；挂起的预解析任务长期持有 reqwest 连接与注意力（ DecryptReader
  挂起前科见 §16 修复 1）。

---

## ③ 设计方案

### 方案 1（对应 #1）：解析移后台 FIFO worker，命令循环消息化

**并发模型（线程/任务边界）**：

```
server/tray/MPRIS ──Request──▶ engine cmd_rx（unbounded，不变）
                                     │
              run() select!（命令循环：只投递/提交/查询，新增第 4 臂）
                                     │
   ┌─────────────────────────────────┼──────────────────────────────────┐
   │ Play/PlayList/PlayNext/         │ EngineMsg::ResolveReply{token,…} │
   │ QueueAppend/OpenUri:            │ （新增 internal_rx 臂）           │
   │   phase=Resolving + publish     │   查 pending 登记 → 按 kind 提交  │
   │   登记 pending(token,kind)      │   （cache_stubs / 队列事务 /      │
   │   resolve_tx.try_send(job)      │    load_and_play / seq / preload)│
   │   立即回到 select（不 await）    │   token 过期（Stop/Quit 已清）→丢│
   └───────────────┬─────────────────┴──────────────────────────────────┘
                   ▼
     resolver worker（单 tokio 任务，FIFO 串行）
       job = SourceIds{src} | IdStubs{ids}（含库内快路径，原 resolve_id_stubs 逻辑）
       持有 Arc<dyn SourceResolver> + Option<Arc<Mutex<LibraryDb>>>
       每作业 tokio::time::timeout(LIST_RESOLVE_TIMEOUT=60s)
       回包 → internal_tx（unbounded）
       preload（schedule_preload）不走此 worker——独立 spawn + 自带超时，
       不被列表解析排队（gapless 语义优先）
```

**要点**：
- **FIFO 单 worker** 保持与旧内联串行**完全相同的提交顺序**（Play A →
  QueueAppend B：A 先提交 replace，B 再 append；乱序完成不会丢失 B）。
- 请求通道 `mpsc::channel(16)`（有界）：`try_send` 满则当场报
  `last_error("resolver queue full")` + 撤销登记（人在回路命令频率下 16
  绰绰有余；满了说明 worker 已被慢作业拖死，快速失败优于排队）。
  **（O4）** QueueAppend 满拒绝**不推进 seq**（对齐既有解析失败路径
  engine.rs:372-375 不推进的语义）；Play/PlayList 满拒绝推进 seq
  （命令已有确定性失败结果，CLI 需要「结果已可见」边界）+ 阶段恢复。
- **（M4）每作业 60s 总超时**（`LIST_RESOLVE_TIMEOUT`，默认 60s，经
  `start_with_options` 注入——循 `load_timeout` 先例 engine.rs:236-244）：
  纯 API 分页无可 Legit 超过 60s 的形态（100 页 × ~0.5s ≈ 50s 封顶）；
  超时 → **（M5）复用既有 `EngineError::Internal("source resolution
  timed out")`**（不改 daemon/player.rs，IPC 映射与行为一致），按既有
  解析失败路径发布。注意：**不**给 `load_and_play` 的单曲
  `resolve_track` 加超时——无 Range CDN 的全量下载回退（分钟级）是合法
  播放路径，硬超时会弄坏它（§6 风险表）。
- **（M2）token 登记/失效**：`pending_resolves: Vec<(token, ResolveKind)>`；
  三个入口清空登记：`Request::Quit`、`Request::QueueClear{all:true}`、
  `PlayerCommand::Stop`（含 server 转发的 `Command(Stop)` 直通；**不含**
  `QueueClear{all:false}`/QueueRemove/导航——FIFO 语义已保证其正确性）。
  清空时**按丢弃条数推进 seq 并 publish**：每个被弃命令获得「结果已
  可见」边界——否则 CLI `await_playing` 类轮询会等到超时，这是新架构
  引入的契约回归（旧内联架构下命令必然串行完成、seq 必然推进）。
  worker 侧作业仍会跑完但回包因登记已清而被丢，60s 内自灭。
- **（O2）worker 每作业 panic 隔离**：作业体经 `tokio::spawn` 子任务 +
  `JoinHandle` await 执行——单作业 panic 被限制在子任务内
  （`JoinError::is_panic()` → 报 `EngineError::Internal("resolver
  panicked")`），worker 存活继续消化后续作业（否则登记永久停在
  Resolving）。不引入新依赖（不用 futures-util catch_unwind）。
- **提交时捕获会话/位置**：`old_session`/`old_position` 从「命令开始时」
  改为「回包提交时」捕获——解析窗口内发生的换曲先闭合，再提交新装载，
  事务式语义（P1）不变。
- **seq 语义不变**：受理不推进、提交完成才推进（engine_tests.rs:1521-1574
  钉死的 CLI 边界），Resolving 中间发布保持旧 seq。
- **既有测试兼容**：FakeResolver 即时完成 → 多一跳（spawn → 回包 → 提交）
  ≈ 数个调度节拍，`wait_idle()`（engine_tests.rs:438-442，20 次 yield）足够。

**备选（均否决）**：
- *B1 全装载状态机化*（load_and_play 整体移后台，含 wait_current_applied
  消息化）：彻底消除 ≤5s/次的装载等待，但要重写 §16/§17 降档/回滚/双条件
  ACK 的全部时序——引擎测试的核心覆盖面，风险/收益不成比例。列为
  后续项。
- *B2 每命令独立 spawn + 最新者胜*（latest-wins token）：实现更少，但
  Play→QueueAppend 乱序会丢 append 语义，需额外的类内互斥，复杂度反而高。
- *B3 仅加命令级超时*：UI 仍冻结至超时，不达标。

**推荐理由**：FIFO worker 以最小结构变化（新增一条内部通道 + 提交分支
提取）换取命令循环「只投递与查询」，串行语义与全部事务式不变量原样保留。

### 方案 2（对应 #2）：播放器侧错误闩 + 引擎侧显式收口

**数据通路**：

```
DecryptReader::read Err（粘滞，reader.rs:272-277）
  → SyncStream::read 置闩 ErrorLatch=Arc<Mutex<Option<String>>>（core.rs:18-43 处改造）
  → rodio symphonia `Err(_) => None`（源静默终止，无法改，绕过判定）
  → drive ticker（core.rs:562-575）Playing && sink.empty():
       闩有错 → status=Error + PlayerEvent::Error{load_gen, Playback("source read error: …")}
                （不发 PlaybackEnded、不置 Ended、position 不前移）
       闩无错 → 现状 Ended + PlaybackEnded
  → engine run() 同代 Error 臂（engine.rs:492-499）三守卫全过才收口：
       (a) ev_gen == current_gen   （旧代事件过滤，现状）
       (b) 驱动 status == Error    （区分 seek 失败事件：其刻意保持播放态，core.rs:456-468）
       (c) error_handled_gen != Some(current_gen)（M1：按代际的已处理标记——
                                    装载期失败在 wait_current_applied 消化
                                    LoadFailed 时即写入该代；run() 收口处同样
                                    写入。全局 last_error 去重会被无关错误
                                    （QueueAppend/play_source 解析失败置
                                    last_error 且旧曲在播）误锁死，故弃用）
       → last_error 置位 + end_session("error")（listened_ms=当前位置）
         + phase=Failed + publish；不 on_ended、不 advance —— 不静默跳歌
```

**要点**：
- `ErrorLatch` 在 `append_location`（core.rs:317-363）创建：Stream 路径挂进
  `SyncStream`；**File 路径同样包裹**（`Decoder::try_from(file)` 的中途读错
  同走静默终止，本地盘错误罕见但同病）。闩随 `LoadCompletion` 回 drive，
  存 `current_latch`；新 Load 分派与 Stop 时清除。
- `error_handled_gen: Option<u64>` 字段（M1）在两处写入：①
  `wait_current_applied` 消费到同代 `PlayerEvent::Error` 返回
  `EngineError::LoadFailed` 时（engine.rs:1270-1274 处）；② run() Error 臂
  收口时。守卫 (c) 比较的是「该代是否已处理过错误」，与 last_error 的
  全局状态无关。
- 双守卫分析（为何不会误伤装载期错误事件）：装载期 Error 由
  `wait_current_applied`（engine.rs:1237-1287）在命令臂内联等待时消化——
  降档重试 bump `current_gen`、终败回滚复原 `prev.load_gen`，run 循环随后
  处理的广播缓冲副本必因 gen 不符被过滤；`(b)(c)` 再兜一层
  （如 `load_decode_failure_without_resolvable_alternative_terminates`
  无旧曲场景 current_gen 恰等：守卫 (c) 的 error_handled_gen==Some(该代)
  挡住重复收口）。
- **产品语义**：错误后**停在原地**（当前曲保持、队列不动、进度冻结），
  用户可见错误条（桌面 §14.2 反馈条读 `last_error`）后自行 Next/重试。
  备选「错误提示 + 自动跳下一首」被否决：网络断流时自动连跳多首会掩盖
  断网事实（静默跳歌的 UX 问题的根源）；如需自动续播可后续加配置。
- **（O1）Error 后的恢复路径**：drive 循环 `Play`/`TogglePlay` 命令臂在
  离开 Error 态时**顺带清闩**——用户点播放获得干净恢复：闩清除后已排空
  的源经 ticker 判 `Ended` + `PlaybackEnded`，引擎按正常 EOS 续播下一首
  （显式用户动作驱动的跳曲，非静默）；`Request::Play`/`PlayList` 走全新
  装载自然重建。不做引擎侧特判。
- 预解析槽与新装载自然清除闩（闩生命周期=单次装载）。

### 方案 3（对应 #3）：seek 移 `spawn_blocking` + 双代际守卫 + ACK 超时

```
engine.driver.seek() ─▶ PlayerCore cmd_rx（不变，非阻塞投递）
drive 循环（tokio 任务，core.rs:413）：
  Seek(pos) 臂：
    seek_gen += 1；记 (seek_gen, state.load_gen)
    spawn_blocking(move || sink_arc.try_seek(pos))  ← sink: Arc<Sink> 克隆
    （可选附加：seek 在途期间 hold pause——见下）
  新增 seek_done 臂（mpsc<SeekOutcome>）：
    outcome 且 seek_gen 未过期 且 load_gen 未变（新装载未发生）：
      Ok  → state.position = pos
      Err → 现状语义（warn + PlayerEvent::Error，不冻结播放，core.rs:456-468）
    过期 → 丢弃（被更新的 seek/新装载取代）
rodio 音频线程：periodic_access(5ms) 执行 attempt（同步，网络源最长 CDN 超时）
  —— 残余：音频停摆有界（≤ connect 10s + read 30s），不再波及命令循环；
     阻塞线程占用 tokio blocking pool（默认上限 512，单占用无碍）
```

**要点**：
- rodio `Sink::try_seek` 是 `&self` 且线程安全（内部 Mutex + mpsc 反馈），
  两个在途 seek 并发安全：后发的 `SeekOrder` 覆盖前者，前者 feedback 通道
  关闭按 rodio 语义返回 `Ok(())`——由 `seek_gen` 守卫丢弃陈旧结果。
- **load_gen 守卫**：seek 在途时发生新装载（completion 置 position=0，
  core.rs:533），迟到的旧 seek 结果不得覆盖新曲位置。
- **SEEK_ACK_TIMEOUT = 10s**（`tokio::time::timeout` 包 outcome 等待）：
  超时仅 warn（seek 可能迟到生效——迟到结果仍按守卫应用），命令循环与
  UI 不等待。远端窗口外 seek 合法耗时可达数秒，10s 内报态、之外不误杀。
- **hold pause（含于 T4）**：dispatch seek 时若 sink 正在播 → 立即
  `sink.pause()`（原子操作，即时），outcome/超时后恢复。对齐现状听感
  （内联 seek 期间音频线程本就停摆=无声）；同时消除 rollback 路径
  「重载后从 0 位出声一瞬」的窗口（见 §6 风险）。用户显式
  Play/Pause/TogglePlay/Stop 命令到达即解除 hold（避免与用户意图打架）。
- **（M3）Stop/Shutdown 失效在途 seek**：`PlayerCommand::Stop` 臂与
  `LoadCommand::Shutdown` 分支 bump `seek_gen`（迟到 outcome 按 gen 守卫
  丢弃，不污染 Stopped 态 position）；Stop 同时清 hold-pause 标志与
  ACK 超时定时器。残余：Stop 后仍在途的 seek 的 `spawn_blocking` 线程
  停泊在 `feedback.recv()`，直至 sink drop/音频线程完成该次 attempt——
  有界（CDN 超时）且不占命令循环（记入 §6 残余清单）。
- 引擎侧零改动（`driver.seek` 本就是通道投递）。

**备选（否决）**：*给 DecryptReader 加「seek 探测读非阻塞」*——reader 无法
区分读来自 seek 探测还是常规解码，改语义伤及装载探测路径；*hmp-media 层
seek 预热*（seek 前预取目标区间）——治本但属 hmp-media 深改，收益/
风险比不如播放器侧隔离，列为后续优化。

### 方案 4（对应 #4）：预解析总超时

```rust
// schedule_preload（engine.rs:1187-1202）内：
tokio::spawn(async move {
    let r = tokio::time::timeout(PRELOAD_RESOLVE_TIMEOUT /*默认 15s，可注入*/,
                                 resolver.resolve_track(&next)).await;
    match r {
        Ok(Ok(res)) => { /* 原写槽逻辑（key 防乱序）不变 */ }
        Ok(Err(e))  => tracing::debug!(%e, "预解析失败（不影响播放）"),   // 现状
        Err(_)      => tracing::debug!("预解析超时（gapless 加速放弃，播放时按需解析）"),
    }
});
```

**要点**：
- 超时即放弃（drop 取消 reqwest 在飞请求；无 Range 全量下载回退被中止时
  只留 `.part` 临时文件，由既有「成功写缓存后清扫陈旧 tmp」（AUDIT §2）
  兜底，无损坏——`.part`+rename 原子性）。
- 15s 依据：流式路径典型 1-2s；预解析价值在曲间切换前完成，>15s 的预解析
  对 gapless 无意义，按需解析路径语义完全一致地兜底。
- `PRELOAD_RESOLVE_TIMEOUT` 做成 `PlaybackEngine` 字段，经
  `start_with_options` 注入（签名加参；调用点仅 engine_tests.rs **9 处**
  （M6：1688/1741/2468/2511/2827/2976/3000/3042/3084，T1 机械加参）+
  `start_with_library` 传默认——daemon.rs:53 用 `start_with_library`，
  签名不变）。

---

## ④ 任务分解

> 每任务独立提交、自带红→绿回归；顺序 T1→T5（小先行、大压轴）。
> 验证纪律：`cargo check -j 4 -p <crate>` 按需；测试逐 crate `cargo test -j 1 -p <crate>`；
> 不跑全仓并发测试、不动 CARGO_TARGET_DIR、不 push。

### T0 基线确认
- 动作：`cargo test -j 1 -p hmp-daemon` 与 `cargo test -j 1 -p hmp-player`
  全绿记录（引擎 67 个测试为后续比对基准，M6 更正——AUDIT §6 的「97」
  为历史计数）。
- 触及文件：无。

### T1（#4）预解析总超时
- 触及：`crates/hmp-daemon/src/engine.rs`、`crates/hmp-daemon/src/engine_tests.rs`。
- 内容：方案 4；`start_with_options` 加 `preload_timeout` 参数
  （`start_with_library`/`start` 传 `Duration::from_secs(15)`）；**（M6）**
  engine_tests.rs 内 9 处 `start_with_options` 调用机械加参
  （1688/1741/2468/2511/2827/2976/3000/3042/3084）。
- 验收：
  - 红→绿：`preload_resolve_timeout_gives_up`（短超时注入 + `resolve_track`
    延迟包装解析器 → 槽永不写入：后续 `Next` 重新 resolve（attempt 计数
    递增）、引擎不 hang、无 `last_error`）。
  - 既有 `preloads_next_track_and_consumes_cache` 等 5 个 preload 测试绿
    （默认 15s ≫ 测试内 150ms 延迟）。

### T2（#2 播放器侧）错误闩与显式 Error 态
- 触及：`crates/hmp-player/src/core.rs`（+ 其内嵌 tests——**不新建文档
  文件**，测试代码允许）。
- 内容：方案 2 播放器半边——`ErrorLatch`、`SyncStream` 置闩、File 包裹、
  `LoadCompletion`/`current_latch` 携带与清理、ticker EOS 分支查闩、
  **（O1）Play/TogglePlay 离开 Error 态时清闩**。
- 验收：
  - 红→绿（hmp-player 测试，`new_silent_for_test` 静默 sink）：
    - `mid_read_error_yields_error_event_not_ended`：假 `MediaStreamSource`
      （合法 WAV 头 + PCM 后 `Err`）→ 收到 `PlayerEvent::Error`、
      `status==Error`、**无** `PlaybackEnded`；
    - `clean_exhaust_still_ends`（正常小 WAV → Ended，现状不回归）；
    - `play_after_error_clears_latch_and_advances`（O1：Error 后发 Play →
      闩清 → EOS → PlaybackEnded 续播，无 Playing→Error 闪烁循环）。
  - `cargo test -j 1 -p hmp-player` 全绿；既有 3 个输出策略测试不受影响。

### T3（#2 引擎侧）同代 Error 收口
- 触及：`crates/hmp-daemon/src/engine.rs`、`engine_tests.rs`。
- 内容：方案 2 引擎半边——run() Error 臂三守卫（**M1：守卫 (c) 用
  `error_handled_gen`，在 wait_current_applied 的 LoadFailed 消费处与
  run() 收口处双写**）+ `end_session("error")` + `phase=Failed`；
  FakeDriver 增 `emit_playback_error(gen)` 辅助（`set_status(Error)` +
  emit Error{gen}，对齐 T2 真实驱动形态）。
- **与 T2 行为成对交付**（提交信息注明；代码可独立回滚，行为耦合：
  引擎收口依赖驱动侧 Error 状态形态）。
- 验收（红→绿，沿用 FakeDriver/FakeResolver）：
  - `midplay_error_sets_last_error_closes_session_holds`：播 a（库在场）→
    同代错误 → `last_error` 有值、`phase==Failed`、play_events 以
    reason="error" 闭合、**队列不前移**（仍 a，无新装载）；
  - `seek_failure_error_not_treated`（emit Error 不动 status → 不收口，
    播放继续，对齐 core.rs seek 失败事件语义）；
  - **（M1）**`unrelated_resolve_error_does_not_lock_error_treatment`：
    先经 QueueAppend 解析失败置 `last_error`（旧曲在播）→ 再发同代播放
    错误 → 收口仍生效（证明守卫 (c) 不被无关 last_error 锁死）；
  - 既有 `stale_gen_error_is_ignored`、两个降档测试
    （`load_decode_failure_*`）、`rollback_after_async_load_failure_*` 全绿。

### T4（#3）seek 去阻塞
- 触及：`crates/hmp-player/src/core.rs`（+ 测试）。
- 内容：方案 3——Seek 臂 spawn_blocking、`seek_done` 臂、(seek_gen,
  load_gen) 双守卫、`SEEK_ACK_TIMEOUT=10s`、hold-pause 状态位
  （Play/Pause/TogglePlay/Stop 到达即解除）、**（M3）Stop/Shutdown 臂
  bump seek_gen 失效在途 seek**。
- 验收（红→绿，hmp-player 测试）：
  - `seek_in_flight_does_not_block_commands`：假源 `seek()` 阻塞（condvar
    门等测试放行）→ 发 Seek 后立即 Pause → 驱动状态即时翻转（红：现状
    内联 try_seek 冻结循环；绿：命令即时处理）；
  - `new_load_invalidates_pending_seek_outcome`：seek 卡住 → 新 Load ACK →
    放行 seek → position 不被旧 seek 覆盖；
  - `second_seek_supersedes_first`：卡住的 seek1 + 后发 seek2 → 终态位置
    = seek2；
  - **（M3）**`stop_invalidates_pending_seek_outcome`：seek 卡住 → Stop →
    放行 seek → position 保持 0/Stopped，不被迟到 seek 污染。
- 既有测试全绿（hmp-player 现无 seek 时序测试，无冲突面）。

### T5（#1）解析去阻塞（引擎消息化）
- 触及：`crates/hmp-daemon/src/engine.rs`、`engine_tests.rs`
  （**M5：不改 daemon/player.rs**——超时/panic/队列满统一复用
  `EngineError::Internal`）。
- 内容：方案 1——`ResolveJob`/`EngineMsg`、resolver worker 任务（**O2**
  每作业 tokio::spawn 子任务 + JoinHandle panic 隔离）、`pending_resolves`
  登记、`play_source`/`play_list`/`QueueAppend` 拆 dispatch/apply 两半
  （apply 提取为 `apply_play_stubs`/`apply_play_list`/
  `apply_queue_append`）、`resolve_id_stubs` 改自由函数（Arc 参数供 worker
  调用）、**（M2）Quit/QueueClear-all/Command(Stop) 三入口清登记并按
  条数推进 seq**、请求通道 16 容量 try_send（**O4** QueueAppend 满拒绝
  不推 seq）、**（M4）`list_resolve_timeout` 经 start_with_options 注入**
  （engine_tests.rs 9 处调用再次机械加参）。
- 验收（红→绿，engine_tests.rs）：
  - `slow_list_resolve_does_not_block_playback_commands`（核心回归）：
    `DelayResolver` 300ms + `Play(歌单)` → 50ms 内发 Pause/SetVolume →
    断言驱动已收到命令（红：现状排队至解析完；绿：即时）；
  - `queue_append_resolve_is_offloop`（QueueAppend 同型）；
  - `resolve_replies_apply_in_dispatch_order`（**O3 措辞**）：慢先快后
    两个 Play——单串行 FIFO worker 下完成序≡派发序，断言**装载顺序
    [a, c] 且队列终态=后发者**（若有人改成并行 worker，乱序应用会红）；
  - `stop_drops_pending_resolve`：慢 Play + 立即 Stop → seq 越过受理值
    （**M2 变体断言**）且回包到达后不装载、不换队列、`phase==Idle`；
  - `eos_during_pending_resolve_then_list_applies`：播 X + 慢 Play(列表) →
    X 的 EOS 先续播 → 回包到达后整表替换装载起播曲（新交互语义钉死，
    见 §6 行为变化）；
  - **（M4）**`list_resolve_timeout`：注入 100ms 超时 + 永挂解析器 →
    `last_error=="source resolution timed out"`、seq 推进、队列不变，
    且 worker 存活（随后正常 Play 成功）；
  - **（M4）**`resolver_queue_full_fails_fast`：仅首作业延迟的解析器 +
    连发 17 个 QueueAppend → 第 17 个即时报 "resolver queue full"
    （**O4**：seq 不推进）且 worker 恢复后前 16 个正常 append；
  - **（O2）**`resolver_job_panic_does_not_kill_worker`：首作业 panic 的
    解析器 → 报 Internal 错误且登记清空，第二个作业正常完成；
  - 既有 67 个测试全绿（重点盯 `seq_does_not_advance_while_source_resolving`
    、`play_list_*`、`playnext_*`、`empty_source_sets_error_and_advances_seq`）。

### T6 收尾
- `cargo clippy -j 4 -p hmp-daemon -p hmp-player -p hmp-media
  --all-targets -- -D warnings`；全量 `cargo test -j 1 -p hmp-daemon`、
  `-p hmp-player`、`-p hmp-media`（回归确认 media 未被 T2 波及）。
- fmt 纪律：只对本任务触改的文件跑 rustfmt --check（全仓 fmt 基线红由
  S5 负责，不越界顺手全仓 fmt）。
- 真机冒烟清单（实施完成后人工）：重建 release `hmp.exe` **并** 桌面端
  （§16 零号/§18/§22 教训：改 daemon 源必须重建全部链接二进制）→
  `hmp quit` 后重拉 → 大歌单 Play 期间点暂停立即生效；远端曲目播放中
  断网 → 错误条 + 停在原曲；拖动进度到未缓冲远端区间期间音量/暂停可操作。

---

## ⑤ 测试计划（新增回归清单汇总）

沿用 FakeDriver / FakeResolver / DelayResolver（engine_tests.rs:45-408、
1492-1517）脚手架模式：

| # | 测试 | 位置 | 对应项 |
|---|---|---|---|
| 1 | `preload_resolve_timeout_gives_up` | engine_tests.rs | #4 |
| 2 | `mid_read_error_yields_error_event_not_ended` | hmp-player | #2 |
| 3 | `clean_exhaust_still_ends` | hmp-player | #2（防误伤） |
| 4 | `play_after_error_clears_latch_and_advances` | hmp-player | O1 |
| 5 | `midplay_error_sets_last_error_closes_session_holds` | engine_tests.rs | #2 |
| 6 | `seek_failure_error_not_treated` | engine_tests.rs | #2/§既有语义 |
| 7 | `unrelated_resolve_error_does_not_lock_error_treatment` | engine_tests.rs | M1 |
| 8 | `seek_in_flight_does_not_block_commands` | hmp-player | #3 |
| 9 | `new_load_invalidates_pending_seek_outcome` | hmp-player | #3 |
| 10 | `second_seek_supersedes_first` | hmp-player | #3 |
| 11 | `stop_invalidates_pending_seek_outcome` | hmp-player | M3 |
| 12 | `slow_list_resolve_does_not_block_playback_commands` | engine_tests.rs | #1 |
| 13 | `queue_append_resolve_is_offloop` | engine_tests.rs | #1 |
| 14 | `resolve_replies_apply_in_dispatch_order`（O3 措辞） | engine_tests.rs | #1 |
| 15 | `stop_drops_pending_resolve`（M2 seq 断言变体） | engine_tests.rs | #1/M2 |
| 16 | `eos_during_pending_resolve_then_list_applies` | engine_tests.rs | #1 |
| 17 | `list_resolve_timeout` | engine_tests.rs | M4 |
| 18 | `resolver_queue_full_fails_fast` | engine_tests.rs | M4/O4 |
| 19 | `resolver_job_panic_does_not_kill_worker` | engine_tests.rs | O2 |

新增脚手架：`DelayTrackResolver`（包装 resolve_track 加延迟，T1）、
`BlockingSeekGate`（condvar 门控 seek 阻塞的 MediaStreamSource，T4）、
`ErroringStream`（合法 WAV 前缀 + 中途 Err 的 MediaStreamSource，T2）、
FakeDriver::`emit_playback_error`（T3）、首作业延迟/永挂/panic 解析器
（T5）。全部私有于测试模块，不进生产面。

---

## ⑥ 风险与回滚

**67 个既有引擎测试影响评估（M6 更正计数）**：
- 最大面 = T5：所有 play 路径多一次异步跳（spawn→回包→提交）。
  `wait_idle()`（engine_tests.rs:438-442，20 次 yield）与 `wait_until`
  （轮询）模式对数个调度节拍的额外延迟不敏感；`#[tokio::test]` 单线程
  runtime 下 spawn 任务靠 yield 推进，20 yield 充裕。风险点：若有测试在
  `cmd(Play)` 后**零等待**断言（排查结论：现有用例均有
  wait_idle/wait_until/sleep 之一）。
- **既有测试的结构性改动面（M6）**：仅 T1/T5 对 engine_tests.rs 的 9 处
  `start_with_options` 调用机械加参（preload_timeout / list_resolve_timeout），
  其余既有测试**零编辑**。
- T5 不触碰 `wait_current_applied` / 降档 / 回滚 / 双条件 ACK 逻辑
  （§16 #2、§17 语义原样）。
- T3 触碰 run() Error 臂：降档/回滚测试的事件双投递路径已按 gen 守卫 +
  error_handled_gen（M1）分析覆盖（见方案 2）；M5 裁定不改 player.rs，
  `clone_error` 穷举匹配无需动。
- T1/T2/T4 与引擎测试正交（T2/T4 在 hmp-player，其现有测试仅 3 个输出
  策略单测）。

**行为变化（有意，需在 PR 描述明示）**：
1. 解析在途不再冻结命令：解析期间 EOS 会先续播旧队列，解析完成后再
   整表替换（旧实现循环冻结、不可能出现此交错；新行为对用户更顺）。
2. 中途读错误从「静默跳歌」变「停在原曲 + 错误条」；Error 后点
   Play/TogglePlay → 清闩并经 EOS 续播下一首（O1）。
3. seek 生效前旧位置继续出声/或 hold-pause 静音（对齐现状听感）；
   rollback 路径重载后可能有 ≤ 命令延迟的 0 位瞬播（hold-pause 后收敛）。

**残余已知边界（记录不修）**：
- `load_and_play` 内联 `resolve_track`（含无 Range 全量下载回退，可分钟级）
  与 `wait_current_applied`（≤5s/次、降档 ×≤8）仍阻塞引擎循环——完整
  装载状态机为后续项（方案 1 备选 B1）。单曲解析不加超时的原因即在此
  （防弄坏全量下载回退）。
- **（O5）** try_seek 期间 rodio 音频线程停摆：**每个底层请求有界**
  （connect 10s / read 30s）但可链式叠加——病态 seek（symphonia 探测
  逐 sync point 逼近目标）可链 2-3 个串行区间请求，停摆时长为「链长 ×
  单请求上界」；外部仅能隔离其对命令循环的传导，无法根治停摆本身。
- **（M3）** Stop 后仍在途的 seek 的 `spawn_blocking` 线程停泊在
  `feedback.recv()`，直至 sink drop/音频线程完成该次 attempt（有界，
  不占命令循环，仅占 blocking pool 一线程）。
- worker FIFO 中，前一个 60s 慢作业会让后续 Play 排队至多 60s（与旧内联
  串行一致；Stop 可弃置）。

**回滚**：T1-T5 每任务一提交、互相独立（T3 依赖 T2 的驱动事件形态但不
依赖其代码——**行为成对交付、提交信息注明**），可按提交粒度 revert；
无数据迁移、无协议变更，回滚零残留。

---

## ⑦ 与并行流的边界确认

- **server.rs 等禁碰文件零改动**：本计划全部改动落在
  `engine.rs`/`engine_tests.rs`/`hmp-player/src/core.rs`（+hmp-player 测试）
  （**M5：不改 daemon/player.rs**）。
  `PlaybackEngine::start`/`start_with_library` 签名不变（daemon.rs:53、
  server.rs:951/1117/1148/1183/1270/1393 调用点不受影响）；仅
  `start_with_options` 加参，其调用点全在 engine_tests.rs（自有，9 处）。
  **无需 S4 裁决**。
- **hmp-core 零改动**：#2 复用既有 `PlaybackStatus::Error`、
  `PlayerEvent::Error`、`EnginePhase::Failed`、`DaemonState.last_error`
  （桌面反馈条/CLI 均已有消费路径，§14.2）；wire 兼容不变，无
  unknown-variant 风险（§14/§16 教训规避）。
- **hmp-media 仅可能被 T2 的测试波及（不改其源码）**；DecryptReader/
  生产者语义不动（§16 修复 1 的窗口内 seek 行为保持）。
- **不动 §6 判定保留项、§17 降档语义（Timeout 不降档）、§19 退出时序**
  （Quit → end_session → driver.shutdown → persist → term_tx 顺序不变；
  worker 随 engine 结构 drop → 通道关闭自然退出，无僵尸任务）。
- 资源纪律：构建 `-j 4 -p` 按需、测试逐 crate `-j 1`、不动主工作区、
  不碰其他 worktree、不 push。

---

## 附：需协调者裁决的问题

1. **#2 产品语义**：中途读错误后「停在原曲 + 错误条、不自动跳」为推荐
   方案；备选「错误条 + 自动跳下一首」是否需要（或做成配置）？
2. **#1 残余阻塞接受度**：装载等待（≤5s/次）与单曲解析（含无 Range
   全量下载回退）仍内联于引擎循环；完整装载状态机化（B1）列为后续项
   ——是否接受本范围划界？
3. **常量取值确认**：`LIST_RESOLVE_TIMEOUT=60s`、`PRELOAD_RESOLVE_TIMEOUT=15s`、
   `SEEK_ACK_TIMEOUT=10s`（均可调，仅确认默认值）。
4. **行为变化知情**：解析在途 EOS 先续播后整表替换（⑤#16 钉死的新交互
   语义）、seek 生效前 hold-pause、Error 后点 Play 续播下一首（O1）——
   如桌面端对这些交互有既定预期请提出。
