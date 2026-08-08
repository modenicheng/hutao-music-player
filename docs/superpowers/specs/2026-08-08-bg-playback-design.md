# 后台播放（service + tray）设计

日期：2026-08-08
状态：已批准（brainstorming 完成）
关联：docs/PROJECT.md §8 播放流程、crates/hmp-core、crates/hmp-cli

## 1. 目标与范围

实现 QQ 音乐播放器的**后台播放**：一个常驻的播放器后端进程（daemon），关闭终端后播放不中断；提供最小系统托盘（tray）入口；CLI 作为**第一个接入的前端**，支持完整状态管理与队列管理。

已确认的约束（brainstorming 结论）：

1. **胖服务**：daemon 独占全部播放职责（凭证读取、QQ API、音质回退、QMC2 解密、播放、队列、MPRIS）。CLI 是纯遥控器。
2. **按需拉起**：CLI 首次使用时自动 spawn daemon（detached）；`hmp serve` 前台调试。**不引入 systemd unit**（不向用户系统安装任何东西）。
3. **前端无关**：daemon 是"播放器后端"，前端（CLI / 未来桌面 Slint / TUI）与后端解耦，后端不感知具体前端。
4. CLI 遥控面 = 基础控制 + 队列管理 + `playnext` + 歌单/专辑播放。
5. tray 最小化：播放/暂停、上一首、下一首、停止、退出（MPRIS 已存在，tray 不臃肿）。
6. 队列 = 标准全功能；`prev` 一律跳上一首（不做 >3s 回开头）。
7. **登录增强**：`hmp login` 将二维码图片解析并渲染为**终端 ASCII 艺术**，用户直接在终端扫码，无需手动打开图片文件。

非目标（YAGNI，明确不做）：歌词、封面落地、`hmp watch` 持续跟随命令、CLI JSON 输出、播放列表持久化、systemd unit、桌面端接入（后续项）、无缝拼接（gapless）。

## 2. 架构总览

```
┌───────────────────────────────────────────────────────────────┐
│ 前端（可并存，后端不感知具体是谁）                                  │
│   CLI (hmp 遥控) · Tray (ksni) · 未来: 桌面 Slint / TUI / 其他    │
└───────────────┬───────────────────────────────────────────────┘
                │ 同一套协议：Unix socket · 长度前缀 JSON 帧
┌───────────────▼───────────────────────────────────────────────┐
│ hmp-daemon —— 播放器后端（setsid 常驻，`hmp serve` 前台调试）      │
│                                                                 │
│  输入适配器（各自翻译成统一指令，互不感知）                          │
│    · socket RPC 服务器（多客户端并发）                             │
│    · MPRIS 服务（zbus，现成组件直接搬入）                          │
│    · Tray（ksni，最小菜单）                                       │
│                          │                                      │
│                          ▼                                      │
│  控制核心：命令通道（单一 mpsc）                                    │
│    → 队列核心（hmp-core，纯逻辑可单测）                            │
│    → PlayerCore（hmp-player-gst）                                │
│                          │                                      │
│                          ▼                                      │
│  状态出口：PlaybackState（watch 单一来源）                         │
│    → 事件广播（fan-out 给所有订阅者：socket 客户端、tray、MPRIS）    │
│                                                                 │
│  领域服务：QqMusicClient · hmp-media 解密 · hmp-storage 凭证      │
└───────────────────────────────────────────────────────────────┘
```

## 3. 解耦原则（贯穿设计）

1. 后端核心（控制核心 + 队列 + 播放 + 领域服务）**不引用任何前端类型**。
2. 每个前端是一个**适配器**：输入侧翻译为统一指令进命令通道，输出侧订阅统一事件；删掉任何一个不碰后端一行。
3. 协议消息类型放 **hmp-core**（领域层，与 `PlayerCommand` 同居）；传输/连接管理放 hmp-daemon。任何前端只要实现该协议即可接入。
4. **多客户端并存**：N 个 CLI 会话 + tray + MPRIS 同时挂着一个 daemon；命令经同一通道串行化，事件广播给所有订阅者。

## 4. 组件与职责

### 4.1 hmp-core 新增（纯领域，无 I/O）

**`queue.rs` — `QueueCore`**：纯逻辑队列（`Vec<TrackId>` + 当前序号 + loop/shuffle）。

- 操作：`replace`（清空+设当前）、`append`、`insert_next`（playnext）、`remove`、`clear`、`current`、`next_track`、`prev_track`、`set_loop_mode`、`set_shuffle`。
- 语义：`prev` 一律跳上一首（不做 >3s 回开头）；`LoopMode::List` 到头回绕、`Track` 单曲循环、`None` 播完停；shuffle 随机选下一首（排除当前）。
- 纯逻辑、无 I/O、可单测。

**`ipc.rs` — 协议消息类型**（serde，跨进程用）：

```rust
pub enum PlayRequest {
    Track(TrackId),
    Playlist(String),  // 歌单/专辑 id，由 daemon 拉取曲目列表
    Album(String),
}

pub enum Request {
    Play(PlayRequest),
    PlayNext(PlayRequest),
    QueueAppend(PlayRequest),
    QueueRemove(usize),          // 队列位置（0 基索引）
    QueueClear,
    Queue,                       // 查询队列
    Command(PlayerCommand),      // Play/Pause/Stop/Seek/Volume/Loop/Shuffle/Next/Previous
    Status,                      // 全量状态（DaemonState）
    Subscribe,                   // 订阅事件流
    Quit,
}

pub enum Response {
    Ok,
    Status(DaemonState),
    Queue(QueueSnapshot),
    Err { code: IpcErrorCode, message: String },
}

pub enum Event {
    StateChanged(DaemonState),
}

pub struct DaemonState {
    pub playback: PlaybackState,
    pub queue: QueueSnapshot,
    pub caps: PlaybackCapabilities,
}

pub enum IpcErrorCode {
    NotLoggedIn, TrackNotFound, PlaylistNotFound, QualityUnavailable, BadRequest, Internal,
}
```

播放器自身错误（如格式不支持）以 `PlaybackStatus::Error` 状态呈现，不走 RPC 错误。

### 4.2 hmp-daemon 新增 crate（后端，不引用任何前端类型）

| 模块 | 职责 |
|---|---|
| `server.rs` | Unix socket（`$XDG_RUNTIME_DIR/hmp.sock`，回退 `/tmp/hmp-$UID.sock`）；accept 循环 + 每连接任务；多客户端并发；长度前缀 JSON 帧（`u32 LE + JSON`，1 MiB 上限）；请求进命令通道；订阅者收事件 fan-out |
| `daemon.rs` | 后端核心：持有 `QueueCore` + `PlayerCore` + `QqMusicClient`；单条命令通道（mpsc）串行化输入；单一 `watch<DaemonState>` 发布复合状态（播放部分转自 PlayerCore watch，队列部分转自 QueueCore 变更） |
| `player.rs` | 播放循环：`PlayRequest` → 详情/歌单拉取 → 音质回退 → `hmp-media::prepare_stream`（解密 guard 随 daemon 存活）→ 加载播放；监听 `Ended` 自动续下一首（按 loop/shuffle 语义）；`PlaybackDriver` trait 可注入（测试用 fake driver） |
| `tray.rs` | ksni 最小菜单（播放/暂停、上一首、下一首、停止、退出），图标随 Play/Pause 切换；前端适配器，与后端核心零耦合 |
| `mpris.rs` | 现有 hmp-mpris 服务原样搬入（已具备 watch + 命令通道两个接口，天然适配） |
| `serve.rs` | `hmp serve` 前台调试 / `hmp serve --background` 供 CLI 拉起（detached 新会话、stdio 丢弃） |

**`PlaybackDriver` trait**（`player.rs`）：`load(track, uri, quality)` / `play()` / `pause()` / `seek()` / `stop()` / `state_watch()`。生产实现包 `PlayerCore`；测试注入 fake。队列裁决、Ended 自动续播、循环/洗牌逻辑在 driver 之上，无 gst/无网络可测。

### 4.3 CLI 变更

| 模块 | 职责 |
|---|---|
| `client.rs` | 连接 socket + 请求/响应；未运行则自动 spawn daemon（`hmp serve --background`）并轮询 socket 就绪（≤3s）；处理 `ECONNREFUSED` 残留清理 |
| `qr_ascii.rs` | 登录二维码终端渲染：解码 `QR.data`（`image` crate，png/jpeg）→ 降采样到终端宽度（环境 `COLUMNS` 或默认 ~60，上下限 32..=120）→ 2:1 纵横比校正（终端字符高≈宽×2，半块 Unicode 字符 `▀▄█` + 空格，每字符承载 2×2 像素）→ 亮度映射输出到 stdout |
| 子命令 | `play/playnext/queue(show\|add\|remove\|clear)/playlist/album` + `pause/resume/next/prev/stop/seek/volume/loop/shuffle/status/quit` + `serve` |
| 保留 | `login`（交互式，仍在 CLI）、`search`（本地搜索，输出 track-id 供 `hmp play` 使用） |

**`hmp login` 增强流程**（`qr_ascii.rs`）：

1. `get_qrcode` 后先尝试解码图像并渲染 ASCII 到 stdout（不依赖 tty 检测，非 tty 也打印）；
2. PNG 仍保存到临时目录（`hmp-qr.png`）作**兜底**：图像解码失败或渲染异常时提示用户手动打开该文件；
3. **过期自动刷新**：渲染后进入轮询；`wait_qrcode_login` 返回超时类错误（二维码过期 `Timeout` 事件或整体超时）时，自动重新 `get_qrcode` → 重新渲染 → 继续轮询，无需用户重跑命令；总墙钟上限（10 分钟）防无限循环（一直不扫也不死循环）；
4. **输出约定**：二维码渲染与轮询提示全部用 `write!` + `stdout().flush()` 显式冲刷，**不使用裸 `println!`**（管道/重定向场景下避免缓冲导致二维码延迟显示；与 `hmp play` 进度行的既有约定一致）。
5. 渲染成功后正常进入 `wait_qrcode_login` 轮询（扫码结果提示沿用现有逻辑）。

## 5. 协议与数据流

- **传输**：Unix socket（SOCK_STREAM），长度前缀 JSON 帧（`u32 LE 长度 + JSON 字节`，上限 1 MiB）。
- **请求/响应**：每请求必有响应（`Ok` / `Status` / `Queue` / `Err{code,message}`）；未知/畸形帧 → `Err(BadRequest)` 或断连；单连接可连续多发（keep-alive）。
- **订阅**：`Subscribe` 后服务端流式推送 `Event::StateChanged(DaemonState)`（状态变更即推，含初始快照）；CLI 默认不订阅（`status` 一次性轮询），tray 在进程内订阅 watch。
- **数据流**：
  - 输入：socket 请求 / tray 菜单 / MPRIS 方法 → 命令通道（串行）→ 队列核心裁决 → PlayerCore。
  - 输出：PlayerCore watch + QueueCore 变更 → `watch<DaemonState>` → 广播到订阅者（MPRIS 属性、tray 图标、socket 事件流）。

## 6. 生命周期与进程管理

- **拉起**：`hmp <cmd>` → 尝试连 socket；`ENOENT` → spawn 当前可执行文件 `hmp serve --background`（新会话 detached、stdio 丢弃）→ 轮询 socket 就绪（≤3s）→ 发指令。`ECONNREFUSED`（残留 sock 文件、进程已死）→ 清理残留文件后重新 spawn。
- **单实例**：bind 失败即已在跑（sock 文件存在即信号）。
- **退出**：`hmp quit` / tray「退出」/ 前台 Ctrl+C（SIGTERM/SIGINT）→ 优雅退出：停播、清理 sock 文件、释放 MPRIS bus 名、关 tray。
- **队列播完**：daemon 保持存活（状态 `Ended`/空闲）等新指令，只有 `quit` 才终止。
- **凭证**：daemon 读与 CLI 共用的 hmp-storage 凭证（登录仍在 CLI）。Play 前置校验：无有效凭证 → `Err(NotLoggedIn)`（同步返回）。

## 7. 错误处理

- **命令-查询分离**：命令（Play/PlayNext/QueueAppend…）只返回"已受理"（`Ok`），真正结果通过 `DaemonState`（Loading→Playing/Error）可见。`hmp play` 发命令后短轮询状态（~15s 上限）确认进入 Playing 或打印 Error 原因。理由：取流是秒级网络操作，不让一个 RPC 响应阻塞到底。
- **错误码全集**：`NotLoggedIn | TrackNotFound | PlaylistNotFound | QualityUnavailable | BadRequest | Internal`。
- 播放器 Error 状态：广播状态，**不自动跳歌**（与桌面一致，等用户指令）。

## 8. 测试策略

- **hmp-core**：`QueueCore` 纯逻辑单测（replace/append/insert/remove/clear、List 回绕、Track 循环、shuffle、prev 恒跳上一首）；ipc 消息 serde round-trip。
- **hmp-daemon**：
  - 播放层 `PlaybackDriver` 注入 fake——队列裁决、Ended 自动续播、循环/洗牌逻辑全部无 gst/无网络可测；
  - 协议集成：真实 socket + 真协议客户端，多客户端并发、订阅 fan-out、畸形帧；
  - wiremock 端到端（真实 gst + 本地生成 wav + 假 QQ API）验证 Play→详情→回退→解密→播放→Ended→下一首 闭环；
  - tray/MPRIS 默认 features 开关——无桌面环境（CI）下跑 backend-only。
- **CLI**：client 单测（ENOENT 拉起、ECONNREFUSED 恢复、超时）；`qr_ascii` 渲染单测（已知像素图 → 断言输出字符序列、宽度钳位 32..=120、纵横比 2:1、解码失败走 PNG 兜底）；`login` 刷新循环单测（伪造超时错误 → 断言重新取码/重渲染调用、墙钟上限生效）；集成：起 daemon → `hmp status`/`hmp pause` 断言。

## 9. 里程碑拆分建议（供 writing-plans 细化）

1. **hmp-core**：`QueueCore` + `ipc` 消息（含单测）。
2. **hmp-daemon 后端**：`PlaybackDriver` + `daemon.rs` + `player.rs`（fake driver 单测）。
3. **hmp-daemon 传输**：`server.rs` socket 服务器 + 协议集成测试。
4. **CLI 接入**：`client.rs` + 子命令 + spawn 逻辑 + 集成测试；`qr_ascii.rs` 终端二维码渲染（`image` crate 解码 + 半块字符映射）。
5. **tray + MPRIS 搬入**：ksni tray + mpris 适配 + 优雅退出。
6. **端到端 + 文档**：wiremock 闭环测试、PROJECT.md/README 更新。
