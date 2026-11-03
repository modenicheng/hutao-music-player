# S2 计划：播放页喜欢键真接线

工作树：`D:\coding\hutao-music-player\.worktrees\s2-like-key`（分支 `feat/like-key`，基线 `cd4a75e`）。
本计划自包含：所有证据均带 文件:行号（相对 worktree 根），实施者无需重新考古。

> **修订记录（已按评审修订）**：
> - **M1**：§3.2 多连点论证改述——UI 收敛到**库事实（最后到达 daemon 者胜出）**；
>   backend.rs:278-299 冷启动重试（4×250ms）可致到达序 ≠ 点击序，极端倒挂窗口下
>   库终值可能与最后一次点击意图相反——已知边界，如实记录（计划+代码注释），不做在途串行化。
> - **O1**：回包处理抽纯函数 `favorite_outcome`（§3.2/T3），决策表单测（⑤-U3）。
> - **O2**：§⑥ 风险表补「无关库写触发回填与在途点击写交错 → 心形瞬时闪动（终态正确）」。
> - **O3**：三处勘误——测试组行段改为「cfg(test) 起于 1271 行约 183 行」；drag.rs 改
>   「多跑无害的邻域回归」；§②两处引用复核（player_bridge 的 NowPlaying 导入实测在
>   :19（评审建议的 :21 实为常量注释行），引用改为不依赖行号的描述；sync.rs 行段精确为
>   :389-408 覆盖 mark_relation_ok/fail_relation 两助手）。
> - **PORTING.md:153 记录更新已获协调者授权归本流**（§④-T5 转无条件，随功能同一提交）。

---

## ① 目标与范围

把播放页（now-playing overlay）标题旁的心形喜欢键从纯本地假交互接成真功能：

1. **换曲回填**：切到新曲时，从本地媒体库读出该曲真实收藏状态并点亮/熄灭心形。
2. **点击写链**：点击心形 → 发 daemon 既有 `Request::Favorite` 写命令 → 本地库立即生效。
3. **失败诚实回滚 + 用户可见反馈**：daemon 拒绝/不可达时，心形回到库中事实状态，反馈条给人话。
4. **「我喜欢」页同步**：写成功后我喜欢页即时刷新（复用既有 LibraryChanged 链，零 bridge.rs 改动）。
5. **远端收藏同步（原可选加分项）**：核实结论为 **已全链存在、零改动**（见 ②§4），本计划只需在文档层面记录。

**范围外（明确不做）**：
- 曲目表（TrackTable）各行不加心形列——`TrackRow`（ui/data.slint）无 liked 字段、当前所有表格不显示心形，不存在「表格心形同步」问题；加列属另一里程碑且涉 bridge.rs（并行流禁碰）。
- 评论区评论点赞（now-playing.slint:797 `comment-like`）是另一域（评论赞，非曲目收藏），保持本地态不动。
- AUDIT §4.2 `list_favorites` INNER JOIN 观感问题：不修（已记录的既有决策项）。
- engine.rs、bridge.rs、backend.rs、daemon 任何文件：零改动（见 ⑦）。

---

## ② 现状与根因（证据）

### 1. 桌面端假交互三处

| 证据 | 内容 |
|---|---|
| `crates/hmp-desktop/ui/now-playing.slint:474-494` | `like-touch := TouchArea` 的 `clicked =>`（481 行）执行 `NowPlaying.liked = !NowPlaying.liked;` —— 纯本地翻转，无 IPC |
| `crates/hmp-desktop/ui/stores.slint:244-246` | `NowPlaying` global 只有 `in-out property <bool> liked: false;`，注释自述「喜欢（换曲回落未点亮，与原型一致；播放页直接写 liked）」，**无回调** |
| `crates/hmp-desktop/src/player_bridge.rs:694-698` | 换曲检测 `mid_changed` 分支：`np.set_liked(false);` —— 恒置 false，不回填真实状态 |
| `crates/hmp-desktop/src/player_bridge.rs:1110` | `clear_now_playing`：`np.set_liked(false);`（无曲清零，本行为正确，保留） |
| `crates/hmp-desktop/PORTING.md:153` | 「播放页喜欢键仍为 mock 态（换曲回落未点亮，与原型一致）」——文档记录在案 |
| `docs/UI-AUDIT-2026-09-28.md` N17 | 2026-09-28 死回调清理时 `NowPlaying.toggle-liked` 回调被**删除**、改为本地态；本轮接真，回调名恢复 `toggle-liked`（历史名，语义未变） |

**根因**：M6 移植期播放页按原型做本地态（UI-AUDIT N17 的处置），M8 数据接线轮未覆盖此键；daemon 侧收藏 IPC 早已存在（下节），桌面端只差「读回填 + 写派发」两段接线。

### 2. daemon 收藏 IPC 语义核实结论（已逐行核实）

- **协议**：`Request::Favorite { source, key, title, desired }`，定义于 `crates/hmp-core/src/ipc.rs:150-159`（source: `"qq" | "local"`；key: QQ mid 或 `local:<路径>`；title 库外新行建行用；desired 收藏/取消）。
- **处理位置：`crates/hmp-daemon/src/server.rs:224-277`，不在 engine.rs**（engine.rs 的命令分发 354-438 无 Favorite）。语义：
  1. `upsert_track`（保底建曲目行，仅 title）；
  2. `set_relation("track", source, key, "liked", desired)`（relations 表 = durable outbox，最后一次意图胜出，`crates/hmp-storage/src/db.rs:1302-1340`）；
  3. 本地源额外 `mark_relation_synced`（本地即事实，不进 outbox）；
  4. 成功 → `library_tx.send_modify`（+1）→ 广播 `Event::LibraryChanged`（server.rs:259），QQ 源再 `sync_handle.trigger()`；
  5. 应答 `Response::Ok` / `Response::Err { Internal }`（库不可用）。
- **免凭证**：`requires_credential`（server.rs:72-88）不含 Favorite —— 未登录/离线意图合法，本地先提交（spec §3.3）。
- **daemon 侧已有测试**：`server.rs:1292-1315` 证明 Favorite 写 → Ok + 订阅端收到 `LibraryChanged`。
- **CLI 同命令先例**：`crates/hmp-cli/src/favorite.rs:21-54`（add/remove 即发同一 Request；`provider_of`（`crates/hmp-cli/src/library.rs:26-32`）：`TrackProvider::from_id(id)==Local → ("local", id全串含 local: 前缀)`，否则 `("qq", id)`；key 与 id 同串）。本地键归一化由 storage 内部 `canonical_local_key`（db.rs:2058-2069）统一，两侧无需预处理。
- **读路径**：`relation_desired("track", source, key, "liked")`（db.rs:1343-1361）按 (source, key) 直查本地事实（无行 → None → 视为 false），与 `is_favorite`（db.rs:1259-1275）同口径但**不需要数字 track_id** —— 桌面端回填用它。
- **`DaemonState`（hmp-core/ipc.rs:336-362）无任何收藏字段** —— 推送不携带 liked，回填只能桌面端直读库（与库页/队列投影同契约：backend.rs:3「媒体库读按 CLI 契约直读 library.sqlite3」）。

### 3. 桌面端可复用的全部管线（均已存在）

| 管线 | 证据 | 复用方式 |
|---|---|---|
| 单发请求 | `backend.rs:278` `pub async fn request(req: Request)`（短连接 + 冷启动重试 + 拒绝码 warn 日志） | 直接调用 |
| 受理回调变体 | `player_bridge.rs:125-145` `dispatch_command_on_ok`（Ok 后 UI 线程钩子；menu_bridge.rs:111-123 歌单写已用同款 + schedule_refresh 钩子） | 收藏写同模式 |
| 库页自动刷新 | server.rs:259 → `Event::LibraryChanged` → backend.rs:424-436 → `UiStateEvent.library_changed` → player_bridge.rs:538-541 `apply_event` → `bridge::schedule_refresh`（bridge.rs:493-526，防抖+后台读库）→ `apply_snapshot`（bridge.rs:536-542 重装 `Data.liked` 我喜欢页模型） | **写成功后我喜欢页自动刷新，bridge.rs 零改动** |
| 异步取数+防串台守卫 | `spawn_comment_fetch`（player_bridge.rs:887-940）：runtime.spawn → 回包 `invoke_from_event_loop` → UI 线程 `Player.current_mid == mid` 守卫 | 回填链同构 |
| 阻塞 rusqlite 在 runtime 线程 | backend.rs:508-511 / bridge.rs:511-514 先例（毫秒级查询直接执行） | 回填读同惯例 |
| 反馈条 | `show_feedback`（player_bridge.rs:178-196，4s 消退）+ `friendly_error`（167-174） | 失败反馈复用 |
| 全局导出 | `NowPlaying` 已在 app.slint:38 export，Rust 侧可用（player_bridge.rs 顶部 `use crate::{...NowPlaying...}` 导入，首个绑定 `on_request_comments`） | 无需改 app.slint |

### 4. 远端收藏同步（加分项）核实结论：**已存在，零接入工作**

daemon SyncWorker 已消费 relations outbox 的 `("track","liked")` 行：`crates/hmp-daemon/src/sync.rs:184-202` —— `resolve_song_id`（mid → 数字 id）后调 `SonglistApi::like_song / unlike_song`（`crates/hmp-qqmusic-api/src/songlist.rs:269-287`，固定 dirid=201「我喜欢」歌单）；失败 `fail_relation` → `mark_relation_error` 指数退避重试、成功 `mark_relation_ok` → `mark_relation_synced`（sync.rs:389-408，退避只动 sync_state/retry，不改 desired_state 本地事实）。即：桌面端只管发 `Request::Favorite`，**本地立即生效 + QQ 异步同步全自动**。已知上游怪癖（songlist.rs:266-268 注释）：部分歌曲远端 like 返回 CGI 80105 —— 只影响远端同步重试，本地事实与 UI 不受影响，无需桌面侧处理。

---

## ③ 设计方案

### 总原则

沿项目纪律：「状态属性由 Rust 权威写入；回调发命令给 Rust」（stores.slint:2）。Slint 侧只报意图（回调），liked 值的每次变更都出自 Rust；库（library.sqlite3）是收藏事实源，UI 心形 = 本地事实 + 单次在途乐观翻转。

### 3.1 状态回填链（换曲时）

```
daemon 推送 StateChanged（10Hz）
  → apply_daemon_state 检测 mid_changed（player_bridge.rs:686-693）
  → UI 线程立即 np.set_liked(false)          // 中性过渡：不残留上一曲点亮态
  → spawn_liked_backfill(ui_weak, runtime, mid)   // 新增，与 spawn_lyric_fetch 并列
      runtime.spawn:
        liked = liked_of(mid)                 // 阻塞读库（新helper，见下）
        invoke_from_event_loop:
          守卫：Player.current_mid == mid（换曲/清空后丢弃陈包）
          np.set_liked(liked)
```

- `liked_of(mid) -> Option<bool>`（player_bridge.rs 新增，两层）：
  - 外层 `liked_of`：`hmp_storage::data_dir().join("library.sqlite3")` 不存在 → `None`；`LibraryDb::open` 失败 → warn 日志 + `None`。
  - 内层 `liked_in_db(db: &mut LibraryDb, mid: &str) -> bool`（纯逻辑，单测锚点）：`TrackProvider::from_id(mid)` 分流 source（`"local"` / `"qq"`，key=mid 全串，与 CLI `provider_of` 同语义）；`db.relation_desired("track", source, mid, "liked")` → `unwrap_or(false)`。
  - `None`（库不可用）时回包不写 liked（保持中性 false）——回填失败静默降级，不打扰用户（无曲收藏历史的新装机库不存在是常态）。
- 补充回填触发点：`apply_event` 的 `library_changed` 分支（player_bridge.rs:538-541）在 `schedule_refresh` 之外**追加一次** `spawn_liked_backfill(当前 mid)` —— 覆盖「后台 QQ reconcile 改了当前曲收藏」「CLI 在别处收藏了当前曲」的窗口，心形随库事实走。

### 3.2 点击写链（乐观翻转 + 库事实收敛）

```
TouchArea clicked（now-playing.slint:481）
  → NowPlaying.toggle-liked()          // 纯意图，Slint 不再本地翻转
  → Rust on_toggle_liked（bind() 内绑定）：
      mid = Player.current_mid；mid 为空 → no-op（无曲不可收藏）
      desired = !NowPlaying.liked
      np.set_liked(desired)            // 乐观翻转（点击即亮/灭，无迟滞）
      source/key = liked_in_db 同款分流；title = Player.title（空则回退 mid，CLI 同款）
      runtime.spawn: request(Request::Favorite { source, key, title, desired })
        回包 invoke_from_event_loop + 守卫（Player.current_mid == mid）:
          Ok(Response::Ok)      → bridge::schedule_refresh（我喜欢页即时；LibraryChanged 推送互补，menu_bridge.rs:5-7 同措辞）
                                  + spawn_liked_backfill(mid)   // 收敛确认：daemon 在应答前已落库，读回必等于乐观值，通常 no-op；多连点时保证终态=库事实
          Ok(Response::Err{..}) → show_feedback(friendly_error(code, msg)) + spawn_liked_backfill(mid)   // 诚实回滚：读库回到事实态
          Err(传输/离线)        → show_feedback("收藏服务未连接，操作没有生效") + spawn_liked_backfill(mid)
```

**为什么回滚 = 重读库而不是翻转回捕获值**：快速连点会在途多个写请求；若 A、B 两点击均失败，按「各自捕获的前值」回滚会互相踩（后回滚者把 UI 写回错误值）。库在 daemon 应答 Ok 之前已同步写盘（server.rs:232-255 无 await 间隔），故「失败后重读库」在单次失败时即等于回滚到点击前，在多次交错时收敛到库事实——单一机制覆盖两态，且天然诚实。成功路径同样重读一次做收敛确认，杜绝任何乐观/事实漂移。

**多连点终态边界（M1 改述）**：每次点击独立短连接、`set_relation` 幂等最后意图胜出（db.rs:1331-1338）；UI 侧最终以「回包触发的库重读」收敛到**库事实（最后到达 daemon 者胜出）**。注意到达序 ≠ 点击序：backend.rs:278-299 对 NotFound/Refused 有 4×250ms 冷启动重试，若点击 A 落入重试窗口而更晚的点击 B 直连先达，两写可反序落库，**极端倒挂窗口下库终值（及收敛后的 UI）可能与最后一次点击意图相反**。本地 IPC 下概率极低；已知边界，接受并记录（代码注释同述），不做在途串行化。

**O1 纯函数**：回包处理抽 `fn favorite_outcome(&Result<Response, BackendError>) -> FavoriteOutcome`（`Accepted / Rejected(String) / Unreachable` 三态枚举；`Ok(_)` 意外形态归 Rejected("应答异常")——收藏路径不会出现查询应答，按未生效诚实处理）；决策表钉 3 行单测（⑤-U3）。回包应用侧（守卫 + 三分支动作）在其上组装。

### 3.3 失败反馈（用户可见）

- daemon 拒绝（`Response::Err`）：复用 `friendly_error`（NotLoggedIn 文案不会出现在收藏路径——免凭证；其余透传「操作未生效：{message}」）。
- 传输失败/离线：`"收藏服务未连接，操作没有生效"`（对齐 report_command_result 的「播放服务未连接」句式，player_bridge.rs:161）。
- 反馈条机制复用 `show_feedback`（4s 自动消退、新消息重置计时）。QQ 远端同步失败**不反馈**——异步 outbox 自带退避重试，属后台域（与 CLI 行为一致）。

### 3.4 UI 同步策略（「我喜欢」页 / 表格）

- **我喜欢页（Data.liked）**：双通道自动同步，零 bridge.rs 改动——① daemon 写后广播 `Event::LibraryChanged` → `apply_event` → `schedule_refresh`；② 成功回包主动 `schedule_refresh`（推送丢失/离线窗口期兜底，menu_bridge 歌单写同款双保险）。
- **曲目表**：当前无任何表格渲染心形（TrackRow 无字段），无同步对象；本流不动表格（范围外，见 ①）。
- **当前曲心形 vs 库**：3.1 的 library_changed 补充回填保证后台变更（reconcile/CLI 写）也反映到播放页。

### 3.5 Slint 改动（最小面）

- `stores.slint` NowPlaying global：liked 属性注释更新 + 新增 `callback toggle-liked();`（仅此两行量级）。
- `now-playing.slint:480-482`：`clicked => { NowPlaying.toggle-liked(); }`（替换本地翻转；图标/配色/40px 命中区 N15 全部不动）。

---

## ④ 任务分解

### T1 Slint：回调声明与接线
**文件**：`crates/hmp-desktop/ui/stores.slint`、`crates/hmp-desktop/ui/now-playing.slint`
- stores.slint:244-246 区块：注释改为真语义（换曲由 Rust 回填、点击发 daemon 写）；新增 `callback toggle-liked();`。
- now-playing.slint:481：`NowPlaying.liked = !NowPlaying.liked;` → `NowPlaying.toggle-liked();`。
**验收**：`cargo check -j 4 -p hmp-desktop` 过；全仓 grep `NowPlaying.liked =` 无写入点（Rust 侧 set_liked 除外）；点击行为移交 Rust（T3 绑定后生效）。

### T2 player_bridge：回填链
**文件**：`crates/hmp-desktop/src/player_bridge.rs`
- 新增 `fn liked_in_db(db: &mut hmp_storage::LibraryDb, mid: &str) -> bool` 与 `fn liked_of(mid: &str) -> Option<bool>`、`fn spawn_liked_backfill(ui_weak: &Weak<AppWindow>, runtime: &Arc<BackendRuntime>, mid: String)`（3.1 规格）。
- `apply_daemon_state` mid_changed 分支（697 行旁）：保留 `np.set_liked(false)`，追加 `spawn_liked_backfill(...)`（与歌词/评论拉取并列）。
- `apply_event` library_changed 分支（538-541）：追加对当前 mid 的 `spawn_liked_backfill`。
- `clear_now_playing`：1110 行 `set_liked(false)` 保留（守卫天然丢弃在途回填包）。
**验收**：单测（⑤-U1/U2）；手验：收藏过的曲切进来心形点亮。

### T3 player_bridge：点击写链与失败反馈
**文件**：`crates/hmp-desktop/src/player_bridge.rs`
- `bind()` 内绑定 `NowPlaying::get(ui).on_toggle_liked(...)`（3.2 规格：空 mid no-op、乐观翻转、发 `Request::Favorite`、三分支回包处理；M1 边界注释随代码落）。
- 新增 `favorite_outcome` 纯函数 + `FavoriteOutcome` 枚举（O1）与离线文案常量；`friendly_error` 复用；不新增全局属性（无 pending 态——乐观翻转无迟滞）。
**验收**：单测（⑤-U1/U2/U3）；集成测试（⑤-I1）；手验：在线点击→我喜欢页即时增删行；`hmp quit` 后点击→反馈条+心形回滚。

### T4 测试落地
**文件**：`crates/hmp-desktop/src/player_bridge.rs`（cfg(test) 追加）、`crates/hmp-desktop/tests/like_key.rs`（新建）
见 ⑤。

### T5 文档收口（已授权，随功能同一提交）
**文件**：`crates/hmp-desktop/PORTING.md`
- 已知偏差列表删除「播放页喜欢键仍为 mock 态」一行（不再是偏差）；M8 模块状态行（PORTING.md:36）追加真接线一句话记录（换曲回填库事实 + Favorite IPC 写 + 失败回读库回滚）。
**验收**：PORTING.md 与现实一致；docs/** 不动。

### T6 全量自验（资源纪律）
- `cargo check -j 4 -p hmp-desktop`；
- `cargo test -p hmp-desktop -j 1 --lib`；
- `cargo test -p hmp-desktop -j 1 --test like_key`；
- 既有测试回归：`cargo test -p hmp-desktop -j 1 --test context_menu`（改动共享文件后必跑）。
- 真机冒烟（可选，如有环境）：release 构建，播放已收藏曲 → 心形亮；点击灭 → `hmp favorite list` 该曲消失；断 daemon 点击 → 反馈条。

---

## ⑤ 测试计划

### U. 单元测试（src/player_bridge.rs `#[cfg(test)] mod tests` 追加；纯逻辑，不建 AppWindow）

- **U1 `favorite_source_split_matches_cli_provider_of`**：`liked_in_db` 的 source/key 分流与 CLI `provider_of` 同构——`"local:C:\\a.flac"` → ("local", 全串)；`"003aQm4F3GJHZq"` → ("qq", mid)。（以行为断言：往 in-memory 库 `add_favorite("local", "local:<p>", ..)` 后 `liked_in_db(db, "local:<p>")` 为 true，验证键前缀口径一致。）
- **U2 `liked_in_db_truth_table`**（`LibraryDb::open_in_memory`）：
  - 收藏过（`add_favorite`）→ true；未收藏 mid → false；
  - `set_relation("track","qq",k,"liked",false)` 取消后 → false；
  - `remove_favorite` 往返（add → remove）→ false；
  - 本地键经 `canonical_local_key` 归一后仍命中（Windows verbatim 路径两侧一致）。
- **U3 回包决策表（O1）**：`favorite_outcome` 三行钉——`Ok(Response::Ok)` → Accepted；`Ok(Err{Internal,..})` → Rejected(_)（携带人话文案）；`Err(BackendError)` → Unreachable。附：离线文案常量含「未连接」；`friendly_error(Internal, msg)` 透传（既有行为回归钉）。

### I. 集成测试（`tests/like_key.rs` 新建；i-slint-backend-testing，单 `#[test]` 单进程——init_no_event_loop 进程级单例，context_menu.rs 同约束）

- **I1 `like_key_click_fires_callback_and_does_not_flip_locally`**：
  1. `init_no_event_loop()` + `AppWindow::new()` + set_size(1280,800)（context_menu.rs:44-49 同款）；
  2. 播种 `Player.current-mid = "like-test-mid"`、`Player.title`、`Player.overlay-visible = true`、`NowPlaying.liked = false`；
  3. 绑定观察者 `NowPlaying::on_toggle_liked`（计数器——测试自身扮演 Rust 处理器，验证 Slint→回调链）；
  4. `settle()`（mock_elapsed_time 16ms×40，PORTING.md:176 布局收敛坑）；
  5. 合成左键点击心形命中点：**标定法**（context_menu.rs:82-99 先例）——标题行右上区域（封面列右侧信息列、大标题行内 40px 命中区，now-playing.slint:474-478；坐标以窗口 1280×800 实测标定一次写死并注释推导，实现期用探针二分确认）；
  6. 断言：计数 == 1 且 `NowPlaying.liked == false`（**本地翻转已删除**——本轮核心回归钉）；
  7. 反向：Rust 侧直接 `NowPlaying::get(&ui).set_liked(true)` → 图标消费属性路径不受影响（可省，属性消费未动）。
- **I2（同文件第二用例不建——单例约束；并入 I1 尾段）**：再点一次计数 == 2、liked 仍 false（连点无本地态漂移）。

### 既有回归必跑

`--lib`（player_bridge.rs 既有测试组，`#[cfg(test)]` 起于 1271 行约 183 行）、`--test context_menu`（同文件 slint 改动）、`--test drag`（多跑无害的邻域回归）。

### 不做的测试（诚实边界）

- 真 daemon 端到端（点击→sqlite 落库）：daemon 侧已有 server.rs:1292-1315 测试钉「Favorite→Ok+LibraryChanged」；桌面→daemon 的传输层由 backend.rs 既有路径承载；live 冒烟归 T6 手验（live_daemon.rs 是 `--ignored` 专项）。

---

## ⑥ 风险与回滚

| 风险 | 缓解 |
|---|---|
| 心形命中点坐标随布局漂移（I1 脆弱） | 标定法 + 注释推导；不行则降级为「回调级」测试（直接调 `NowPlaying` 回调绑定断言）保底——计划允许实施期降级，PLAN 评审时可裁 |
| 换曲在途回填包晚到（跨曲竞态） | `Player.current_mid == mid` 守卫（spawn_comment_fetch 同款，player_bridge.rs:909）；clear_now_playing 置空 mid 后陈包必弃 |
| 多连点交错失败回滚互踩 | 回滚=重读库（3.2 论证），不以捕获值回滚 |
| **（O2）无关库写与在途点击写交错**：扫描等无关库写触发 `library_changed` 回填时，若点击写恰在途（乐观已翻转、daemon 未落库），回填会把心形瞬时读回旧值、点击回包后再翻回终值——**终态正确、仅一次闪动**，如实记录接受（串行化代价大于收益） |
| 多连点到达序倒挂（M1） | 接受为已知边界：UI 收敛到库事实（最后到达者胜出）；backend.rs 冷启动重试窗口下终值可能与最后意图相反，概率极低，不做在途串行化 |
| 乐观翻转与 LibraryChanged 补充回填抖动 | 回填报的是 daemon 应答前的已落库事实（server.rs 同步写库后才应答），正常路径读回=乐观值，无闪动 |
| sqlite 读在 async 上下文阻塞 | 毫秒级单查询 + runtime 工作线程执行（backend.rs:508-511 既例）；每曲换歌仅 1 次 |
| 桌面与 daemon 双连接写库冲突 | 桌面只读库（写全部走 daemon）；WAL 跨进程并发读既有事实（library_view.rs:156） |
| 远端同步失败（80105 等）污染 UI | 远端属 outbox 异步域（sync.rs 退避），UI 只表达本地事实——设计即隔离 |
| 并行流冲突 | 触碰文件全在本流所有权内；PORTING.md 一行属条件项（⑦）；无 engine.rs/bridge.rs/backend.rs 改动 |

**回滚方案**：整链集中在 3 个文件 + 1 新测试文件；`git revert` 单提交即回 mock 态（本地翻转一行是独立可退的 Slint 改动）。

---

## ⑦ 与并行流的边界确认

1. **engine.rs 裁决：不需要**。核实收藏命令处理在 `server.rs:224`（非 engine.rs 命令分发 354-438）；且本方案**零 daemon 改动**——桌面直发既有 `Request::Favorite`，server.rs/storage/sync 全部只读复用。方案优先论证成立：**「桌面端直发既有 Request 命令、零 daemon 改动」为本计划采用路径**。`server.rs`/`content.rs`/`transport.rs`/`serve.rs`/`engine.rs`/`engine_tests.rs` 均不触碰。
2. **hmp-qqmusic-api**：远端同步已由 daemon SyncWorker 全链承载（sync.rs:184-202 → songlist.rs:269/281），本流零改动零接触。
3. **backend.rs**：不在本流所有权清单，亦不在禁碰清单；本计划**不改它**（request/订阅原样用），规避归属模糊。
4. **bridge.rs / ui 其余 slint**：零触碰；「我喜欢」页刷新靠既有 `schedule_refresh` 调用（调用非编辑，player_bridge.rs:539 既有先例）。
5. **PORTING.md（crates/hmp-desktop/PORTING.md）**：协调者已裁决——153 行 mock 记录更新（连同 M8 状态行一句话）归本流，随功能同一提交（T5）。
6. **PLAN.md**：本文件（worktree 根），本流所有。
