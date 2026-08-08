# 媒体库 / 播放会话重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 daemon 从"能播"升级为媒体库/播放会话管理器：修复播放正确性（300 首分页、shuffle 顺序/历史、MPRIS 同步、PlayNext 多曲目、skip/EOS 语义），引入 SQLite 本地媒体库 + 音质策略，建立 provider 模型接入本地音乐。

**Architecture:** 三阶段独立可交付。A = 播放核心正确性（不碰数据库）；B = hmp-storage 扩展为 SQLite 媒体库（tracks/play_events/favorites/playlists/local_files）+ 配置化音质策略 + 播放会话粒度历史；C = provider-aware TrackRef + CompositeSourceResolver + LocalSourceResolver，解除本地资源对 QQ 登录态依赖。

**Tech Stack:** Rust 2024 workspace；rusqlite(bundled) + toml + lofty(标签)（新增依赖，均加入各自 crate Cargo.toml 的 `[dependencies]`，workspace 统一声明版本）；tokio/watch/mpsc 不变。

## Global Constraints

- 全部命令/消息保持 serde 兼容：新增字段必须 `#[serde(default)]`（老客户端帧可反序列化）。
- IPC 单帧上限 `MAX_FRAME = 1 MiB` 不变。
- 数据库只存稳定身份与元数据，**绝不写临时播放 URL**（`Track.url` 是取流后 URI，会失效）。
- 播放历史用"会话"粒度（INSERT on start / UPDATE on end），禁止 position 轮询写库。
- 新增依赖：`rusqlite = { version = "0.32", features = ["bundled"] }`、`toml = "0.8"`、`lofty = "0.21"`（workspace 级声明）。
- 每个任务 = TDD：先写失败测试 → 验证失败 → 实现 → 验证通过 → commit。
- 提交信息前缀：`fix(core)` / `feat(storage)` / `feat(daemon)` / `feat(cli)` 按 crate。

---

# Plan A — 播放核心正确性（优先级最高）

## A1: 分页终止条件改用服务端状态（歌单 + 专辑）

**Files:**
- Modify: `crates/hmp-daemon/src/player.rs:375-410`（两处 `for page in 1..=3`）
- Test: 新增于 `crates/hmp-daemon/src/player.rs` 测试模块（复用现有 wiremock 或 fake 模式——先读现有分页测试怎么构造）

**现状（已核实）：** 循环体内已有 `if resp.hasmore == 0 || out.len() >= resp.total { break }`（歌单）与 `out.len() >= resp.total_num`（专辑），但外层 `for page in 1..=3` 硬截断为 300 首。

- [ ] **Step 1: 写失败测试** — 构造 4 页 × 100（hasmore=1, total=400），断言收集 400 首（旧代码只能 300）。
- [ ] **Step 2: 运行确认失败**（300 ≠ 400）。
- [ ] **Step 3: 实现** — 两处改为：
```rust
let mut page = 1;
loop {
    let resp = api.get_detail(list_id, 0, 100, page, true, false, false)?;
    out.extend(resp.songlist.iter().map(|s| s.mid.clone()));
    if resp.hasmore == 0 || out.len() as i64 >= resp.total || page >= 100 {
        break; // 服务端终止 + 安全上限 100 页防死循环
    }
    page += 1;
}
```
专辑同理（`total_num`，上限 100 页）。
- [ ] **Step 4: 验证通过** + `cargo test -p hmp-daemon` 全绿。
- [ ] **Step 5: Commit** `fix(daemon): paginate playlist/album until server says done`

## A2: MPRIS Shuffle 属性回同步

**Files:**
- Modify: `crates/hmp-mpris/src/service.rs`（`update_props`，~449 行；结构体加 `shuffle: bool` 字段）
- Test: 同文件测试模块

**现状（已核实）：** `update_props` 同步 status/loop/volume/can_seek/can_play/can_pause/metadata，**无 shuffle**；`PlaybackState.shuffle: bool` 已存在（hmp-core/src/player.rs:62）。

- [ ] **Step 1: 写失败测试** — `update_props` 传入 `state.shuffle=true`，断言 `changed` 含 `"Shuffle"=Bool(true)`；再传 false 断言再次出现。
- [ ] **Step 2: 确认失败**。
- [ ] **Step 3: 实现** — 结构体加 `shuffle: bool`（init false），update_props 内：
```rust
if self.shuffle != state.shuffle {
    self.shuffle = state.shuffle;
    changed.insert("Shuffle", Value::Bool(state.shuffle));
}
```
（确认 Shuffle 属性已声明 writable 且 Set 处理器存在——已核实链路通；若无 getter 同步需补。）
- [ ] **Step 4: 验证通过。**
- [ ] **Step 5: Commit** `fix(mpris): sync Shuffle property from playback state`

## A3: CanGoNext/CanGoPrevious 考虑 shuffle

**Files:**
- Modify: `crates/hmp-daemon/src/engine.rs`（`publish()` 的 caps 计算，~178-198 行）
- Test: engine 测试——shuffle on + 队尾，断言 `caps.can_go_next == true`

**现状（已核实）：** caps 计算只看 `loop_mode` 与位置；shuffle 时队尾真实引擎可回绕继续，但 caps 报 false。

- [ ] **Step 1: 写失败测试**（shuffle on、3 曲、current 在队尾 → can_go_next 应为 true；shuffle off 同位置 → false）。
- [ ] **Step 2: 确认失败。**
- [ ] **Step 3: 实现** — 由 A5 的新 `QueueCore::can_go_next()/can_go_previous()` 提供（shuffle 时 = 有队列即 true；非 shuffle = loop 模式回绕或 cursor 未到界）。engine publish() 改为调用之。
- [ ] **Step 4: 验证通过。**
- [ ] **Step 5: Commit** `fix(daemon): caps consider shuffle (CanGoNext wraps in random order)`

## A4: PlayNext 多曲目完整插入

**Files:**
- Modify: `crates/hmp-core/src/queue.rs`（新增 `insert_after_current(&mut self, ids: Vec<TrackId>) -> Option<usize>`；保留 `insert_next` 兼容或删除后改调用点）
- Modify: `crates/hmp-daemon/src/engine.rs:266-274`（playnext 路径）
- Test: queue 测试 + engine 测试

**现状（已核实）：** engine playnext 只插 `ids[0]`，其余丢弃。逐条 insert_next 会反转顺序（后插者顶到 current+1）。

- [ ] **Step 1: 写失败测试** — queue：3 曲当前=1，`insert_after_current([A,B,C])` → 规范顺序 [t0, A, B, C, t1, t2]，current 定位 A；空队列调用 → 返回 None 且队列=A,B,C。
- [ ] **Step 2: 确认失败。**
- [ ] **Step 3: 实现** — `insert_after_current`：
  - 队列空 → `set_queue(ids)`，返回 None。
  - 否则在规范 `cur+1` 处插入整片（索引顺移），order 中在 cursor 后插入新索引序列；返回 `Some(cur+1)`。
- [ ] **Step 4: engine 改** — playnext：`if let Some(_) = self.queue.insert_after_current(ids.clone()) { self.load_and_play(ids[0].clone()).await } else { /* 空队列：set_queue 已建，直接播 */ }`；删除 `insert_next`+`set_current` 旧路径。engine 测试：playnext 歌单 3 首 → snapshot 队列含全部 3 首且顺序正确、播放第一首。
- [ ] **Step 5: 验证通过。**
- [ ] **Step 6: Commit** `fix(core,daemon): playnext inserts full playlist after current`

## A5: QueueCore 重构 — 播放顺序 + 历史（shuffle 语义）

**Files:**
- Modify: `crates/hmp-core/src/queue.rs`（核心重构）
- Modify: `crates/hmp-daemon/src/engine.rs`（调用点：next_track/prev_track 拆分 skip/EOS；publish caps 用新方法）
- Test: queue.rs 测试重写 shuffle 部分 + 新增；engine 测试调整

**新模型（规范顺序 + 播放顺序 + cursor）：**
```rust
pub struct QueueCore {
    tracks: Vec<TrackId>,   // 规范顺序（快照/显示不变）
    order: Vec<usize>,      // 播放顺序：非 shuffle = 0..n；shuffle = 排列
    cursor: usize,          // 当前曲在 order 中的下标
    has_current: bool,
    loop_mode: LoopMode,
    shuffle: bool,
    rng: Rng,
}
```
公开 API（替换旧 `next_track`/`prev_track`/`shuffled_next`/`insert_next`/`set_current` 语义）：
```rust
pub fn current(&self) -> Option<&TrackId>          // tracks[order[cursor]]
pub fn current_idx(&self) -> Option<usize>         // 规范下标 order[cursor]
pub fn set_current(&mut self, canonical: usize)    // 定位 order[p]==canonical → cursor=p, has_current=true
pub fn set_queue(&mut self, ids: Vec<TrackId>)     // 重置；shuffle 时生成排列，cursor=0, has_current=true
pub fn append(&mut self, id: TrackId)
pub fn insert_after_current(&mut self, ids: Vec<TrackId>) -> Option<usize>  // A4
pub fn remove(&mut self, idx: usize) -> Option<TrackId>   // 规范下标；维护 order 一致性
pub fn clear(&mut self)
pub fn set_loop_mode / loop_mode / set_shuffle(&mut self, on: bool) / shuffle
pub fn skip_next(&mut self) -> Option<TrackId>     // 用户主动下一首：Track 不阻塞（回绕同 List）
pub fn advance_on_eos(&mut self) -> Option<TrackId> // EOS：Track → 重播当前；否则同 skip_next
pub fn prev_track(&mut self) -> Option<TrackId>    // Track → 当前；List/shuffle → 回绕；None → cursor>0 才退
pub fn can_go_next(&self) -> bool                  // len>0 && (shuffle || List || Track || cursor+1<len)
pub fn can_go_previous(&self) -> bool              // len>0 && (shuffle || List || Track || cursor>0)
```
规则：
- `set_shuffle(true)`：生成 0..n 的随机排列（Fisher-Yates），并交换使当前曲停在原 cursor 槽；`set_shuffle(false)`：order=0..n。
- `remove(i)`：tracks 删 i；order 删值为 i 的元素 p；>i 的条目 -1；cursor 修正（p<cursor → -1；p==cursor → has_current 由调用方决定——engine 负责接替）。
- `advance_on_eos` 在 `LoopMode::Track` 返回当前曲且不推进 cursor。

**engine 调用点映射：**
- `Request::Next` → `skip_next()`；EOS 事件 → `advance_on_eos()`（现 engine.rs:237 与 279-285 各有一处 next_track——分别对应，先读代码确认哪处是 EOS）。
- `Request::Previous` → `prev_track()`。
- `publish()` caps → `queue.can_go_next()/can_go_previous()`（A3）。
- playnext → `insert_after_current`（A4）；`replace(ids,0)` 保留（Play 用）；`set_current` 仅剩一处（若 playnext 不再需要则删）。

- [ ] **Step 1: 重写 queue.rs 核心 + 全部测试**（next/prev 模式矩阵、shuffle 排列唯一性/不重放/回绕、历史 prev 回真实上一首、remove 一致性、insert_after_current 顺序、set_shuffle 保持当前）。
- [ ] **Step 2: 编译 + 测试失败于旧断言** → 更新为新材料断言。
- [ ] **Step 3: engine 调用点改造** + caps 用新方法。
- [ ] **Step 4: `cargo test --workspace` 全绿**（含 CLI/daemon/server 对 current 语义的既有断言，逐个修正为规范下标语义——注意 `queue.current` snapshot 仍是规范下标 `order[cursor]`）。
- [ ] **Step 5: Commit** `fix(core): queue playback order + shuffle history; split skip/EOS semantics`

## A6: 收尾验证

- [ ] **Step 1:** `cargo clippy --workspace --all-targets --all-features -- -D warnings` + `cargo fmt --all -- --check` 干净。
- [ ] **Step 2:** `cargo test --workspace` 全绿；`cargo build --workspace`。
- [ ] **Step 3: Commit** 任何残留调整。
- [ ] **Step 4:** 更新 `docs/USAGE.md` §5（prev 恒上一首改为"随机模式下按播放顺序回退"；shuffle 说明）。

---

# Plan B — SQLite 媒体库 + 音质策略

## B1: hmp-storage SQLite 基础设施（migrations + 核心表）

**Files:**
- Create: `crates/hmp-storage/src/db.rs`
- Modify: `crates/hmp-storage/src/lib.rs`（`pub mod db;` + 重导出）、`crates/hmp-storage/Cargo.toml`（rusqlite）
- Test: `crates/hmp-storage/src/db.rs` 内测试（in-memory 连接）

**Schema（v1，PRAGMA user_version=1）：**
```sql
CREATE TABLE tracks (
  id INTEGER PRIMARY KEY,
  source TEXT NOT NULL,            -- 'qq' | 'local'
  source_key TEXT NOT NULL,        -- QQ mid | local 身份
  title TEXT NOT NULL,
  album TEXT, artist TEXT, duration_ms INTEGER, cover_uri TEXT,
  play_count INTEGER NOT NULL DEFAULT 0,
  last_played_at INTEGER,
  UNIQUE(source, source_key)
);
CREATE TABLE local_files (
  track_id INTEGER PRIMARY KEY REFERENCES tracks(id),
  path TEXT NOT NULL UNIQUE, file_size INTEGER, mtime INTEGER,
  format TEXT, bitrate INTEGER, sample_rate INTEGER
);
CREATE TABLE favorites (track_id INTEGER PRIMARY KEY REFERENCES tracks(id), created_at INTEGER);
CREATE TABLE playlists (id INTEGER PRIMARY KEY, name TEXT NOT NULL, created_at INTEGER, updated_at INTEGER);
CREATE TABLE playlist_tracks (playlist_id INTEGER REFERENCES playlists(id), track_id INTEGER REFERENCES tracks(id), position INTEGER, added_at INTEGER);
CREATE TABLE play_events (
  id INTEGER PRIMARY KEY, track_id INTEGER REFERENCES tracks(id),
  started_at INTEGER NOT NULL, ended_at INTEGER, listened_ms INTEGER, end_reason TEXT
);
CREATE INDEX idx_play_events_started ON play_events(started_at DESC);
```
**API（全部同步、`Arc<Mutex<LibraryDb>>` 共享）：**
```rust
pub struct LibraryDb { conn: rusqlite::Connection }   // 私有
pub fn open(path: &Path) -> rusqlite::Result<LibraryDb>  // 建目录、WAL、migrate
pub fn open_in_memory() -> rusqlite::Result<LibraryDb>   // 测试用
pub fn upsert_track(&mut self, t: &Track) -> rusqlite::Result<i64>
pub fn record_play_start(&mut self, track_id: i64, started_at: i64) -> rusqlite::Result<()>
pub fn record_play_end(&mut self, event: &PlayEnd) -> rusqlite::Result<()>
pub fn recent_plays(&mut self, limit: u32) -> rusqlite::Result<Vec<RecentPlay>>
pub struct PlayEnd { track_id: i64, ended_at: i64, listened_ms: i64, reason: &'static str }
```
（upsert_track 消费 `hmp_core::Track` → 需 hmp-storage 依赖 hmp-core——检查依赖方向：hmp-storage 目前不依赖 hmp-core；改为 db.rs 接受窄参数 `TrackRow{source,source_key,title,album,artist,duration_ms,cover_uri}`，由调用方（daemon）从 Track 投影，避免新增依赖环。**决定：不引 hmp-core，用窄结构体。**）

- [ ] **Step 1: 写失败测试**（open 建表+user_version=1；upsert 幂等；play start→end 后 recent_plays 返回含 listened_ms/end_reason；两表 FK 联动）。
- [ ] **Step 2: 确认失败。**
- [ ] **Step 3: 实现**（rusqlite bundled；`PRAGMA journal_mode=WAL`；migrate 按 user_version 逐级）。
- [ ] **Step 4: 验证通过。**
- [ ] **Step 5: Commit** `feat(storage): SQLite library schema v1 (tracks/play_events/favorites/playlists/local_files)`

## B2: 配置持久化 + 音质策略

**Files:**
- Create: `crates/hmp-storage/src/config.rs`（`pub mod config;`）
- Modify: `crates/hmp-core/src/media.rs`（`fallback_chain()` 修正 + `AudioQuality` serde 别名 + from_str）
- Test: config round-trip + chain 生成

**模型：**
```rust
pub enum QualityMode { Auto, Fixed(AudioQuality) }      // serde: "auto" / "flac" 等
pub struct QualityPref { pub mode: QualityMode, pub fallback: bool }
pub struct Config { pub quality: QualityPref }
impl Config {
    pub fn load() -> Config;                  // 读 config_dir()/config.toml，缺失 → default
    pub fn save(&self) -> Result<()>;         // 建目录 + 原子写（tmp+rename）
    pub fn chain(&self) -> Vec<AudioQuality>; // Auto → fallback_chain(Master)；Fixed(q) → q.fallback_chain()；!fallback → [q]
}
```
- 修正 `AudioQuality::fallback_chain()`：Master 分支补 Atmos（现 CHAIN 注释明说 fallback_chain 漏 Atmos）→ Master: [Master, HiRes, Atmos, Flac, Mp3_320, Mp3_128]。
- `AudioQuality` 加 `#[serde(rename_all = "lowercase")]` 兼容？——现有 serde 用默认枚举名（Mp3_128 等）；改为别名方案：`impl AudioQuality { pub fn from_alias(s: &str) -> Option<Self> }`（auto/master/hires/atmos/flac/aac/320/128），序列化保持现状不变（IPC 兼容）。

- [ ] **Step 1: 写失败测试**（load 默认 Auto；save→load round-trip；chain: Fixed(Flac) → [Flac, Mp3_320, Mp3_128]；fallback=false → [Flac]；Auto → 含 Atmos 的 Master 链）。
- [ ] **Step 2: 确认失败。**
- [ ] **Step 3: 实现**（toml crate）。
- [ ] **Step 4: 验证通过。**
- [ ] **Step 5: Commit** `feat(storage): persistent quality preference (config.toml) + complete fallback chains`

## B3: resolver 应用音质策略 + available/actual 拆分

**Files:**
- Modify: `crates/hmp-core/src/media.rs`（`Track.qualities` → `available_qualities`，serde 兼容保留旧名 alias）
- Modify: `crates/hmp-core/src/player.rs`（`PlaybackState` 加 `actual_quality: Option<AudioQuality>`，`#[serde(default)]`）
- Modify: `crates/hmp-daemon/src/player.rs`（resolve 用 `Config::chain()` 替代固定 CHAIN；available 从 size 字段 + 成功探测收集；选定档写入 actual）
- Modify: `crates/hmp-daemon/src/engine.rs`（publish 时把 actual_quality 带入 PlaybackState）
- Modify: `crates/hmp-cli/src/commands.rs`（status 显示音质）
- Test: player 单测（FakeResolver 断言请求的音质顺序按链）+ engine 测试

**细节：**
- resolve_track_impl 用 `config.chain()`；`available_qualities` 初始从 QQ 响应 size 字段（`size_128mp3>0 → Mp3_128`，`size_320mp3>0 → Mp3_320`，`size_flac>0 → Flac`），并把探测成功的档并入（去重、从高到低）。
- `Track` serde：`#[serde(alias = "qualities")] available_qualities`。
- CLI status 增加 `音质: FLAC` 行（actual_quality）。

- [ ] **Step 1: 写失败测试**（chain 驱动请求顺序；available 收集）。
- [ ] **Step 2: 确认失败。**
- [ ] **Step 3: 实现。**
- [ ] **Step 4: 验证 + workspace 全绿**（既有 `qualities` 引用点：engine.rs:303、commands.rs:277、server.rs:276、e2e.rs、desktop app.rs:970——逐个改 `available_qualities`）。
- [ ] **Step 5: Commit** `feat(core,daemon): quality policy drives fallback chain; available vs actual quality`

## B4: daemon 播放会话写库（history）

**Files:**
- Modify: `crates/hmp-daemon/src/engine.rs`（挂 LibraryDb：成功 start → upsert+record_play_start；end 事件 → record_play_end）
- Modify: `crates/hmp-daemon/src/daemon.rs`（`Daemon::start` 打开 `data_dir()/library.sqlite3`，失败仅 warn 不阻断）
- Test: engine 测试——Fake 驱动下断言 db 调用序列（注入 `Arc<Mutex<LibraryDb>>` 内存库）

**事件映射：** EOS/Next/Prev/Stop/Play(换曲)/Quit → `record_play_end(reason)`；换曲开始 → start。end_reason: `ended|next|previous|stop|manual|quit`。
- `hmp-core::Track` → `TrackRow` 投影（见 B1 决定）。

- [ ] **Step 1: 写失败测试**（start→end 序列写库正确）。
- [ ] **Step 2: 确认失败。**
- [ ] **Step 3: 实现。**
- [ ] **Step 4: 验证 + workspace 全绿。**
- [ ] **Step 5: Commit** `feat(daemon): persist play sessions to library (session granularity)`

## B5: CLI — `hmp quality` / `hmp history`

**Files:**
- Modify: `crates/hmp-cli/src/main.rs`（两个子命令）、新 `crates/hmp-cli/src/quality.rs`、`crates/hmp-cli/src/history.rs`
- Modify: `crates/hmp-cli/Cargo.toml`（hmp-storage 已有；无需新依赖）
- Test: 各文件内单测

**`hmp quality`**：无参 → 显示当前策略与生效链（`自动：Master→HiRes→Atmos→FLAC→320→128` / `FLAC（回退 320/128）`）；`hmp quality auto|master|hires|atmos|flac|aac|320|128` → 写配置并显示确认。`--no-fallback` 开关（`fallback=false`）。
**`hmp history [n]`**：直接读库（daemon 与 CLI 同机同文件；WAL 支持并发），默认 10 条：`曲名 — 歌手 (时长, 原因) 时间`。

- [ ] **Step 1: 写失败测试**（quality 解析/格式化；history 格式化空库与有库）。
- [ ] **Step 2: 确认失败。**
- [ ] **Step 3: 实现。**
- [ ] **Step 4: 验证。**
- [ ] **Step 5: Commit** `feat(cli): hmp quality (preference) and hmp history (recent plays)`

## B6: 收尾

- [ ] **Step 1:** clippy/fmt/test 全绿 + `cargo build --workspace`。
- [ ] **Step 2:** 更新 `docs/USAGE.md`（§6 音质策略命令、§9 测试）、README 用法行。
- [ ] **Step 3: Commit** `docs: quality policy + history usage`.

---

# Plan C — Provider 模型 + 本地音乐

## C1: TrackProvider/TrackRef + PlayRequest::Local

**Files:**
- Modify: `crates/hmp-core/src/ipc.rs`（`PlayRequest` 加 `Local(TrackId)` 变体；新 `TrackProvider`）
- Test: ipc round-trip（Local 变体序列化）

**模型：**
```rust
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum TrackProvider { QqMusic, Local }
impl TrackProvider { pub fn from_id(id: &str) -> Self }  // "local:" 前缀 → Local
pub struct TrackRef { pub provider: TrackProvider, pub id: String }
impl TrackRef { pub fn from_play_request(r: &PlayRequest) -> TrackRef }
```
`TrackId` 保持 String newtype 不变（渗透面最小）；local id 格式 `local:<db-id>`。`PlayRequest::Local(TrackId)`。

- [ ] **Step 1: 写失败测试**（round-trip；from_play_request 映射）。
- [ ] **Step 2: 确认失败。**
- [ ] **Step 3: 实现。**
- [ ] **Step 4: 验证。**
- [ ] **Step 5: Commit** `feat(core): provider-aware track refs and PlayRequest::Local`

## C2: LocalSourceResolver + Composite

**Files:**
- Create: `crates/hmp-daemon/src/local.rs`
- Modify: `crates/hmp-daemon/src/daemon.rs`（`Daemon::start` 组 Composite）
- Modify: `crates/hmp-daemon/src/player.rs`（`QqSourceResolver::resolve_source_ids` 只对 QQ 源加载凭证；抽出 trait 或组合分发）
- Test: local resolver（内存库 + 临时文件）与组合分发

**LocalSourceResolver：** `Local(TrackId("local:<id>"))` → 查 db tracks(source='local', source_key=id) + local_files → `file://` URI → `ResolvedTrack`。无凭证要求。
**Composite：** 按 PlayRequest 变体分发到 Qq / Local。登录门（server.rs is_play_request）改为：`Play/PlayNext/QueueAppend` 的 QQ 变体才要求凭证；Local 不要求。

- [ ] **Step 1: 写失败测试**（local 解析；未登录时 Local play 成功而 QQ play 仍拒）。
- [ ] **Step 2: 确认失败。**
- [ ] **Step 3: 实现。**
- [ ] **Step 4: 验证 + workspace 全绿。**
- [ ] **Step 5: Commit** `feat(daemon): local source resolver + composite dispatch; login scoped to QQ`

## C3: 本地扫描 + 标签元数据

**Files:**
- Create: `crates/hmp-cli/src/scan.rs`（`hmp library scan <dir>`）
- Modify: `crates/hmp-cli/src/main.rs`、`hmp-cli/Cargo.toml`（lofty）
- Modify: `crates/hmp-storage/src/db.rs`（`add_local_file(path, tags...) -> track_id`）
- Test: 临时目录扫描（造几个假文件：真实标签用 lofty 写最小 mp3 或用无标签文件名回退——测试覆盖无标签回退 + 去重）

**扫描：** 递归 walk，扩展名 `mp3|flac|ogg|m4a|opus|wav`；`lofty` 读标签（title/artist/album/duration）；无标签 → 文件名（去扩展名）为 title；同 path 幂等 upsert。输出 `扫描完成：+N 曲（M 新）`。
**`hmp play local:<id>`** 已有 C2 支持；`hmp library list` 列出本地曲目（可选，若时间允许）。

- [ ] **Step 1: 写失败测试**（扫描 temp dir；无标签回退；重复扫描去重）。
- [ ] **Step 2: 确认失败。**
- [ ] **Step 3: 实现。**
- [ ] **Step 4: 验证。**
- [ ] **Step 5: Commit** `feat(cli,storage): local music scan with tag metadata (lofty)`

## C4: MPRIS OpenUri(file://) 接入

**Files:**
- Modify: `crates/hmp-mpris/src/service.rs`（`open_uri`）
- Modify: `crates/hmp-daemon/src/mpris.rs`（转发：OpenUri → daemon 命令——需新 IPC 变体 `Request::OpenUri(String)` 或复用 Local play 路径）

**决定：** 加 `Request::OpenUri(String)`（hmp-core ipc.rs，serde 兼容——新变体不破坏旧帧？枚举加变体会破坏反序列化旧帧——检查 deserialize 策略；如需要则先实现为 `PlayRequest::Local` 之外的路由）。`file://` → 单文件扫描入库 + Local play；`http(s)://` → 由 QQ resolver 的"直接 URI"能力（若存在）或暂 NotSupported 保留。已核实 `SupportedUriSchemes=["file","http","https"]` 而 OpenUri 恒 NotSupported——本任务至少让 file:// 真实可播。

- [ ] **Step 1: 写失败测试**（service 层 open_uri 对 file:// 不再返回 NotSupported；daemon 收到 OpenUri 后队列出现该曲）。
- [ ] **Step 2: 确认失败。**
- [ ] **Step 3: 实现。**
- [ ] **Step 4: 验证。**
- [ ] **Step 5: Commit** `feat(mpris,daemon): OpenUri plays file:// via local resolver`

## C5: 收尾 + 文档

- [ ] **Step 1:** clippy/fmt/test/build 全绿。
- [ ] **Step 2:** `docs/USAGE.md` 增本地音乐节（scan / play local:<id> / OpenUri）；README 用法行。
- [ ] **Step 3: Commit** `docs: local music usage`.
- [ ] **Step 4:** 三阶段分支 `feat/media-library` 全部提交后 → 终验 → 合并 main → push。

---

## Self-Review 记录

- **Spec 覆盖：** 审计 10 项 → A1(300页) A2,A3(MPRIS) A4(playnext) A5(shuffle/prev/skip) B1(B3-4 SQLite) B2,B3(音质) B4(历史) B5(命令) C1-C4(本地/登录解耦/OpenUri) ✓。TrackRef 结构体、CompositeSourceResolver 图、PlaybackState.actual_quality、schema 全部落位。
- **占位符扫描：** 无 TBD；engine EOS 调用点标注"先读代码确认"（执行时核实，非占位）。
- **类型一致：** `insert_after_current -> Option<usize>`（A4 与 A5 一致）；`can_go_next/can_go_previous`（A3/A5 一致）；`TrackRow`（B1 定义 B4 使用）；`PlayEnd`（B1/B4）；`Config::chain()`（B2/B3）；`PlayRequest::Local(TrackId)`（C1/C2）；`Request::OpenUri(String)`（C4）。
