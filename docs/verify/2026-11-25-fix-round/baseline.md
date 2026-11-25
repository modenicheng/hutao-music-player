# §30.7 修复轮基线（2026-11-25）

合入任何修复**之前**，在三个全新 worktree（各自独立 `target/`，`.worktrees/fix-{storage,daemon,desktop}`）上对 HEAD `e24e843` 测得。全部 cargo 命令带 `--locked --offline`，以 `nice -n 19` 顺序执行。

| 工作树 | 命令 | 结果 |
|---|---|---|
| fix-storage | `cargo test -p hmp-storage` | 73 passed |
| | `cargo clippy -p hmp-storage --all-targets --all-features -- -D warnings` | 0 warnings |
| fix-daemon | `cargo test -p hmp-core -p hmp-qqmusic-api -p hmp-daemon` | hmp-core 72；hmp-daemon lib 166 + e2e 5 + reconcile_playlist_tracks 4 + remote_details 6；hmp-qqmusic-api lib 135 + login 20 + musicu 10 + quick_search 4 + regression_browse 25 + regression_pagination 17 + regression_song 20 + regression_user_songlist 20 |
| | `cargo clippy -p hmp-core -p hmp-qqmusic-api -p hmp-daemon --all-targets -- -D warnings` | 0 warnings |
| | `cargo check -p hmp-cli -p hmp-desktop` | 通过 |
| fix-desktop | `cargo test -p hmp-desktop` | lib 105 + 18 个集成二进制各 1 |
| | `cargo clippy -p hmp-desktop --all-targets -- -D warnings` | 0 warnings |

- 合计：hmp-storage 73、hmp-core 72、hmp-daemon 253、hmp-qqmusic-api 251、hmp-desktop 123。
- `#[ignore]` 的真机测试（需要真实声卡或运行中的 daemon）未跑：`hmp-daemon/tests/daemon_cli.rs`、`e2e.rs` 末项、`hmp-cli/tests/daemon_cli.rs`、`hmp-desktop/tests/live_daemon.rs`。
- 用途：三路修复各自跑同一组命令，与上表相减即为净新增/回归。
- 原始输出：`prewarm.txt`（预热 + clippy）、`baseline-tests.txt`（测试）。
