# HMP (hutao-music-player)

HMP is a lightweight, Rust-native QQ Music player for Linux with complete MPRIS integration.

HMP 是一个面向 Linux 的轻量 Rust QQ 音乐播放器，重点提供完整的 MPRIS 系统媒体控制体验。

## 仓库结构

```text
hutao-music-player/
├── Cargo.toml              # workspace 根（resolver 3, edition 2024）
├── crates/
│   └── hmp-qqmusic-api/    # QQ 音乐 API 移植 crate（独立发布 crates.io）
├── docs/
│   ├── PROJECT.md          # 项目总纲
│   └── QQMUSIC_PORTING.md  # 移植跟踪（上游模块 → Rust 模块映射）
├── fixtures/               # 差分测试原始录制
└── scripts/
```

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

## 许可证

GPL-3.0-or-later。参考实现 L-1124/QQMusicApi 同样以 GPL-3.0-or-later 发布。
本项目不包含用户 Cookie、官方客户端二进制或受版权保护的音乐文件。
