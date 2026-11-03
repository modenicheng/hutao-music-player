# HMP (hutao-music-player)

HMP is a lightweight, Rust-native QQ Music player for Windows and Linux (MPRIS on Linux, SMTC on Windows).

HMP 是一个跨平台（Windows / Linux）的轻量 Rust QQ 音乐播放器：Linux 侧提供完整 MPRIS 系统媒体控制，Windows 侧接入 SMTC 与托盘。

## 仓库结构

```text
hutao-music-player/
├── Cargo.toml              # workspace 根（resolver 3, edition 2024）
├── crates/
│   ├── hmp-core/           # 领域模型：Track/PlayerCommand/PlaybackState/QueueCore/IPC 协议
│   ├── hmp-qqmusic-api/    # QQ 音乐 API 移植 crate（独立发布 crates.io；tests/fixtures/ 存差分测试录制 ×24）
│   ├── hmp-player/     # Rodio 播放核心（PlayerCore）
│   ├── hmp-media/          # 下载/QMC2 解密/缓存/进程内随机访问解密源（流式）
│   ├── hmp-storage/        # 凭证存储 + 媒体库（SQLite，扫描/歌单/封面缓存）
│   ├── hmp-mpris/          # MPRIS D-Bus 服务
│   ├── hmp-smtc/           # Windows SMTC 系统媒体控制适配
│   ├── hmp-daemon/         # 后台播放后端（socket 服务器 + 播放引擎 + tray/MPRIS 适配）
│   ├── hmp-desktop/        # Slint 桌面端（daemon 前端：库页/播放已接真实后端）
│   └── hmp-cli/            # CLI（登录/搜索/遥控子命令，二进制名 `hmp`）
├── docs/
│   ├── PROJECT.md          # 项目总纲
│   ├── USAGE.md            # ★ 使用文档（命令参考/队列语义/故障排查/测试指南）
│   ├── QQMUSIC_PORTING.md  # 移植跟踪（上游模块 → Rust 模块映射）
│   ├── DESIGN.md           # 胡桃木设计语言（桌面端移植基准，v0.13）
└── scripts/
```

## 快速上手

```bash
hmp login                    # 终端 ASCII 二维码登录
hmp auth                     # 显示登录状况
hmp search "歌曲名"           # 搜索
hmp play <track-id>          # 后台播放（自动拉起常驻 daemon）
hmp player quality flac      # 音质策略（auto/master/hires/atmos/flac/aac/320/128）
hmp scan ~/Music             # 扫描本地音乐入库
hmp play local:/path/x.flac  # 播放本地文件（无需登录）
hmp library history          # 最近播放
hmp status                   # 状态
hmp pause / next / seek 60   # 遥控
hmp quit                     # 退出后端

# 桌面 UI（Slint，连接/自动拉起同一 daemon，与 CLI 同一播放状态源）
cargo run --release -p hmp-desktop --bin hmp-desktop
```

完整使用文档（命令参考、队列语义、音质与解密、MPRIS/托盘、故障排查、测试指南）见 **[docs/USAGE.md](docs/USAGE.md)**。

## Crate 说明

`hmp-qqmusic-api` 是 HMP 项目的 QQ 音乐协议实现：

- 独立版本号，独立发布到 crates.io（`hmp-qqmusic-api = "0.1"`）；
- 以 Python 参考实现 [L-1124/QQMusicApi](https://github.com/L-1124/QQMusicApi)
  （GPL-3.0-or-later）为行为规范和差分测试 Oracle，Rust 侧重新定义模块边界、
  错误类型与并发模型；
- 许可证 GPL-3.0-or-later，与上游一致。

## 开发命令

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

## 命令参考

完整命令参考（播放源 / 播放控制 / 队列 / 歌单 / 媒体库 / 收藏 / 评论 / 后端管理）见
**[docs/USAGE.md](docs/USAGE.md) §4**。

所有遥控命令（`hmp play/status/pause/...`）自动拉起常驻 daemon（单例 IPC 端点：Unix
socket / Windows 命名管道），CLI 退出播放不中断；Linux 亦可用 `playerctl -p hmp ...`
经 MPRIS 遥控，Windows 走 SMTC 系统媒体键（USAGE §3 / §7）。

## 鸣谢 / Acknowledgements

HMP 的 QMC2 加密音质解密实现基于以下开源项目的研究与代码（许可证均与 GPL-3.0-or-later 兼容）：

- [jixunmoe/qmc2-rust](https://github.com/jixunmoe/qmc2-rust)（MIT）——ekey 派生（含 EncV2 两段 TEA）与 map/RC4 流密码的 Rust 参考实现及测试向量；
- [bczhc/qmc-decode](https://github.com/bczhc/qmc-decode)（GPL-3.0）——QMC2 文件尾部（QTag/STag）检测与格式研究；
- [bczhc/qmc-decrypt](https://github.com/bczhc/qmc-decrypt)（GPL-3.0）——STag 解密流程与 ekey 用法；
- TarsCpp [`tc_tea`](https://github.com/TarsCloud/TarsCpp)（BSD-3-Clause）——TEA-CBC 加解密变体（`oi_symmetry_encrypt2/decrypt2`）；
- [unlock-music](https://github.com/ix64/unlock-music) 研究（GPL-3.0-or-later）——QMC 格式的早期研究与文档（仓库现因 DMCA 不可访问，本实现基于上述维护中的衍生项目）。

## 许可证

GPL-3.0-or-later。参考实现 L-1124/QQMusicApi 同样以 GPL-3.0-or-later 发布。
本项目不包含用户 Cookie、官方客户端二进制或受版权保护的音乐文件。
