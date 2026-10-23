# AGENTS.md

HMP：Rust QQ 音乐播放器（Windows SMTC / Linux MPRIS），常驻 daemon + 多前端（CLI / Slint 桌面端）。

## 必读
- 总纲/架构：docs/PROJECT.md
- 命令与语义：docs/USAGE.md
- 桌面端移植坑位：crates/hmp-desktop/PORTING.md
- 审计与开放项：docs/AUDIT.md（动手前核对 §4 / §16+）
- 设计基准：docs/DESIGN.md（胡桃木设计语言 v0.13）

## 验证
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace   # 覆盖面见 USAGE §9

## 纪律
- 全仓测试逐 crate 跑，勿单包即宣称全绿。
- 多 worktree 各自持有 target/，不共享构建产物。
- 改播放/IPC 链后：重建**所有**链接 hmp-daemon 的二进制（hmp 与 hmp-desktop 同源同指纹），
  `hmp quit` 杀常驻旧进程后实测——陈旧 daemon/二进制是本仓高频事故（AUDIT §18/§22）。
- UI 不做 HTTP、不碰凭证；播放状态单一来源是 daemon（PROJECT §4.2/§8.6）。
