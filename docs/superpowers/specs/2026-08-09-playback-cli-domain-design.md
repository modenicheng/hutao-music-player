# 2026-08-09 播放正确性收尾 + CLI/domain 重构设计

来源：用户审计（基于 `38a1913`，当前 HEAD `7ea220f`）。CI 由独立 agent 负责，本设计不含 CI 改动。

## 范围

- **第 1 步 Playback correctness**：`QueueRemove(current)` 事务化；`QueueClear` 语义拆分；Repeat One 只影响 EOS。
- **第 2 步 CLI/domain 边界**：clap 真正二级命令；`QueueItemView/QueuePage` 分页投影；resolver 返回 `TrackStub` 并批量缓存 SQLite。
- 不做：CI（独立 agent）、UserApi/CommentApi/SyncWorker（第 3-6 步）、`library sync` 等新功能。

## 第 1 步：播放正确性

### 1.1 QueueRemove(current) 事务化（对齐 navigate_next 模式）

现状（engine.rs ~144）：先 `queue.remove(i)` → `end_session("manual")` → publish → 再 `load_and_play`（失败被吞，无回滚）。

改为：

```
save_state + 记 old_db_track
queue.remove(i)
was_current?：
  有接替曲 → load_and_play：
    成功 → close_session(old, "manual")（同曲延续则跳过）【load_and_play 内已 publish】
    失败 → restore_state（旧队列/旧曲继续播放；last_error 已由 load_and_play 设置）
  无接替曲 → end_session("manual") + driver.stop() + publish（确定性停止）
非 current → publish
```

### 1.2 QueueClear 语义拆分

- IPC：`Request::QueueClear` → `Request::QueueClear { all: bool }`。
- `clear`（all=false，**保留当前曲**）：QueueCore 新方法 `clear_pending()`——删除除当前曲外全部曲目，order 重建为 `[0]`，cursor=0，has_current 保持。播放器/会话不动。
- `clear --all`（all=true）：`queue.clear()` + `end_session("stop")` + `driver.stop()` + publish。
- 不再允许「队列已空但 current 正在播」的中间态（用户定义）。

### 1.3 Repeat One 只影响 EOS

- `prev_track()` 的 `LoopMode::Track` 分支改为与 `None` 一致（回退、队首即停），不再重播当前曲。
- `advance_on_eos` 保持 Track 重播（唯一 Track 语义）。
- 更新 queue.rs 既有 Track 模式测试。

## 第 2 步：CLI/domain 重构

### 2.1 TrackStub（hmp-core）

```rust
pub struct TrackStub {
    pub id: TrackId,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
}
```

`SourceResolver::resolve_source_ids` 返回类型 `Vec<TrackId>` → `Vec<TrackStub>`。

- QQ resolver：歌单/专辑分页响应的 `Song`（已有 mid/name/singer/album/interval）直接提取 stub——不再丢弃元数据。
- local resolver：`LocalSourceResolver` 持有 library 引用（已 clone），从 `tracks`/`local_files` 查 meta；查不到回退 title=文件名。
- 引擎 `play_source`/`QueueAppend` 解析成功后：stubs 批量 upsert 媒体库（**缓存层**，library 为 None 时跳过、warn）→ 队列仍只存 `TrackId`（QueueCore 不变）。

### 2.2 SQLite 批量投影（hmp-storage）

- `upsert_tracks_batch(&[TrackRow])`：单事务批量 upsert（解析 1500 曲时避免 1500 次独立提交）。
- `track_meta_batch(source, keys) -> Vec<TrackMeta>`：`IN` 查询；SQLite 变量上限 999 → 按 500 分片。
- 返回 `TrackMeta { source, source_key, title, artist, album }`。

### 2.3 IPC 分页查询

- `Request::QueueList { offset: usize, limit: usize }` → `Response::QueueList(QueuePage)`。
- `QueuePage { total, offset, items: Vec<QueueEntry> }`，`QueueEntry { track_id, is_current }`（纯 ID——server 无 library，投影在 CLI 侧做，符合「read 本地 DB 快读」方向）。
- server 处理 QueueList：从 `state_rx` 快照切片（与 Queue 同路径）。
- CLI `queue list`：默认 `--limit 50`；`--all` 自动翻页到 total；拿 IDs 后本地 SQLite `track_meta_batch` 组装打印：

```
    #   MID               TITLE                  ARTIST
▶   0   003OUlho2HcRHC    夜曲                   周杰伦
```

### 2.4 clap 二级命令

命令面（高频短命令保留顶层 alias；`Queue/Playlist/Favorite` 停止 `Vec<String>` 手工解析）：

```
顶层（alias）：play play-next pause resume next prev stop seek volume status quit serve search login auth scan favorite
player：  status pause resume next prev stop seek volume quality      （quality 迁入）
queue：   list show add play-next remove clear[--all] shuffle loop    （shuffle/loop 迁入；clear 加 --all）
playlist：list show create rename add remove delete                   （现有 CRUD 迁移，rm→delete、rm-track→remove）
library： history                                                     （history 迁入）
```

- `Queue` 变体改为 `queue: QueueCommand` 等真 enum；`favorite::run`/`playlist::run` 改收结构化参数。
- `hmp queue show` 保留为 `list` 别名（daemon_cli 集成测试兼容）。
- 第 3-6 步的命令（library tracks/albums/sync、account、comment）**不预留空壳**。

## 测试

- QueueCore：`clear_pending` 保留 current；`prev_track` Track=回退；既有 Track 测试更新。
- engine：QueueRemove(current) 装载失败回滚（队列恢复、会话未关、错误可见）；QueueClear 两语义；ipc 序列化测试更新。
- resolver：FakeResolver 改返回 stub；local stub 回退；QQ Song→stub 映射单测。
- storage：batch upsert 事务、meta_batch 分片（>999 keys）。
- CLI：queue list 投影（库外 ID 回退显示）；clap 子命令解析。
- 全量：cargo test --workspace / clippy / fmt / e2e / daemon_cli。
