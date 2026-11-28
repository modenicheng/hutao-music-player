# 任务书 · 曲目信息停止丢（§30.2.6 phase 1）

> 依据：`docs/AUDIT.md` §30.2.1、§30.2.6 phase 1；前置修复见 §30.7 与 §31。
> 这是「曲目—音源」拆分的第 1 步，只做「停止丢信息」，**不建 `track_sources`、不改 UI 导航**。

## 背景（现状即缺陷）

- QQ `Song` 自带 `singer[].mid` 与 `album.mid`，但三条入库路径（`reconcile::qq_track_row`、
  `engine::stub_row`、`engine::track_row`）只留名字，mid 入库即丢。
- 多艺人三种写法：播放路径 `" / "`，列表缓存与 reconcile `", "`，本地标签原文。
- 本地专辑按专辑名分组，同名不同专辑歌手会被合并；`album_artist` 已落库但分组未用。
- `track_artists` 只有 `artist` 文本，没有 mid；`tracks` 没有 `album_mid`。
- 本地行没有「标签解析版本」：修了 `read_meta` 的解析规则后，存量行不会自动重扫
  （§30.2.4 D4/D9 的同类问题）。

## 目标 schema（v9，只加列/表，可回滚）

```sql
ALTER TABLE tracks ADD COLUMN album_mid TEXT;
ALTER TABLE track_artists ADD COLUMN artist_mid TEXT;      -- qq: singer mid；local: NULL
ALTER TABLE local_files ADD COLUMN tag_version INTEGER NOT NULL DEFAULT 0;
```

- `user_version` 8 → 9；迁移逐级事务，旧库可直接升。
- 不需要回填 mid（旧行保持 NULL，等下一轮 reconcile/扫描自然补）。
- 索引可选：`track_artists(track_id)` 已有/按需；不新增唯一约束（现有无 UNIQUE，保持语义）。

## 数据流改动

1. **领域模型**
   - `hmp_core::TrackStub` 增加 `album_mid: Option<String>`、`artist_refs: Vec<ArtistRef>`
     （`ArtistRef { id: ArtistId, name }` 已存在；QQ 用 mid 作 id，本地用 `"local"`）。
     `artists: Vec<String>` 保留（展示/兼容），但生产方必须同时填 `artist_refs`。
   - `hmp_storage::TrackRow` 增加 `album_mid: Option<String>`、
     `artist_refs: Vec<ArtistRefRow>`（storage 本地结构：`{ mid: Option<String>, name: String }`，
     不要让 storage 依赖 hmp-core）。
   - 展示串统一：所有生产方用同一个 helper 拼 `" / "`（`Track::artist_names` 的口径），
     不再出现 `", "` 与标签原文混用。
2. **入库**
   - `qq_track_row`：从 `song.singer[].mid` / `song.album.mid` 填 `artist_refs` / `album_mid`；
     展示串走统一 helper。
   - `stub_row` / `track_row`：从 `TrackStub`/`Track` 的 refs 填；没有 mid 时 `None`。
   - `upsert_track`：写 `tracks.album_mid`（`COALESCE(excluded, 旧值)`，空/None 不覆盖）；
     `write_track_artists` 改为写入 `(track_id, artist, artist_mid, position)`，先删后插保持幂等；
     新值为 NULL 时保留旧 mid（同 COALESCE 语义）。
   - 本地 `read_meta`：`artist_refs` 的 mid 为 `None`；`album_mid` 为 `None`。
3. **分组**
   - `library_albums`：`GROUP BY t.album, COALESCE(NULLIF(t.album_artist,''), t.artist)`
     （同名不同专辑歌手不再合并）；`library_tracks` 排序不变。
   - `AlbumGroup` 增加 `album_mid: Option<String>`（取 `MAX(t.album_mid)`），桌面端只透传/编译适配，
     不在本任务里改导航。
4. **tag_version**
   - `hmp_storage::LOCAL_TAG_VERSION: i64 = 2`（本次解析规则版本；今后改 `read_meta` 就 +1）。
   - `record_scan_file`：即使 `mtime_ns + size` 未变，若 `local_files.tag_version < LOCAL_TAG_VERSION`
     也重读标签并更新（`ScanOutcome::Updated`），写回新版本号。
   - `add_local_file` 写入当前版本；旧行（0）在下一轮扫描自然升级。
   - 不做后台批量重扫；只在扫描/播放路径增量升级。

## 验收测试（必须）

- 迁移：v8 库升 v9，`user_version == 9`，旧数据仍在；重复打开幂等。
- QQ 入库：构造带 `singer[].mid`、`album.mid` 的 `Song`，`qq_track_row` 后
  `track_artists.artist_mid` 与 `tracks.album_mid` 都有值。
- 展示串：同一组歌手经 `qq_track_row`/`stub_row`/`track_row` 产出的 `artist` 完全一致。
- 分组：两张同名专辑、不同 `album_artist` → `library_albums` 两行；同专辑多曲仍一行。
- tag_version：插入一行 `tag_version = 0`、`mtime_ns/size` 与文件一致 → 扫描后
  `Updated` 且标签字段被刷新、版本变 2；再扫一次 `Skipped`。
- 全仓 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
  `cargo test --workspace` 全绿；逐 crate 跑，别只跑一个包。

## 约束

- 不加依赖；不改 schema 之外的生产行为（播放/队列/同步语义不动）。
- 不做 §30.2.6 phase 2/3（artists/albums 实体表、track_sources、local-first 回退），
  不建 `track_sources`，不删旧列。
- 每个逻辑改动单独 commit（英文 Conventional Commits），不夹带无关文件。
