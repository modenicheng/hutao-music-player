# HMP 使用文档

HMP 是面向 Windows 与 Linux 的 Rust QQ 音乐播放器：**后台常驻播放后端（daemon）+ 多前端遥控**（CLI、桌面应用、SMTC/MPRIS 媒体键），支持无损/高解析度加密音质（QMC2）流式解密播放。

## 目录

1. [构建与安装](#1-构建与安装)
2. [登录](#2-登录)
3. [播放：后台播放架构](#3-播放后台播放架构)
4. [命令参考（完整）](#4-命令参考完整)
5. [队列与播放语义](#5-队列与播放语义)
6. [音质与 QMC2 解密](#6-音质与-qmc2-解密)
7. [系统集成：SMTC / MPRIS / 托盘](#7-系统集成mpris--托盘)
8. [故障排查](#8-故障排查)
9. [测试指南](#9-测试指南)

---

## 1. 构建与安装

```bash
cargo build --release
# 二进制位于 target/release/hmp
```

依赖：Rust 1.85+。音频由 Rodio/CPAL 提供；Windows 无需安装额外媒体 SDK，Linux 需要发行版的 ALSA 开发库。MPRIS 需要 D-Bus session bus（托盘在 Linux 走 D-Bus StatusNotifierItem，Windows 走 Win32 原生）；无桌面会话时后端仍可运行（托盘自动跳过）。

## 2. 登录

```bash
hmp login
```

- 二维码**直接以 ASCII 艺术渲染在终端**（半块字符，自动适配终端宽度 32..=120），无需手动打开图片；
- 用 QQ 手机版扫码并确认；
- 二维码过期**自动刷新**（总等待上限 10 分钟）；用户拒绝/取消则立即退出，不会无限重试；
- 成功后在终端打印用户信息；凭证存入系统密钥环（SecretService）；无密钥环环境回退为明文文件（会明确提示）；
- 登录是 CLI 交互操作；**后台 daemon 只读同一份凭证**，无需在 daemon 里重复登录。

查看登录状况：

```bash
hmp auth
# Logged in: yes
# User: 939861972 (musicid: 939861972)
# Expiry: valid
# Backend: system keyring (SecretService)
```

（本地凭证检查，不依赖 daemon；未登录时提示运行 `hmp login`。）

## 3. 播放：后台播放架构

```text
hmp play <track-id> ──┐
hmp status            ├─►  IPC JSON 帧 ──►  hmp daemon（常驻）
playerctl -p hmp ...  │   (Unix socket / Windows 命名管道，127.0.0.1 本机)
系统托盘菜单 ──────────┘                    │
                                            ▼
                         队列核心 → 音质回退 → 进程内解密源 → Rodio 播放
```

- **单例常驻**：`hmp play/status/...` 等遥控命令发现 daemon 未运行时会**自动拉起**（detached：Unix setsid / Windows 隐藏窗口独立进程组，终端关闭播放不中断）；已运行则复用。`hmp quit` 只连接不拉起（daemon 未运行时幂等成功）。
- **控制面**：Unix socket（`$XDG_RUNTIME_DIR/hmp.sock`，无 XDG_RUNTIME_DIR 时 ` /tmp/hmp-<uid>/hmp.sock`，权限 0600）或 Windows 命名管道（`\\.\pipe\hmp`），长度前缀 JSON 帧；多个客户端（多终端 + tray + MPRIS/SMTC）可并发。
- **单实例保证**：Unix `flock` 锁文件（`<socket>.lock`）/ Windows `FILE_FLAG_FIRST_PIPE_INSTANCE` 原子抢占；后启动的实例检测到已在运行即退出。
- **状态单一来源**：daemon 发布 `DaemonState`（播放状态 + 队列 + 能力），CLI/tray/MPRIS 均只读它。
- **wire 兼容**：daemon 是常驻单例，升级窗口内旧进程仍占端点——响应/事件结构体新增字段一律带 `#[serde(default)]`，桌面订阅对解码失败断线重连不静默（CLI 解码失败会提示 `hmp quit` 后重试）。
- **退出**：`hmp quit` 或托盘「退出」→ 停止播放、清理 socket、释放 MPRIS、关闭 tray，进程退出；SIGINT/SIGTERM 同样处理。

## 4. 命令参考（完整）

### 4.1 播放源

| 命令 | 说明 |
|---|---|
| `hmp play <id>` | 清空队列并立即播放。`<id>` 支持三种源：`<songmid>`（单曲）、`playlist:<id>`（歌单）、`album:<id>`（专辑） |
| `hmp login` | QQ 扫码登录（终端 ASCII 二维码） |
| `hmp auth` | 显示登录状况（用户/过期/凭证后端，本地检查） |
| `hmp search <关键词>` | 搜索歌曲，输出 track-id |
| `hmp discover [--page N] [--area 地区]` | 发现页：推荐歌单广场 + 新歌速递（免登录；地区：内地/欧美/日本/韩国/最新/港台） |
| `hmp top [榜单ID] [--page N]` | 排行榜：无 ID 列分组榜单（含预览前 3 首）；带 ID 出完整曲目（免登录） |
| `hmp guess` | 猜你喜欢（需登录） |
| `hmp playnext <id>` | 把 `<id>` 插到当前曲之后并**立即播放**（同三种源语法，多曲源取第一首） |
| `hmp queue add <id>` | 追加到队尾（不打断当前播放） |
| `hmp queue show` | 列出队列（`▶` 标记当前曲） |
| `hmp queue remove <idx>` | 移除 0 基位置曲目；**移除当前曲目 = 立即接替播放下一首** |
| `hmp queue clear` | 清空队列 |

> 如何拿 songmid：`hmp search "歌名"` 输出 track-id 列表，直接 `hmp play <id>`。

### 4.2 播放控制

| 命令 | 说明 |
|---|---|
| `hmp pause` / `hmp resume` | 暂停 / 从当前位置继续 |
| `hmp next` / `hmp prev` | 下一首 / 上一首（**prev 一律跳上一首**，不做"进度>3s 回开头"的推测性行为） |
| `hmp stop` | 停止 |
| `hmp seek <秒>` | 跳转进度 |
| `hmp volume <0..1>` | 音量（如 `hmp volume 0.5`） |
| `hmp queue loop <none\|list\|track>` | 循环模式：顺序播完停 / 列表循环 / 单曲循环 |
| `hmp queue shuffle <on\|off>` | 随机播放 |
| `hmp status` | 显示当前曲目/状态/进度/音量/音质/ReplayGain/循环/队列 |
| `hmp player quality [auto\|master\|hires\|atmos\|flac\|aac\|320\|128] [--no-fallback]` | 查看/设置音质策略（见 §6） |
| `hmp library history [n]` | 最近播放（直读媒体库，默认 10 条） |
| `hmp quit` | 优雅退出后端 |

### 4.2.1 其余二级命令面（完整清单）

| 命令组 | 说明 |
|---|---|
| `hmp player` | status/pause/resume/next/prev/stop/seek/volume/quality（与顶层短命令等价） |
| `hmp queue` | list/show/add/play-next/remove/clear/shuffle/loop |
| `hmp playlist` | list（--scope all\|local\|owned\|favorite）/show/create/rename/add/remove/delete |
| `hmp library` | history/sync/sync-status/tracks（--search/--artist/--album/--liked）/albums/artists/scan |
| `hmp favorite` | add/remove/list（本地先提交，QQ 由 daemon 异步同步） |
| `hmp account` | profile（昵称等主页头部）/vip |
| `hmp comment` | list（--sort hot\|new\|recommend）/post/reply/delete |

### 4.3 后端进程管理

| 命令 | 说明 |
|---|---|
| `hmp serve` | 前台运行 daemon（调试用，Ctrl+C 退出） |
| `hmp serve --background` | 后台运行（detached：Unix setsid / Windows 隐藏窗口独立进程组，命令立即返回）；遥控命令自动拉起时也走此路径 |

## 5. 队列与播放语义

- **播放模型**：队列是**规范顺序**（显示/快照）；播放沿**播放顺序**推进（`shuffle off` 时二者一致，`on` 时是随机排列）——上一首/下一首都沿播放顺序走。
- **播完自动续播**：单曲结束（EOS）→ 自动播放队列下一首；
- **循环**：`none` 播完队列最后一首即停（daemon 保持存活等新指令）；`list` 整体回绕；`track` 单曲重播（**只影响 EOS 续播**：`track` 模式下按“下一首”仍会跳歌，不被单曲循环卡住）；
- **随机**：`shuffle on` 生成一次性随机播放顺序，周期内不重复；`none` 模式下随机周期结束即停（不隐含列表循环），`list` 模式回绕；`shuffle off` 恢复规范顺序（当前曲不变）；
- **上一首**：进度 ≤ 3s → 播放顺序中的前一曲（随机模式下回到真正刚播过的那首；`list` 回绕）；进度 > 3s → 只回曲首，不换曲；
- **队列播完**：状态 `Ended`，daemon 不退出，等待 `hmp play/...` 新指令；
- **播放失败**：某音质不可用自动回退下一档；全部不可用 → `hmp play` 报错并给出最后错误（含 `last_error` 类型化错误码：`NotLoggedIn/TrackNotFound/PlaylistNotFound/QualityUnavailable/Internal`）。

## 6. 音质与 QMC2 解密

- **音质策略**（持久化于 `~/.config/hmp/config.toml`，`hmp player quality` 查看/设置）：
  ```bash
  hmp player quality                 # 查看当前策略与生效链
  hmp player quality auto            # 自动：从最高档起逐级回退（默认）
  hmp player quality flac            # 固定 FLAC，失败回退 320/128
  hmp player quality 320 --no-fallback  # 只尝试 320k，不降级
  # 可用档位：auto | master | hires | atmos | flac | aac | 320 | 128
  ```
- **回退链**：`auto` = `Master → HiRes → Atmos → Flac → Mp3_320 → Mp3_128`；固定档位从该档起降级。音质是 **source resolution policy**（resolver 按链取流），不是播放器参数。
- **可用 vs 实际**：曲目的 `available_qualities`（QQ size 字段 + 本次探测成功档位）与播放状态的 `actual_quality` 分离；`hmp status` 显示实际音质。
- **加密音质**（`.mflac`/`.mgg`/`.mmp4` 等，FLAC 及以上）：daemon 用接口 `ekey` 经进程内随机访问解密源**流式播放**（Range 按需解密 + 边播边缓存），支持即时 Seek；CDN 不支持 Range 时回退整文件解密缓存；
- OGG 系列（`O8M1` 等）尚未纳入回退链（后续项）。

## 6.5 本地音乐（媒体库）

本地音乐走 **provider 模型**：`qq:<mid>` 网络取流，`local:<路径>` 本地文件
（`file://`），播放 URI 恒为路径本身、**不依赖 QQ 登录**。

```bash
hmp scan ~/Music            # 递归扫描入库（标签元数据 + 文件名回退，幂等）
hmp play local:/home/user/Music/x.flac   # 播放本地文件（未登录也可）
hmp library history         # 最近播放（会话粒度：开始/结束/收听时长/原因）
```

MPRIS `OpenUri`（`playerctl open file:///...`）经同一路径播放。

## 7. 系统集成：MPRIS / 托盘

- **MPRIS**：daemon 注册 `org.mpris.MediaPlayer2.hmp`；用标准工具控制：
  ```bash
  playerctl -p hmp play-pause
  playerctl -p hmp next
  playerctl -p hmp status        # Playing / Paused
  playerctl -p hmp metadata      # 曲目元数据
  ```
  `CanGoNext`/`CanGoPrevious` 按队列位置与循环模式实时上报；`xesam:url` 为当前曲 URI（元数据）。
- **托盘**：菜单 = 播放/暂停、上一首、下一首、停止、退出，与 CLI/MPRIS 同源生效。图标随播放态切换（暂停=双竖条，播放=音符），tooltip 显示当前曲目。
  - Windows：Win32 原生（explorer 通知区）。左键单击 = 播放/暂停，右键 = 菜单。
  - Linux：D-Bus StatusNotifierItem（KDE 等原生支持；GNOME 需 AppIndicator 扩展）。无桌面会话时自动跳过，不影响播放。

### 桌面 UI（Slint，M8 起接真实后端）

```bash
cargo build --release
cargo run --release -p hmp-desktop --bin hmp-desktop
```

- 启动时自动连接常驻 daemon（`$XDG_RUNTIME_DIR/hmp.sock`）；未运行则自动拉起 `hmp serve --background`——与 `hmp play`/CLI 遥控共用同一后端、同一播放状态源（MPRIS/托盘同源生效）。
- 库页（我喜欢/最近播放/音乐库/歌单）在启动时直读媒体库（`$XDG_DATA_HOME/hmp/library.sqlite3`）：先 `hmp scan ~/Music` 入库本地曲目；QQ 侧登录后 `hmp library sync` 同步歌单/收藏。
- 播放条/队列与 CLI 同源：CLI 换歌桌面即时可见，反之亦然；`hmp quit` 后界面保持打开但呈离线空态，不会自动拉活后端。
- 空数据是诚实状态：未扫描/未同步时对应页面为空，音乐库页"扫描本地音乐"按钮在桌面端禁用（走 `hmp scan`）。
- 已知边界（docs/AUDIT.md §8）：下载/已购两页后端无对应域（诚实空态）；远端内容卡跳详情仍按展示名查本地库。内容页（发现/排行榜/猜你喜欢/搜索）、歌词页与封面（daemon CoverGet：远程三域白名单 + 本地产物回写 + 歌单封面，无源时中性占位）均已接入。

## 8. 故障排查

| 现象 | 处理 |
|---|---|
| `hmp play` 报 `NotLoggedIn` | 先运行 `hmp login`；凭证过期同理 |
| `后端启动超时` | daemon 拉起失败（见下）；可先手动 `hmp serve` 看前台错误 |
| 端口/socket 冲突或残留 | 删除 `$XDG_RUNTIME_DIR/hmp.sock*` 与 `/tmp/hmp-<uid>/` 后重试（flock 锁保证不会双实例） |
| 无声音 | 确认系统存在默认音频输出设备；Linux 同时检查 ALSA/PipeWire 兼容层 |
| 托盘不显示 | Linux：桌面需支持 StatusNotifierItem（GNOME 装 AppIndicator 扩展）；Windows：确认 explorer 通知区可见。无碍播放，`hmp quit` 等价退出 |
| `playerctl` 无响应 | `playerctl -p hmp` 前缀必须带 `-p hmp`；确认 daemon 在运行（`hmp status`） |

## 9. 测试指南

### 9.1 自动化测试（无需账号/网络）

```bash
cargo test --workspace          # 全量：核心队列/IPC + daemon 引擎（fake 驱动）+ 真 socket 服务器 + CLI + 既有 500+ 测试
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

覆盖面：
- **hmp-core**：`QueueCore` 纯逻辑（循环回绕/prev/播放顺序洗牌/历史回退/插队整片/移除）、IPC 帧编解码（round-trip、长度上限、截断）；
- **hmp-daemon 引擎**：FakeDriver/FakeResolver 注入——Play 替换队列、Next/Prev 导航、EOS 自动续播、List/Track 循环、移除当前曲立即接替、quit 终止、seq/last_error 发布；
- **服务器**：真实 Unix socket + 协议客户端——Status/Queue/订阅推送（含空闲订阅者事件流）、畸形帧、未登录前置校验、多客户端；
- **CLI**：状态格式化、播放源解析、二维码渲染（已知像素图断言）、登录刷新判定、`hmp quit` 进程级优雅退出、`hmp player quality`/`hmp library history` 格式化；
- **存储**：SQLite 媒体库（迁移 v1、upsert 幂等、播放会话 start→end 闭环、WAL 并发）、配置 round-trip、回退链生成；
- **e2e（wiremock）**：QQ 详情/取流契约、音质回退链顺序（Auto 含 Atmos；固定 FLAC 只试 F0M0）。

### 9.2 真机验收（需 QQ 账号 + 桌面环境 + Rodio）

```bash
# 1) 登录（终端二维码）
hmp login

# 2) 播放一首已购/会员歌（建议无损）
hmp play <songmid>

# 3) 状态与遥控
hmp status                 # 应显示 Playing + 曲目 + 进度
hmp seek 60 && hmp status  # 进度跳转
hmp next / hmp prev / hmp pause / hmp resume

# 4) 后台不中断：新开终端跑 hmp play 后关闭原终端 → 音乐继续
# 5) 队列与循环
hmp play playlist:<歌单id>   # 连续播放；播完队列（loop none）后 hmp status 应停在 Ended
hmp queue loop list && hmp next   # 回绕
hmp queue shuffle on && hmp next  # 随机

# 6) 音质验证：hmp play 后 hmp status 无报错即解密播放成功（日志可看音质档位）
# 7) MPRIS
playerctl -p hmp play-pause && playerctl -p hmp status
# 8) 托盘：可见图标（Windows 通知区 / KDE）；右键菜单五项可用（Play/Pause、Previous、Next、Stop、Quit），播放中 Play 项变 Pause、图标切换；Windows 左键单击 = 播放/暂停；点 Quit 后 hmp status 应报无法连接
# 9) 退出干净
hmp quit && ls $XDG_RUNTIME_DIR/hmp.sock   # 应不存在

# 端到端冒烟（本机需 Rodio；已按默认忽略，显式运行）：
cargo test -p hmp-daemon --test e2e -- --ignored
cargo test -p hmp-daemon --test daemon_cli -- --ignored
cargo test -p hmp-cli --test daemon_cli -- --ignored
```
