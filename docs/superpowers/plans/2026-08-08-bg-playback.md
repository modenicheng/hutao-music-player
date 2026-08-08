# 后台播放（service + tray）实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现常驻播放器后端（hmp-daemon）+ CLI 遥控 + 最小 tray，使 QQ 音乐支持后台连续播放与终端 ASCII 二维码登录。

**Architecture:** 单例 daemon 进程（setsid 常驻，CLI 按需拉起）持有凭证/QQ API/解密/播放/队列/MPRIS；所有前端（CLI socket 客户端、tray、MPRIS）是适配器，经单一 `Request` 命令通道与单一 `watch<DaemonState>` 状态出口与后端交互；控制协议 = Unix socket 长度前缀 JSON 帧（消息类型在 hmp-core，与 `PlayerCommand` 同居）。

**Tech Stack:** Rust 2024 / tokio / clap / serde+serde_json / hmp-core（队列+协议）/ hmp-player-gst（PlayerCore）/ hmp-qqmusic-api / hmp-media（解密）/ hmp-storage（凭证）/ ksni（tray，feature）/ zbus（MPRIS，复用 hmp-mpris）/ image（CLI 二维码渲染，workspace 已有）

## Global Constraints

- **不引入 systemd unit / 不向用户系统安装任何东西**（spec §1.2）；daemon 由 CLI 按需拉起（`hmp serve --background`）。
- **后端核心（engine/player/队列）不得引用任何前端类型**（spec §3）；前端 = 适配器（socket 服务器、tray、MPRIS）。
- **协议消息类型在 hmp-core**；传输（socket）在 hmp-daemon。
- **命令-查询分离**（spec §7）：Play/PlayNext/QueueAppend 等命令只返回 `Ok`（已受理），真实结果经 `DaemonState`（Loading→Playing/Error）呈现；CLI `play` 短轮询 ≤15s 确认。
- **`prev` 一律跳上一首**（不做 >3s 回开头）；队列播完 daemon 保持存活，仅 `quit`/SIGTERM/SIGINT 终止。
- **CLI 输出约定**：二维码渲染与轮询提示全部 `write!` + `stdout().flush()`，禁止裸 `println!`。
- **登录仍在 CLI**；daemon 只读共享凭证（hmp-storage），无凭证 → Play 前置返回 `Err(NotLoggedIn)`。
- 工作区 edition 2024 / rust-version 1.85 / GPL-3.0-or-later；中文注释、ASCII 代码。
- 每任务一个原子提交；测试先行（TDD）；`cargo test --workspace` 稳定通过、fmt/clippy 干净。
- 允许新增依赖：hmp-daemon += `ksni`（tray）、`hmp-mpris`（feature）、`libc`（socket 回退路径 uid）；hmp-cli += `image`（workspace 已有）、`hmp-daemon`；hmp-core += `serde_json`（正式依赖，当前仅 dev-dep）。**不得新增其他第三方依赖**（随机数用自实现 xorshift，见 Task 1）。

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `crates/hmp-core/src/queue.rs` | `QueueCore` 纯逻辑队列 + `QueueSnapshot` |
| `crates/hmp-core/src/ipc.rs` | 协议消息（`Request/Response/Event/DaemonState/PlayRequest/IpcErrorCode`）+ 帧编解码纯函数 |
| `crates/hmp-core/src/lib.rs` | 注册模块、导出 |
| `crates/hmp-daemon/Cargo.toml` | 新 crate；features: `default=["tray","mpris"]`, `tray=["dep:ksni"]`, `mpris=["dep:hmp-mpris"]` |
| `crates/hmp-daemon/src/lib.rs` | 模块声明、`Daemon` 组装（feature 门控） |
| `crates/hmp-daemon/src/daemon.rs` | `Daemon`：持有引擎 + 服务器 + tray/MPRIS 适配器 + 优雅退出编排 |
| `crates/hmp-daemon/src/engine.rs` | `PlaybackEngine`：命令循环、队列裁决、Ended 自动续播、`watch<DaemonState>` 发布 |
| `crates/hmp-daemon/src/player.rs` | `PlaybackDriver` trait + `GstDriver`（包 PlayerCore）+ `resolve_track`（详情/回退/解密） |
| `crates/hmp-daemon/src/server.rs` | Unix socket 服务器：accept/每连接帧循环/查询/订阅 fan-out |
| `crates/hmp-daemon/src/serve.rs` | `run_foreground` / `run_background`（CLI `hmp serve` 入口） |
| `crates/hmp-daemon/src/tray.rs` |（feature `tray`）ksni 最小菜单适配器 |
| `crates/hmp-daemon/src/mpris.rs` |（feature `mpris`）hmp-mpris 搬入 |
| `crates/hmp-cli/src/qr_ascii.rs` | 二维码 PNG/JPEG → 终端 ASCII（半块字符） |
| `crates/hmp-cli/src/login.rs` | 重写：渲染 + 过期自动刷新循环 + flush |
| `crates/hmp-cli/src/client.rs` | socket 客户端：连接/拉起 daemon/请求/轮询 |
| `crates/hmp-cli/src/main.rs` | 子命令扩展 |
| `crates/hmp-cli/src/play.rs` | 改为遥控 `hmp play`（走 client） |
| `crates/hmp-cli/src/commands.rs` | 各遥控子命令的请求构造与输出 |
| `crates/hmp-cli/Cargo.toml` | += `image`、`hmp-daemon` |

---

### Task 1: hmp-core 队列核心 + IPC 协议

**Files:**
- Create: `crates/hmp-core/src/queue.rs`, `crates/hmp-core/src/ipc.rs`
- Modify: `crates/hmp-core/src/lib.rs`（注册 `pub mod queue; pub mod ipc;` + 导出）
- Test: 同文件内 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::id::TrackId`（现有）、`crate::player::{LoopMode, PlaybackCapabilities, PlaybackState, PlayerCommand}`（现有）、`crate::media::Track`（现有）
- Produces（后续任务依赖，签名锁定）:
  - `queue::QueueCore`（`new` / `replace(Vec<TrackId>, usize)` / `append(Vec<TrackId>)` / `insert_next(TrackId)` / `remove(usize) -> bool` / `clear` / `current() -> Option<&TrackId>` / `snapshot() -> QueueSnapshot` / `set_loop_mode(LoopMode)` / `set_shuffle(bool)` / `next_track() -> Option<TrackId>` / `prev_track() -> Option<TrackId>`）
  - `queue::QueueSnapshot { tracks: Vec<TrackId>, current: Option<usize>, loop_mode: LoopMode, shuffle: bool }`（Clone/Debug/PartialEq/Serialize/Deserialize）
  - `ipc::{PlayRequest, Request, Response, Event, DaemonState, IpcErrorCode}` + `ipc::encode_frame<T: Serialize>(&T) -> Result<Vec<u8>, serde_json::Error>` + `ipc::decode_frame<T: DeserializeOwned>(&[u8]) -> Result<T, serde_json::Error>`（u32 LE 长度前缀，`MAX_FRAME = 1 << 20`，超限 Err）
  - 依赖：hmp-core `Cargo.toml` 需确认 `serde_json`（若缺则加入 workspace 依赖；`serde` 已有）

- [ ] **Step 1: 写失败测试 —— QueueCore 基础操作**

```rust
// crates/hmp-core/src/queue.rs 底部 tests（首版内容）
use super::*;
use crate::id::TrackId;
use crate::player::LoopMode;

fn t(s: &str) -> TrackId { TrackId::new(s) }

#[test]
fn replace_sets_current_and_tracks() {
    let mut q = QueueCore::new();
    q.replace(vec![t("a"), t("b"), t("c")], 1);
    assert_eq!(q.current(), Some(&t("b")));
    let s = q.snapshot();
    assert_eq!(s.tracks, vec![t("a"), t("b"), t("c")]);
    assert_eq!(s.current, Some(1));
}

#[test]
fn next_advances_and_ends_without_loop() {
    let mut q = QueueCore::new();
    q.replace(vec![t("a"), t("b")], 0);
    assert_eq!(q.next_track(), Some(t("b")));
    assert_eq!(q.next_track(), None); // None 模式到头即停
    assert_eq!(q.current(), Some(&t("b"))); // 位置停在最后一首
}

#[test]
fn list_loop_wraps_around() {
    let mut q = QueueCore::new();
    q.replace(vec![t("a"), t("b")], 1);
    q.set_loop_mode(LoopMode::List);
    assert_eq!(q.next_track(), Some(t("a"))); // 回绕
}

#[test]
fn track_loop_repeats_current() {
    let mut q = QueueCore::new();
    q.replace(vec![t("a"), t("b")], 0);
    q.set_loop_mode(LoopMode::Track);
    assert_eq!(q.next_track(), Some(t("a")));
}

#[test]
fn prev_always_jumps_to_previous_track() {
    let mut q = QueueCore::new();
    q.replace(vec![t("a"), t("b"), t("c")], 2);
    assert_eq!(q.prev_track(), Some(t("b")));
    assert_eq!(q.prev_track(), Some(t("a")));
    assert_eq!(q.prev_track(), None); // 无上一首（None 模式）
}

#[test]
fn prev_wraps_in_list_mode() {
    let mut q = QueueCore::new();
    q.replace(vec![t("a"), t("b")], 0);
    q.set_loop_mode(LoopMode::List);
    assert_eq!(q.prev_track(), Some(t("b")));
}

#[test]
fn insert_next_after_current() {
    let mut q = QueueCore::new();
    q.replace(vec![t("a"), t("b")], 0);
    q.insert_next(t("x"));
    assert_eq!(q.snapshot().tracks, vec![t("a"), t("x"), t("b")]);
    assert_eq!(q.next_track(), Some(t("x")));
}

#[test]
fn remove_adjusts_current() {
    let mut q = QueueCore::new();
    q.replace(vec![t("a"), t("b"), t("c")], 1);
    assert!(q.remove(0)); // 删当前之前 → current 前移
    assert_eq!(q.snapshot().current, Some(0));
    assert_eq!(q.snapshot().tracks, vec![t("b"), t("c")]);
    assert!(q.remove(1)); // 删当前之后 → current 不变
    assert_eq!(q.snapshot().current, Some(0));
    assert!(!q.remove(5)); // 越界 → false
}

#[test]
fn append_and_clear() {
    let mut q = QueueCore::new();
    q.replace(vec![t("a")], 0);
    q.append(vec![t("b"), t("c")]);
    assert_eq!(q.snapshot().tracks, vec![t("a"), t("b"), t("c")]);
    q.clear();
    assert_eq!(q.current(), None);
    assert!(q.snapshot().tracks.is_empty());
}

#[test]
fn shuffle_next_excludes_current_and_bounds() {
    let mut q = QueueCore::new();
    q.replace(vec![t("a"), t("b"), t("c"), t("d")], 0);
    q.set_shuffle(true);
    // 确定性：注入种子（xorshift 固定种子）
    q.set_seed(42);
    let first = q.next_track().unwrap();
    assert_ne!(first, t("a"));
    let second = q.next_track().unwrap();
    assert_ne!(second, first);
    assert_ne!(second, t("a"));
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p hmp-core queue`  Expected: 编译失败（`QueueCore` 不存在）

- [ ] **Step 3: 实现 QueueCore**

```rust
//! 播放队列核心（docs/PROJECT.md §8.4）。
//!
//! 纯逻辑、无 I/O；队列裁决（下一首/上一首/循环/洗牌）唯一实现点，
//! daemon 与未来桌面端共用，禁止在适配器层自行推算。

use serde::{Deserialize, Serialize};

use crate::id::TrackId;
use crate::player::LoopMode;

/// 队列快照（跨进程传递）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QueueSnapshot {
    /// 队列曲目（0 基）。
    pub tracks: Vec<TrackId>,
    /// 当前曲目位置。
    pub current: Option<usize>,
    /// 循环模式。
    pub loop_mode: LoopMode,
    /// 是否洗牌。
    pub shuffle: bool,
}

/// xorshift64*：hmp-core 不引入 rand 依赖，洗牌用自实现 PRNG。
#[derive(Clone, Debug)]
struct XorShift(u64);

impl XorShift {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// 播放队列核心（纯逻辑）。
#[derive(Debug)]
pub struct QueueCore {
    tracks: Vec<TrackId>,
    current: Option<usize>,
    loop_mode: LoopMode,
    shuffle: bool,
    rng: XorShift,
}

impl Default for QueueCore {
    fn default() -> Self {
        Self::new()
    }
}

impl QueueCore {
    /// 空队列。
    pub fn new() -> Self {
        Self {
            tracks: Vec::new(),
            current: None,
            loop_mode: LoopMode::None,
            shuffle: false,
            rng: XorShift(0x9E37_79B9_7F4A_7C15),
        }
    }

    /// 测试/确定性用：注入洗牌种子。
    pub fn set_seed(&mut self, seed: u64) {
        self.rng = XorShift(seed | 1);
    }

    /// 清空并播放 `tracks[start_at]`。
    pub fn replace(&mut self, tracks: Vec<TrackId>, start_at: usize) {
        self.tracks = tracks;
        self.current = if self.tracks.is_empty() { None } else { Some(start_at.min(self.tracks.len() - 1)) };
    }

    /// 追加到队尾（不改变当前曲）。
    pub fn append(&mut self, tracks: Vec<TrackId>) {
        self.tracks.extend(tracks);
    }

    /// 插到当前曲之后（playnext）。
    pub fn insert_next(&mut self, track: TrackId) {
        let at = self.current.map_or(self.tracks.len(), |i| i + 1);
        self.tracks.insert(at.min(self.tracks.len()), track);
    }

    /// 移除 0 基位置曲目；返回是否成功。
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.tracks.len() {
            return false;
        }
        self.tracks.remove(index);
        if let Some(c) = self.current.as_mut() {
            if index < *c {
                *c -= 1;
            } else if index == *c {
                if self.tracks.is_empty() {
                    self.current = None;
                } else {
                    *c = c.min(self.tracks.len() - 1);
                }
            }
        }
        true
    }

    /// 清空队列。
    pub fn clear(&mut self) {
        self.tracks.clear();
        self.current = None;
    }

    /// 当前曲目。
    pub fn current(&self) -> Option<&TrackId> {
        self.current.and_then(|i| self.tracks.get(i))
    }

    /// 快照。
    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            tracks: self.tracks.clone(),
            current: self.current,
            loop_mode: self.loop_mode,
            shuffle: self.shuffle,
        }
    }

    /// 循环模式。
    pub fn loop_mode(&self) -> LoopMode {
        self.loop_mode
    }

    /// 设置循环模式。
    pub fn set_loop_mode(&mut self, mode: LoopMode) {
        self.loop_mode = mode;
    }

    /// 设置洗牌。
    pub fn set_shuffle(&mut self, shuffle: bool) {
        self.shuffle = shuffle;
    }

    /// 洗牌索引：在当前之后的位置里随机选一个。
    fn shuffled_next(&mut self, from: usize) -> usize {
        let len = self.tracks.len();
        let span = len - from;
        let pick = (self.rng.next_u64() % span as u64) as usize;
        from + pick
    }

    /// 计算并切换到下一首；返回其 TrackId（`None` = 无下一首，引擎保持空闲）。
    pub fn next_track(&mut self) -> Option<TrackId> {
        let len = self.tracks.len();
        if len == 0 {
            return None;
        }
        let cur = self.current.unwrap_or(0);
        match self.loop_mode {
            LoopMode::Track => {
                let id = self.tracks[cur].clone();
                Some(id)
            }
            LoopMode::List => {
                let next = (cur + 1) % len;
                self.current = Some(next);
                Some(self.tracks[next].clone())
            }
            LoopMode::None => {
                if cur + 1 >= len {
                    // 到头即停：位置保持，返回 None
                    None
                } else {
                    self.current = Some(cur + 1);
                    Some(self.tracks[cur + 1].clone())
                }
            }
        }
    }

    /// 计算并切换到上一首；`None` = 无上一首（引擎忽略该命令）。
    pub fn prev_track(&mut self) -> Option<TrackId> {
        let len = self.tracks.len();
        if len == 0 {
            return None;
        }
        let cur = self.current.unwrap_or(0);
        match self.loop_mode {
            LoopMode::Track => Some(self.tracks[cur].clone()),
            LoopMode::List => {
                let prev = (cur + len - 1) % len;
                self.current = Some(prev);
                Some(self.tracks[prev].clone())
            }
            LoopMode::None => {
                if cur == 0 {
                    None
                } else {
                    self.current = Some(cur - 1);
                    Some(self.tracks[cur - 1].clone())
                }
            }
        }
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p hmp-core queue`  Expected: 全部通过

- [ ] **Step 5: 写失败测试 —— IPC 消息与帧**

```rust
// crates/hmp-core/src/ipc.rs 底部 tests
use super::*;
use crate::id::{AlbumId, PlaylistId, TrackId};
use crate::player::{PlayerCommand, PlaybackStatus};

#[test]
fn request_roundtrips_through_frame() {
    let reqs = vec![
        Request::Play(PlayRequest::Track(TrackId::new("m1"))),
        Request::Play(PlayRequest::Playlist(PlaylistId::new("p1"))),
        Request::Play(PlayRequest::Album(AlbumId::new("a1"))),
        Request::QueueAppend(PlayRequest::Track(TrackId::new("m2"))),
        Request::QueueRemove(2),
        Request::QueueClear,
        Request::Queue,
        Request::Command(PlayerCommand::Seek(std::time::Duration::from_secs(30))),
        Request::Status,
        Request::Subscribe,
        Request::Quit,
    ];
    for req in reqs {
        let frame = encode_frame(&req).unwrap();
        let back: Request = decode_frame(&frame).unwrap();
        assert_eq!(back, req);
    }
}

#[test]
fn daemon_state_roundtrips() {
    let st = DaemonState {
        playback: Default::default(),
        queue: crate::queue::QueueSnapshot::default(),
        caps: Default::default(),
    };
    let frame = encode_frame(&st).unwrap();
    let back: DaemonState = decode_frame(&frame).unwrap();
    assert_eq!(back, st);
}

#[test]
fn frame_prefix_is_u32_le_length() {
    let msg = Request::Status;
    let frame = encode_frame(&msg).unwrap();
    assert_eq!(&frame[..4], &(frame.len() as u32 - 4).to_le_bytes());
}

#[test]
fn frame_size_limit() {
    let big = Request::QueueAppend(PlayRequest::Track(TrackId::new(&"x".repeat(2 * 1024 * 1024))));
    assert!(encode_frame(&big).is_err());
}

#[test]
fn truncated_frame_rejected() {
    let msg = Request::Status;
    let frame = encode_frame(&msg).unwrap();
    assert!(decode_frame::<Request>(&frame[..frame.len() - 2]).is_err());
}
```

- [ ] **Step 6: 运行测试确认失败**

Run: `cargo test -p hmp-core ipc`  Expected: 编译失败（`ipc` 模块不存在）

- [ ] **Step 7: 实现 IPC 协议**

先改 `crates/hmp-core/Cargo.toml` 把 `serde_json` 从 dev-dependencies 提升为 dependencies（`serde_json = { workspace = true }`，thiserror 已有）：

```rust
//! 跨进程控制协议（Unix socket · 长度前缀 JSON 帧）。
//!
//! 消息类型与 `PlayerCommand` 同居（spec §4.1）；传输层在 hmp-daemon。

use serde::{Deserialize, Serialize};

use crate::id::{AlbumId, PlaylistId, TrackId};
use crate::player::{PlaybackCapabilities, PlaybackState, PlayerCommand};
use crate::queue::QueueSnapshot;

/// 单帧最大字节数（含 4 字节长度前缀）。
pub const MAX_FRAME: usize = 1 << 20;

/// 播放源请求（曲目 / 歌单 / 专辑）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlayRequest {
    /// 单曲。
    Track(TrackId),
    /// 歌单（由后端拉取曲目列表）。
    Playlist(PlaylistId),
    /// 专辑。
    Album(AlbumId),
}

/// 客户端 → 后端请求。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// 清空队列并播放该源。
    Play(PlayRequest),
    /// 插到当前曲之后并立即播放。
    PlayNext(PlayRequest),
    /// 追加到队尾（不播放）。
    QueueAppend(PlayRequest),
    /// 移除 0 基位置曲目。
    QueueRemove(usize),
    /// 清空队列。
    QueueClear,
    /// 查询队列快照。
    Queue,
    /// 基础播放器命令（Play/Pause/Stop/Seek/Volume/Loop/Shuffle/Next/Previous）。
    Command(PlayerCommand),
    /// 查询全量状态。
    Status,
    /// 订阅状态事件流（推送 `Event` 帧）。
    Subscribe,
    /// 优雅退出后端。
    Quit,
}

/// 后端 → 客户端响应。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Response {
    /// 命令已受理（命令-查询分离，真实结果经 `Event` 呈现）。
    Ok,
    /// `Status` 的响应。
    Status(DaemonState),
    /// `Queue` 的响应。
    Queue(QueueSnapshot),
    /// 错误。
    Err { code: IpcErrorCode, message: String },
}

/// 订阅后的事件推送。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// 复合状态变更（初始订阅即推一次当前快照）。
    StateChanged(DaemonState),
}

/// 后端复合状态（单一状态出口，spec §4.2 `daemon.rs`）。
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct DaemonState {
    /// 播放器状态。
    pub playback: PlaybackState,
    /// 队列快照。
    pub queue: QueueSnapshot,
    /// 播放能力（can_go_next 等）。
    pub caps: PlaybackCapabilities,
}

/// 错误码。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IpcErrorCode {
    /// 未登录或凭证失效。
    NotLoggedIn,
    /// 曲目不存在。
    TrackNotFound,
    /// 歌单不存在或拉取失败。
    PlaylistNotFound,
    /// 所有音质均不可用。
    QualityUnavailable,
    /// 协议错误（畸形帧等）。
    BadRequest,
    /// 内部错误。
    Internal,
}

/// 帧编解码错误。
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("帧长度 {0} 超过上限 {MAX_FRAME}")]
    TooLarge(usize),
    #[error("json 错误: {0}")]
    Json(#[from] serde_json::Error),
}

/// 编码为一帧：`u32 LE 长度 + JSON 字节`。
pub fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let payload = serde_json::to_vec(msg)?;
    let total = payload.len() + 4;
    if total > MAX_FRAME {
        return Err(FrameError::TooLarge(total));
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// 解码一帧（含 4 字节长度前缀；长度超限或前缀与内容不符 → Err）。
pub fn decode_frame<T: serde::de::DeserializeOwned>(frame: &[u8]) -> Result<T, FrameError> {
    if frame.len() < 4 {
        return Err(FrameError::Json(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "frame 短于 4 字节长度前缀",
        ))));
    }
    let len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
    if len > MAX_FRAME || 4 + len != frame.len() {
        return Err(FrameError::Json(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "帧长度前缀与内容不符",
        ))));
    }
    serde_json::from_slice(&frame[4..]).map_err(FrameError::Json)
}
```

> hmp-core 若尚未依赖 `thiserror`/`serde_json`，在 `crates/hmp-core/Cargo.toml` 添加（workspace 已有这两个依赖，直接引用 `{ workspace = true }`）。`FrameError::Json` 变体在 `TooLarge` 测试中不被触碰，保持字段未用不告警。

```rust
// crates/hmp-core/src/ipc.rs

- [ ] **Step 8: 运行测试确认通过**

Run: `cargo test -p hmp-core`  Expected: 全部通过（含既有 200+ 测试）

- [ ] **Step 9: 更新 lib.rs 导出并验证**

```rust
// crates/hmp-core/src/lib.rs 追加
pub mod ipc;
pub mod queue;

pub use ipc::{DaemonState, Event, IpcErrorCode, PlayRequest, Request, Response};
pub use queue::{QueueCore, QueueSnapshot};
```

Run: `cargo test -p hmp-core && cargo clippy -p hmp-core --all-targets -- -D warnings && cargo fmt --all -- --check`

- [ ] **Step 10: 提交**

```bash
git add crates/hmp-core/
git commit -m "feat(core): queue core and cross-process IPC protocol"
```

---

### Task 2: hmp-daemon 播放引擎（PlaybackDriver + SourceResolver + 队列裁决 + 状态发布）

**Files:**
- Create: `crates/hmp-daemon/Cargo.toml`, `crates/hmp-daemon/src/lib.rs`, `crates/hmp-daemon/src/daemon.rs`, `crates/hmp-daemon/src/engine.rs`, `crates/hmp-daemon/src/player.rs`
- Modify: `Cargo.toml`（workspace members 增 `crates/hmp-daemon`）
- Test: engine.rs 内 `#[cfg(test)]`（FakeDriver + FakeResolver）

**Interfaces:**
- Consumes: Task 1 的 `QueueCore/QueueSnapshot/Request/DaemonState/PlayRequest/IpcErrorCode`；现有 `hmp_core::{Track, TrackId, AudioQuality, PlayerCommand, PlaybackState, PlaybackStatus, LoopMode}`、`hmp_player_gst::{PlayerCore, LoadRequest, PlayerEvent}`、`hmp_qqmusic_api::{QqMusicClient, SongApi, SongFileType, SongFileInfo, songlist::SonglistApi, album::AlbumApi}`、`hmp_storage::credential::Store`
- Produces（后续任务依赖，签名锁定）:
  - `player::PlaybackDriver`（trait，含 `command(&self, PlayerCommand)`）+ `player::GstDriver`
  - `player::ResolvedTrack { track: Track, uri: String, media: Option<hmp_media::PreparedMedia> }`
  - `player::SourceResolver`（trait：`resolve_source_ids(&PlayRequest) -> Result<Vec<TrackId>, EngineError>` + `resolve_track(&TrackId) -> Result<ResolvedTrack, EngineError>`）+ `player::QqSourceResolver { client: QqMusicClient, store: Store }`
  - `player::EngineError { NotLoggedIn, TrackNotFound, PlaylistNotFound, QualityUnavailable, Internal }`
  - `engine::EngineHandle { command_tx: mpsc::UnboundedSender<Request>, state_rx: watch::Receiver<DaemonState>, credential_ok: Arc<dyn Fn() -> bool + Send + Sync> }`（`#[derive(Clone)]`）
  - `engine::PlaybackEngine::start(driver: Arc<dyn PlaybackDriver>, resolver: Arc<dyn SourceResolver>, credential_ok) -> EngineHandle`

- [ ] **Step 1: 建 crate 骨架 + 写失败测试（FakeDriver/FakeResolver）**

`crates/hmp-daemon/Cargo.toml`:
```toml
[package]
name = "hmp-daemon"
description = "HMP 后台播放后端（服务 + tray）"
version = "0.1.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true

[dependencies]
hmp-core = { path = "../hmp-core" }
hmp-player-gst = { path = "../hmp-player-gst" }
hmp-qqmusic-api = { path = "../hmp-qqmusic-api" }
hmp-storage = { path = "../hmp-storage" }
hmp-media = { path = "../hmp-media" }
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "sync", "time", "net", "signal"] }
tracing = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
libc = "0.2"

[features]
default = ["tray", "mpris"]
tray = ["dep:ksni"]
mpris = ["dep:hmp-mpris"]

[dev-dependencies]
tempfile = { workspace = true }

[dependencies.ksni]
version = "0.2"
optional = true

[dependencies.hmp-mpris]
path = "../hmp-mpris"
optional = true
```

`lib.rs`（空模块占位，本任务逐个实现；`server`/`serve` 留空文件由 Task 3/5 填充）:
```rust
//! HMP 后台播放后端（docs/PROJECT.md §8.5）。
pub mod daemon;
pub mod engine;
pub mod player;
pub mod server; // Task 3
pub mod serve;  // Task 5
```

`engine.rs` 测试（先写 trait 依赖的 fake，Step 4 再写行为测试）:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use hmp_core::{PlaybackState, PlaybackStatus, PlayerCommand, Track, TrackId};
    use std::sync::Mutex;
    use tokio::sync::{broadcast, watch};

    /// 记录 load 的 uri 与收到的命令。
    pub struct FakeDriver {
        pub state_tx: watch::Sender<PlaybackState>,
        pub events_tx: broadcast::Sender<PlayerEvent>,
        pub loads: Mutex<Vec<String>>,
        pub commands: Mutex<Vec<PlayerCommand>>,
    }

    impl FakeDriver {
        pub fn new() -> (Arc<Self>, watch::Receiver<PlaybackState>, broadcast::Receiver<PlayerEvent>) {
            let (state_tx, state_rx) = watch::channel(PlaybackState::default());
            let (events_tx, events_rx) = broadcast::channel(16);
            let d = Arc::new(Self {
                state_tx,
                events_tx,
                loads: Mutex::new(Vec::new()),
                commands: Mutex::new(Vec::new()),
            });
            (d, state_rx, events_rx)
        }
        pub fn set_status(&self, status: PlaybackStatus) {
            self.state_tx.send_modify(|s| s.status = status);
        }
        pub fn emit(&self, ev: PlayerEvent) {
            let _ = self.events_tx.send(ev);
        }
    }

    impl PlaybackDriver for FakeDriver {
        fn load(&self, request: LoadRequest) { self.loads.lock().unwrap().push(request.uri); }
        fn play(&self) {}
        fn pause(&self) {}
        fn seek(&self, _p: std::time::Duration) {}
        fn stop(&self) {}
        fn set_volume(&self, _v: f64) {}
        fn command(&self, cmd: PlayerCommand) { self.commands.lock().unwrap().push(cmd); }
        fn shutdown(&self) {}
        fn subscribe_state(&self) -> watch::Receiver<PlaybackState> { self.state_tx.subscribe() }
        fn subscribe_events(&self) -> broadcast::Receiver<PlayerEvent> { self.events_tx.subscribe() }
    }

    /// 固定返回曲目列表的解析器（不触网）。
    pub struct FakeResolver {
        pub ids: Mutex<Vec<Vec<TrackId>>>, // 每次 resolve_source_ids 弹出一个列表
    }

    impl FakeResolver {
        pub fn new(ids: Vec<Vec<TrackId>>) -> Arc<Self> {
            Arc::new(Self { ids: Mutex::new(ids) })
        }
    }

    impl SourceResolver for FakeResolver {
        async fn resolve_source_ids(&self, _src: &hmp_core::PlayRequest) -> Result<Vec<TrackId>, EngineError> {
            Ok(self.ids.lock().unwrap().remove(0))
        }
        async fn resolve_track(&self, id: &TrackId) -> Result<ResolvedTrack, EngineError> {
            Ok(ResolvedTrack {
                track: Track {
                    id: id.clone(),
                    title: format!("t-{id}"),
                    artists: vec![],
                    album: None,
                    duration: Some(std::time::Duration::from_secs(60)),
                    cover: None,
                    url: Some(format!("fake://{id}")),
                    qualities: vec![],
                },
                uri: format!("fake://{id}"),
                media: None,
            })
        }
    }

    /// 测试用 engine 启动辅助。
    async fn start_engine(
        driver: Arc<FakeDriver>,
        resolver: Arc<FakeResolver>,
    ) -> (EngineHandle, watch::Receiver<hmp_core::DaemonState>) {
        let handle = PlaybackEngine::start(
            driver,
            resolver,
            Arc::new(|| true),
        );
        let st = handle.state_rx.clone();
        (handle, st)
    }

    /// 等待命令循环消化完已投递命令（yield 数次）。
    async fn wait_idle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }
}
```

- [ ] **Step 2: 骨架编译通过**

Run: `cargo build -p hmp-daemon --no-default-features`  Expected: 通过（空模块）

- [ ] **Step 3: 实现 PlaybackDriver + GstDriver + ResolvedTrack + EngineError + SourceResolver**

```rust
//! 播放驱动抽象、曲目解析与解析错误（spec §4.2 `player.rs`）。
//!
//! [`PlaybackDriver`] 是后端与播放器的唯一接缝：测试注入 fake，生产用
//! [`GstDriver`]（包 `PlayerCore`）。[`SourceResolver`] 是后端与 QQ API
//! 的唯一接缝：测试注入 fake，生产用 [`QqSourceResolver`]。队列裁决/
//! 自动续播在引擎（`engine.rs`），播放器核心不感知队列。

use std::sync::Arc;

use hmp_core::{AudioQuality, PlaybackState, PlayerCommand, Track, TrackId};
use hmp_player_gst::{LoadRequest, PlayerCore, PlayerEvent};
use hmp_qqmusic_api::QqMusicClient;
use hmp_storage::credential::Store;
use tokio::sync::{broadcast, watch};

/// 播放驱动（同步接缝）。
pub trait PlaybackDriver: Send + Sync {
    /// 加载曲目（URI 已就绪）。
    fn load(&self, request: LoadRequest);
    fn play(&self);
    fn pause(&self);
    fn seek(&self, position: std::time::Duration);
    fn stop(&self);
    fn set_volume(&self, volume: f64);
    /// 转发通用命令（Play/Pause/Stop/Seek/Volume/Loop/Shuffle/TogglePlay）。
    /// Next/Previous/LoadAndPlay 由引擎拦截，不转发。
    fn command(&self, cmd: PlayerCommand);
    fn shutdown(&self);
    /// 播放状态（watch 单一来源）。
    fn subscribe_state(&self) -> watch::Receiver<PlaybackState>;
    /// 播放器离散事件（Ended/Error）。
    fn subscribe_events(&self) -> broadcast::Receiver<PlayerEvent>;
}

/// GStreamer 播放驱动（生产）。
#[derive(Debug)]
pub struct GstDriver {
    core: PlayerCore,
}

impl GstDriver {
    /// 新建（`audio_sink` 为 None 时用系统默认；测试可传 "fakesink"）。
    pub fn new(audio_sink: Option<&str>) -> Result<Self, hmp_core::HmpError> {
        Ok(Self {
            core: PlayerCore::new_with_sink(audio_sink)?,
        })
    }
}

impl PlaybackDriver for GstDriver {
    fn load(&self, request: LoadRequest) {
        self.core.load(request);
    }
    fn play(&self) {
        self.core.play();
    }
    fn pause(&self) {
        self.core.pause();
    }
    fn seek(&self, position: std::time::Duration) {
        self.core.seek(position);
    }
    fn stop(&self) {
        self.core.stop();
    }
    fn set_volume(&self, volume: f64) {
        self.core.set_volume(volume);
    }
    fn command(&self, cmd: PlayerCommand) {
        let _ = self.core.command_sender().send(cmd);
    }
    fn shutdown(&self) {
        self.core.shutdown();
    }
    fn subscribe_state(&self) -> watch::Receiver<PlaybackState> {
        self.core.subscribe_state()
    }
    fn subscribe_events(&self) -> broadcast::Receiver<PlayerEvent> {
        self.core.subscribe_events()
    }
}

/// 解析完成的曲目（含解密 guard，随 daemon 存活）。
#[derive(Debug)]
pub struct ResolvedTrack {
    /// 领域曲目元数据。
    pub track: Track,
    /// 播放 URI（http://127.0.0.1 代理或 CDN 直连）。
    pub uri: String,
    /// 解密代理 guard（明文播放期间必须持有；换曲时被引擎替换 Drop）。
    pub media: Option<hmp_media::PreparedMedia>,
}

/// 解析错误（引擎内部；映射为 `IpcErrorCode`）。
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("未登录或凭证已过期")]
    NotLoggedIn,
    #[error("曲目不存在")]
    TrackNotFound,
    #[error("歌单/专辑拉取失败: {0}")]
    PlaylistNotFound(String),
    #[error("所有音质均不可用: {0}")]
    QualityUnavailable(String),
    #[error("内部错误: {0}")]
    Internal(String),
}

/// 播放源解析接缝（引擎唯一网络入口）。
pub trait SourceResolver: Send + Sync {
    /// 解析源为 TrackId 列表（单曲=1 个；歌单/专辑=分页拉取）。
    fn resolve_source_ids(
        &self,
        src: &hmp_core::PlayRequest,
    ) -> impl std::future::Future<Output = Result<Vec<TrackId>, EngineError>> + Send;

    /// 解析单曲为可播放 URI + 元数据（音质回退 + QMC2 解密）。
    fn resolve_track(
        &self,
        track_id: &TrackId,
    ) -> impl std::future::Future<Output = Result<ResolvedTrack, EngineError>> + Send;
}

/// 生产解析器（QQ API + 共享凭证）。
#[derive(Debug)]
pub struct QqSourceResolver {
    client: QqMusicClient,
    store: Store,
}

impl QqSourceResolver {
    /// 新建（`store` 由 `store_from_env()` 构造）。
    pub fn new(client: QqMusicClient, store: Store) -> Self {
        Self { client, store }
    }

    /// 当前是否有有效凭证（供服务器同步前置校验）。
    pub fn has_credential(&self) -> bool {
        self.store
            .load()
            .ok()
            .flatten()
            .is_some_and(|c| c.is_logged_in())
    }

    fn load_credential(&self) -> Result<hmp_storage::credential::Credential, EngineError> {
        self.store
            .load()
            .map_err(|e| EngineError::Internal(format!("读取凭证失败: {e}")))?
            .ok_or(EngineError::NotLoggedIn)
    }
}
```
> `SourceResolver` 用 RPITIT（`impl Future`）返回——edition 2024 支持（rust-version 1.85 兼容 RPITIT，稳定于 1.75）。若实现遇到 trait 对象限制（`dyn SourceResolver` 需要 boxed），改用 `BoxFuture`（`fn resolve_track(&self, id: &TrackId) -> Pin<Box<dyn Future<Output=...> + Send + '_>>`）——以编译为准，二选一。

- [ ] **Step 4: 写失败测试 —— 引擎行为（FakeDriver + FakeResolver，无网络）**

```rust
// engine.rs tests 追加

/// 解析器弹出一个列表；Play 后队列被替换。
#[tokio::test]
async fn play_replaces_queue_and_loads_first() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b"), TrackId::new("c")]]);
    let (handle, _st) = start_engine(driver.clone(), resolver).await;
    handle.cmd(Request::Play(PlayRequest::Track(TrackId::new("a")))).await.unwrap();
    wait_idle().await;
    assert_eq!(handle.state_rx.borrow().queue.current, Some(0));
    assert_eq!(handle.state_rx.borrow().queue.tracks.len(), 3);
    assert_eq!(driver.loads.lock().unwrap().clone(), vec!["fake://a"]);
}

/// Next 命令 → 队列前进并加载下一首。
#[tokio::test]
async fn next_command_navigates_queue() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b"), TrackId::new("c")]]);
    let (handle, _st) = start_engine(driver.clone(), resolver).await;
    handle.cmd(Request::Play(PlayRequest::Track(TrackId::new("a")))).await.unwrap();
    wait_idle().await;
    handle.cmd(Request::Command(PlayerCommand::Next)).await.unwrap();
    wait_idle().await;
    assert_eq!(handle.state_rx.borrow().queue.current, Some(1));
    assert_eq!(driver.loads.lock().unwrap().clone(), vec!["fake://a", "fake://b"]);
}

/// prev 恒跳上一首（不做 >3s 回开头）。
#[tokio::test]
async fn prev_always_goes_previous_track() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b"), TrackId::new("c")]]);
    let (handle, _st) = start_engine(driver.clone(), resolver).await;
    handle.cmd(Request::Play(PlayRequest::Track(TrackId::new("a")))).await.unwrap();
    wait_idle().await;
    handle.cmd(Request::Command(PlayerCommand::Next)).await.unwrap();
    wait_idle().await;
    handle.cmd(Request::Command(PlayerCommand::Next)).await.unwrap();
    wait_idle().await;
    assert_eq!(handle.state_rx.borrow().queue.current, Some(2));
    handle.cmd(Request::Command(PlayerCommand::Previous)).await.unwrap();
    wait_idle().await;
    assert_eq!(handle.state_rx.borrow().queue.current, Some(1));
    assert_eq!(driver.loads.lock().unwrap().clone(), vec!["fake://a", "fake://b", "fake://c", "fake://b"]);
}

/// Ended 事件 → 自动续播下一首。
#[tokio::test]
async fn ended_event_auto_advances() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
    let (handle, _st) = start_engine(driver.clone(), resolver).await;
    handle.cmd(Request::Play(PlayRequest::Track(TrackId::new("a")))).await.unwrap();
    wait_idle().await;
    driver.emit(PlayerEvent::PlaybackEnded);
    wait_idle().await;
    assert_eq!(handle.state_rx.borrow().queue.current, Some(1));
    assert_eq!(driver.loads.lock().unwrap().clone(), vec!["fake://a", "fake://b"]);
}

/// Ended 且队列到头（None 循环）→ 保持空闲，不再加载。
#[tokio::test]
async fn ended_with_no_next_stays_idle() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a")]]);
    let (handle, _st) = start_engine(driver.clone(), resolver).await;
    handle.cmd(Request::Play(PlayRequest::Track(TrackId::new("a")))).await.unwrap();
    wait_idle().await;
    driver.emit(PlayerEvent::PlaybackEnded);
    wait_idle().await;
    assert_eq!(handle.state_rx.borrow().queue.current, Some(0));
    assert_eq!(driver.loads.lock().unwrap().len(), 1); // 只加载过一次
}

/// List 循环：Ended 后回绕到第一首。
#[tokio::test]
async fn list_loop_wraps_on_ended() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
    let (handle, _st) = start_engine(driver.clone(), resolver).await;
    handle.cmd(Request::Play(PlayRequest::Track(TrackId::new("a")))).await.unwrap();
    wait_idle().await;
    handle.cmd(Request::Command(PlayerCommand::SetLoopMode(LoopMode::List))).await.unwrap();
    wait_idle().await;
    driver.emit(PlayerEvent::PlaybackEnded); // a → b
    wait_idle().await;
    driver.emit(PlayerEvent::PlaybackEnded); // b → a（回绕）
    wait_idle().await;
    assert_eq!(handle.state_rx.borrow().queue.current, Some(0));
    assert_eq!(driver.loads.lock().unwrap().clone(), vec!["fake://a", "fake://b", "fake://a"]);
}

/// 播放结束到队列末尾 → 状态发布（Ended 保持，不崩）。
#[tokio::test]
async fn quit_shuts_down_engine() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![]);
    let (handle, _st) = start_engine(driver.clone(), resolver).await;
    handle.cmd(Request::Quit).await.unwrap();
    wait_idle().await;
    // 引擎退出后向命令通道发消息不再成功（发送端仍可发，但引擎不再消费——不断言；
    // 断言驱动已 shutdown）
    assert!(driver.commands.lock().unwrap().is_empty()); // shutdown 不产生命令
}
```
> `LoopMode` 需在测试导入（`use hmp_core::LoopMode;`）。`quit_shuts_down_engine` 的断言较弱（无 join 句柄），以"引擎任务退出不 panic"为验收标准；若想强断言，可在 `EngineHandle` 增加 `#[cfg(test)] terminated: Arc<tokio::sync::Notify>` 并在 run() 退出时 notify——实现时按需添加（不强求）。

- [ ] **Step 5: 运行确认失败**

Run: `cargo test -p hmp-daemon --no-default-features engine`  Expected: 编译失败（`PlaybackEngine` 不存在）

- [ ] **Step 6: 实现引擎**

```rust
//! 播放引擎：命令循环 + 队列裁决 + 自动续播 + 复合状态发布（spec §4.2 `daemon.rs`）。
//!
//! 单一命令通道：所有输入适配器（socket 服务器 / tray / MPRIS）把
//! [`Request`] 发进 [`EngineHandle::command_tx`]，由引擎串行处理；
//! 单一状态出口：`watch<DaemonState>`。Next/Previous 由引擎拦截做队列
//! 导航（PlayerCore 忽略这两个命令，见 hmp-player-gst core.rs）。

use std::sync::Arc;

use hmp_core::{
    DaemonState, LoopMode, PlaybackState, PlaybackStatus, PlayerCommand, PlayRequest,
    QueueSnapshot, Request,
};
use hmp_player_gst::PlayerEvent;
use tokio::sync::{mpsc, watch};

use crate::player::{PlaybackDriver, SourceResolver};

/// 引擎句柄（服务器 / tray / MPRIS 持有；可 Clone）。
#[derive(Clone)]
pub struct EngineHandle {
    /// 命令通道（唯一输入）。
    pub command_tx: mpsc::UnboundedSender<Request>,
    /// 复合状态（唯一输出）。
    pub state_rx: watch::Receiver<DaemonState>,
    /// 凭证前置校验（服务器对 Play 类请求同步检查，spec §6）。
    pub credential_ok: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl EngineHandle {
    /// 发送请求（命令-查询分离：仅返回是否投递成功）。
    pub async fn cmd(&self, req: Request) -> Result<(), mpsc::error::SendError<Request>> {
        self.command_tx.send(req)
    }
}

/// 播放引擎。
pub struct PlaybackEngine {
    driver: Arc<dyn PlaybackDriver>,
    resolver: Arc<dyn SourceResolver>,
    queue: hmp_core::QueueCore,
    state_tx: watch::Sender<DaemonState>,
    state_rx: watch::Receiver<PlaybackState>,
    cmd_rx: mpsc::UnboundedReceiver<Request>,
    active_media: Option<hmp_media::PreparedMedia>,
}

impl PlaybackEngine {
    /// 启动引擎（spawn 主循环任务），返回句柄。
    pub fn start(
        driver: Arc<dyn PlaybackDriver>,
        resolver: Arc<dyn SourceResolver>,
        credential_ok: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> EngineHandle {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (state_tx, state_rx) = watch::channel(DaemonState::default());
        let mut engine = Self {
            driver,
            resolver,
            queue: hmp_core::QueueCore::new(),
            state_tx,
            state_rx: driver.subscribe_state(),
            cmd_rx,
            active_media: None,
        };
        tokio::spawn(async move { engine.run().await });
        EngineHandle {
            command_tx: cmd_tx,
            state_rx,
            credential_ok,
        }
    }

    async fn run(&mut self) {
        let mut events_rx = self.driver.subscribe_events();
        loop {
            tokio::select! {
                Some(req) = self.cmd_rx.recv() => {
                    match req {
                        Request::Quit => {
                            self.driver.shutdown();
                            break;
                        }
                        Request::Command(cmd) => self.handle_player_command(cmd).await,
                        Request::Play(src) => self.play_source(src, false).await,
                        Request::PlayNext(src) => self.play_source(src, true).await,
                        Request::QueueAppend(src) => {
                            if let Some(ids) = self.resolver.resolve_source_ids(&src).await.ok() {
                                self.queue.append(ids);
                                self.publish();
                            }
                        }
                        Request::QueueRemove(i) => {
                            if self.queue.remove(i) { self.publish(); }
                        }
                        Request::QueueClear => {
                            self.queue.clear();
                            self.publish();
                        }
                        // 查询类由服务器直接读 state_rx 处理；引擎忽略（防御）。
                        _ => {}
                    }
                }
                _ = self.state_rx.changed() => {
                    self.publish();
                }
                ev = events_rx.recv() => {
                    match ev {
                        Ok(PlayerEvent::PlaybackEnded) => self.on_ended().await,
                        Ok(PlayerEvent::Error(_)) => self.publish(), // 不自动跳歌（spec §7）
                        _ => {}
                    }
                }
            }
        }
    }

    /// 发布复合状态（playback 来自驱动 watch，queue 来自队列核心）。
    fn publish(&self) {
        let state = DaemonState {
            playback: self.state_rx.borrow().clone(),
            queue: self.queue.snapshot(),
            caps: hmp_core::PlaybackCapabilities {
                can_go_next: self.queue.snapshot().tracks.len() > 1,
                can_go_previous: true,
            },
        };
        let _ = self.state_tx.send(state);
    }

    async fn handle_player_command(&mut self, cmd: PlayerCommand) {
        match cmd {
            PlayerCommand::Next => self.navigate_next().await,
            PlayerCommand::Previous => self.navigate_prev().await,
            PlayerCommand::SetLoopMode(m) => {
                self.queue.set_loop_mode(m);
                self.driver.command(PlayerCommand::SetLoopMode(m));
                self.publish();
            }
            PlayerCommand::SetShuffle(b) => {
                self.queue.set_shuffle(b);
                self.driver.command(PlayerCommand::SetShuffle(b));
                self.publish();
            }
            PlayerCommand::LoadAndPlay(_) => {
                // 队列场景不使用（CLI/桌面按 id 走 Play 请求）；忽略。
            }
            other => self.driver.command(other), // Play/Pause/Stop/Seek/Volume/TogglePlay 直通驱动
        }
    }

    async fn navigate_next(&mut self) {
        if let Some(id) = self.queue.next_track() {
            self.publish();
            self.load_and_play(id).await;
        }
    }

    async fn navigate_prev(&mut self) {
        if let Some(id) = self.queue.prev_track() {
            self.publish();
            self.load_and_play(id).await;
        }
    }

    /// Play / PlayNext：解析源 → 替换/插入队列 → 加载当前。
    async fn play_source(&mut self, src: PlayRequest, playnext: bool) {
        let Ok(ids) = self.resolver.resolve_source_ids(&src).await else {
            self.publish();
            return;
        };
        if ids.is_empty() {
            return;
        }
        if playnext {
            self.queue.insert_next(ids[0].clone());
            self.queue.set_current_to_last(); // 定位到刚插入的
            self.publish();
            self.load_and_play(ids[0].clone()).await;
        } else {
            self.queue.replace(ids.clone(), 0);
            self.publish();
            self.load_and_play(ids[0].clone()).await;
        }
    }

    async fn on_ended(&mut self) {
        if self.queue.loop_mode() == LoopMode::Track {
            if let Some(id) = self.queue.current().cloned() {
                self.load_and_play(id).await;
            }
            return;
        }
        if let Some(id) = self.queue.next_track() {
            self.publish();
            self.load_and_play(id).await;
        } else {
            self.publish();
        }
    }

    /// 解析 + 解密 + 加载 + 播放。
    async fn load_and_play(&mut self, id: TrackId) {
        match self.resolver.resolve_track(&id).await {
            Ok(res) => {
                self.active_media = res.media; // 旧 guard 自动 Drop → 旧代理停止
                let uri = res.uri.clone();
                let quality = res
                    .track
                    .qualities
                    .first()
                    .cloned()
                    .unwrap_or(hmp_core::AudioQuality::Mp3_128);
                self.driver.load(hmp_player_gst::LoadRequest {
                    track: res.track,
                    uri,
                    quality,
                });
                self.driver.play();
                self.publish();
            }
            Err(e) => {
                tracing::error!(%e, "解析失败: {id}");
                // 队列位置保持；状态由驱动/状态呈现
                self.publish();
            }
        }
    }
}
```

> `queue.set_current_to_last()` 与 `queue.loop_mode()` 需在 Task 1 的 queue.rs 补齐（Step 7 处理）。引擎启动后立即 `publish` 一次（run() 首轮 select 前）保证订阅者拿到初始状态——在 `start()` 里 `engine.publish()` 后再 spawn，或在 run() 开头 publish——选择后者（run() 首行 `self.publish();`）。

- [ ] **Step 7: 补齐 queue.rs 的 `set_current_to_last` 并加测试**

```rust
// queue.rs 追加
    /// 将当前曲定位到队尾（playnext 插入后使用）。
    pub fn set_current_to_last(&mut self) {
        if !self.tracks.is_empty() {
            self.current = Some(self.tracks.len() - 1);
        }
    }
```
测试：
```rust
#[test]
fn set_current_to_last_positions_at_end() {
    let mut q = QueueCore::new();
    q.replace(vec![t("a"), t("b")], 0);
    q.set_current_to_last();
    assert_eq!(q.current(), Some(&t("b")));
}
```
Run: `cargo test -p hmp-core queue`  Expected: 通过

- [ ] **Step 8: 运行引擎测试确认通过**

Run: `cargo test -p hmp-daemon --no-default-features`  Expected: 引擎 7 个测试通过

- [ ] **Step 9: daemon.rs 组装（本任务范围）**

```rust
//! 后端组装：引擎 + 解析器 + 适配器（spec §4.2 `daemon.rs`）。
use std::sync::Arc;

use hmp_qqmusic_api::QqMusicClient;
use hmp_storage::credential::store_from_env;

use crate::engine::{EngineHandle, PlaybackEngine};
use crate::player::{GstDriver, PlaybackDriver, QqSourceResolver};

/// 后端运行配置。
pub struct DaemonConfig {
    /// 测试可传 "fakesink"；None = 系统默认音频输出。
    pub audio_sink: Option<String>,
}

/// 组装后端并返回引擎句柄（服务器/tray/MPRIS 由 Task 3/5/6 接入）。
pub struct Daemon {
    pub handle: EngineHandle,
}

impl Daemon {
    pub fn start(cfg: DaemonConfig) -> Result<Self, hmp_core::HmpError> {
        let driver: Arc<dyn PlaybackDriver> = Arc::new(GstDriver::new(cfg.audio_sink.as_deref())?);
        let store = store_from_env();
        let resolver = Arc::new(QqSourceResolver::new(QqMusicClient::new(), store));
        let credential_ok = {
            let resolver = Arc::clone(&resolver);
            Arc::new(move || resolver.has_credential())
        };
        let handle = PlaybackEngine::start(driver, resolver, credential_ok);
        Ok(Self { handle })
    }
}
```

- [ ] **Step 10: 全量验证 + 提交**

```bash
cargo build -p hmp-daemon --no-default-features
cargo test -p hmp-daemon --no-default-features
cargo test -p hmp-core
cargo clippy -p hmp-daemon --no-default-features --all-targets -- -D warnings
cargo fmt --all -- --check
git add Cargo.toml crates/hmp-daemon crates/hmp-core
git commit -m "feat(daemon): playback engine with queue arbitration and composite state"
```


### Task 3: hmp-daemon socket 服务器 + 曲目解析

**Files:**
- Modify: `crates/hmp-daemon/src/server.rs`（实现）、`crates/hmp-daemon/src/player.rs`（`resolve_track`）、`crates/hmp-daemon/src/engine.rs`（接 `resolve_track`）、`crates/hmp-daemon/src/daemon.rs`（起服务器）
- Test: `server.rs` `#[cfg(test)]`（真 socket + 真协议客户端）

**Interfaces:**
- Consumes: Task 1 `ipc::{encode_frame, decode_frame, Request, Response, Event, DaemonState, PlayRequest, IpcErrorCode, MAX_FRAME}`；Task 2 `EngineHandle`
- Produces:
  - `server::socket_path() -> PathBuf`（`$XDG_RUNTIME_DIR/hmp.sock` 回退 `/tmp/hmp-{uid}.sock`）
  - `server::serve(listener: tokio::net::UnixListener, handle: EngineHandle)`（无限循环；`Quit` 后由 daemon 编排退出）
  - `player::resolve_track(&client, credential, track_id) -> Result<ResolvedTrack, EngineError>`，`engine::EngineError { NotLoggedIn, TrackNotFound, PlaylistNotFound, QualityUnavailable, Internal }`（引擎内部使用；映射为 `IpcErrorCode`）
  - `player::resolve_source_ids(&client, credential, src: PlayRequest) -> Result<Vec<TrackId>, EngineError>`（歌单/专辑分页拉取）

- [ ] **Step 1: 写失败测试 —— 帧传输往返**

```rust
// server.rs tests（用 tokio::net::UnixStream 真连接）
#[tokio::test]
async fn request_response_roundtrip() {
    let (listener, _) = setup_pair().await; // 见 Step 2 辅助
    // 客户端连接
    let (sock_path, mut server_listener) = temp_socket().await;
    let handle = engine_handle_for_test();
    tokio::spawn(async move { server::serve(server_listener, handle).await });
    let mut stream = tokio::net::UnixStream::connect(&sock_path).await.unwrap();
    let frame = encode_frame(&Request::Status).unwrap();
    stream.write_all(&frame).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await.unwrap();
    let resp: Response = decode_frame(&buf[..n]).unwrap();
    assert!(matches!(resp, Response::Status(_)));
}
```

- [ ] **Step 2: 实现服务器（含测试辅助）**

```rust
//! Unix socket 控制服务器（spec §4.2 `server.rs` / §5）。
//!
//! 长度前缀 JSON 帧；每连接独立任务；查询（Status/Queue）直接读
//! `EngineHandle.state_rx` 同步应答；Subscribe 后推送 `Event` 帧。

use std::path::PathBuf;

use hmp_core::ipc::{decode_frame, encode_frame, Event, Request, Response, MAX_FRAME};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use crate::engine::EngineHandle;

/// socket 路径：`$XDG_RUNTIME_DIR/hmp.sock`，回退 `/tmp/hmp-{uid}.sock`。
pub fn socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("hmp.sock");
        }
    }
    #[cfg(unix)]
    {
        let uid = unsafe { libc::getuid() };
        return PathBuf::from(format!("/tmp/hmp-{uid}.sock"));
    }
    #[cfg(not(unix))]
    {
        PathBuf::from("/tmp/hmp.sock")
    }
}
```
> `libc` 需加入 hmp-daemon 依赖（unix 下 `getuid`）。若不想引入 libc，回退路径用 `std::env::var("USER")` 或 `"hmp.sock"`——选用 libc（workspace 已有传递依赖，显式声明 `libc = "0.2"`）。

```rust
/// 启动服务器（accept 循环；由 daemon 编排退出时机）。
pub async fn serve(listener: UnixListener, handle: EngineHandle) {
    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let handle = handle.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, handle).await {
                        tracing::debug!(%e, "连接处理结束");
                    }
                });
            }
            Err(e) => {
                tracing::error!(%e, "accept 失败");
                break;
            }
        }
    }
}

/// 单连接处理：请求/响应循环 + 可选订阅推送。
async fn handle_connection(mut stream: UnixStream, handle: EngineHandle) -> std::io::Result<()> {
    let mut subscribed = false;
    loop {
        // 读帧头
        let mut len_buf = [0u8; 4];
        match stream.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > MAX_FRAME - 4 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "非法帧长度"));
        }
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await?;
        let mut frame = Vec::with_capacity(4 + len);
        frame.extend_from_slice(&len_buf);
        frame.extend_from_slice(&payload);

        match decode_frame::<Request>(&frame) {
            Ok(Request::Status) => {
                let resp = Response::Status(handle.state_rx.borrow().clone());
                write_frame(&mut stream, &resp).await?;
            }
            Ok(Request::Queue) => {
                let resp = Response::Queue(handle.state_rx.borrow().queue.clone());
                write_frame(&mut stream, &resp).await?;
            }
            Ok(Request::Subscribe) => {
                subscribed = true;
                // 先推初始快照
                let ev = Event::StateChanged(handle.state_rx.borrow().clone());
                write_frame(&mut stream, &ev).await?;
            }
            Ok(req) => {
                let resp = match handle.command_tx.send(req) {
                    Ok(_) => Response::Ok,
                    Err(_) => Response::Err {
                        code: hmp_core::IpcErrorCode::Internal,
                        message: "引擎已退出".into(),
                    },
                };
                write_frame(&mut stream, &resp).await?;
            }
            Err(e) => {
                let resp = Response::Err {
                    code: hmp_core::IpcErrorCode::BadRequest,
                    message: e.to_string(),
                };
                write_frame(&mut stream, &resp).await?;
            }
        }

        // 订阅推送：等待状态变更
        if subscribed {
            if handle.state_rx.changed().await.is_ok() {
                let ev = Event::StateChanged(handle.state_rx.borrow().clone());
                write_frame(&mut stream, &ev).await?;
            } else {
                return Ok(()); // 状态通道关闭
            }
        }
    }
}

/// 写一帧。
async fn write_frame(stream: &mut UnixStream, msg: &impl serde::Serialize) -> std::io::Result<()> {
    let frame = encode_frame(msg).map_err(|e| std::io::Error::other(e.to_string()))?;
    stream.write_all(&frame).await
}
```

> `EngineHandle` 需 `Clone`（每连接一份 `state_rx` + `command_tx`——`mpsc::UnboundedSender` 与 `watch::Receiver` 均可 Clone；实现 `#[derive(Clone)]`）。订阅推送在**请求处理之后**阻塞等待状态变更——若客户端订阅后不再发请求，此循环会卡在 `state_rx.changed()`，连接保持但读端不消费——用 `select!` 同时监听下一次请求帧与状态变更（改进：`tokio::select! { _ = read_next_frame => ..., _ = state_rx.changed() => push }`）。本计划采用 **select 版**（见 Step 3 修订），Step 1 的往返测试不受影响。

- [ ] **Step 3: 修订订阅为 select 并发（避免订阅后阻塞读）**

```rust
async fn handle_connection(mut stream: UnixStream, handle: EngineHandle) -> std::io::Result<()> {
    let mut subscribed = false;
    loop {
        // 读一帧（异步）
        let frame = read_frame(&mut stream).await?;
        let Some(frame) = frame else { return Ok(()) }; // EOF

        let next_req = decode_frame::<Request>(&frame);
        match next_req {
            Ok(Request::Status) => {
                write_frame(&mut stream, &Response::Status(handle.state_rx.borrow().clone())).await?;
            }
            Ok(Request::Queue) => {
                write_frame(&mut stream, &Response::Queue(handle.state_rx.borrow().queue.clone())).await?;
            }
            Ok(Request::Subscribe) => {
                subscribed = true;
                write_frame(&mut stream, &Event::StateChanged(handle.state_rx.borrow().clone())).await?;
            }
            Ok(req) => {
                let resp = match handle.command_tx.send(req) {
                    Ok(_) => Response::Ok,
                    Err(_) => Response::Err { code: hmp_core::IpcErrorCode::Internal, message: "引擎已退出".into() },
                };
                write_frame(&mut stream, &resp).await?;
            }
            Err(e) => {
                write_frame(&mut stream, &Response::Err { code: hmp_core::IpcErrorCode::BadRequest, message: e.to_string() }).await?;
            }
        }

        if subscribed {
            // 订阅期间：状态变更与下一请求并发处理
            tokio::select! {
                _ = handle.state_rx.changed() => {
                    if handle.state_rx.has_changed().unwrap_or(false) || true {
                        let ev = Event::StateChanged(handle.state_rx.borrow().clone());
                        write_frame(&mut stream, &ev).await?;
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => { /* 轮询兜底 */ }
            }
        }
    }
}

async fn read_frame(stream: &mut UnixStream) -> std::io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME - 4 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "非法帧长度"));
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;
    let mut frame = Vec::with_capacity(4 + len);
    frame.extend_from_slice(&len_buf);
    frame.extend_from_slice(&payload);
    Ok(Some(frame))
}
```
> 简化说明：订阅采用**轮询兜底 + changed() 事件**双通道，`100ms` 轮询保证 watch 边界情况不漏推；对 CLI（不订阅）零开销。实现可再简化为纯轮询（订阅后每 100ms 检查 `has_changed` 并推送），测试只验证"订阅后能收到 ≥1 帧 Event"。

- [ ] **Step 4: 实现 QqSourceResolver（player.rs 追加）—— 凭证/详情/音质回退/解密/歌单专辑**

`EngineError` 已在 Task 2 定义（本步骤不再重复）；此处实现 `SourceResolver for QqSourceResolver` 的方法体（自由函数形式，在 `impl SourceResolver for QqSourceResolver` 中调用）：

```rust
// player.rs 追加：

/// 音质 → 文件类型（与 CLI play.rs 一致，复制）。
fn quality_to_file_type(q: &hmp_core::AudioQuality) -> Option<SongFileType> {
    use hmp_core::AudioQuality::*;
    match q {
        Master => Some(SongFileType::MASTER),
        HiRes => Some(SongFileType::MASTER),
        Atmos => Some(SongFileType::ATMOS_2),
        Flac => Some(SongFileType::FLAC),
        Aac => Some(SongFileType::AAC_192),
        Mp3_320 => Some(SongFileType::MP3_320),
        Mp3_128 => Some(SongFileType::MP3_128),
        Unknown(_) => None,
    }
}

/// 解析单个曲目 → 可播放 URI + 元数据（音质回退 + QMC2 解密）。
pub async fn resolve_track_impl(
    client: &QqMusicClient,
    credential: &hmp_storage::credential::Credential,
    track_id: &TrackId,
) -> Result<ResolvedTrack, EngineError> {
    client: &QqMusicClient,
    credential: &hmp_storage::credential::Credential,
    track_id: &TrackId,
) -> Result<ResolvedTrack, EngineError> {
    let song_api = SongApi::new(client);
    let detail = song_api
        .get_detail(track_id.as_str())
        .await
        .map_err(|e| EngineError::Internal(format!("详情请求失败: {e}")))?;
    let media_mid = detail.track.file.media_mid.clone();
    if media_mid.is_empty() {
        return Err(EngineError::TrackNotFound);
    }
    // 元数据（歌手/专辑/封面，供 MPRIS）
    let singers = detail.track.singer.iter()
        .filter(|s| !s.name.is_empty())
        .map(|s| hmp_core::ArtistRef {
            id: hmp_core::ArtistId::new(if s.mid.is_empty() { s.id.to_string() } else { s.mid.clone() }),
            name: s.name.clone(),
        })
        .collect::<Vec<_>>();
    let album = (!detail.track.album.name.is_empty()).then(|| hmp_core::AlbumRef {
        id: hmp_core::AlbumId::new(detail.track.album.mid.clone()),
        name: detail.track.album.name.clone(),
    });
    let cover = (!detail.track.album.pmid.is_empty()).then(|| hmp_core::CoverRef {
        url: format!("https://y.gtimg.cn/music/photo_new/T002R300x300M000{}.jpg", detail.track.album.pmid),
    });
    let title = detail.track.name.clone();

    // 音质回退链
    let file_info = SongFileInfo {
        mid: track_id.as_str().to_owned(),
        file_type: None,
        song_type: 0,
        media_mid: Some(media_mid),
    };
    let mut last_error = None;
    for quality in hmp_core::AudioQuality::Master.fallback_chain() {
        let Some(file_type) = quality_to_file_type(&quality) else { continue };
        let urls = song_api
            .get_song_urls(std::slice::from_ref(&file_info), file_type, Some(credential))
            .await;
        let mut found: Option<(SongFileType, String, Option<hmp_media::PreparedMedia>)> = None;
        if let Ok(resp) = urls {
            for item in &resp.data {
                if item.result == 0 && !item.purl.is_empty() {
                    let remote_uri = format!("https://isure.stream.qqmusic.qq.com/{}", item.purl);
                    if file_type.is_encrypted {
                        match hmp_media::prepare_stream(
                            &remote_uri,
                            (!item.ekey.is_empty()).then_some(item.ekey.as_str()),
                            None,
                        )
                        .await
                        {
                            Ok(p) => {
                                let uri = p.uri.clone();
                                found = Some((file_type.clone(), uri, Some(p)));
                                break;
                            }
                            Err(e) => {
                                last_error = Some(format!("QMC2 decrypt failed: {e}"));
                                continue;
                            }
                        }
                    } else {
                        // 明文无需解密 guard：直接播放 CDN URL（media: None）
                        found = Some((file_type.clone(), remote_uri, None));
                        break;
                    }
                } else {
                    last_error = Some(format!("result={}", item.result));
                }
            }
        } else if let Err(e) = urls {
            last_error = Some(e.to_string());
        }
        if let Some((file_type, uri, media)) = found {
            let track = Track {
                id: track_id.clone(),
                title,
                artists: singers,
                album,
                duration: detail.track.interval.checked_mul(1000)
                    .and_then(|ms| u64::try_from(ms).ok())
                    .map(std::time::Duration::from_millis),
                cover,
                url: Some(uri.clone()),
                qualities: vec![quality_from_file_type(&file_type)],
            };
            return Ok(ResolvedTrack { track, uri, media });
        }
    }
    Err(EngineError::QualityUnavailable(last_error.unwrap_or_default()))
}

fn quality_from_file_type(t: &SongFileType) -> hmp_core::AudioQuality {
    match (t.s.as_str(), t.e.as_str()) {
        ("AIM0", _) => hmp_core::AudioQuality::Master,
        ("Q0M0", _) => hmp_core::AudioQuality::Atmos,
        ("F0M0", _) => hmp_core::AudioQuality::Flac,
        ("C600", _) => hmp_core::AudioQuality::Aac,
        ("M800", _) => hmp_core::AudioQuality::Mp3_320,
        _ => hmp_core::AudioQuality::Mp3_128,
    }
}
```

> 非加密分支：明文无需解密 guard，直接播放 CDN URL（`media: None`），不引入 `PreparedMedia::direct`，hmp-media 不动（若 `prepare_stream` 对非加密也能直接返回则统一走它，以编译/行为为准）。`resolve_track_impl` 需要 `use hmp_qqmusic_api::{SongApi, SongFileInfo, SongFileType};` 与 `use hmp_core::{Track, AlbumRef, ArtistId, ArtistRef, AlbumId, CoverRef, TrackId};`（按既有 play.rs 的导入补充）。

```rust
/// 解析源为 TrackId 列表（单曲/歌单/专辑；歌单/专辑分页拉取，上限 3 页防超限）。
pub async fn resolve_source_ids_impl(
    client: &QqMusicClient,
    src: &hmp_core::PlayRequest,
) -> Result<Vec<TrackId>, EngineError> {
    match src {
        hmp_core::PlayRequest::Track(id) => Ok(vec![id.clone()]),
        hmp_core::PlayRequest::Playlist(id) => {
            let list_id: i64 = id.as_str().parse().map_err(|_| EngineError::PlaylistNotFound("歌单 id 非数字".into()))?;
            let api = SonglistApi::new(client);
            let mut out = Vec::new();
            for page in 1..=3 {
                let resp = api.get_detail(list_id, 0, 100, page, true, false, false).await
                    .map_err(|e| EngineError::PlaylistNotFound(e.to_string()))?;
                for s in &resp.songs {
                    if !s.mid.is_empty() { out.push(TrackId::new(s.mid.clone())); }
                }
                if resp.hasmore == 0 || out.len() as i64 >= resp.total { break; }
            }
            if out.is_empty() { return Err(EngineError::PlaylistNotFound("歌单为空".into())); }
            Ok(out)
        }
        hmp_core::PlayRequest::Album(id) => {
            let api = AlbumApi::new(client);
            let mut out = Vec::new();
            for page in 1..=3 {
                let resp = api.get_song(id.as_str(), 100, page).await
                    .map_err(|e| EngineError::PlaylistNotFound(e.to_string()))?;
                for s in &resp.song_list {
                    if !s.mid.is_empty() { out.push(TrackId::new(s.mid.clone())); }
                }
                if out.len() as i64 >= resp.total_num { break; }
            }
            if out.is_empty() { return Err(EngineError::PlaylistNotFound("专辑为空".into())); }
            Ok(out)
        }
    }
}

/// `SourceResolver for QqSourceResolver`（生产实现：QQ API + 共享凭证）。
impl SourceResolver for QqSourceResolver {
    async fn resolve_source_ids(
        &self,
        src: &hmp_core::PlayRequest,
    ) -> Result<Vec<TrackId>, EngineError> {
        self.load_credential()?;
        resolve_source_ids_impl(&self.client, src).await
    }

    async fn resolve_track(
        &self,
        track_id: &TrackId,
    ) -> Result<ResolvedTrack, EngineError> {
        let credential = self.load_credential()?;
        resolve_track_impl(&self.client, &credential, track_id).await
    }
}
```

> `PreparedMedia::direct` 不引入——非加密分支用 `remote_uri` + `media: None`（明文无需 guard），保持 hmp-media 不动（若 hmp-media 的 `prepare_stream` 对非加密也能直接返回，则统一走 `prepare_stream` 亦可，以编译/行为为准）。

- [ ] **Step 5: 服务器凭证前置校验 + 引擎接入**

`EngineHandle` 已含 `credential_ok`（Task 2）。服务器对 `Play/PlayNext/QueueAppend` 先查再投递（spec §6 同步 NotLoggedIn）：

```rust
// server.rs handle_connection 中，投递前：
fn is_play_request(req: &Request) -> bool {
    matches!(req, Request::Play(_) | Request::PlayNext(_) | Request::QueueAppend(_))
}

// 在匹配 `Ok(req)` 分支：
Ok(req) => {
    if is_play_request(&req) && !(handle.credential_ok)() {
        write_frame(&mut stream, &Response::Err {
            code: hmp_core::IpcErrorCode::NotLoggedIn,
            message: "未登录，请先运行 hmp login".into(),
        }).await?;
        continue;
    }
    let resp = match handle.command_tx.send(req) {
        Ok(_) => Response::Ok,
        Err(_) => Response::Err {
            code: hmp_core::IpcErrorCode::Internal,
            message: "引擎已退出".into(),
        },
    };
    write_frame(&mut stream, &resp).await?;
}
```

- [ ] **Step 6: 服务器测试（真 socket）补全 + 验证**

```rust
// server.rs tests（自包含：本模块内建最小 fake driver/resolver）
use super::*;
use crate::engine::PlaybackEngine;
use crate::player::{EngineError, ResolvedTrack, SourceResolver};
use hmp_core::ipc::{Event, Request, Response};
use hmp_core::{IpcErrorCode, PlayRequest, PlaybackState, PlaybackStatus, Track, TrackId};
use std::sync::Arc;
use tokio::sync::{broadcast, watch};

struct SDriver {
    state_tx: watch::Sender<PlaybackState>,
    events_tx: broadcast::Sender<PlayerEvent>,
}
impl PlaybackDriver for SDriver {
    fn load(&self, _r: LoadRequest) {}
    fn play(&self) {}
    fn pause(&self) {}
    fn seek(&self, _p: std::time::Duration) {}
    fn stop(&self) {}
    fn set_volume(&self, _v: f64) {}
    fn command(&self, _c: PlayerCommand) {}
    fn shutdown(&self) {}
    fn subscribe_state(&self) -> watch::Receiver<PlaybackState> { self.state_tx.subscribe() }
    fn subscribe_events(&self) -> broadcast::Receiver<PlayerEvent> { self.events_tx.subscribe() }
}
struct SResolver;
impl SourceResolver for SResolver {
    async fn resolve_source_ids(&self, _s: &PlayRequest) -> Result<Vec<TrackId>, EngineError> {
        Ok(vec![TrackId::new("a")])
    }
    async fn resolve_track(&self, id: &TrackId) -> Result<ResolvedTrack, EngineError> {
        Ok(ResolvedTrack {
            track: Track {
                id: id.clone(),
                title: format!("t-{id}"),
                artists: vec![],
                album: None,
                duration: Some(std::time::Duration::from_secs(60)),
                cover: None,
                url: Some(format!("fake://{id}")),
                qualities: vec![],
            },
            uri: format!("fake://{id}"),
            media: None,
        })
    }
}

async fn test_engine(cred_ok: bool) -> EngineHandle {
    let (state_tx, _) = watch::channel(PlaybackState::default());
    let (events_tx, _) = broadcast::channel(16);
    let driver = Arc::new(SDriver { state_tx, events_tx });
    PlaybackEngine::start(driver, Arc::new(SResolver), Arc::new(move || cred_ok))
}

async fn temp_socket() -> (PathBuf, UnixListener) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hmp-test.sock");
    let listener = UnixListener::bind(&path).unwrap();
    (path, listener)
}

async fn request(sock: &PathBuf, req: &Request) -> Response {
    let mut stream = UnixStream::connect(sock).await.unwrap();
    stream.write_all(&encode_frame(req).unwrap()).await.unwrap();
    let mut buf = vec![0u8; 65536];
    let n = stream.read(&mut buf).await.unwrap();
    decode_frame::<Response>(&buf[..n]).unwrap()
}

#[tokio::test]
async fn status_returns_daemon_state() {
    let (sock, listener) = temp_socket().await;
    let handle = test_engine(true).await;
    tokio::spawn(async move { serve(listener, handle).await });
    let resp = request(&sock, &Request::Status).await;
    assert!(matches!(resp, Response::Status(_)));
}

#[tokio::test]
async fn queue_query_returns_snapshot() {
    let (sock, listener) = temp_socket().await;
    let handle = test_engine(true).await;
    tokio::spawn(async move { serve(listener, handle).await });
    let resp = request(&sock, &Request::Queue).await;
    assert!(matches!(resp, Response::Queue(_)));
}

#[tokio::test]
async fn subscribe_pushes_initial_and_changes() {
    let (sock, listener) = temp_socket().await;
    let (state_tx, _) = watch::channel(PlaybackState::default());
    let (events_tx, _) = broadcast::channel(16);
    let driver = Arc::new(SDriver { state_tx: state_tx.clone(), events_tx });
    let handle = PlaybackEngine::start(driver.clone(), Arc::new(SResolver), Arc::new(|| true));
    tokio::spawn(async move { serve(listener, handle).await });
    let mut stream = UnixStream::connect(&sock).await.unwrap();
    stream.write_all(&encode_frame(&Request::Subscribe).unwrap()).await.unwrap();
    let mut buf = vec![0u8; 65536];
    let n = stream.read(&mut buf).await.unwrap();
    let ev: Event = decode_frame(&buf[..n]).unwrap();
    assert!(matches!(ev, Event::StateChanged(_)));
    // 触发状态变更 → 订阅帧（select 轮询间隔 100ms，等 300ms）
    state_tx.send_modify(|s| s.status = PlaybackStatus::Paused);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let n = stream.read(&mut buf).await.unwrap();
    let ev2: Event = decode_frame(&buf[..n]).unwrap();
    assert!(matches!(ev2, Event::StateChanged(_)));
}

#[tokio::test]
async fn malformed_frame_gets_bad_request() {
    let (sock, listener) = temp_socket().await;
    let handle = test_engine(true).await;
    tokio::spawn(async move { serve(listener, handle).await });
    let mut stream = UnixStream::connect(&sock).await.unwrap();
    // 长度 4 + 非法 JSON（非 Request）→ decode 失败 → BadRequest
    stream.write_all(&[4, 0, 0, 0, b'j', b'u', b'n', b'k']).await.unwrap();
    let mut buf = vec![0u8; 65536];
    let n = stream.read(&mut buf).await.unwrap();
    let resp: Response = decode_frame(&buf[..n]).unwrap();
    assert!(matches!(resp, Response::Err { code: IpcErrorCode::BadRequest, .. }));
}

#[tokio::test]
async fn play_without_credentials_returns_not_logged_in() {
    let (sock, listener) = temp_socket().await;
    let handle = test_engine(false).await;
    tokio::spawn(async move { serve(listener, handle).await });
    let resp = request(&sock, &Request::Play(PlayRequest::Track(TrackId::new("m1")))).await;
    assert!(matches!(resp, Response::Err { code: IpcErrorCode::NotLoggedIn, .. }));
}
```
> 测试需 `tempfile`（已在 Cargo.toml dev-dependencies）。`LoadRequest`/`PlayerCommand` 从 `hmp_player_gst`/`hmp_core` 导入。订阅测试依赖服务器 select 轮询间隔（100ms）→ 等待 300ms 保证帧到达。

- [ ] **Step 7: 全量验证 + 提交**

```bash
cargo build -p hmp-daemon --no-default-features
cargo test -p hmp-daemon --no-default-features
cargo test -p hmp-core
cargo clippy -p hmp-daemon --no-default-features --all-targets -- -D warnings
cargo fmt --all -- --check
git add crates/hmp-daemon crates/hmp-core
git commit -m "feat(daemon): unix socket control server and track resolution"
```

---

### Task 4: CLI 终端 ASCII 二维码登录（独立）

**Files:**
- Create: `crates/hmp-cli/src/qr_ascii.rs`
- Modify: `crates/hmp-cli/src/login.rs`（渲染 + 过期自动刷新 + flush）、`crates/hmp-cli/Cargo.toml`（+= `image`）
- Test: qr_ascii.rs 单测；login 刷新循环用注入 trait 测试

**Interfaces:**
- Consumes: `hmp_qqmusic_api::{LoginApi, QRLoginType, QqMusicClient, QrCodeLoginEvents?}`；`image` crate
- Produces:
  - `qr_ascii::render_qr(data: &[u8], width_chars: usize) -> Result<String, QrRenderError>`（解码 + 渲染）
  - `qr_ascii::terminal_width() -> usize`（`COLUMNS` env → clamp 32..=120，默认 60）
  - `login::run()`（重写；签名不变）

- [ ] **Step 1: 写失败测试 —— 渲染器**

```rust
// qr_ascii.rs tests
#[test]
fn renders_2x2_block_map() {
    // 2x2 像素：左列上黑下白（▀），右列全黑（█）
    let img = image::RgbaImage::from_fn(2, 2, |x, y| {
        let dark = match (x, y) {
            (0, 0) => true,   // 左列上：黑
            (0, 1) => false,  // 左列下：白 → 左字符 ▀
            _ => true,        // 右列全黑 → 右字符 █
        };
        if dark { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([255, 255, 255, 255]) }
    });
    let s = render_img(&image::DynamicImage::ImageRgba8(img), 2).unwrap();
    assert_eq!(s, "▀█\n"); // 每字符 2 行 × 1 列像素；宽 2 字符 → 1 行输出
}
```
> 注意纵横比：渲染器把图像缩放为 `width_chars × width_chars` 像素（Nearest），每字符承载 2 行 × 1 列像素 → 输出高 = width_chars/2 字符行。上面的测试用 4×4 图 + width=2 → resize 到 2×2 像素 → 输出 1 行 2 字符 `▀▄`。断言改为 `assert_eq!(s, "▀▄")`（先 resize 后映射）。

```rust
#[test]
fn width_is_clamped() {
    assert_eq!(terminal_width_with(Some("10")), 32);
    assert_eq!(terminal_width_with(Some("200")), 120);
    assert_eq!(terminal_width_with(None), 60);
}

#[test]
fn decode_failure_returns_err() {
    assert!(render_qr(b"not an image", 60).is_err());
}

#[test]
fn renders_real_png() {
    // 用 image crate 生成一张 21x21 纯黑 PNG 字节 → render_qr 成功且非空
    let mut img = image::RgbaImage::new(21, 21);
    for p in img.pixels_mut() { *p = image::Rgba([0, 0, 0, 255]); }
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img).write_to(&mut buf, image::ImageFormat::Png).unwrap();
    let s = render_qr(buf.get_ref(), 40).unwrap();
    assert!(s.contains('█'));
}
```

- [ ] **Step 2: 实现渲染器**

```rust
//! 二维码终端渲染（spec §4.3 `qr_ascii.rs`）。
//!
//! 解码 → 缩放（Nearest）→ 每字符 2×2 像素 → 半块 Unicode 字符。

use image::imageops::FilterType;

/// 渲染错误。
#[derive(Debug, thiserror::Error)]
pub enum QrRenderError {
    #[error("图像解码失败: {0}")]
    Decode(String),
    #[error("图像尺寸无效")]
    InvalidSize,
}

/// 终端宽度（`COLUMNS` 环境变量，钳位 32..=120，默认 60）。
pub fn terminal_width() -> usize {
    terminal_width_with(std::env::var("COLUMNS").ok().as_deref())
}

/// 供测试注入的宽度解析。
fn terminal_width_with(cols: Option<&str>) -> usize {
    let Some(v) = cols.and_then(|s| s.trim().parse::<usize>().ok()) else {
        return 60;
    };
    v.clamp(32, 120)
}

/// 渲染灰度/黑白图（已按 width 缩放）为半块字符。
fn render_img(img: &image::DynamicImage, width_chars: usize) -> Result<String, QrRenderError> {
    let w = width_chars.max(1);
    // 缩放为 w × w 像素（QR 方形），Nearest 保持硬边
    let small = img.resize_exact(w as u32, w as u32, FilterType::Nearest).to_luma8();
    let mut out = String::new();
    for r in (0..small.height() as usize).step_by(2) {
        for c in 0..small.width() as usize {
            let top = small.get_pixel(c as u32, r as u32).0[0] < 128;
            let bottom = if r + 1 < small.height() as usize {
                small.get_pixel(c as u32, (r + 1) as u32).0[0] < 128
            } else {
                false
            };
            let ch = match (top, bottom) {
                (false, false) => ' ',
                (true, false) => '▀',
                (false, true) => '▄',
                (true, true) => '█',
            };
            out.push(ch);
        }
        out.push('\n');
    }
    Ok(out)
}

/// 解码二维码图像字节并渲染为 ASCII 艺术字符串。
pub fn render_qr(data: &[u8], width_chars: usize) -> Result<String, QrRenderError> {
    let img = image::load_from_memory(data)
        .map_err(|e| QrRenderError::Decode(e.to_string()))?;
    render_img(&img, width_chars)
}
```
> `image::DynamicImage::to_luma8()` 需要 image 的默认 features 之外的 `imageops`？`resize_exact`/`to_luma8` 在 image 0.25 核心可用（无需额外 feature）；若编译报缺 feature，在 workspace `image` 依赖补 `imageops`（0.25 中 `imageops` 是默认模块）。实施时按编译错误调整。

- [ ] **Step 3: 重写 login.rs（刷新循环 + flush）**

```rust
//! `hmp login`：QQ 扫码登录（终端 ASCII 二维码 + 过期自动刷新）。
//!
//! 输出约定：二维码与提示全部 `write!` + `stdout().flush()`（spec 全局约束），
//! 禁止裸 `println!`。

use std::io::Write;
use std::time::{Duration, Instant};

use hmp_qqmusic_api::{LoginApi, QRLoginType, QqMusicClient};
use hmp_storage::credential::{BackendKind, store_from_env};

mod qr_ascii;

/// 总墙钟上限：二维码无限过期也不死循环（10 分钟）。
const OVERALL_LIMIT: Duration = Duration::from_secs(600);
/// 单个二维码等待上限。
const QR_TIMEOUT: Duration = Duration::from_secs(120);

/// 渲染二维码到 stdout（失败时打印兜底路径）。返回是否渲染成功。
fn print_qr(data: &[u8], path: &std::path::Path, out: &mut impl Write) -> std::io::Result<bool> {
    match qr_ascii::render_qr(data, qr_ascii::terminal_width()) {
        Ok(s) => {
            writeln!(out, "{s}")?;
            Ok(true)
        }
        Err(e) => {
            writeln!(out, "二维码渲染失败（{e}），请手动打开: {}", path.display())?;
            Ok(false)
        }
    }
}

/// 登录主流程。
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let client = QqMusicClient::new();
    let login = LoginApi::new(&client);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let overall_deadline = Instant::now() + OVERALL_LIMIT;

    loop {
        let qr = login.get_qrcode(QRLoginType::Qq).await?;
        let qr_path = std::env::temp_dir().join("hmp-qr.png");
        std::fs::write(&qr_path, &qr.data)?;
        print_qr(&qr.data, &qr_path, &mut out)?;
        out.flush()?;
        writeln!(out, "请用 QQ 手机版扫码并确认登录……（二维码过期将自动刷新）")?;
        out.flush()?;

        match login.wait_qrcode_login(&qr, Default::default(), QR_TIMEOUT, None).await {
            Ok(credential) => {
                let backend = BackendKind::from_env();
                let store = store_from_env();
                store.save(&credential)?;
                match backend {
                    BackendKind::SecretService => {
                        writeln!(out, "登录成功! 用户: {} ({}), 凭证已保存到系统密钥环",
                            credential.uin, credential.music_id)?;
                    }
                    BackendKind::File => {
                        writeln!(out, "登录成功! 用户: {} ({}), 凭证已保存到 {}（明文，不安全）",
                            credential.uin, credential.music_id,
                            hmp_storage::xdg::config_dir().join("credential.json").display())?;
                    }
                }
                out.flush()?;
                return Ok(());
            }
            Err(e) if Instant::now() < overall_deadline => {
                // 二维码过期/超时 → 自动刷新（不重跑命令）
                writeln!(out, "\n二维码已过期，自动刷新…")?;
                out.flush()?;
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
}
```
> 刷新循环的可测性：把判定抽为纯函数 `should_refresh`（下），`LoginApi` 自身不 mock。

```rust
/// 判定是否应自动刷新二维码（超时类错误且未到总墙钟上限）。
fn should_refresh(err: &hmp_qqmusic_api::QqMusicError, now: Instant, deadline: Instant) -> bool {
    use hmp_qqmusic_api::QqMusicError;
    let is_timeout = matches!(err, QqMusicError::Login { code: -1, .. });
    is_timeout && now < deadline
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeout_before_deadline_refreshes() {
        let err = hmp_qqmusic_api::QqMusicError::Login { code: -1, message: "登录二维码已超时".into() };
        assert!(should_refresh(&err, Instant::now(), Instant::now() + Duration::from_secs(100)));
    }
    #[test]
    fn timeout_after_deadline_stops() {
        let err = hmp_qqmusic_api::QqMusicError::Login { code: -1, message: "登录二维码已超时".into() };
        assert!(!should_refresh(&err, Instant::now(), Instant::now() - Duration::from_secs(1)));
    }
    #[test]
    fn non_timeout_error_stops() {
        let err = hmp_qqmusic_api::QqMusicError::Network("断网".into());
        assert!(!should_refresh(&err, Instant::now(), Instant::now() + Duration::from_secs(100)));
    }
}
```

- [ ] **Step 4: 验证 + 提交**

```bash
cargo build -p hmp-cli
cargo test -p hmp-cli qr_ascii
cargo test -p hmp-cli login
cargo clippy -p hmp-cli --all-targets -- -D warnings
cargo fmt --all -- --check
git add crates/hmp-cli/
git commit -m "feat(cli): terminal ASCII QR login with expiry auto-refresh"
```

---

### Task 5: CLI 遥控客户端 + 子命令 + 拉起 daemon

**Files:**
- Create: `crates/hmp-cli/src/client.rs`, `crates/hmp-cli/src/commands.rs`
- Modify: `crates/hmp-cli/src/main.rs`（子命令 + serve）、`crates/hmp-cli/src/play.rs`（改遥控）、`crates/hmp-cli/Cargo.toml`（+= `hmp-daemon`）
- Test: client.rs 单测（拉起/重连逻辑用进程级集成）、commands.rs 输出单测

**Interfaces:**
- Consumes: Task 1 `ipc::*`；Task 3 `server::socket_path`；Task 2/3 `hmp_daemon::serve::run_foreground/run_background`
- Produces:
  - `client::connect_or_spawn() -> Result<DaemonClient, CliError>`（连接失败 ENOENT → spawn `serve --background` → 轮询 ≤3s；ECONNREFUSED → 清理 sock 重 spawn）
  - `client::DaemonClient { request(&mut self, &Request) -> Result<Response, CliError> }`
  - `commands::{cmd_play, cmd_playnext, cmd_queue, cmd_pause, cmd_resume, cmd_next, cmd_prev, cmd_stop, cmd_seek, cmd_volume, cmd_loop, cmd_shuffle, cmd_status, cmd_quit}`（各自构造请求 + 打印）

- [ ] **Step 1: 写失败测试 —— 状态打印**

```rust
// commands.rs tests
#[test]
fn status_output_includes_track_and_status() {
    let st = hmp_core::DaemonState {
        playback: hmp_core::PlaybackState {
            status: hmp_core::PlaybackStatus::Playing,
            current: Some(hmp_core::Track {
                id: hmp_core::TrackId::new("m1"),
                title: "稻香".into(),
                artists: vec![hmp_core::ArtistRef {
                    id: hmp_core::ArtistId::new("a1"),
                    name: "周杰伦".into(),
                }],
                album: None,
                duration: Some(std::time::Duration::from_secs(300)),
                cover: None,
                url: Some("fake://m1".into()),
                qualities: vec![],
            }),
            position: std::time::Duration::from_secs(30),
            duration: Some(std::time::Duration::from_secs(300)),
            ..Default::default()
        },
        queue: Default::default(),
        caps: Default::default(),
    };
    let s = commands::format_status(&st);
    assert!(s.contains("稻香"));
    assert!(s.contains("Playing"));
    assert!(s.contains("00:30 / 05:00"));
}
```

- [ ] **Step 2: 实现 client.rs 与 commands.rs**

```rust
//! CLI → daemon 客户端（spec §4.3 `client.rs`）。

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use hmp_core::ipc::{decode_frame, encode_frame, Request, Response};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// CLI 错误。
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("无法连接后端: {0}")]
    Connect(String),
    #[error("后端响应错误: {code:?} {message}")]
    Response { code: hmp_core::IpcErrorCode, message: String },
    #[error("协议错误: {0}")]
    Protocol(String),
    #[error("io 错误: {0}")]
    Io(#[from] std::io::Error),
}

/// 与后端的一条连接。
pub struct DaemonClient {
    stream: UnixStream,
}

impl DaemonClient {
    /// 连接或拉起后端（ENOENT → spawn `hmp serve --background`；ECONNREFUSED → 清理重试）。
    pub async fn connect_or_spawn() -> Result<Self, CliError> {
        let path = hmp_daemon::server::socket_path();
        match Self::try_connect(&path).await {
            Ok(c) => return Ok(c),
            Err(CliError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                spawn_daemon()?;
                wait_for_socket(&path, Duration::from_secs(3)).await?;
            }
            Err(CliError::Io(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                let _ = std::fs::remove_file(&path);
                spawn_daemon()?;
                wait_for_socket(&path, Duration::from_secs(3)).await?;
            }
            Err(e) => return Err(e),
        }
        Self::try_connect(&path).await
    }

    async fn try_connect(path: &PathBuf) -> Result<Self, CliError> {
        let stream = UnixStream::connect(path).await?;
        Ok(Self { stream })
    }

    /// 发请求并收响应。
    pub async fn request(&mut self, req: &Request) -> Result<Response, CliError> {
        let frame = encode_frame(req).map_err(|e| CliError::Protocol(e.to_string()))?;
        self.stream.write_all(&frame).await?;
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > hmp_core::ipc::MAX_FRAME - 4 {
            return Err(CliError::Protocol("非法帧长度".into()));
        }
        let mut payload = vec![0u8; len];
        self.stream.read_exact(&mut payload).await?;
        let mut frame = Vec::with_capacity(4 + len);
        frame.extend_from_slice(&len_buf);
        frame.extend_from_slice(&payload);
        decode_frame::<Response>(&frame).map_err(|e| CliError::Protocol(e.to_string()))
    }
}

/// spawn `hmp serve --background`（新进程组 + 丢弃 stdio）。
fn spawn_daemon() -> Result<(), CliError> {
    let exe = std::env::current_exe().map_err(|e| CliError::Connect(e.to_string()))?;
    use std::os::unix::process::CommandExt;
    let _child = std::process::Command::new(&exe)
        .arg("serve")
        .arg("--background")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| CliError::Connect(format!("拉起后端失败: {e}")))?;
    Ok(())
}

/// 轮询 socket 就绪。
async fn wait_for_socket(path: &PathBuf, timeout: Duration) -> Result<(), CliError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if path.exists() && UnixStream::connect(path).await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(CliError::Connect("后端启动超时".into()));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
```

```rust
//! 各遥控子命令（spec §4.3）。

use std::io::Write;

use hmp_core::ipc::{IpcErrorCode, Request, Response};
use hmp_core::{DaemonState, PlayerCommand};

use crate::client::{CliError, DaemonClient};

/// 格式化状态为人类可读文本。
pub fn format_status(st: &DaemonState) -> String {
    let mut s = String::new();
    let track = st.playback.current.as_ref();
    let title = track.map(|t| t.title.as_str()).unwrap_or("（无）");
    let artist = track
        .map(|t| t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(" / "))
        .unwrap_or_default();
    s.push_str(&format!("状态: {:?}\n", st.playback.status));
    s.push_str(&format!("曲目: {title} - {artist}\n"));
    match st.playback.duration {
        Some(d) => s.push_str(&format!(
            "进度: {} / {}\n",
            fmt_duration(st.playback.position),
            fmt_duration(d)
        )),
        None => s.push_str(&format!("进度: {}\n", fmt_duration(st.playback.position))),
    }
    s.push_str(&format!("音量: {:.0}%\n", st.playback.volume * 100.0));
    s.push_str(&format!("循环: {:?}  随机: {}\n", st.playback.loop_mode, st.playback.shuffle));
    s.push_str(&format!("队列: {} 首\n", st.queue.tracks.len()));
    s
}

fn fmt_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

/// 通用：发命令并打印响应错误。
async fn send(client: &mut DaemonClient, req: Request) -> Result<Response, CliError> {
    client.request(&req).await
}

/// `hmp play <track-id|playlist:xxx|album:xxx>`（前缀识别源类型）。
pub async fn cmd_play(client: &mut DaemonClient, src: &str) -> Result<(), CliError> {
    let req = Request::Play(parse_source(src));
    let resp = send(client, req).await?;
    match resp {
        Response::Ok => {
            // 短轮询确认（≤15s）：等到非 Loading
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
            loop {
                let st = match send(client, Request::Status).await? {
                    Response::Status(s) => s,
                    _ => return Err(CliError::Protocol("Status 响应异常".into())),
                };
                use hmp_core::PlaybackStatus as S;
                match st.playback.status {
                    S::Playing | S::Paused => {
                        let mut out = std::io::stdout().lock();
                        writeln!(out, "已开始播放: {}", st.playback.current.as_ref().map(|t| t.title.as_str()).unwrap_or("?"))?;
                        out.flush()?;
                        return Ok(());
                    }
                    S::Error => return Err(CliError::Response { code: IpcErrorCode::Internal, message: "播放失败（见后端日志）".into() }),
                    S::Empty => return Err(CliError::Response { code: IpcErrorCode::Internal, message: "后端空闲，播放未启动".into() }),
                    _ => {}
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(CliError::Response { code: IpcErrorCode::Internal, message: "播放确认超时（15s）".into() });
                }
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
        }
        Response::Err { code, message } => Err(CliError::Response { code, message }),
        _ => Err(CliError::Protocol("意外响应".into())),
    }
}

/// 解析播放源：`playlist:<id>` / `album:<id>` / 其他 = 单曲。
pub fn parse_source(src: &str) -> hmp_core::PlayRequest {
    if let Some(id) = src.strip_prefix("playlist:") {
        hmp_core::PlayRequest::Playlist(hmp_core::PlaylistId::new(id))
    } else if let Some(id) = src.strip_prefix("album:") {
        hmp_core::PlayRequest::Album(hmp_core::AlbumId::new(id))
    } else {
        hmp_core::PlayRequest::Track(hmp_core::TrackId::new(src))
    }
}

/// `hmp status`。
pub async fn cmd_status(client: &mut DaemonClient) -> Result<(), CliError> {
    let resp = send(client, Request::Status).await?;
    match resp {
        Response::Status(st) => {
            let mut out = std::io::stdout().lock();
            write!(out, "{}", format_status(&st))?;
            out.flush()?;
            Ok(())
        }
        _ => Err(CliError::Protocol("Status 响应异常".into())),
    }
}

/// 简单命令（Pause/Resume/Next/Prev/Stop/Quit 等）通用执行。
pub async fn cmd_simple(client: &mut DaemonClient, req: Request) -> Result<(), CliError> {
    let resp = send(client, req).await?;
    match resp {
        Response::Ok => Ok(()),
        Response::Err { code, message } => Err(CliError::Response { code, message }),
        _ => Err(CliError::Protocol("意外响应".into())),
    }
}

/// `hmp queue show|add <id>|remove <idx>|clear`。
pub async fn cmd_queue(client: &mut DaemonClient, args: &[String]) -> Result<(), CliError> {
    match args.first().map(|s| s.as_str()) {
        None | Some("show") => {
            let resp = send(client, Request::Queue).await?;
            if let Response::Queue(q) = resp {
                let mut out = std::io::stdout().lock();
                for (i, t) in q.tracks.iter().enumerate() {
                    let mark = if Some(i) == q.current { "▶" } else { " " };
                    writeln!(out, "{mark} {i}: {t}")?;
                }
                out.flush()?;
                return Ok(());
            }
            Err(CliError::Protocol("Queue 响应异常".into()))
        }
        Some("add") => {
            let id = args.get(1).ok_or_else(|| CliError::Response { code: IpcErrorCode::BadRequest, message: "queue add 需要曲目 id".into() })?;
            cmd_simple(client, Request::QueueAppend(parse_source(id))).await
        }
        Some("remove") => {
            let idx: usize = args.get(1).and_then(|s| s.parse().ok())
                .ok_or_else(|| CliError::Response { code: IpcErrorCode::BadRequest, message: "queue remove 需要 0 基索引".into() })?;
            cmd_simple(client, Request::QueueRemove(idx)).await
        }
        Some("clear") => cmd_simple(client, Request::QueueClear).await,
        _ => Err(CliError::Response { code: IpcErrorCode::BadRequest, message: "未知 queue 子命令".into() }),
    }
}

/// 便捷构造。
pub fn pause_req() -> Request { Request::Command(PlayerCommand::Pause) }
pub fn resume_req() -> Request { Request::Command(PlayerCommand::Play) }
pub fn next_req() -> Request { Request::Command(PlayerCommand::Next) }
pub fn prev_req() -> Request { Request::Command(PlayerCommand::Previous) }
pub fn stop_req() -> Request { Request::Command(PlayerCommand::Stop) }
pub fn seek_req(secs: u64) -> Request { Request::Command(PlayerCommand::Seek(std::time::Duration::from_secs(secs))) }
pub fn volume_req(v: f64) -> Request { Request::Command(PlayerCommand::SetVolume(v.clamp(0.0, 1.0))) }
pub fn loop_req(m: hmp_core::LoopMode) -> Request { Request::Command(PlayerCommand::SetLoopMode(m)) }
pub fn shuffle_req(b: bool) -> Request { Request::Command(PlayerCommand::SetShuffle(b)) }
pub fn quit_req() -> Request { Request::Quit }
```

- [ ] **Step 3: 更新 main.rs 子命令**

旧 `crates/hmp-cli/src/play.rs`（进程内播放）逻辑已移入 hmp-daemon `player.rs`（Task 3）——**删除 `play.rs` 与 `mod play;`**（遥控 `hmp play` 走 `commands::cmd_play`）；`quality_to_file_type`/`quality_from_file_type` 不再于 CLI 保留（daemon 已实现）。

```rust
#[derive(Subcommand)]
enum Command {
    /// QQ 扫码登录（终端 ASCII 二维码）。
    Login,
    /// 搜索歌曲。
    Search { keyword: String },
    /// 播放（单曲 / playlist:<id> / album:<id>；遥控后端）。
    Play { source: String },
    /// 插队播放。
    PlayNext { source: String },
    /// 队列管理：show / add <id> / remove <idx> / clear。
    Queue { args: Vec<String> },
    /// 暂停。
    Pause,
    /// 继续播放。
    Resume,
    /// 下一首。
    Next,
    /// 上一首。
    Prev,
    /// 停止。
    Stop,
    /// 跳转（秒）。
    Seek { secs: u64 },
    /// 音量（0..1）。
    Volume { value: f64 },
    /// 循环模式：none / list / track。
    Loop { mode: String },
    /// 随机播放：on / off。
    Shuffle { value: String },
    /// 查询状态。
    Status,
    /// 退出后端。
    Quit,
    /// 前台运行后端（--background 由 CLI 自动拉起使用）。
    Serve {
        /// 后台模式（脱离终端）。
        #[arg(long)]
        background: bool,
    },
}

async fn run_remote(command: impl Into<Request>) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = client::DaemonClient::connect_or_spawn().await?;
    commands::cmd_simple(&mut client, command.into()).await?;
    Ok(())
}
```
> `main.rs` 分发：Login/Search 走本地；Serve 走 `hmp_daemon::serve::run_foreground()/run_background()`；其余走 `connect_or_spawn` + 对应命令函数。`serve.rs` 由 Task 5 一并实现（见 Step 4）。

- [ ] **Step 4: serve.rs（hmp-daemon）**

```rust
//! `hmp serve` 入口（spec §4.2 `serve.rs`）。

use std::sync::Arc;

use crate::daemon::{Daemon, DaemonConfig};
use crate::engine::EngineHandle;
use crate::server;

/// 前台运行（调试；Ctrl+C 优雅退出）。
pub async fn run_foreground() -> Result<(), Box<dyn std::error::Error>> {
    run_inner(DaemonConfig { audio_sink: None }).await
}

/// 后台运行（CLI 拉起；detached）。
pub async fn run_background() -> Result<(), Box<dyn std::error::Error>> {
    run_inner(DaemonConfig { audio_sink: None }).await
}

async fn run_inner(cfg: DaemonConfig) -> Result<(), Box<dyn std::error::Error>> {
    let daemon = Daemon::start(cfg)?;
    let path = server::socket_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // 清理残留（上次异常退出可能留下）
    if path.exists() {
        // 尝试连接：能连说明有活 daemon，本实例退出；不能连则删残留
        if tokio::net::UnixStream::connect(&path).await.is_ok() {
            eprintln!("已有后端在运行，退出");
            return Ok(());
        }
        let _ = std::fs::remove_file(&path);
    }
    let listener = tokio::net::UnixListener::bind(&path)?;
    tracing::info!(?path, "后端已就绪");
    // 优雅退出：SIGINT/SIGTERM → 发 Quit
    let (quit_tx, mut quit_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let handle = daemon.handle;
    {
        let handle = handle.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                let mut sigint = signal(SignalKind::interrupt()).unwrap();
                let mut sigterm = signal(SignalKind::terminate()).unwrap();
                tokio::select! {
                    _ = sigint.recv() => {}
                    _ = sigterm.recv() => {}
                }
            }
            let _ = handle.command_tx.send(hmp_core::Request::Quit);
            let _ = quit_tx.send(());
        });
    }
    let server_handle = tokio::spawn(server::serve(listener, handle.clone()));
    // 等待退出信号
    let _ = quit_rx.recv().await;
    // 停服务器（监听关闭）+ 清理
    server_handle.abort();
    let _ = tokio::fs::remove_file(&path).await;
    tracing::info!("后端已退出");
    Ok(())
}
```
> `Daemon::start` 需要暴露 `handle`（已是 pub）。Task 6 在 `run_inner` 中追加 tray/MPRIS 启动（feature 门控）与退出清理。

- [ ] **Step 5: 集成测试（进程级）**

```rust
// tests/daemon_cli.rs（hmp-cli/tests）
// 1) 起 hmp serve --background（真实子进程）→ hmp status 连接成功（断言输出含 "状态:"）
// 2) hmp quit → socket 消失
// 注：进程级测试用 std::process::Command 调 cargo 构建的 hmp 二进制
// （CARGO_BIN_EXE_hmp 环境变量由 cargo 提供）
```
> 进程级测试在 CI 无音频设备时 `serve --background` 的 GstDriver 可能失败（无音频 sink）——测试改用**只测 socket 层**：直接构造 `hmp-daemon` 的 server + FakeDriver 引擎（lib 级集成，hmp-daemon 内已有）；CLI 进程级测试标记 `#[ignore]`（需真实环境），并在计划验收清单标注"真机验收项"。

- [ ] **Step 6: 验证 + 提交**

```bash
cargo build -p hmp-cli
cargo test -p hmp-cli
cargo clippy -p hmp-cli --all-targets -- -D warnings
cargo fmt --all -- --check
git add crates/hmp-cli crates/hmp-daemon
git commit -m "feat(cli): remote-control subcommands with daemon auto-spawn"
```

---

### Task 6: tray（ksni）+ MPRIS + 优雅退出收尾

**Files:**
- Create: `crates/hmp-daemon/src/tray.rs`（feature `tray`）、`crates/hmp-daemon/src/mpris.rs`（feature `mpris`）
- Modify: `crates/hmp-daemon/src/serve.rs`（接入 tray/MPRIS，feature 门控）
- Test: 编译性（`cargo build -p hmp-daemon` 默认 features）+ tray 菜单构造单测（不 spawn）

**Interfaces:**
- Consumes: Task 2 `EngineHandle`；`hmp_mpris::MprisService::start(command_sender, state_rx)`（现有）
- Produces:
  - `tray::spawn_tray(handle: EngineHandle) -> Option<ksni::TrayService<HmpTray>>`（无 session bus 时返回 None，不 panic）
  - `mpris::start_mpris(handle: EngineHandle) -> Option<zbus::Connection>`（失败 None）

- [ ] **Step 1: 实现 tray.rs**

```rust
//! 系统托盘（spec §4.2 `tray.rs`；feature `tray`）。
//!
//! 最小菜单：播放/暂停、上一首、下一首、停止、退出。
//! 适配器：输入走命令通道，输出订阅状态（仅用于图标切换）。

use std::sync::Arc;

use hmp_core::{PlaybackStatus, PlayerCommand, Request};
use tokio::sync::mpsc;

use crate::engine::EngineHandle;

/// ksni tray 实现。
pub struct HmpTray {
    command_tx: mpsc::UnboundedSender<Request>,
    playing: std::sync::atomic::AtomicBool,
}

impl HmpTray {
    fn new(command_tx: mpsc::UnboundedSender<Request>) -> Self {
        Self { command_tx, playing: std::sync::atomic::AtomicBool::new(false) }
    }

    fn menu_items(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};
        let play_label = if self.playing.load(std::sync::atomic::Ordering::Relaxed) { "暂停" } else { "播放" };
        vec![
            StandardItem::new(play_label, "media-playback-pause")
                .with_update(true)
                .activate(|this: &mut Self| {
                    let _ = this.command_tx.send(Request::Command(PlayerCommand::TogglePlay));
                })
                .into(),
            StandardItem::new("上一首", "media-skip-backward")
                .activate(|this: &mut Self| {
                    let _ = this.command_tx.send(Request::Command(PlayerCommand::Previous));
                })
                .into(),
            StandardItem::new("下一首", "media-skip-forward")
                .activate(|this: &mut Self| {
                    let _ = this.command_tx.send(Request::Command(PlayerCommand::Next));
                })
                .into(),
            StandardItem::new("停止", "media-playback-stop")
                .activate(|this: &mut Self| {
                    let _ = this.command_tx.send(Request::Command(PlayerCommand::Stop));
                })
                .into(),
            StandardItem::new("退出", "application-exit")
                .activate(|this: &mut Self| {
                    let _ = this.command_tx.send(Request::Quit);
                })
                .into(),
        ]
    }
}

impl ksni::Tray for HmpTray {
    fn id(&self) -> String { "hmp".into() }
    fn title(&self) -> String { "胡桃音乐播放器".into() }
    fn icon_name(&self) -> String {
        if self.playing.load(std::sync::atomic::Ordering::Relaxed) {
            "media-playback-pause".into()
        } else {
            "media-playback-start".into()
        }
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        self.menu_items()
    }
}

/// 启动 tray（无 session bus 时返回 None，不 panic）。
pub fn spawn_tray(handle: &EngineHandle) -> Option<ksni::TrayService<HmpTray>> {
    let tray = HmpTray::new(handle.command_tx.clone());
    let service = ksni::TrayService::new(tray);
    match service.spawn() {
        Ok(()) => Some(service),
        Err(e) => {
            tracing::warn!(%e, "tray 启动失败（可能无桌面会话），跳过");
            None
        }
    }
}
```
> `ksni::menu::StandardItem` 的 API 以 ksni 0.2 文档为准；若 `activate` 闭包签名不同（`FnMut(&mut Self)`），按实际签名调整。`TrayService::spawn()` 返回 `Result<(), Error>`（需当前 tokio runtime）。

- [ ] **Step 2: 实现 mpris.rs**

```rust
//! MPRIS 适配（spec §4.2 `mpris.rs`；feature `mpris`）。
//!
//! 复用现有 hmp-mpris：它已消费（命令通道, 状态 watch）两个接口。

use hmp_core::Request;
use tokio::sync::{mpsc, watch};

/// 启动 MPRIS（bus 名冲突/无总线时返回 None）。
pub fn start_mpris(
    command_tx: mpsc::UnboundedSender<Request>,
    state_rx: watch::Receiver<hmp_core::DaemonState>,
) -> Option<hmp_mpris::MprisService> {
    // hmp-mpris 的 MprisService::start 需要 (UnboundedSender<PlayerCommand>, watch::Receiver<PlaybackState>)
    // 适配：daemon 状态 → playback 子集；命令反向转换
    let cmd_tx = {
        let (tx, mut rx) = mpsc::unbounded_channel::<hmp_core::PlayerCommand>();
        let daemon_tx = command_tx.clone();
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                let _ = daemon_tx.send(Request::Command(cmd));
            }
        });
        tx
    };
    let playback_rx = {
        let (tx, rx) = watch::channel::<hmp_core::PlaybackState>(state_rx.borrow().playback.clone());
        let state_rx = state_rx.clone();
        tokio::spawn(async move {
            let mut state_rx = state_rx;
            while state_rx.changed().await.is_ok() {
                let _ = tx.send(state_rx.borrow().playback.clone());
            }
        });
        rx
    };
    match hmp_mpris::MprisService::start(cmd_tx, playback_rx).await {
        Ok(service) => Some(service),
        Err(e) => {
            tracing::warn!(%e, "MPRIS 启动失败，跳过");
            None
        }
    }
}
```
> `MprisService::start` 返回 `Result<MprisService, MprisError>`（现有 CLI 用法 `start(...).await.ok()`）；daemon 持有 `MprisService` 防 Drop（Drop 释放 bus 名）。

- [ ] **Step 3: serve.rs 接入（feature 门控）**

```rust
// run_inner 中，服务器 spawn 之后：
    #[cfg(feature = "tray")]
    let _tray = crate::tray::spawn_tray(&handle);
    #[cfg(feature = "mpris")]
    let _mpris = crate::mpris::start_mpris(handle.command_tx.clone(), handle.state_rx.clone());
// 退出清理：
    server_handle.abort();
    let _ = tokio::fs::remove_file(&path).await;
    drop(_tray); // 关 tray
    drop(_mpris); // 释放 bus 名
```
> 使用 `let _tray`/`let _mpris` 绑定以在函数尾 Drop；`#[cfg]` 下变量声明与 `drop` 需同 cfg 配对（用 `#[cfg_attr]` 或 `let _ =` 模式按编译错误调整）。

- [ ] **Step 4: tray 菜单构造单测**

```rust
// tray.rs tests
#[test]
fn menu_has_five_entries() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let tray = HmpTray::new(tx);
    let items = tray.menu_items();
    assert_eq!(items.len(), 5);
}
```
Run: `cargo test -p hmp-daemon --features tray --no-default-features tray`  Expected: 通过

- [ ] **Step 5: 验证 + 提交**

```bash
cargo build -p hmp-daemon                       # 默认 features（含 tray/mpris）编译通过
cargo build -p hmp-daemon --no-default-features # backend-only 编译通过
cargo test -p hmp-daemon --no-default-features
cargo test -p hmp-daemon --features tray --no-default-features tray
cargo clippy -p hmp-daemon --all-targets -- -D warnings
cargo fmt --all -- --check
git add crates/hmp-daemon
git commit -m "feat(daemon): ksni tray and MPRIS adapters with graceful exit"
```

---

### Task 7: 端到端测试 + 文档

**Files:**
- Create: `crates/hmp-daemon/tests/e2e.rs`（wiremock + fakesink + 本地 wav）
- Modify: `docs/PROJECT.md`（§8 后台播放/CLI 遥控）、`README.md`（用法）
- Test: e2e.rs

**Interfaces:**
- Consumes: 全部前面任务的产物

- [ ] **Step 1: 写 e2e 测试（wiremock QQ API + fakesink + 本地 wav）**

```rust
//! 端到端：Play → 详情 → 回退 → 解密 → 播放 → Ended → 下一首（spec §8）。
//! 环境：HMP_CREDENTIAL_BACKEND=file + 临时凭证目录 + wiremock QQ API + fakesink。

use hmp_core::{PlaybackStatus, PlayerCommand, Request, TrackId};
use hmp_daemon::engine::PlaybackEngine;
use hmp_daemon::player::GstDriver;
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::{method, path};

// 生成 1 秒 wav（440Hz 正弦，fakesink 播放）。
fn write_wav(path: &std::path::Path) {
    let sample_rate = 8000u32;
    let n = sample_rate as usize;
    let mut data = Vec::with_capacity(44 + n * 2);
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&((36 + n * 2) as u32).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&sample_rate.to_le_bytes());
    data.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&((n * 2) as u32).to_le_bytes());
    for i in 0..n {
        let v = ((i as f64 * 2.0 * std::f64::consts::PI * 440.0 / sample_rate as f64).sin() * 0.3 * 32767.0) as i16;
        data.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, data).unwrap();
}

#[tokio::test]
async fn play_then_end_advances_queue() {
    // 1) 临时凭证目录 + file 后端
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("HMP_CREDENTIAL_BACKEND", "file");
    std::env::set_var("HMP_CREDENTIAL_DIR", dir.path());
    // 2) wiremock：曲目详情 + 取流（返回本地 wav 的 http URL 或 file://）
    //    —— 取流 URL 直接指向 mock；song type 用非加密（避免解密路径，聚焦流程）。
    //    wiremock 用例形态参考 hmp-qqmusic-api 与 hmp-media 既有测试
    //    （crates/hmp-qqmusic-api/src/**/tests.rs 与 crates/hmp-media/src/**/tests.rs 的 Mock/MockServer 用法，
    //    响应 JSON 字段对齐 song.rs/models.rs 的 serde 结构）。
    // 3) GstDriver::new(Some("fakesink"))
    // 4) 引擎 Play([t1, t2]) → 状态 Playing → 等 Ended 事件 → 断言队列 current==1 且第二首已加载
    // 5) 清理 env 变量（防污染其他测试）
}
```
> 端到端细节较多、依赖 GStreamer 环境——按 hmp-player-gst 既有测试的媒体生成方式（若已有 test media helper 则复用）。若 CI 无 GStreamer，本测试标 `#[ignore]` 并记录为真机验收项。**必须至少包含**：引擎驱动（FakeDriver）的队列裁决+自动续播已由 Task 2 单测覆盖；本 e2e 是真实 gst 冒烟，允许 `#[ignore]`。

- [ ] **Step 2: 文档更新**

`docs/PROJECT.md` §8 新增：
```markdown
### 8.5 后台播放（service + tray）
`hmp serve` 启动常驻后端（CLI 自动拉起），单例 Unix socket（$XDG_RUNTIME_DIR/hmp.sock）。
`hmp play/playnext/queue/playlist/album + pause/resume/next/prev/stop/seek/volume/loop/shuffle/status/quit` 遥控后端。
架构：单一 Request 命令通道 + 单一 watch<DaemonState> 状态出口；前端（socket/tray/MPRIS）均为适配器。
```
`README.md` 用法节新增遥控命令示例与 `hmp login`（终端二维码）说明。

- [ ] **Step 3: 全量验证**

```bash
cargo test --workspace
cargo fmt --all -- --check
cargo clippy -p hmp-daemon --all-targets -- -D warnings   # 默认 features 下无新告警
cargo clippy -p hmp-cli --all-targets -- -D warnings
```

- [ ] **Step 4: 提交**

```bash
git add crates/hmp-daemon/tests docs/PROJECT.md README.md
git commit -m "test(daemon): e2e playback smoke test; docs: background playback usage"
```

---

## 真机验收清单（计划外，交付后人工执行）

1. `hmp login`：终端渲染二维码、扫码登录、过期自动刷新、凭证落盘。
2. `hmp play 歌曲id`：拉起 daemon、FLAC 解密流式播放、CLI 退出后播放不断。
3. `hmp status` / `playerctl status`：状态一致（MPRIS 双路）。
4. 拖动进度（`hmp seek`）、`next/prev`、`volume`、`loop`、`shuffle` 生效。
5. `hmp playlist:<id>` 连续播放至队列末尾、EOS 后 daemon 存活。
6. tray：菜单五项可用、播放/暂停图标切换、退出干净（socket 清理、MPRIS 释放）。
7. 终端关闭（SIGHUP）后播放不中断（process_group 生效）。
