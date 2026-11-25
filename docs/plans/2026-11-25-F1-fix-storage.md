# 修复任务书 · 存储层（hmp-storage）

> §30.7 修复轮任务书（F1 · 存储）。审计依据：`docs/AUDIT.md` §30.1、§30.2.4、§30.7；基线见 `docs/verify/2026-11-25-fix-round/baseline.md`。

你在仓库 hutao-music-player（HMP，Rust 写的 QQ 音乐播放器）的一个**独立 git worktree** 里做修复。背景见主检出里的审计文档 §30（只读，不要改它）：
`/mnt/d/code-linux/hutao-music-player/docs/AUDIT.md` 的「## §30」一节，重点读 30.1、30.2.4、30.7（约 100 行，不必通读全文档）。

## 你的工作树（只在这里改）
`/mnt/d/code-linux/hutao-music-player/.worktrees/fix-storage`

**文件所有权**：你只能改 `crates/hmp-storage/` 下的文件（主要是 `src/db.rs`、`src/local.rs`、`src/db_tests.rs`、`src/local.rs` 内的测试模块）。
其它 crate、其它工作树、主检出 `/mnt/d/code-linux/hutao-music-player/`（你的工作树之外）一律不动。如果你认为所有权之外的文件必须改，**停下来在报告里说明**，不要自己改。

## 硬性约束
- 不 commit、不建分支、不 stash、不 reset。改动留在工作树里即可（主代理负责合入）。
- 不新增第三方依赖；**不做 schema 迁移**（`user_version` 保持 8，不加列、不建表）。
- 不跑 `--ignored` 测试（它们需要真实声卡）。不运行任何 `hmp` / `hmp-desktop` 二进制；不读写 `~/.local/share/hmp`、`~/.config/hmp`、`~/.cache/hmp`。新测试一律用内存库或 `tempfile`。
- cargo 只跑下面「验证」里的命令，并带 `--locked --offline`；不要跑整个 workspace 的 build/test。
- 改动要小而准，不做顺手重构；注释与命名跟周围代码一致（中文文档注释、相同的密度）；运行期告警用 `tracing::warn!`。
- shell 是 zsh：glob 要加引号（`--include='*.rs'`）；循环变量别叫 `path`；`echo ===` 要加引号。
- 只读任务书点名的函数及其直接依赖，不要通读仓库。

## 修复项（行号以当前工作树为准，可能有±几行偏差）

### 1. [P0] 分片 `NOT IN` 把「缺席清理」变成「全清」
位置：`crates/hmp-storage/src/db.rs` 的 `reconcile_remove_absent`（约 1453-1487）与 `delete_playlists_absent`（约 1492-1516）。

现状：`present_keys.chunks(500)`，每片执行一次 `… AND entity_key NOT IN (本片)`。片 1 清掉片 2 的行，片 2 再清掉片 1 的行——远端快照超过 500 条时，该 provider 所有 synced 行全被清掉（用户「我喜欢」866 首 → 865 行被置 0，已在真实库副本上复现）。

要求：
- 改成**单次、整体**的排除语义。二选一：临时表（`CREATE TEMP TABLE … ` + 分片做正向 `INSERT` + 一条 `NOT IN (SELECT key FROM temp)`），或 `json_each(?1)` 传一个 JSON 数组。
- 每次**函数调用只执行一条** UPDATE / DELETE，不能按片重复执行。
- **保持**「`present_keys` 为空 → 清理该 provider 全部 synced 行」这一既有语义不变（空快照的策略由 daemon 一侧负责，不归你）。
- `delete_playlists_absent` 同样处理。
- 把注释里过时的「SQLite 变量上限 999」改掉（默认上限 32766；分片只是为了稳妥）。
  （原文此处写「内置 3.54.1」有误——实测内置 SQLite 为 3.53.2 / libsqlite3-sys 0.38.2，存储代理指出，2026-11-25 更正。）

测试（`db_tests.rs`，沿用现有写法）：
- 600 个在场 key + 1 个缺席 key → 只有缺席的那行被置 0，600 行保持 `desired=1`。
- 1200 个在场 key 的同款断言。
- `delete_playlists_absent`：600 个在场 + 1 个缺席的 subscribed 歌单 → 只删缺席的。
- 既有的 pending 保护测试保持通过。

### 2. 移动/改名本地文件后「喜欢」失联
位置：`db.rs` `record_scan_file` 的移动复用分支（约 739-790，`find_by_fingerprint` 命中且 `mtime_ns` 相等、`key_free` 的那一支）。

现状：只改 `local_files.path` 和 `tracks.source_key`（`UPDATE tracks SET source_key = ?1 WHERE id = ?2`），`relations.entity_key` 仍是旧路径；而 `list_favorites`、`library_tracks(liked_only)`、`is_favorite` 都按 `source_key = entity_key` 关联 → 这首歌的喜欢静默丢失。

要求：在同一分支里把该曲目的本地关系行搬到新键（当前本地行只有 `liked` 一种关系）：
- 先读出旧的 `tracks.source_key`，再做现有的两个 UPDATE。
- 对 `entity_type='track' AND provider='local' AND entity_key = 旧键` 的每一行：若新键下已有同一 `relation` 的行，删除旧行；否则把 `entity_key` 改成新键。
- 尽量与移动本身处于同一事务（先看 `record_scan_file` 现在是否已在事务里）。
- 不要动历史迁移代码（v5/v7）。

测试：内存库里注册路径 A（带指纹）→ `set_relation("track","local","local:A","liked",true)` 并标记 synced → 用同一指纹、同一 `mtime_ns` 在路径 B 调 `record_scan_file` → 断言 `list_favorites` 仍包含该曲、`relation_desired(…"local:B"…) == Some(true)`、旧键下没有残留行。

### 3. 带标准日期标签的本地文件年份恒空
位置：`crates/hmp-storage/src/local.rs`（约 123-125）：
`year: tag.and_then(|t| t.get_string(ItemKey::Year)).and_then(|y| y.trim().parse::<i64>().ok())`

现状：lofty 0.25 把 `DATE` / `TDRC` / `©day` / `ICRD` 映射为 `RecordingDate`，只有非标准的 `YEAR` 才是 `ItemKey::Year`，所以 FLAC/MP3/M4A 的年份读不到；而且 `"2020-05-01".parse::<i64>()` 也会失败。

要求：先读 `ItemKey::RecordingDate`，没有再退 `ItemKey::Year`；取字符串里**第一段连续 4 位数字**作为年份（`2020-05-01` → 2020），没有则 `None`。把解析抽成一个纯函数。

测试：用 lofty 在内存里构造标签（VorbisComments 的 `DATE=2020-05-01`、`YEAR=2019`、空/无日期）断言解析结果。

### 4. 批量 upsert 把真实标题打回 mid
位置：`db.rs` `upsert_tracks_batch`（约 495-510）。现状 `title = excluded.title` 无条件覆盖；单条版 `upsert_track`（约 326-337）有保护：`excluded.title` 等于（原始）key 时保留 `tracks.title`。

要求：批量版使用与单条版**完全相同**的保护规则（先读懂单条版 `?13` 参数是什么）。
测试：单条 upsert 写入真实标题 → 批量 upsert 传入「标题 == key」的 stub 行 → 标题不变；批量传入另一个真实标题 → 更新。

### 5. 本地封面只认内嵌图
位置：`local.rs` `read_meta`（约 74-132，内嵌图在约 87-102）。

要求：没有内嵌图时，探测音频文件所在目录的封面文件：主名 `cover` / `folder` / `front` / `album` / `albumart`（含首字母大写变体，如 `Cover`），扩展名 `jpg` / `jpeg` / `png`；取第一个存在的常规文件，大小不超过现有的 `MAX_COVER_BYTES` 常量（超限视为没有）；返回其字节，走与内嵌图相同的返回通道。命中即停，不要遍历目录。把探测抽成纯函数 `folder_cover(dir) -> Option<Vec<u8>>`。

测试：`tempfile::tempdir()` 里放一个最小合法 PNG 的 `cover.jpg` 与一个不放封面的目录，断言纯函数结果。如果现有测试里有生成最小音频文件的办法（先看 `db_tests.rs` / `scan.rs` 的测试怎么造文件），再补一条走 `read_meta` 的端到端断言；造不出就只测纯函数，并在报告里说明。

### 6. 专辑曲序与专辑歌手
位置：`db.rs` `local_tracks_by_album`（约 1138-1159，排序在 1143）与 `library_albums`（约 1090-1116）。

要求：
- 曲序改为 `ORDER BY COALESCE(t.disc_number, 1), COALESCE(t.track_number, 999), t.title`（多碟专辑不再交错）。
- 专辑歌手：有 `album_artist` 时用它——`COALESCE(MAX(NULLIF(t.album_artist, '')), MAX(t.artist))`；没有时保持现有 `MAX(t.artist)` 行为。**不要改分组键**（仍按专辑名分组）。

测试：双碟曲序；有 `album_artist` 时取它；没有时行为不变。

## 验证（在你的工作树里跑；依赖已预热，通常是增量编译）
```
cargo fmt --all -- --check
cargo clippy -p hmp-storage --all-targets --all-features --locked --offline -- -D warnings
cargo test -p hmp-storage --locked --offline
```
三项都必须通过。`cargo fmt --all -- --check` 不过就先 `cargo fmt --all` 再复查。

## 交付报告（中文，≤ 500 字 + 清单）
1. 每个修复项一行：改了什么（`文件:行号`）、新增测试名。
2. 三条验证命令的结果（通过/失败数）。
3. 没做完、拿不准、或你认为是任务书错的地方，明确写出来。
4. 新增文件清单（若有）。
不要写补丁文件，不要写总结文档。
