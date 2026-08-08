# HMP 使用文档

HMP 是面向 Linux 的 Rust QQ 音乐播放器：**后台常驻播放后端（daemon）+ 多前端遥控**（CLI、系统托盘、MPRIS 媒体键），支持无损/高解析度加密音质（QMC2）流式解密播放。

## 目录

1. [构建与安装](#1-构建与安装)
2. [登录](#2-登录)
3. [播放：后台播放架构](#3-播放后台播放架构)
4. [命令参考（完整）](#4-命令参考完整)
5. [队列与播放语义](#5-队列与播放语义)
6. [音质与 QMC2 解密](#6-音质与-qmc2-解密)
7. [系统集成：MPRIS / 托盘](#7-系统集成mpris--托盘)
8. [故障排查](#8-故障排查)
9. [测试指南](#9-测试指南)

---

## 1. 构建与安装

```bash
cargo build --release
# 二进制位于 target/release/hmp
```

依赖：Rust 1.85+、GStreamer（`gstreamer` 及其插件：`gst-plugins-base/good`，播放用 `playbin`）、Linux 桌面会话（tray/MPRIS 需要 D-Bus session bus；无桌面环境时后端照常运行，仅 tray/MPRIS 跳过）。

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
# 登录: 已登录
# 用户: 939861972 (musicid: 939861972)
# 过期: 未过期
# 后端: 系统密钥环 (SecretService)
```

（本地凭证检查，不依赖 daemon；未登录时提示运行 `hmp login`。）

## 3. 播放：后台播放架构

```text
hmp play <track-id> ──┐
hmp status            ├─►  Unix socket JSON-RPC ──►  hmp daemon（常驻）
playerctl -p hmp ...  │        (127.0.0.1 本机)          │
系统托盘菜单 ──────────┘                                 │
                                                         ▼
                                      队列核心 → 音质回退 → QMC2 解密 → GStreamer 播放
```

- **单例常驻**：`hmp play/status/...` 等遥控命令发现 daemon 未运行时会**自动拉起**（detached + setsid，终端关闭播放不中断）；已运行则复用。
- **控制面**：Unix socket（`$XDG_RUNTIME_DIR/hmp.sock`，无 XDG_RUNTIME_DIR 时 ` /tmp/hmp-<uid>/hmp.sock`，权限 0600），长度前缀 JSON 帧；多个客户端（多终端 + tray + MPRIS）可并发。
- **单实例保证**：`flock` 锁文件（`<socket>.lock`）原子抢占；后启动的实例检测到已在运行即退出。
- **状态单一来源**：daemon 发布 `DaemonState`（播放状态 + 队列 + 能力），CLI/tray/MPRIS 均只读它。
- **退出**：`hmp quit` 或托盘「退出」→ 停止播放、清理 socket、释放 MPRIS、关闭 tray，进程退出；SIGINT/SIGTERM 同样处理。

## 4. 命令参考（完整）

### 4.1 播放源

| 命令 | 说明 |
|---|---|
| `hmp play <id>` | 清空队列并立即播放。`<id>` 支持三种源：`<songmid>`（单曲）、`playlist:<id>`（歌单）、`album:<id>`（专辑） |
| `hmp login` | QQ 扫码登录（终端 ASCII 二维码） |
| `hmp auth` | 显示登录状况（用户/过期/凭证后端，本地检查） |
| `hmp search <关键词>` | 搜索歌曲，输出 track-id |
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
| `hmp loop <none\|list\|track>` | 循环模式：顺序播完停 / 列表循环 / 单曲循环 |
| `hmp shuffle <on\|off>` | 随机播放 |
| `hmp status` | 显示当前曲目/状态/进度/音量/循环/队列 |
| `hmp quit` | 优雅退出后端 |

### 4.3 后端进程管理

| 命令 | 说明 |
|---|---|
| `hmp serve` | 前台运行 daemon（调试用，Ctrl+C 退出） |
| `hmp serve --background` | 后台运行（detached + setsid，命令立即返回）；遥控命令自动拉起时也走此路径 |

## 5. 队列与播放语义

- **播完自动续播**：单曲结束（EOS）→ 自动播放队列下一首；
- **循环**：`none` 播完队列最后一首即停（daemon 保持存活等新指令）；`list` 整体回绕；`track` 单曲重播；
- **随机**：`shuffle on` 后下一首从剩余曲目随机选（排除当前）；
- **上一首**：恒为物理队列的前一首（`list`/`track` 模式回绕）；
- **队列播完**：状态 `Ended`，daemon 不退出，等待 `hmp play/...` 新指令；
- **播放失败**：某音质不可用自动回退下一档；全部不可用 → `hmp play` 报错并给出最后错误（含 `last_error` 类型化错误码：`NotLoggedIn/TrackNotFound/PlaylistNotFound/QualityUnavailable/Internal`）。

## 6. 音质与 QMC2 解密

- **质量回退链**：`Master → HiRes → Atmos → Flac → Mp3_320 → Mp3_128`（会员音质优先，逐级降级）；
- **加密音质**（`.mflac`/`.mgg`/`.mmp4` 等，FLAC 及以上）：daemon 用接口 `ekey` 经本地回环解密代理（127.0.0.1 随机端口，Range 按需解密）**流式播放**，边下边播、支持即时 Seek；CDN 不支持 Range 时回退整文件解密缓存；
- OGG 系列（`O8M1` 等）尚未纳入回退链（后续项）。

## 7. 系统集成：MPRIS / 托盘

- **MPRIS**：daemon 注册 `org.mpris.MediaPlayer2.hmp`；用标准工具控制：
  ```bash
  playerctl -p hmp play-pause
  playerctl -p hmp next
  playerctl -p hmp status        # Playing / Paused
  playerctl -p hmp metadata      # 曲目元数据
  ```
  `CanGoNext`/`CanGoPrevious` 按队列位置与循环模式实时上报；`xesam:url` 为本地解密代理 URI。
- **托盘**：KDE 等桌面显示图标，菜单 = 播放/暂停、上一首、下一首、停止、退出（GNOME 需 AppIndicator 扩展；无桌面会话时自动跳过，不影响播放）。

## 8. 故障排查

| 现象 | 处理 |
|---|---|
| `hmp play` 报 `NotLoggedIn` | 先运行 `hmp login`；凭证过期同理 |
| `后端启动超时` | daemon 拉起失败（见下）；可先手动 `hmp serve` 看前台错误 |
| 端口/socket 冲突或残留 | 删除 `$XDG_RUNTIME_DIR/hmp.sock*` 与 `/tmp/hmp-<uid>/` 后重试（flock 锁保证不会双实例） |
| 无声音 | 检查 GStreamer 音频插件（`gst-inspect-1.0 autoaudiosink`）；确认音频输出设备 |
| 托盘不显示 | 桌面需支持 StatusNotifierItem（GNOME 装 AppIndicator 扩展）；无碍播放 |
| `playerctl` 无响应 | `playerctl -p hmp` 前缀必须带 `-p hmp`；确认 daemon 在运行（`hmp status`） |

## 9. 测试指南

### 9.1 自动化测试（无需账号/网络）

```bash
cargo test --workspace          # 全量：核心队列/IPC + daemon 引擎（fake 驱动）+ 真 socket 服务器 + CLI + 既有 250+ 测试
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

覆盖面：
- **hmp-core**：`QueueCore` 纯逻辑（循环回绕/prev/洗牌/插队/移除）、IPC 帧编解码（round-trip、长度上限、截断）；
- **hmp-daemon 引擎**：FakeDriver/FakeResolver 注入——Play 替换队列、Next/Prev 导航、EOS 自动续播、List/Track 循环、移除当前曲立即接替、quit 终止、seq/last_error 发布；
- **服务器**：真实 Unix socket + 协议客户端——Status/Queue/订阅推送（含空闲订阅者事件流）、畸形帧、未登录前置校验、多客户端；
- **CLI**：状态格式化、播放源解析、二维码渲染（已知像素图断言）、登录刷新判定、`hmp quit` 进程级优雅退出。

### 9.2 真机验收（需 QQ 账号 + 桌面环境 + GStreamer）

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
hmp loop list && hmp next   # 回绕
hmp shuffle on && hmp next  # 随机

# 6) 音质验证：hmp play 后 hmp status 无报错即解密播放成功（日志可看音质档位）
# 7) MPRIS
playerctl -p hmp play-pause && playerctl -p hmp status
# 8) 托盘：KDE 桌面可见图标，菜单五项可用；点「退出」后 hmp status 应报无法连接
# 9) 退出干净
hmp quit && ls $XDG_RUNTIME_DIR/hmp.sock   # 应不存在

# 端到端冒烟（本机需 GStreamer；已按默认忽略，显式运行）：
cargo test -p hmp-daemon --test e2e -- --ignored
cargo test -p hmp-daemon --test daemon_cli -- --ignored
cargo test -p hmp-cli --test daemon_cli -- --ignored
```
