# 任务书 · 桌面实体导航（§30.2.6 phase 2b）

> 前置：`feat/entities-storage`（schema v10、实体表、按 id 查询，见 `docs/plans/2026-11-28-M2-entities.md`）。
> 本任务只改桌面与 daemon 的“导航身份”，不改存储 schema。

## 背景

phase 2a 已提供：

- `ArtistGroup { id, artist, artist_mid, track_count }`
- `AlbumGroup { id, album, artist, album_mid, track_count, year, cover_uri }`
- `LibraryDb::artist_tracks_by_id(id)` / `album_tracks_by_id(id)` / `artist_by_id(id)` / `album_by_id(id)`

桌面仍按展示名猜：

- `SongRow::to_track_row` 把 `artist_mid`/`album_mid` 填成展示串（`library_view.rs`），
  `TrackTable`/播放页拿它当导航参数。
- `album_detail(name)`/`artist_detail(name)` 按名查；同名专辑的另一个实体仍会混入；
  歌手页靠“最长包含匹配”兜底，点“知更鸟 / HOYO-MiX / Chevy”会落到 HOYO-MiX。
- 播放页本地曲的 `ArtistRef.id` 是 `"local"`，没有实体 id。

## 目标（导航契约）

`Nav.param` 唯一字符串，按前缀分派：

| 前缀 | 含义 | 处理 |
|---|---|---|
| `qq:<mid>` | QQ 远端歌手/专辑 | 既有 `remote_details` 路径，不动 |
| `artist:<id>` / `album:<id>` | 本地实体 id（十进制） | 新本地详情路径，按 id 查 |
| 其它（裸展示名） | 旧链接/远端搜索兜底 | 保留现有按名逻辑，不能回归 |

- QQ 曲目行 → `qq:<真实 mid>`（phase 1 已把 mid 落库；`SongRow` 必须带出来，
  不能再填展示名）。
- 本地曲目行 → `artist:<entity_id>` / `album:<entity_id>`。
- 实体 id 不存在/0（历史直插行）→ 回退裸展示名，保持旧行为。

## 改动清单

1. **`SongRow`**（`crates/hmp-desktop/src/library_view.rs`）
   - 增 `artist_id: i64`、`album_id: i64`、`artist_mid: Option<String>`、`album_mid: Option<String>`；
     保留 `artists`/`album` 展示串。
   - 本地行从 `track_artists.artist_id` / `tracks.album_id` / `artist_mid` / `album_mid` 投影；
     QQ 行从 `track_meta_batch`/`track_artists` 投影（没有 entity id 时 0）。
   - `to_track_row` 生成两个新字段（或直接生成 param 串）：本地优先 `artist:<id>`，
     QQ 用 `qq:<mid>`，id/mid 都缺 → 裸展示名。
2. **详情解析**（`library_view.rs` + `bridge.rs::load_detail`）
   - `album_detail(param)`：`album:<id>` → `album_by_id` + `album_tracks_by_id`；
     `qq:` 保持远端分流；其它走按名旧路径。
   - `artist_detail(param)`：`artist:<id>` → `artist_by_id` + `artist_tracks_by_id`；
     其它走按名旧路径。
   - `apply_route` 的本地/远端分流保持“本地命中优先、miss 再远端”，但 `album:<id>`
     不得被 `remote_param` 误判。
3. **播放页/队列链接**（`player_bridge.rs` + `now-playing.slint` 若需要）
   - 本地曲：daemon 下发的 `ArtistRef.id`/`AlbumRef.id` 改为 `artist:<id>`/`album:<id>`
     （`crates/hmp-daemon/src/local.rs::resolve_local` 在入库/查询后取实体 id；
     `Track` 的展示名不变）。
   - QQ 曲：保持 `qq:<mid>`。
   - `player_bridge` 的 `artist_link`/`album_link` 直接消费该 id；本地没有实体 id 时回退展示名。
4. **列表/卡片入口**：`TrackTable` 行、专辑卡、歌手卡、搜索结果里的歌手/专辑链接
   全部走上述 param；不要另拼展示名。
5. **测试**
   - 本地曲目行点专辑 → `album:<id>` → 只出现该实体的曲目；同名另一实体不混入。
   - 本地曲目行点歌手 → `artist:<id>` → 该实体曲目；不再走“最长包含匹配”。
   - QQ 曲目行 → `qq:<mid>`，仍走远端详情（mock 回包）。
   - 实体 id=0/旧裸名 → 回退旧路径（回归测试）。
   - 播放页本地曲的歌手/专辑链接 param 正确（单测直接断言 `Player` 字段或 bridge 函数）。
   - `cargo test -p hmp-desktop` 全绿；`cargo test --workspace` 不回归。

## 约束

- 不做 phase 3（`track_sources`、本地优先回退、手动关联）；不改存储 schema。
- 不改视觉设计/布局；只改导航身份与数据接线。
- 远端详情（`remote_details`）行为不变；`qq:` 优先级不变。
- 不加依赖；`fmt`/`clippy`/`doc -D warnings` 全绿（CI 会跑 doc）。
