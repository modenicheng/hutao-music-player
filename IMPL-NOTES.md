# S11 实施笔记（四发实施代理，增量落盘防驱逐）

> cargo = ~/.cargo/bin/cargo；worktree = .worktrees/s11-deps-latest @ chore/deps-latest（基 a5a0236）。
> 每阶段完成后 append；被驱逐后继代理从此文件 + PLAN.md §6.2/§6.3 续接。

## 阶段 0：现场盘点（第四发接手时，2026-10-24）

git 脏面 13 项（11 modified + 2 untracked PLAN/REVIEW-NOTES），**零提交**：

| 文件 | 内容 | 归属判定 |
|---|---|---|
| Cargo.toml | rust-version 1.92 + 注释、slint "1.17"→"1.18" | 提交 1 ✓ |
| clippy.toml | msrv 1.85→1.92 | 提交 1 ✓ |
| crates/hmp-desktop/Cargo.toml | slint/slint-build/i-slint-backend-testing 三行 1.18；serial_test="3"（未被试验态污染） | 提交 1 ✓（D4-4 可选①：slint 三行统一入提交1，现场已按此落） |
| ui/primitives.slint | viewport-*→content-* 9 处 | 提交 1 ✓ |
| ui/viewport.slint | 同改名 1 处（注释行） | 提交 1 ✓ |
| Cargo.lock | **T1 semver 态**：slint 1.18.1 / rodio 0.21.1 / lofty 0.21.1 / zbus 5.19.0 / tray-icon 0.24.2 / resvg 0.47(+0.48 传递) / serial_test **3.5.0**（Q1=A 已回锁） / rusqlite 0.32.1 / reqwest 0.12.28 / notify 6.1.1 / windows 0.61.3 | 提交 1 ✓ |
| hmp-core/src/ipc.rs、queue.rs | collapsible_if→let-chains（2 处） | **§6.2 清单外** → 判定入提交 1（见下） |
| qqmusic-api qmc2/cipher.rs | collapsible_if→let-chains（1 处） | 同上 |
| qqmusic-api qmc2/tea.rs、qrc.rs | `len % 8 != 0`→`!len.is_multiple_of(8)`（manual_is_multiple_of） | 同上 |

**5 个清单外源码文件归属判定**：均为 clippy.toml msrv 1.92 抬升后激活的 msrv 门控 lint
（let-chains 需 ≥1.88+edition2024、is_multiple_of 需 ≥1.87）之机械修复，即 REVIEW-NOTES
D4-2a 预判的「提交 1 最可能红点」——第三发已修在案。不入提交 1 则该提交 clippy -D warnings
必红，故按「msrv 抬升连带适配」归入提交 1（commit message 注明；revert 提交 1 连带回滚，粒度自洽）。

**提交 2 全部待重建**：6 manifest 的跨 major req + 7 源码适配文件 + M1 lofty 第 8 处。

## 阶段 1：提交 1 验证与落笔

**commit1 = 7500ce5**（七发·持久优先战术，2026-10-24）。

- 接手时现场已比阶段 0 表多 13 文件：逐 diff 核实全部为 clippy.toml msrv 1.92 抬升后
  激活的门控 lint 机械修复（let-chains / is_multiple_of / as_chunks），归属判定同阶段 0
  的 5 文件逻辑（msrv 连带适配入提交1）。16 源码文件清单：
  core ipc/queue、daemon comment/content、media decrypt/stream source（均测试内）、
  mpris service、api live_browse(示例)/qmc2 cipher/tea/qrc/login/protocol cgi/singer、
  storage config/local/xdg。
- Cargo.lock 复核实为 T1 semver 态（slint 1.18.1/rodio 0.21.1/lofty 0.21.1/zbus 5.19.0/
  tray-icon 0.24.2/serial_test 3.5.0/rusqlite 0.32.1/reqwest 0.12.28/windows 0.61.3，
  resvg 0.47+0.48 双版 = D4-1 预判的提交1态）。desktop manifest serial_test 已是 "3"（Q1=A 落实）。
- 23 文件（含上述全部）作为提交 1 落盘，**未 push**。PLAN/REVIEW-NOTES/IMPL-NOTES 三 .md
  保持 untracked（非 §6.2 切分内容，留待独立审计后处置）。
- 提交后验证（先提交后验证战术）：fmt / clippy -D warnings → 红则 fixup amend 7500ce5。

### 阶段 1 收口（七发）

- **fmt 红 1 处**（content.rs let-chain 换行）→ cargo fmt → amend → **fd67309**。
- **clippy 红**：hmp-daemon lib 14 处 collapsible_if 等遗漏（第四发只修了非 --all-features 面；
  tray/win_tray.rs 等 tray feature 门控文件未入其清单）+ desktop/cli 若干。
  处置: `cargo clippy --fix --allow-dirty --workspace --all-targets --all-features`（4m05s）
  批修 12 文件 + covers.rs:133 `chunks_exact_to_as_chunks` 手改
  （`dst.chunks_exact_mut(4)`→`dst.as_chunks_mut::<4>().0.iter_mut()`）→ fmt+clippy 双绿
  → amend → **commit1 终版 = dba92fe（35 文件，1608+/1703-）**。
  msrv 连带修复全量清单 = 前发 16 文件 + 本发 13 文件（engine/local/login/server/win_tray、
  shot-headless、bridge/cover_cache/covers/player_bridge/track_theme、qr_ascii）。
- 教训: 前发 T1「clippy exit 0」系 clippy.toml msrv 仍 1.85 时所跑（§5 已注），msrv 1.92 后
  门控 lint 大面积激活属预期红点，--fix 一次收敛。

## 主进程直接实施收尾（用户指令：不开子代理）

- commit1 dba92fe（子代理八发遗留+协调者 verify）/ commit2 f6c073c（主进程：manifest 6+源码 7 文件，workspace check+clippy 双绿，定向 5 crate+哨兵全绿）/ commit3 3b67f06（msrv 转正+deny 删 6 留 3）→ amend e3e4286（cargo doc -D warnings 修复：10 处 private intra-doc link 反引号化+1 处通配符链接+1 处 unclosed HTML tag，全仓首次 doc 严格化通过）
- 矩阵：fmt✓ / doc✓ / 十 crate 831 测试 0 失败 / MSRV +1.92 check exit 0 / release 双二进制✓ / 隔离 smoke：播放/控制正常 + **quit 后桌面 1s 干净退出（S10 修复在新依赖栈下复核通过）**
- 发现即修：cpal 0.18 name() 移除→Display（core.rs 3 处+probe_output 2 处）；reqwest 0.13 query/form 移 feature；zbus 6 Builder session() 也去 Result（计划未列，编译器抓出）
