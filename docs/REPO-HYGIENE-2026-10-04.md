# 仓库卫生审计 — 2026-10-04

> 范围：仓库物理卫生、文档漂移、CI/工程面、代码结构信号。**代码 bug 与功能开放项不在此登记**——那些以 [AUDIT.md](AUDIT.md) §4/§16 为权威。
> 方法：repo-steward 四脚本（repo_scan / file_metrics / docs_inventory / docs_drift，均零/低信号）+ 人工核对 README、PROJECT、USAGE、AUDIT、PORTING、tauri README、CI、workspace manifest 与 git 历史。复现命令见文末。

## 总体结论

基础卫生非常好：扫描零泄漏零垃圾文件、全仓 TODO/FIXME 为 0、node_modules/dist/target 均未跟踪、测试与 fmt/clippy/deny 纪律齐备、USAGE/PORTING 维护活跃且与代码逐条对得上。改进空间集中在三件事：**遗留 Vue 前端占 tracked 文件 29% 且 README 描述着不存在的架构**、**README/PROJECT 两份门面文档漂移**、**CI 完全没有 Windows 面**。

---

## P1 — 建议尽快处理

### 1. `apps/hmp-tauri` 遗留前端的处置（需决策，Review 级）

- 129 个 tracked 文件（全仓 442 的 **29%**）；最后实质功能提交是 `ca790ef`（2026-09-07，feat(web) v0.13），此后唯一触及是 `57ea666` 改 DESIGN.md 一处（作为 Slint 移植的设计基准）。
- 不在 workspace、不在 CI、根 README 未提及；其 README 的构建命令 `cargo build -p hmp-daemon --bin hmpd` 指向**不存在的 bin**（hmp-daemon 是纯 lib crate，daemon 由 `hmp serve` 承载），`hmp-control` crate、`--frontend-owned`/`--autonomous` 旗标均不存在——它描述的是一套早已被替代的架构。
- `src-tauri/` 无任何 tracked 源码（只剩被 gitignore 的 binaries/gen/target），Tauri 壳根本无法构建。
- 仍有真实价值的：**DESIGN.md**（v0.13 胡桃木设计语言，PORTING.md 的移植基准、UI-AUDIT 的对照基准）与 `src/assets/icons/`（Slint 侧已拷贝使用）。

建议三选一（按本仓库"保留设计基准"的惯例推荐 b）：

- a. 最小动作：tauri README 顶部加"已冻结"声明并纠正失实命令，其余不动；
- b. DESIGN.md 收纳进 `docs/`（或原位标注其唯一存活价值），删除 Vue 实现（src/、scripts/、锁文件、构建配置），icons 已有拷贝可一并删；
- c. 维持现状，接受 29% 仓库重量与误导性 README。

`scripts/prepare-sidecar.mjs`、`stage-sidecar.ps1`、`tauri.mjs`（全部指向 hmpd sidecar 流程）随本项同生死。

### 2. README.md 三处失实（十分钟可修）

| 位置 | 问题 | 事实 |
|---|---|---|
| `README.md:11-20` 结构图 | 缺 `hmp-smtc` crate | `Cargo.toml:9` workspace 成员含它 |
| `README.md:25` | 列出 `fixtures/` "差分测试原始录制" | 根目录无此目录；实际 fixture 在 `crates/hmp-qqmusic-api/tests/fixtures`（24 个 tracked 文件） |
| `README.md:53` | 自指句 "`hmp-qqmusic-api` 是 `hmp-qqmusic-api` 项目的 QQ 音乐协议实现" | 应为"是 HMP 项目的…"之类 |

（轻）README 自身"快速上手"与"用法"两段命令清单重叠，且与 USAGE.md §4 三处承载同一事实——命令参考的唯一 canonical owner 是 USAGE，README 保持 quick-start 级即可。

### 3. CI 没有 Windows 面

- `.github/workflows/ci.yml` 四个 job 全部 `ubuntu-latest`；而 README/USAGE 定位是 Windows + Linux。
- 后果：`hmp-smtc` 整个 crate、daemon `transport.rs` 命名管道分支、hmp-desktop 的 Known Folders/prefs Windows 路径，在 CI 里**零编译**；PORTING.md「Windows 适配」整章没有机器验证。AUDIT §0 记录过 cfg 死代码在 Linux clippy 爆炸的前科——同一类问题在 Windows 侧无人看守。
- 建议：加 `windows-latest` job，起步 `cargo check --workspace --all-targets`（顺带让 cfg(windows) 代码进入 clippy/lint 视野）；有余力再 `cargo test -p hmp-core -p hmp-storage -p hmp-cli`（进程级 e2e 已 `#[cfg(unix)]` 门控，不会拖慢）。顺带可指定 1.85 toolchain 跑一遍 check，让 `rust-version = 1.85` 有机器背书（现在只有 stable）。

---

## P2 — 择机处理

### 4. PROJECT.md 门面漂移

- `PROJECT.md:6`「当前阶段：CLI 原型 / crate 骨架」——现实是桌面端已全量接 daemon、内存治理两轮收口。
- §19 路线图 v0.2/v0.3/v0.4 复选框全未勾，但队列/循环/随机/歌词/封面缓存/SQLite 迁移 v8 均已上线（USAGE §4-6、AUDIT §8 汇总表为证）——与同文档「桌面 UI 功能状态」表自相矛盾。
- §23 一句话定位「面向 Linux」与 README/USAGE「Windows 与 Linux」冲突。

建议：不重写历史章节；改头部「当前阶段」、勾选已完成项、§23 与 README 对齐。§5 的 crate 名（`hmp-qqmusic`、`hmp-desktop` 含 `hmp` 二进制）是方案期的旧名，可在 §5 顶部加一行"实际成员见根 Cargo.toml"免得读者对表。

### 5. PORTING.md「已定工程决策」#2 过时

`crates/hmp-desktop/PORTING.md:41`「app.rs（AppCore）暂不动…M8 重接」——app.rs/mock.rs 已于 2026-09-29 删除（M8 记录自证）。改为「已随 M8 删除」，防止后来者按图索骥。

### 6. 缺 AGENTS.md（≤30 行指针文档，非新文档）

本仓库的标准工作流是并行子代理 + PORTING 坑位必读 + AUDIT 开放项核对，但入口目前只存在于会话记忆。建议一份 30 行以内的 AGENTS.md：必读文档指针（PROJECT/USAGE/PORTING/AUDIT）、验证命令、既有纪律（全仓测试逐 crate 跑、多 worktree 不共享 target、改播放/IPC 链后重建 + `hmp quit` 再实测）。事实全部 link 到现有 canonical 文档，不复制内容。

---

## P3 — 顺手级

7. **大文件观察清单**（file_metrics `high`，均为调查信号、不给机械拆分结论）：
   - `crates/hmp-storage/src/db.rs`（2344 非空行）：schema + 迁移 + 查询三职责同居，未来 schema 演进时优先审视职责边界；测试已拆 `db_tests.rs`，健康。
   - hmp-desktop 桥接三件套 `backend.rs`/`bridge.rs`/`player_bridge.rs` 合计约 3300 行：职责边界目前靠 PORTING 叙述维系，下次动桥接时可顺势复核。
   - `crates/hmp-daemon/src/engine_tests.rs`（3399）：测试文件，已有 `#[path]` 拆分先例（AUDIT §3），不动。
   - `apps/hmp-tauri/src/lib/api/mock-data.ts`（1205）：随 P1-1 处置消失。
8. `ci.yml` deny job `continue-on-error: true`：依赖/许可证审计长期不挡合并；若当前全绿，考虑转 blocking。
9. `cargo doc --no-deps` 无 `RUSTDOCFLAGS: -D warnings`，rustdoc 警告不拦截（CI test job）。
10. `AUDIT_WINDOWS_CLIENT.md:9`「中间产物已固化在 /tmp/qqmusic_audit/」——/tmp 易失；正文四域情报已自含，删该句或注明已失。
11. `AUDIT.md:1` 标题仍写「（2026-09-03，Linux 侧）」，实际 §18-§22 已是 Windows 专题；滚动文档标题更新一下即可。

---

## 明确不动清单（防过度清理）

- `.zcodeignore` 与 `.gitignore` 大段重复：同步机制使然，设计如此。
- `.shots/`、`.deps/`、`.superpowers/`、`.worktrees/`、`.v2c/`、`.zcode/`：全部已 ignore 的本地工作区产物（截图配方、Python 参考源、会话计划）。
- `node_modules/`、`dist/`、`target/`：未跟踪。`pnpm-lock.yaml` 跟踪正确。
- `crates/hmp-qqmusic-api/tests/fixtures/`（24 文件）：有意的仓库输入，非垃圾。
- 凭证/敏感信息：repo_scan 零发现；脱敏 Debug/keyring 纪律有专门审计背书（AUDIT §2、PROJECT §13）。
- `git worktree list` 仅主工作树，无遗留注册。

## 处置矩阵

| 项 | 类别 | 建议动作 | 前置 |
|---|---|---|---|
| P1-1 tauri 遗留 | Review | 按 b 处置：DESIGN.md 收纳、Vue 实现删除 | 用户确认 |
| P1-2 README 失实 | Safe | 直接修 | — |
| P1-3 CI Windows | Safe | 加 windows check job | — |
| P2-4/5 文档漂移 | Safe | 直接修 | — |
| P2-6 AGENTS.md | 增量 | 30 行指针 | 用户确认 |
| P3-7..11 | Safe/Review | 顺手修 | — |

## 复现

```bash
python ~/.zcode/skills/repo-steward/scripts/repo_scan.py .
python ~/.zcode/skills/repo-steward/scripts/file_metrics.py .
python ~/.zcode/skills/repo-steward/scripts/docs_inventory.py .
git ls-files | cut -d/ -f1 | sort | uniq -c | sort -rn   # 29% 占比的出处
grep -rn "hmpd" crates/                                  # 空：tauri README 构建命令失实的出处
```
