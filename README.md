# HMP (hutao-music-player)

HMP is a lightweight, Rust-native QQ Music player for Linux with complete MPRIS integration.

HMP 是一个面向 Linux 的轻量 Rust QQ 音乐播放器，重点提供完整的 MPRIS 系统媒体控制体验。

## 仓库结构

```text
hutao-music-player/
├── Cargo.toml              # workspace 根（resolver 3, edition 2024）
├── crates/
│   ├── hmp-core/           # 领域模型：Track/PlayerCommand/PlaybackState/QueueCore/IPC 协议
│   ├── hmp-qqmusic-api/    # QQ 音乐 API 移植 crate（独立发布 crates.io）
│   ├── hmp-player-gst/     # GStreamer 播放核心（PlayerCore）
│   ├── hmp-media/          # 下载/QMC2 解密/缓存/本地回环解密代理
│   ├── hmp-storage/        # 凭证存储
│   ├── hmp-mpris/          # MPRIS D-Bus 服务
│   ├── hmp-daemon/         # 后台播放后端（socket 服务器 + 播放引擎 + tray/MPRIS 适配）
│   ├── hmp-desktop/        # Slint 桌面端（接入中）
│   └── hmp-cli/            # CLI（登录/搜索/遥控子命令，二进制名 `hmp`）
├── docs/
│   ├── PROJECT.md          # 项目总纲
│   ├── USAGE.md            # ★ 使用文档（命令参考/队列语义/故障排查/测试指南）
│   └── QQMUSIC_PORTING.md  # 移植跟踪（上游模块 → Rust 模块映射）
├── fixtures/               # 差分测试原始录制
└── scripts/
```

## 快速上手

```bash
hmp login                    # 终端 ASCII 二维码登录
hmp auth                     # 显示登录状况
hmp search "歌曲名"           # 搜索
hmp play <track-id>          # 后台播放（自动拉起常驻 daemon）
hmp status                   # 状态
hmp pause / next / seek 60   # 遥控
hmp quit                     # 退出后端
```

完整使用文档（命令参考、队列语义、音质与解密、MPRIS/托盘、故障排查、测试指南）见 **[docs/USAGE.md](docs/USAGE.md)**。

## Crate 说明

`hmp-qqmusic-api` 是 `hmp-qqmusic-api` 项目的 QQ 音乐协议实现：

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

## 用法

```bash
hmp login                    # QQ 扫码登录：终端 ASCII 二维码，扫码后凭证存入系统密钥环
hmp auth                     # 显示登录状况
hmp search "歌曲名"           # 搜索歌曲
hmp play <track-id>          # 遥控后端播放（track-id | playlist:<id> | album:<id>）
hmp playnext <id>            # 插队播放
hmp queue show|add <id>|remove <idx>|clear
hmp status                   # 查询后端状态
hmp pause / resume / next / prev / stop \
   / seek 60 / volume 0.5 / loop list / shuffle on
hmp quit                     # 优雅退出后端
hmp serve                    # 前台运行后端（--background 后台运行，由遥控命令自动拉起）
```

后台播放：`hmp play/status/...` 等遥控命令自动拉起常驻 daemon（单例 Unix socket
`$XDG_RUNTIME_DIR/hmp.sock`，`flock` 保证单实例），CLI 退出后播放不中断；亦可用
`playerctl -p hmp ...` 经 MPRIS 遥控（见 [docs/USAGE.md](docs/USAGE.md) §7）。

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
