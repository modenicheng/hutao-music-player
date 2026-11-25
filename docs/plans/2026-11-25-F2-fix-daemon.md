# 修复任务书 · daemon 一侧（hmp-daemon + hmp-core 的 IPC 类型 + hmp-qqmusic-api 的 user.rs）

> §30.7 修复轮任务书（F2 · daemon 一侧）。审计依据：`docs/AUDIT.md` §30.1、§30.2.4（D2、D5）、§30.3.3（U5）、§30.7；基线见 `docs/verify/2026-11-25-fix-round/baseline.md`。

你在仓库 hutao-music-player（HMP，Rust 写的 QQ 音乐播放器：常驻 daemon 经 IPC 被桌面端/CLI 遥控）的一个**独立 git worktree** 里做修复。背景见主检出的审计文档 §30（只读，不要改它）：
`/mnt/d/code-linux/hutao-music-player/docs/AUDIT.md` 的「## §30」一节，重点读 30.1、30.2.4（D2、D5）、30.3.3（U5）、30.7（不必通读全文档）。

## 你的工作树（只在这里改）
`/mnt/d/code-linux/hutao-music-player/.worktrees/fix-daemon`

**文件所有权**（只允许改这些）：
- `crates/hmp-daemon/src/`：`reconcile.rs`、`sync.rs`、`server.rs`（含其内测试模块）、`daemon.rs`（仅当接线需要）
- `crates/hmp-core/src/ipc.rs`
- `crates/hmp-qqmusic-api/src/user.rs` 及其测试（`crates/hmp-qqmusic-api/tests/`）
- `crates/hmp-desktop/src/menu_bridge.rs`（仅限第 4 项的一处调用点）
- `crates/hmp-cli/src/playlist.rs`（仅当第 4 项的构造点需要）

`crates/hmp-storage/`（存储层）与桌面端的其余文件正由另外两个代理并行修改，**一律不碰**。需要新的存储层函数时，先看现有的能否够用；确实不够就停下来在报告里说明，不要自己加。其它工作树与主检出（你的工作树之外）不动。

## 硬性约束
- 不 commit、不建分支、不 stash、不 reset。改动留在工作树里即可（主代理负责合入）。
- 不新增第三方依赖；**不做 schema 迁移**（`user_version` 保持 8）；跨轮状态只放在内存里。
- 不跑 `--ignored` 测试（它们需要真实声卡）。不运行任何 `hmp` / `hmp-desktop` 二进制、不 `cargo run`；不读写 `~/.local/share/hmp`、`~/.config/hmp`、`~/.cache/hmp`；不要用真实 QQ 账号或真实网络（测试用现有的 wiremock / FakeResolver 套路）。
- cargo 只跑下面「验证」里的命令，并带 `--locked --offline`；不要跑整个 workspace 的 build/test。
- 改动要小而准，不做顺手重构；注释与命名跟周围代码一致（中文文档注释、相同的密度）；daemon 面向用户的文案是英文。
- shell 是 zsh：glob 要加引号（`--include='*.rs'`）；循环变量别叫 `path`；`echo ===` 要加引号。
- 只读任务书点名的函数及其直接依赖，不要通读仓库。

## 修复项（行号以当前工作树为准，可能有 ±几行偏差）

### 1. [D2] 软失败被当成空快照；空快照被当成权威空
**1a. 接口层要报错而不是返回空**（`crates/hmp-qqmusic-api/src/user.rs`，`get_fav_song` 约 205-232）。
现状：`data.get("data").cloned().unwrap_or(json!({}))` 之后直接反序列化成全字段 `#[serde(default)]` 的结构体——响应缺 `data`（风控、登录态问题、截断）会被解析成「成功但 0 首」。
要求：`data` 缺失或不是对象 → `Err(QqMusicError::InvalidResponse(..))`；`data` 在场且业务码非 0 → 若响应结构体带业务码，就按仓库里歌单详情接口（`crates/hmp-qqmusic-api/src/songlist.rs` 的 `get_detail`）既有的处理口径办，不要自创风格。「`data` 在场、列表为空」仍是 `Ok(空)`。`user.rs` 里被 reconcile 用到的同类列表接口（`get_fav_songlist` / `get_created_songlist` / `get_fav_album`）若是同一写法，一并处理。**先 grep 所有调用点**，确认改成 `Err` 后调用方的处理是合理的（reconcile 失败只 warn 并保留本地）。测试沿用 `crates/hmp-qqmusic-api/tests/` 里现有的 wiremock 写法：缺 `data` → Err；`data` 在场且为空 → Ok(空)。

**1b. 空快照需要连续两轮一致才允许清理**（`crates/hmp-daemon/src/reconcile.rs` + `sync.rs`）。
现状：`reconcile_fav_songs` 拉到 0 首就 `reconcile_remove_absent(.., &[])`，而存储层对空集合的语义是「全量清理 synced 行」；`reconcile_fav_songlists` → `delete_playlists_absent` 同理。
要求：把判断做成一个小的、可单测的纯类型（例如带连续计数的闸门：空快照计数 +1，非空清零；只有连续两次为空才返回「允许清理」，否则本轮跳过清理并 `tracing::warn!`）。闸门状态放在 `SyncWorker`（`crates/hmp-daemon/src/sync.rs`，`run` 循环在 81-104）的内存里，通过参数传给 `reconcile_user_library` / `reconcile_fav_songs` / `reconcile_fav_songlists`；非空快照下的清理行为完全不变。收藏专辑（`reconcile_fav_albums`）若同形，同样处理。

**1c. 关闭卡死的 error 行**（`reconcile.rs` `reconcile_fav_songs` 约 167-205）。
现状：本地意图 `desired=1` 但同步一直失败（如 QQ 返回 1101）的行停在 `error`，每 10 分钟重试一次；即使远端其实已经收藏了，`reconcile_relation` 也会因为「非 synced 行本地意图优先」跳过它，永远不合拢。
要求：拿到完整的「我喜欢」快照后，对**在场**且关系行非 synced、`desired=1` 的 mid，调用现有的 `LibraryDb::mark_relation_synced` 标为已同步（远端已反映该意图）。只做「在场 + desired=1」这一种确定的情况，不要处理缺席方向（缺席需要完整快照前提，本轮不做）。

### 2. [D5] 歌单曲目补抓没有游标
位置：`reconcile.rs` 的 `reconcile_playlist_tracks`（约 341-420，常量 `PLAYLIST_TRACKS_BUDGET = 24` 在约 30）与 `sync.rs` 的 `run` 循环。
现状：每轮都从第 0 行开始、预算 24 个，注释写「下一轮续跑」但没有游标 → QQ 歌单超过 24 个时排在后面的永远补不到曲目。
要求（不改 schema）：每轮预算仍是 24，但从上一轮的游标续跑；一轮预算用完且还有剩余行时，在短延时后（例如 5 秒）自动续跑下一片，直到这一遍走完再把游标归零。续跑要交还控制权（在 `run` 的 `select!` 里加一个定时分支或一条新的 `SyncMsg`），不要在一个 `reconcile` 调用里 sleep 循环，以免阻塞其它同步消息。游标放在 `SyncWorker` 的内存里。切片算术抽成纯函数并单测（例如 `plan_slice(rows_len, cursor, budget) -> (start, end, next_cursor, has_more)`）。保留既有语义：单个歌单抓取失败或不完整 → 保留本地现状、下一轮重试；有待处理意图的歌单跳过。daemon 重启后从 0 开始可以接受。

### 3. 歌单写操作的拒绝文案误导
位置：`crates/hmp-daemon/src/server.rs` 的 `PlaylistWrite` 处理（约 336-470，尤其是 456-459 的 `Some(Err(rusqlite::Error::InvalidQuery)) => … "remote QQ playlists are read-only (subscribed: unlike only; owned: no rename)"`）。
现状：多种互不相干的原因（订阅歌单只读、QQ 自建歌单拒收本地曲目……）都复用 `InvalidQuery`，用户看到的是同一句与实际原因无关的英文。
要求：给这个处理器引入一个小的内部错误枚举（如 `SubscribedReadOnly` / `LocalTrackInQqPlaylist` / `PlaylistNotFound` / 数据库错误……），逐因映射到合适的 `IpcErrorCode`（见 `crates/hmp-core/src/ipc.rs` 的 `IpcErrorCode`）与**准确的英文**文案。除文案与错误码外不改变任何行为。更新断言旧行为/旧文案的既有测试。

### 4. [U5] 删除歌单曲目没有 key 校验
现状：桌面端传界面下标（`crates/hmp-desktop/src/menu_bridge.rs` 约 186-206），daemon 直接 `DELETE … WHERE playlist_id AND position`（`server.rs` 约 408-433 → 存储 `remove_playlist_track`）；快照过期时会删错行，owned 歌单还会同步到远端。
要求：
- `crates/hmp-core/src/ipc.rs`：`PlaylistWriteOp::RemoveTrack` 增加 `#[serde(default)] expect_key: Option<String>`。两个方向都要兼容：旧客户端不带该字段 → `None` → 不校验；旧 daemon 收到新客户端多出来的字段 → 结构体变体默认忽略未知字段（先确认该枚举没有 `deny_unknown_fields`）。更新所有构造点：`hmp-cli/src/playlist.rs:105`、`hmp-desktop/src/menu_bridge.rs:201`、`server.rs` 的测试（约 1552）。
- `server.rs`：取到 `track_key_at` 之后，若 `expect_key` 为 `Some(k)` 且与库内 `source_key` 不一致 → **不删除**，返回 `IpcErrorCode::BadRequest` 与一句说明（歌单已变化，请刷新后重试）。
- `menu_bridge.rs`：把该行的 key 作为 `expect_key` 传入。先确认表示法一致：`track_key_at` 返回的是 `tracks.source_key`（QQ 曲目是裸 mid，本地曲目是 `local:<路径>`），而 `TrackMenu.track.mid` 存的是什么需要你读代码核对；若两者不能保证一致，就传 `None` 并在报告里说明原因。
- 测试（沿用 `server.rs` 测试模块的写法）：不一致 → Err 且行仍在；一致 → 删除；`None` → 删除。

## 验证（在你的工作树里跑；依赖已预热，通常是增量编译）
```
cargo fmt --all -- --check
cargo clippy -p hmp-core -p hmp-qqmusic-api -p hmp-daemon --all-targets --locked --offline -- -D warnings
cargo test -p hmp-core -p hmp-qqmusic-api -p hmp-daemon --locked --offline
cargo check -p hmp-cli -p hmp-desktop --locked --offline
```
四项都必须通过（clippy 若报出你没碰过的文件里的既有问题，写进报告，不要顺手修）。`cargo fmt --all -- --check` 不过就先 `cargo fmt --all` 再复查。最后一条只需要通过编译——桌面端的测试由另一个代理负责，不要在这里跑 `cargo test -p hmp-desktop`。

## 交付报告（中文，≤ 500 字 + 清单）
1. 每个修复项一行：改了什么（`文件:行号`）、新增测试名。
2. 四条验证命令的结果（通过/失败数）。
3. 没做完、拿不准、或你认为是任务书错的地方，明确写出来（尤其是 1a 改成 `Err` 后影响到的调用点）。
4. 新增文件清单（若有）。
不要写补丁文件，不要写总结文档。

---

## 更正（2026-11-25 合入时补）

- **1a「按 `get_detail` 的既有口径处理非 0 业务码」前提有误**：`songlist.rs` 的 `get_detail` 自己也不查内层 `data.code`，只把它反序列化进 `code` 字段；请求层只映射外层 `req_0.code`。所以接口层没有这个「既有口径」。实际做法：只把 `data` 缺失 / 非对象改成报错；`data` 在场而 `data.code` 非 0（如歌单不存在 -100006）仍是 `Ok(空)`，只靠 1b 的闸门兜底。要不要改成直接拒绝待实测（AUDIT §30.8）。
- **1b 的闸门只拦空快照，半份快照（翻页中途空页）当时没拦**：合入审阅时补上，三个快照源都要求取全才定缺席（提交 `1fecaf7`）。
- 所有权清单漏了 `crates/hmp-daemon/tests/reconcile_playlist_tracks.rs`：`reconcile_user_library` 签名变更后 7 处调用点必须跟着改，代理越界改动予以采纳。
