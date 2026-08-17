# HMP 使用文档

HMP 是跨 Windows/Linux 的 Rust QQ 音乐播放器：**后台播放内核（`hmpd`）+ 多控制器**（CLI、Tauri/未来 Slint、桌面 tray；Linux 另有 MPRIS），支持无损/高解析度加密音质（QMC2）流式解密播放。播放内核不创建窗口或 tray，也不依赖任何交互前端。

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

依赖：Rust 1.85+、GStreamer（`gstreamer` 及其插件：`gst-plugins-base/good`，播放用 `playbin`）。Linux 的 MPRIS 需要 D-Bus session bus；CLI/headless 模式始终不创建窗口或 tray。

### 1.1 Windows 原生依赖与桌面打包

安装官方 MSVC x86_64 GStreamer 的同版本 **Runtime** 与 **Development** 安装包，然后在同一个 PowerShell 会话执行：

```powershell
./scripts/setup-gstreamer-windows.ps1
cargo build -p hmp-daemon --bin hmpd --release --no-default-features
./apps/hmp-tauri/scripts/stage-sidecar.ps1
Push-Location apps/hmp-tauri
pnpm install --frozen-lockfile
pnpm test
pnpm tauri build
Pop-Location
```

设置脚本只探测 SDK 并配置当前进程的 `PATH`、`PKG_CONFIG_PATH` 和 `GSTREAMER_1_0_ROOT_MSVC_X86_64`，不会下载软件或修改系统环境。暂存脚本依据 `rustc -vV` 的 host triple 生成 Tauri 所需的 `hmpd-<target>.exe`；源二进制不存在时会明确失败。

当前 Tauri bundle 捆绑 `hmpd`，但不自动收集 GStreamer DLL/插件树；运行安装包的 Windows 机器仍需安装官方 GStreamer Runtime。发布前的 clean-runtime 依赖收集与安装包验证是显式剩余验收项。

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
hmp / Tauri / tray ──►  hmp-control ──► hmpd（单一状态与播放进程）
                         │
                         ├─ Linux：owner-only Unix socket
                         └─ Windows：session-scoped named pipe
                                                         │
                                                         ▼
                                      队列核心 → 音质回退 → QMC2 解密 → GStreamer 播放
```

- **单例**：endpoint 在初始化 GStreamer 前抢占。Linux 用 `flock` 与 Unix socket；Windows 用 named pipe 的 first-instance 语义。后启动实例不会创建第二个播放器。
- **控制面**：Linux socket 位于 `$XDG_RUNTIME_DIR/hmp.sock`（回退 `/tmp/hmp-<uid>/hmp.sock`），目录 0700、socket 0600；Windows pipe 名包含用户与登录 session，拒绝远程客户端，并以当前用户 DACL 限制访问。
- **状态单一来源**：`hmpd` 发布 `DaemonState`，CLI、GUI 和 tray 都只发送接口命令并消费同一快照。WebView 不播放音频。
- **自主模式**：CLI 发现 daemon 缺失时以 `hmpd --autonomous` 分离拉起；关闭终端不影响播放。
- **前端托管模式**：GUI 发现 daemon 缺失时以 `hmpd --frontend-owned` 拉起并持有 lease。最后一个 GUI lease 断开后等待 30 秒；未重连则优雅退出。GUI 崩溃不再产生无期限孤儿进程。
- **退出**：`hmp quit` 或桌面 tray「完整退出」发送统一 `Request::Quit`。GUI 最多等待 3 秒后清理自己的唯一 tray 并退出。

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
| `hmp status` | 显示当前曲目/状态/进度/音量/音质/循环/队列 |
| `hmp quality [auto\|master\|hires\|atmos\|flac\|aac\|320\|128] [--no-fallback]` | 查看/设置音质策略（见 §6） |
| `hmp history [n]` | 最近播放（直读媒体库，默认 10 条） |
| `hmp quit` | 优雅退出后端 |

### 4.3 后端进程管理

| 命令 | 说明 |
|---|---|
| `hmp serve` | 前台运行 daemon（调试用，Ctrl+C 退出） |
| `hmp serve --background` | 兼容入口：后台拉起 autonomous `hmpd`；Windows 使用无控制台新进程组，Linux 使用 `setsid` |

## 5. 队列与播放语义

- **播放模型**：队列是**规范顺序**（显示/快照）；播放沿**播放顺序**推进（`shuffle off` 时二者一致，`on` 时是随机排列）——上一首/下一首都沿播放顺序走。
- **播完自动续播**：单曲结束（EOS）→ 自动播放队列下一首；
- **循环**：`none` 播完队列最后一首即停（daemon 保持存活等新指令）；`list` 整体回绕；`track` 单曲重播（**只影响 EOS 续播**：`track` 模式下按“下一首”仍会跳歌，不被单曲循环卡住）；
- **随机**：`shuffle on` 生成一次性随机播放顺序，周期内不重复，周期结束回绕；`shuffle off` 恢复规范顺序；
- **上一首**：恒为播放顺序中的前一曲——随机模式下回到真正刚播过的那首；`list`/`track` 模式回绕；
- **队列播完**：状态 `Ended`，daemon 不退出，等待 `hmp play/...` 新指令；
- **播放失败**：某音质不可用自动回退下一档；全部不可用 → `hmp play` 报错并给出最后错误（含 `last_error` 类型化错误码：`NotLoggedIn/TrackNotFound/PlaylistNotFound/QualityUnavailable/Internal`）。

## 6. 音质与 QMC2 解密

- **音质策略**（持久化于 `~/.config/hmp/config.toml`，`hmp quality` 查看/设置）：
  ```bash
  hmp quality                 # 查看当前策略与生效链
  hmp quality auto            # 自动：从最高档起逐级回退（默认）
  hmp quality flac            # 固定 FLAC，失败回退 320/128
  hmp quality 320 --no-fallback  # 只尝试 320k，不降级
  # 可用档位：auto | master | hires | atmos | flac | aac | 320 | 128
  ```
- **回退链**：`auto` = `Master → HiRes → Atmos → Flac → Mp3_320 → Mp3_128`；固定档位从该档起降级。音质是 **source resolution policy**（resolver 按链取流），不是播放器参数。
- **可用 vs 实际**：曲目的 `available_qualities`（QQ size 字段 + 本次探测成功档位）与播放状态的 `actual_quality` 分离；`hmp status` 显示实际音质。
- **加密音质**（`.mflac`/`.mgg`/`.mmp4` 等，FLAC 及以上）：daemon 用接口 `ekey` 经本地回环解密代理（127.0.0.1 随机端口，Range 按需解密）**流式播放**，边下边播、支持即时 Seek；CDN 不支持 Range 时回退整文件解密缓存；
- OGG 系列（`O8M1` 等）尚未纳入回退链（后续项）。

## 6.5 本地音乐（媒体库）

本地音乐走 **provider 模型**：`qq:<mid>` 网络取流，`local:<路径>` 本地文件
（`file://`），播放 URI 恒为路径本身、**不依赖 QQ 登录**。

```bash
hmp scan ~/Music            # 递归扫描入库（标签元数据 + 文件名回退，幂等）
hmp play local:/home/user/Music/x.flac   # 播放本地文件（未登录也可）
hmp history                 # 最近播放（会话粒度：开始/结束/收听时长/原因）
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
  `CanGoNext`/`CanGoPrevious` 按队列位置与循环模式实时上报；`xesam:url` 为本地解密代理 URI。
- **桌面 tray**：由唯一桌面控制器维护，不属于 daemon。菜单 = 显示/隐藏、播放/暂停、上一首、下一首、停止、完整退出，并随 daemon 快照更新可用性。Windows 左键恢复窗口；Linux 仍可使用 tray 菜单，但 Tauri 底层不保证发送 tray 左键事件。CLI/headless 模式没有 tray。

## 8. 故障排查

| 现象 | 处理 |
|---|---|
| `hmp play` 报 `NotLoggedIn` | 先运行 `hmp login`；凭证过期同理 |
| `后端启动超时` | daemon 拉起失败（见下）；可先手动 `hmp serve` 看前台错误 |
| socket 冲突或残留（Linux） | 确认没有 `hmpd` 后再删除 `$XDG_RUNTIME_DIR/hmp.sock*` 与 `/tmp/hmp-<uid>/`；锁保证不会双实例 |
| named pipe 连接失败（Windows） | 确认 GUI/CLI 与 daemon 属于同一用户和登录 session；正常情况下无需也无法手动删除 pipe |
| Windows 编译找不到 `gstreamer-1.0.pc` | 同时安装官方 Runtime/Development，并在当前 PowerShell 先运行 `./scripts/setup-gstreamer-windows.ps1` |
| Tauri 报 sidecar 不存在 | 先 release 构建 `hmpd`，再运行 `./apps/hmp-tauri/scripts/stage-sidecar.ps1` |
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
- **hmp-core**：`QueueCore` 纯逻辑（循环回绕/prev/播放顺序洗牌/历史回退/插队整片/移除）、IPC 帧编解码（round-trip、长度上限、截断）；
- **hmp-daemon 引擎**：FakeDriver/FakeResolver 注入——Play 替换队列、Next/Prev 导航、EOS 自动续播、List/Track 循环、移除当前曲立即接替、quit 终止、seq/last_error 发布；
- **服务器/传输**：共享握手与帧协议；Linux 验证 Unix socket 权限，Windows 验证多 named-pipe 客户端、远程拒绝与 second-listener 排他；
- **CLI**：状态格式化、播放源解析、二维码渲染（已知像素图断言）、登录刷新判定、`hmp quit` 进程级优雅退出、`hmp quality`/`hmp history` 格式化；
- **存储**：SQLite 媒体库（迁移 v1、upsert 幂等、播放会话 start→end 闭环、WAL 并发）、配置 round-trip、回退链生成；
- **e2e（wiremock）**：QQ 详情/取流契约、音质回退链顺序（Auto 含 Atmos；固定 FLAC 只试 F0M0）。

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

### 9.3 Windows 桌面生命周期验收

完成 §1.1 的 release 打包后逐项验证：

1. 启动桌面应用并播放本地 FLAC，确认实际音频由 `hmpd`/GStreamer 输出。
2. 关闭主窗口，确认窗口隐藏、播放继续；单击 tray 恢复窗口。
3. 分别从 GUI、tray 与 CLI 执行播放/暂停、上一首、下一首、停止，确认三者状态同步且只有一个 `hmpd.exe`。
4. 在 GUI 启动 daemon 的场景强制结束 GUI，确认 30 秒内重启 GUI 可重新接管；不重启时 daemon 在宽限期后自行退出。
5. 从 CLI 先启动 autonomous daemon，再打开 GUI，确认 GUI 复用已有实例；tray「完整退出」后确认 `hmpd.exe` 与 named pipe 均消失。

自动化检查覆盖协议、named pipe 多客户端/单例、lease 计时器、Tauri 生命周期 reducer 和 Vue bridge；真实音频、Explorer tray 行为及安装包 clean-runtime 启动仍属于必须在装有官方 GStreamer SDK/运行时的 Windows 主机上完成的人工验收，不能以 `DOCS_RS=1` 类型检查代替。
