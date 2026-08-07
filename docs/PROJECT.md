# HMP（hutao-music-player）项目设计与开发文档

> 项目名：hutao-music-player
> 缩写：HMP
> 主程序命令：`hmp`
> 当前阶段：CLI 原型（login/search/play 闭环）/ 主项目 crate 骨架
> 目标平台：Linux 桌面，优先 Wayland；首要适配 Arch Linux + Niri
> 文档用途：作为仓库内的项目总纲、架构说明、开发路线和验收标准

---

## 1. 项目摘要

HMP 是一个以 QQ 音乐为首要音源、使用 Rust 构建的轻量桌面音乐播放器。

项目的核心目标不是复刻 QQ 音乐官方客户端的全部功能，而是建立一个稳定、可维护、资源占用可控，并且完整适配 Linux 桌面协议的播放器。重点解决官方 Linux 客户端存在的系统媒体控制不完整、专辑封面未正确导出、播放进度无法由外部组件调整等问题。

HMP 第一阶段采用以下技术路线：

```text
Rust 2024
+ Tokio
+ Reqwest
+ Serde
+ GStreamer
+ mpris-server
+ Slint
+ SQLite
+ Linux Secret Service / keyring
```

总体原则：

1. QQ 音乐协议和业务逻辑全部在 Rust 中实现。
2. Python QQMusicApi 仅作为参考实现、行为规范和差分测试 Oracle，不作为运行时依赖。
3. UI 不接触 Cookie、签名、播放 URL 鉴权等敏感信息。
4. 播放状态只保留一个权威来源，UI 与 MPRIS 均订阅同一状态。
5. 首版不追求全功能，先完成扫码登录、搜索、会员取流、播放、Seek、封面、歌词和完整 MPRIS 闭环。

---

## 2. 项目定位

### 2.1 核心用户

HMP 首先服务以下用户：

- 主要使用 QQ 音乐账号和会员曲库；
- 使用 Linux 桌面，尤其是 Wayland 合成器；
- 希望系统媒体键、状态栏、通知中心、锁屏或桌面 Shell 能完整控制播放器；
- 不需要直播、社区、短视频、K 歌、弹幕、游戏专区等附属功能；
- 关注内存、后台 CPU、启动时间和长期运行稳定性；
- 接受第三方客户端的接口维护成本，但不希望依赖 Python、Node 服务或额外守护进程。

### 2.2 项目目标

HMP 的正式目标包括：

- QQ / 微信扫码登录；
- 登录态安全持久化与自动续期；
- QQ 音乐歌曲搜索；
- 用户歌单、收藏歌单和歌单详情；
- 使用用户自身会员权限获取播放地址；
- 支持常见有损、无损和 Hi-Res 音质；
- 支持播放、暂停、上一首、下一首、Seek、音量、循环和随机；
- 完整实现 MPRIS Player 接口；
- 向 MPRIS 提供本地可访问的专辑封面；
- 支持原文歌词和翻译歌词；
- 提供轻量、适合桌面日常使用的图形界面；
- 遵循 XDG 目录规范；
- 可通过 Arch Linux 包安装；
- 核心 API、播放逻辑和 UI 之间保持清晰边界。

### 2.3 非目标

以下内容不作为 v1.0 的必需目标：

- 完整复刻 QQ 音乐官方 UI；
- MV 播放；
- 评论、动态、私信和社区功能；
- 直播、播客、K 歌、听歌识曲；
- Windows 和 macOS 的首发支持；
- 多音源聚合；
- 浏览器端或移动端；
- 云端账户服务；
- 破解会员、绕过版权或规避区域限制；
- 下载并解密官方缓存格式；
- 向外部网络开放本地控制 API。

---

## 3. 核心技术决策

### 3.1 语言

全部核心组件使用 Rust。

原因：

- 能够构建单一可执行程序；
- 运行时占用可控；
- 适合实现状态机、异步网络、D-Bus 和长期运行服务；
- 类型系统适合约束复杂接口响应；
- 能够把 API、播放、MPRIS 和 UI 组织在同一工作区；
- 对 Linux 打包和发行较友好。

### 3.2 UI：Slint

首选 Slint，而不是 Tauri。

选择 Slint 的原因：

- 不依赖 WebView；
- 适合自定义消费级播放器界面；
- 可实现虚拟化歌曲列表；
- 能将 UI 声明与 Rust 业务逻辑分离；
- 更符合本项目的低资源目标；
- 对 Wayland 和高 DPI 桌面友好。

限制：

- 需要维护 `.slint` 文件；
- Linux 输入法、无障碍和桌面原生细节需要专门测试；
- 不能直接复用 Web 生态组件。

备选方案：

- 若 Slint 在输入法、可访问性或特定 Wayland 环境中出现无法接受的问题，第二选择为 Relm4 + GTK4。
- Tauri 仅保留为快速原型或未来跨平台 UI 的备选，不作为首版默认架构。

### 3.3 音频后端：GStreamer

首版使用 GStreamer 高层播放接口。

原因：

- 能处理网络流、缓冲、格式探测和 Seek；
- 支持常见音频格式；
- Linux 桌面部署成熟；
- 避免首版自行实现 HTTP Range、解码线程、设备切换和错误恢复。

首版不采用纯 Rust 解码链。未来只有在性能、依赖体积或部署方面出现明确问题时，再评估 Symphonia + CPAL/Rodio。

### 3.4 桌面媒体协议：MPRIS

使用 `mpris-server` 让 HMP 自身成为 MPRIS 播放器。

必须完整支持：

```text
PlaybackStatus
LoopStatus
Shuffle
Volume
Position
CanGoNext
CanGoPrevious
CanPlay
CanPause
CanSeek
Play
Pause
PlayPause
Stop
Next
Previous
Seek
SetPosition
Seeked
Metadata
```

Metadata 至少包含：

```text
mpris:trackid
mpris:length
mpris:artUrl
xesam:title
xesam:album
xesam:artist
xesam:albumArtist
xesam:url
```

专辑封面应下载到本地缓存，并通过 `file://` URL 暴露，避免状态栏或桌面 Shell 无法访问远程图片。

### 3.5 数据存储

- SQLite：本地歌单缓存、歌曲元数据、播放历史、封面索引、歌词缓存和迁移版本。
- keyring / Secret Service：登录凭据、Cookie、music key、refresh key。
- TOML：非敏感配置。

不得将完整 Cookie 以明文写入普通配置文件或数据库。

---

## 4. 总体架构

### 4.1 逻辑结构

```text
┌──────────────────────────────────────────┐
│                  Slint UI                │
│ 搜索 / 歌单 / 播放栏 / 歌词 / 设置       │
└───────────────────┬──────────────────────┘
                    │ UI Command / UI State
                    ▼
┌──────────────────────────────────────────┐
│              HMP Application Core        │
│  单一播放状态源 / 队列 / 会话 / 业务编排   │
└──────────────┬───────────────┬───────────┘
               │               │
               ▼               ▼
┌─────────────────────┐  ┌─────────────────┐
│ QQ Music Rust Client│  │ GStreamer Player │
│ 登录/搜索/歌单/取流 │  │ 播放/缓冲/Seek   │
└──────────┬──────────┘  └────────┬────────┘
           │                       │
           ▼                       ▼
┌─────────────────────┐  ┌─────────────────┐
│ Storage / Credential│  │ MPRIS Service    │
│ SQLite + keyring    │  │ 系统媒体控制      │
└─────────────────────┘  └─────────────────┘
```

### 4.2 单一状态源

播放状态必须只由 `PlayerCore` 管理。

```text
UI ───────┐
          ├── PlayerCommand ──> PlayerCore ──> Audio Backend
MPRIS ────┘                         │
                                    ▼
                              PlaybackState
                                    │
                         ┌──────────┴──────────┐
                         ▼                     ▼
                        UI                   MPRIS
```

禁止出现以下结构：

- UI 自己维护一份播放进度；
- MPRIS 自己推算另一份进度；
- GStreamer 状态只在后端内部可见；
- 当前歌曲由多个模块分别修改。

### 4.3 并发模型

推荐：

- Tokio 多线程运行时；
- `mpsc` 发送命令；
- `watch` 发布当前播放状态；
- `broadcast` 发布离散事件；
- API 请求使用结构化任务；
- UI 线程只负责渲染和事件转发。

示例：

```rust
pub enum PlayerCommand {
    LoadAndPlay(TrackId),
    Play,
    Pause,
    Stop,
    Seek(std::time::Duration),
    Next,
    Previous,
    SetVolume(f64),
    SetLoopMode(LoopMode),
    SetShuffle(bool),
}

pub struct PlaybackState {
    pub status: PlaybackStatus,
    pub current: Option<Track>,
    pub position: std::time::Duration,
    pub duration: Option<std::time::Duration>,
    pub volume: f64,
    pub can_seek: bool,
    pub buffering: Option<f64>,
}
```

### 桌面 UI 功能状态

| 功能 | 状态 | 说明 |
| --- | --- | --- |
| 登录 | 已接入 | QQ 音乐扫码登录与凭据状态 |
| 搜索 | 已接入 | 使用 QQ Music Rust API |
| 播放控制 | 已接入 | 播放、暂停、上一首、下一首、Seek、音量 |
| 队列展示 | 已接入 | 展示 AppCore 当前真实队列 |
| 歌词展示 | 部分接入 | 已接入接口与空状态，按真实返回展示 |
| 推荐内容 | 开发中 / 演示数据 | 当前使用本地演示数据 |
| 收藏与资料库同步 | 开发中 | 尚未接入账号云端同步 |

---

## 5. Cargo 工作区设计

### 5.1 推荐的初始结构

不要一开始拆出十几个 crate。首阶段建议控制在以下范围：

```text
hutao-music-player/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── LICENSE
├── rustfmt.toml
├── clippy.toml
├── deny.toml
├── .editorconfig
├── .gitignore
├── docs/
│   ├── PROJECT.md
│   ├── ARCHITECTURE.md
│   ├── QQMUSIC_PORTING.md
│   ├── MPRIS.md
│   └── TESTING.md
├── crates/
│   ├── hmp-core/
│   ├── hmp-qqmusic/
│   ├── hmp-player-gst/
│   ├── hmp-mpris/
│   ├── hmp-storage/
│   └── hmp-desktop/
├── ui/
│   ├── app.slint
│   ├── components/
│   ├── pages/
│   └── theme/
├── fixtures/
│   └── qqmusic/
├── scripts/
└── xtask/
```

### 5.2 各 crate 职责

#### `hmp-core`

只存放稳定领域模型和应用层协议：

- `Track`
- `Artist`
- `Album`
- `Playlist`
- `CredentialSummary`
- `PlaybackState`
- `PlayerCommand`
- `LoopMode`
- `Quality`
- 核心错误分类

不得依赖 Slint、GStreamer、SQLite 或具体 QQ 接口字段。

#### `hmp-qqmusic`

QQ 音乐 Rust 客户端：

- HTTP Client；
- 默认 `comm` 参数；
- Cookie 和登录态；
- QQ / 微信扫码；
- Token 刷新；
- 搜索；
- 歌曲、专辑、歌手和歌单；
- 播放 URL；
- 歌词；
- 原始响应到领域模型的转换；
- 接口 fixture 测试。

该 crate 不依赖 UI、MPRIS 或播放器。

#### `hmp-player-gst`

- GStreamer 初始化；
- 播放、暂停、停止；
- URI 加载；
- 缓冲；
- Seek；
- 音量；
- 错误和 EOS；
- 状态事件转换。

该 crate 不直接调用 QQ API。

#### `hmp-mpris`

- MPRIS 服务注册；
- 将 MPRIS 命令转换为 `PlayerCommand`；
- 将 `PlaybackState` 转换成 MPRIS 属性；
- Metadata 和封面 URL；
- Seeked 信号；
- D-Bus 生命周期。

#### `hmp-storage`

- SQLite 初始化和迁移；
- XDG 路径；
- 配置读写；
- 封面和歌词缓存索引；
- keyring 凭据访问；
- 缓存清理策略。

#### `hmp-desktop`

主程序：

- Tokio 运行时；
- Slint UI；
- 应用状态编排；
- API、播放器、MPRIS 和存储组合；
- 程序生命周期；
- 单实例处理；
- 桌面通知；
- `hmp` 可执行文件。

### 5.3 可选的后期拆分

当 `hmp-qqmusic` 体积明显增长后，再考虑拆分：

```text
hmp-qqmusic-protocol
hmp-qqmusic-client
```

首阶段不建议拆分，避免 API 尚未稳定时增加跨 crate 修改成本。

### 5.4 根 Cargo.toml 示例

```toml
[workspace]
members = [
    "crates/hmp-core",
    "crates/hmp-qqmusic",
    "crates/hmp-player-gst",
    "crates/hmp-mpris",
    "crates/hmp-storage",
    "crates/hmp-desktop",
    "xtask",
]
resolver = "3"

[workspace.package]
edition = "2024"
license = "GPL-3.0-or-later"
repository = "https://github.com/<owner>/hutao-music-player"
rust-version = "<project-msrv>"

[workspace.dependencies]
anyhow = "1"
async-trait = "0.1"
base64 = "0.22"
bytes = "1"
directories = "6"
futures = "0.3"
reqwest = { version = "0.12", default-features = false, features = [
    "json",
    "cookies",
    "gzip",
    "brotli",
    "deflate",
    "rustls-tls",
] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = [
    "rt-multi-thread",
    "macros",
    "sync",
    "time",
    "fs",
] }
tracing = "0.1"
tracing-subscriber = "0.3"
url = "2"
uuid = { version = "1", features = ["v4", "serde"] }
```

实际依赖版本应通过 `cargo add` 使用当前稳定版，再提交 `Cargo.lock`。本文不要求长期固定所有次版本。

---

## 6. QQMusicApi Rust 移植方案

### 6.1 原则

移植目标是复制行为，不是逐行翻译 Python 语法。

Python 项目的角色：

- 接口目录；
- 请求参数参考；
- 登录状态机参考；
- 签名和加密算法参考；
- 响应字段参考；
- 差分测试 Oracle。

Rust 项目应重新定义：

- 模块边界；
- 错误类型；
- 数据模型；
- 并发模型；
- 凭据管理；
- 测试结构。

### 6.2 分层

```text
QQ 原始请求/响应
        │
        ▼
wire DTO / serde_json::Value
        │
        ▼ normalize
领域模型 Track / Album / Artist / Playlist
        │
        ▼
应用核心 / UI / MPRIS
```

初期允许使用 `serde_json::Value` 处理不稳定接口，稳定后逐步替换为强类型 DTO。

不得让 UI 直接依赖 QQ 返回字段，例如：

```text
songmid
albumMid
singerMID
midurlinfo
v_playlist
```

这些字段只能存在于 `hmp-qqmusic` 内部。

### 6.3 通用 Client

```rust
pub struct QqMusicClient {
    http: reqwest::Client,
    config: ClientConfig,
}

pub struct ClientConfig {
    pub timeout: std::time::Duration,
    pub user_agent: String,
    pub max_retries: usize,
}
```

所有接口统一走同一个请求入口：

```rust
impl QqMusicClient {
    async fn musicu_request<T>(
        &self,
        request: CgiRequest,
        credential: Option<&Credential>,
    ) -> Result<T, QqMusicError>
    where
        T: serde::de::DeserializeOwned,
    {
        todo!()
    }
}
```

该入口负责：

- 构造 `comm`；
- 注入 Cookie（来自请求级传入的 `credential`）；
- 注入 Referer 和 User-Agent；
- 超时；
- HTTP 状态检查；
- QQ 业务错误码；
- JSON 解包；
- 可重试错误；
- 日志脱敏；
- 请求追踪 ID。

### 6.4 凭据模型

**设计决策（2026-08-06）：凭证完全解耦，客户端无全局凭证状态。**

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Credential {
    pub uin: String,
    pub music_id: String,
    pub music_key: String,
    pub refresh_key: Option<String>,
    pub login_type: LoginType,
    pub raw_cookie: String,
}
```

要求：

- `Debug` 输出必须自定义脱敏，不能直接 derive 后打印；
- 日志只能输出是否存在某字段，不能输出字段内容；
- UI 只接收 `LoggedInUser`，不接收 Credential；
- keyring 中保存敏感信息；
- SQLite 只保存非敏感账户摘要和缓存数据。

凭证生命周期约定：

- **本 crate 不负责任何凭证轮换**：不存在自动刷新任务、定时器或会话恢复逻辑；
- **刷新只通过显式接口**：`LoginApi::refresh_credential(&self, credential: &Credential) -> Credential`，
  调用方传入需要刷新的凭证，取回刷新后的新凭证；
- **请求级传参**：需要登录态的请求由调用方逐次传入 `credential: Option<&Credential>`，
  便于调用方同时管理多个账号凭证；
- 调用方负责凭证的存储（keyring）与过期判断；客户端仅在响应中返回业务错误码
  （如 `CredentialExpired`）供调用方决定是否刷新。

> 实现参考：上游 `LoginApi.refresh_credential`。

### 6.5 登录状态机

```text
Idle
→ CreatingQr
→ WaitingForScan
→ WaitingForConfirm
→ ExchangingToken
→ LoggedIn

异常：
Expired / Refused / NetworkError / InvalidResponse
```

登录实现必须支持取消。用户关闭登录弹窗后，应停止轮询任务。

实现要点（2026-08-06，阶段 B）：

- `LoginApi::get_qrcode(QRLoginType::Qq)` → 扫码图片 + `identifier`（qrsig）；
- `LoginApi::check_qrcode(&QR)` → 单次状态检查（`QRCodeLoginEvents`）；
- `LoginApi::wait_qrcode_login(qrcode, PollInterval, timeout, Option<&CancellationToken>)`
  内置轮询/去重/指数退避/超时，取消时立即返回错误；
- 完整链路：ptqrshow → ptqrlogin → check_sig → oauth authorize → QQLogin CGI；
- 登录/刷新均不产生全局状态，凭证由调用方显式传入并自行存储（§6.4）。

### 6.6 移植顺序

#### 阶段 A：基础请求层

1. Client 构造；
2. 通用 Header；
3. `musicu.fcg` 请求；
4. 错误码检查；
5. 日志脱敏；
6. fixture 读写工具。

验收：免登录搜索请求成功，能够解析至少一个歌曲结果。

#### 阶段 B：登录

1. QQ 二维码创建；
2. QQ 二维码轮询；
3. 授权码交换；
4. Cookie 解析；
5. 凭据刷新；
6. 微信扫码；
7. keyring 持久化（hmp-storage，Secret Service）；
8. 重启恢复登录态。

验收：重启 HMP 后不需要重新扫码，并能获取会员播放 URL。

#### 阶段 C：最小播放闭环

1. 搜索歌曲；
2. 歌曲基础信息；
3. 可用音质；
4. 播放 URL；
5. 专辑封面；
6. 原文歌词；
7. 翻译歌词。

验收：

```text
扫码登录
→ 搜索
→ 播放会员歌曲
→ 显示封面
→ 显示歌词
→ MPRIS 可拖动进度
```

#### 阶段 D：账号和歌单

1. 当前用户；
2. 用户歌单；
3. 收藏歌单；
4. 歌单详情；
5. 专辑详情；
6. 歌手详情；
7. 每日推荐。

#### 阶段 E：写操作

1. 收藏 / 取消收藏；
2. 添加到歌单；
3. 从歌单删除；
4. 新建歌单；
5. 删除歌单。

写操作必须在 UI 中明确显示失败原因，且不得静默重试可能造成重复提交的请求。

### 6.7 差分测试

fixtures 目录建议：

```text
fixtures/qqmusic/
├── auth/
│   ├── qr_created.json
│   ├── qr_waiting.txt
│   ├── qr_confirmed.txt
│   └── token_exchange.json
├── search/
│   └── track.json
├── song/
│   ├── detail.json
│   └── vkey.json
├── playlist/
│   ├── user_playlists.json
│   └── detail.json
└── lyric/
    └── lyric.json
```

测试分为三类：

1. 纯解析测试：离线、稳定、CI 默认运行；
2. Python/Rust 差分测试：本地开发运行；
3. Live 测试：需要真实账号和网络，默认忽略。

Live 测试不得在公共 CI 中使用个人 Cookie。

---

## 7. 领域模型

### 7.1 Track

```rust
pub struct Track {
    pub id: TrackId,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album: Option<AlbumRef>,
    pub duration: Option<std::time::Duration>,
    pub cover: Option<CoverRef>,
    pub qualities: Vec<AudioQuality>,
}
```

### 7.2 标识符

不要在整个程序中直接传递裸 `String`。

```rust
pub struct TrackId(pub String);
pub struct AlbumId(pub String);
pub struct ArtistId(pub String);
pub struct PlaylistId(pub String);
```

这样可以避免把歌单 ID 误传给歌曲接口。

### 7.3 音质

```rust
pub enum AudioQuality {
    Mp3_128,
    Mp3_320,
    Aac,
    Flac,
    HiRes,
    Atmos,
    Master,
    Unknown(String),
}
```

播放请求应允许质量回退：

```text
用户选择 HiRes
→ HiRes 不可用
→ FLAC
→ MP3 320
→ MP3 128
```

是否自动回退由设置控制。发生回退时 UI 应提示一次，不应静默让用户误以为正在播放目标音质。

> **加密音质**：QQ 音乐的无损及以上音质（FLAC/HiRes/Atmos/Master，即 `.mflac`/`.mgg`/`.mmp4` 等）为加密文件，需要客户端用接口返回的 `ekey` 解密后才能播放；当前播放器尚未实现解密，取流时将这些格式视为不可用并直接回退到可播放的明文音质（MP3/AAC）。实现 QMC 解密后应恢复无损链。

---

## 8. 播放器核心

### 8.1 状态机

```text
Empty
Loading
Buffering
Playing
Paused
Stopped
Ended
Error
```

状态转换只能发生在播放器核心中。

### 8.2 播放流程

```text
选择歌曲
→ 查询播放 URL
→ 校验 URL 和有效期
→ 设置 GStreamer URI
→ 进入 Loading
→ preroll
→ Playing
→ 周期性发布 Position
→ EOS 后执行队列策略
```

### 8.3 URL 过期

QQ 播放 URL 可能存在有效期。队列中不应长期保存已解析 URL。

应保存：

```text
TrackId + Quality
```

真正开始播放前再解析 URL。暂停较长时间后若恢复失败，可重新取流一次。

### 8.4 进度

- 播放状态中记录 `position` 和 `duration`；
- UI 更新频率建议 4–10 Hz；
- MPRIS `Position` 使用后端真实位置；
- Seek 成功后发出 `Seeked`；
- 用户拖动时 UI 可本地预览，但释放后必须以播放器返回位置为准。

### 8.5 队列

```rust
pub struct PlaybackQueue {
    pub tracks: Vec<TrackId>,
    pub current_index: Option<usize>,
    pub loop_mode: LoopMode,
    pub shuffle: bool,
    pub shuffle_order: Vec<usize>,
}
```

随机播放不能每次 Next 都重新随机，否则 Previous 无法返回上一首。应生成并维护稳定的随机顺序。
即需要 shuffle 一个新的播放队列

---

## 9. MPRIS 实现要求

### 9.1 身份

建议：

```text
Bus name: org.mpris.MediaPlayer2.hmp
Identity: HMP
DesktopEntry: hmp
```

### 9.2 Seek

必须同时实现：

- `CanSeek = true`；
- `Seek(offset)`；
- `SetPosition(track_id, position)`；
- `Seeked(position)`。

`SetPosition` 中必须校验传入的 TrackId 是否为当前曲目，避免旧控制器对新歌曲执行错误 Seek。

### 9.3 封面

流程：

```text
远程封面 URL
→ 下载
→ 内容哈希 / AlbumId 命名
→ 写入 XDG cache
→ MPRIS mpris:artUrl = file://...
```

缓存失败时可以回退远程 URL，但本地文件应作为默认方案。

### 9.4 测试命令

```bash
playerctl -l
playerctl -p hmp metadata
playerctl -p hmp position
playerctl -p hmp position 60
playerctl -p hmp play-pause
playerctl -p hmp next
```

还应使用 `busctl --user` 检查属性和方法。

---

## 10. UI 设计

### 10.1 首版页面

```text
主窗口
├── 侧边栏
│   ├── 搜索
│   ├── 我的歌单
│   ├── 每日推荐（后续）
│   └── 设置
├── 内容区域
│   ├── 搜索结果
│   ├── 歌单详情
│   └── 歌词页
└── 底部播放栏
    ├── 封面
    ├── 标题 / 歌手
    ├── 上一首 / 播放暂停 / 下一首
    ├── 进度
    ├── 音量
    ├── 音质
    └── 队列
```

### 10.2 首版登录 UI

- 显示二维码；
- 展示“等待扫码 / 已扫码待确认 / 已过期 / 登录成功”；
- 提供重新生成；
- 提供 Cookie 登录作为高级选项；
- 不在界面中显示 Cookie 明文；
- 关闭窗口时取消轮询。

### 10.3 UI 与业务层边界

Slint 回调只发送意图：

```text
search(query)
play(track_id)
pause()
seek(ms)
select_playlist(id)
login()
logout()
```

UI 不直接执行：

- HTTP 请求；
- SQLite；
- keyring；
- GStreamer；
- D-Bus；
- Cookie 拼接；
- QQ 响应解析。

### 10.4 列表性能

- 使用虚拟化列表；
- 避免一次性实例化数千个歌曲行；
- 封面异步加载；
- 搜索输入设置防抖；
- 不在 UI 线程解码图片；
- 大图生成缩略图后缓存。

---

## 11. 存储和 XDG 路径

建议路径：

```text
~/.config/hmp/config.toml
~/.local/share/hmp/hmp.db
~/.cache/hmp/covers/
~/.cache/hmp/lyrics/
~/.cache/hmp/http/
```

凭据存入桌面 Secret Service，服务名建议：

```text
application: hmp
account: qqmusic:<uin>
```

### 11.1 SQLite 初始表

```text
schema_migrations
tracks
artists
albums
playlists
playlist_tracks
play_history
cover_cache
lyric_cache
```

QQ 在线歌单不应完全复制成永久本地真相。SQLite 中的数据主要是缓存，应保留同步时间和来源。

### 11.2 配置示例

```toml
[playback]
preferred_quality = "flac"
allow_quality_fallback = true
volume = 0.8
resume_on_start = false

[ui]
theme = "system"
show_translation = true
close_to_tray = false

[cache]
max_cover_mib = 256
max_lyric_mib = 64

[network]
timeout_seconds = 15
retry_count = 2
```

---

## 12. 错误模型

```rust
#[derive(Debug, thiserror::Error)]
pub enum HmpError {
    #[error("network error: {0}")]
    Network(String),

    #[error("authentication required")]
    AuthenticationRequired,

    #[error("credential expired")]
    CredentialExpired,

    #[error("QQ Music API error {code}: {message}")]
    QqApi { code: i64, message: String },

    #[error("track is unavailable")]
    TrackUnavailable,

    #[error("quality is unavailable")]
    QualityUnavailable,

    #[error("playback error: {0}")]
    Playback(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("invalid response: {0}")]
    InvalidResponse(String),
}
```

UI 错误需要转换为用户可读提示，但日志中保留结构化上下文。

不得将所有错误转换成：

```text
Something went wrong
```

至少区分：

- 未登录；
- 登录失效；
- 网络失败；
- 接口响应变化；
- 无版权；
- 会员等级不足；
- 目标音质不可用；
- 音频后端失败；
- 本地存储失败。

---

## 13. 日志与诊断

使用 `tracing`。

建议环境变量：

```bash
RUST_LOG=hmp=debug,hmp_qqmusic=trace hmp
```

日志字段示例：

```text
request_id
module
method
http_status
qq_code
track_id
quality
latency_ms
retry
```

禁止记录：

- 完整 Cookie；
- music key；
- refresh key；
- OAuth code；
- 二维码授权跳转 URL 中的敏感参数；
- 完整播放 URL 查询参数。

建议提供诊断导出功能，自动清除敏感字段后生成文本报告。

---

## 14. 测试策略

### 14.1 单元测试

- Cookie 解析；
- 登录回调解析；
- hash / sign / crypto；
- API 错误码解析；
- 搜索响应解析；
- 歌词解析；
- 质量回退；
- 队列随机顺序；
- MPRIS Metadata 构建；
- XDG 路径。

### 14.2 集成测试

- 模拟 HTTP 服务；
- 固定 fixture；
- URL 刷新；
- 播放状态转换；
- SQLite 迁移；
- MPRIS D-Bus 调用。

### 14.3 Live 测试

使用 feature 或 ignored test：

```text
cargo test --features live-tests -- --ignored
```

需要环境变量：

```text
HMP_QQMUSIC_COOKIE
HMP_LIVE_TEST_TRACK_ID
```

这些变量不得写入仓库。

### 14.4 UI 测试

首阶段不追求完整截图测试，但应人工覆盖：

- 100%、125%、150%、200% 缩放；
- 中文输入法；
- Wayland；
- Niri；
- 窗口缩放；
- 深色 / 浅色；
- 长标题和多歌手；
- 断网；
- 登录过期；
- 1000 首以上歌单。

---

## 15. 性能目标

这些是工程目标，不是发布承诺。应在真实设备上持续测量。

### 15.1 首版目标

- 空闲后台 CPU：接近 0%，不持续轮询 UI；
- 播放普通音频时 CPU：保持低水平，无异常忙循环；
- UI 关闭动画后不持续高刷新；
- 常规运行内存显著低于官方 Chromium 客户端；
- 1000 首歌曲列表滚动无明显卡顿；
- 播放控制响应延迟低于用户可感知阈值；
- Seek 后 MPRIS 和 UI 在短时间内同步到真实位置；
- 长时间播放无持续内存增长。

### 15.2 测量

```bash
/usr/bin/time -v hmp
systemd-cgtop --user
ps -C hmp -o pid,rss,%cpu,cmd
playerctl -p hmp metadata
```

可在 `xtask bench-runtime` 中加入统一测量流程。

---

## 16. 构建、打包与发布

### 16.1 Arch Linux 开发依赖

具体包名以当前 Arch 仓库为准，预计包括：

```text
rustup 或 rust
gstreamer
gst-plugins-base
gst-plugins-good
gst-plugins-bad
gst-libav
sqlite
libsecret
pkgconf
clang
cmake
```

### 16.2 开发命令

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
cargo run -p hmp-desktop
```

### 16.3 `xtask`

建议提供：

```text
cargo xtask dev
cargo xtask check
cargo xtask fixtures
cargo xtask package-arch
cargo xtask diagnose
```

### 16.4 发布格式

优先级：

1. Arch Linux `PKGBUILD`；
2. AUR `hmp-git`；
3. AUR 稳定版 `hmp`；
4. 通用 tar.zst；
5. 其他发行版打包。

AppImage 可后置。GStreamer 插件和 Secret Service 依赖使完全自包含打包需要额外评估。

### 16.5 桌面文件

```ini
[Desktop Entry]
Name=HMP
Comment=Lightweight QQ Music player
Exec=hmp
Icon=hmp
Terminal=false
Type=Application
Categories=Audio;AudioVideo;Player;
StartupWMClass=hmp
X-GNOME-UsesNotifications=true
```

---

## 17. 许可证

项目许可证必须在开始大规模移植前确定。

若 Python QQMusicApi 的源代码许可证为 GPL，并且 HMP 采用直接翻译、结构性复制或大量改写其实现的方式，项目应按 GPL 衍生项目风险处理。最保守的方案是将 HMP 定为：

```text
GPL-3.0-or-later
```

若希望使用 MIT / Apache-2.0，则需要进行更严格的 clean-room 风格重实现，并保留独立协议研究记录。发布前应重新核对所参考仓库的实际许可证文本，而不是只依赖仓库页面标签。

本项目不应包含：

- 用户 Cookie；
- 官方客户端二进制；
- 受版权保护的音乐文件；
- 用于绕过付费权益的代码或默认配置。

---

## 18. Git 和工程规范

### 18.1 分支

```text
main        可构建、可测试
feature/*   功能开发
fix/*       修复
refactor/*  重构
```

### 18.2 Commit 示例

```text
feat(qqmusic): implement QR login polling
feat(player): add seek support
fix(mpris): publish local cover URI
refactor(core): introduce TrackId newtype
test(qqmusic): add vkey response fixture
```

### 18.3 Rust 规范

- `cargo fmt` 必须通过；
- Clippy warnings 在 CI 中视为错误；
- 公共 API 提供 rustdoc；
- 不滥用 `unwrap()`；
- 后台任务必须有明确退出路径；
- 对用户输入和远端响应做边界校验；
- 不在持锁期间执行网络请求；
- 不让 UI 线程执行阻塞操作；
- 不以 `Arc<Mutex<Everything>>` 代替架构设计。

---

## 19. 版本路线

### v0.0.1：工作区骨架

- [ ] 创建 workspace；
- [ ] 创建核心 crate；
- [ ] CI；
- [ ] fmt / clippy / test；
- [ ] tracing；
- [ ] 基础文档；
- [ ] Slint 空窗口；
- [ ] GStreamer 初始化测试；
- [ ] MPRIS 注册测试。

### v0.1.0：最小可播放版本

- [x] QQ 扫码登录（`hmp login`，二维码轮询）；
- [x] 凭据保存（keyring Secret Service / 显式文件回退）；
- [x] 搜索歌曲（`hmp search` + UI 搜索页）；
- [x] 获取播放 URL（音质回退链，加密取流 GetEVkey）；
- [x] GStreamer 播放（hmp-player-gst）；
- [x] 播放 / 暂停（PlayerCommand::TogglePlay）；
- [x] Seek（UI 进度条 / MPRIS position）；
- [~] 封面（占位渐变；远程封面下载待接入）；
- [x] 完整基础 MPRIS（playerctl 实测）；
- [x] 最小搜索 UI（Slint，Apple Music 风格）；
- [x] 底部播放栏。

验收定义：通过系统面板显示标题、歌手、封面和正确进度，并可拖动进度。

### v0.2.0：账号和歌单

- [ ] 用户资料；
- [ ] 用户歌单；
- [ ] 歌单详情；
- [ ] 播放队列；
- [ ] 上一首 / 下一首；
- [ ] 循环和随机；
- [ ] 登录态刷新；
- [ ] 重启恢复。

### v0.3.0：歌词和缓存

- [ ] 原文歌词；
- [ ] 翻译歌词；
- [ ] 歌词自动滚动；
- [ ] 封面缓存；
- [ ] 歌词缓存；
- [ ] SQLite 迁移；
- [ ] 缓存清理设置。

### v0.4.0：桌面体验

- [ ] 媒体键；
- [ ] 通知；
- [ ] 单实例；
- [ ] 托盘可选；
- [ ] 高 DPI；
- [ ] 中文输入法验证；
- [ ] Niri / KDE / GNOME 测试；
- [ ] Arch PKGBUILD。

### v1.0.0

- [ ] 登录、搜索、歌单、播放和 MPRIS 稳定；
- [ ] 无已知严重凭据泄漏风险；
- [ ] 无持续内存增长；
- [ ] 完整文档；
- [ ] 可重复构建；
- [ ] Arch Linux 安装体验稳定；
- [ ] 上游接口变化时有明确诊断方式。

---

## 20. 当前优先任务

建议从以下顺序开始，不要先做完整 UI：

```text
1. ✅ 确认工作区结构和许可证
2. ✅ 建立 hmp-core
3. ✅ 建立 hmp-qqmusic 的通用请求层
4. ✅ 移植免登录搜索
5. ✅ 建立 fixture 测试
6. ✅ 移植 QQ 扫码登录
7. ✅ 移植播放 URL（含加密取流）
8. ✅ 建立 GStreamer 播放原型
9. ✅ 建立完整 MPRIS 原型
10. ✅ 最后接 Slint 最小 UI（Apple Music 风格）
```

第一个真正有意义的里程碑不是“窗口能打开”，而是以下命令行原型能够工作：

```text
hmp login
hmp search "歌曲名"
hmp play <track-id>
```

同时：

```bash
playerctl -p hmp metadata
playerctl -p hmp position 60
```

可以正确工作。

完成这个闭环后，再投入主 UI 开发，可以显著降低排查成本。

---

## 21. 风险清单

### 21.1 QQ 接口变化

应对：

- fixture 测试；
- 原始响应可选诊断保存；
- 协议层与领域层分离；
- 每个接口独立模块；
- 统一错误码。

### 21.2 登录失效或风控

应对：

- 避免高频轮询；
- 使用合理 User-Agent；
- 明确刷新周期；
- 不自动反复登录；
- 出现账号风险提示时停止请求并通知用户。

### 21.3 GStreamer 插件缺失

应对：

- 启动时检查必要插件；
- 给出明确 Arch 安装提示；
- 错误信息包含缺失 decoder / demuxer 名称。

### 21.4 UI 框架限制

应对：

- 先做输入法、缩放和长列表原型；
- UI 与核心彻底解耦；
- 保留切换 Relm4/GTK4 的可能。

### 21.5 许可证风险

应对：

- 在首次提交前记录参考来源；
- 核对上游 LICENSE；
- 不混用许可证不兼容代码；
- 发布前做一次许可证审查。

---

## 22. Definition of Done

一个功能只有满足以下条件才算完成：

- 有明确接口或用户行为；
- 正常路径可用；
- 关键错误路径有处理；
- 不泄漏敏感信息；
- 有单元测试或 fixture 测试；
- 日志足以定位问题；
- 不阻塞 UI 线程；
- 能被正常关闭，不遗留后台任务；
- 文档同步更新；
- `cargo fmt`、`cargo clippy` 和测试通过。

---

## 23. 项目一句话说明

建议 README 使用：

> HMP is a lightweight, Rust-native QQ Music player for Linux with complete MPRIS integration.

中文：

> HMP 是一个面向 Linux 的轻量 Rust QQ 音乐播放器，重点提供完整的 MPRIS 系统媒体控制体验。

---

## 24. 建议的下一份文档

在本文件落地后，紧接着建立：

```text
docs/QQMUSIC_PORTING.md
```

该文档逐模块记录：

- Python 源文件；
- Rust 目标模块；
- 已移植接口；
- 未移植接口；
- fixture；
- 已知差异；
- Live 测试结果；
- 上游变化记录。

这样可以避免移植过程逐渐失去边界，也方便以后定位 QQ 接口变更。
