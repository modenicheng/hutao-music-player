# 2026-08-09 媒体库内核 + 用户库 + 评论 + 播放状态机设计（审计第 3–7 步）

承接 `docs/superpowers/specs/2026-08-09-playback-cli-domain-design.md`（第 1–2 步已落地：`cbfd91b`/`e08fcfc`）。上游 oracle：L-1124/QQMusicApi（本会话已取 `modules/user.py`/`modules/comment.py` 全文）。

## 3. LibraryService + SyncWorker（本地先提交，QQ 乐观同步）

### 3.1 迁移 v2（`PRAGMA user_version` 1→2）

```sql
CREATE TABLE relations (                -- 统一关系表 = durable outbox 一体
  entity_type TEXT NOT NULL,            -- track|album|playlist
  provider    TEXT NOT NULL,            -- qq|local
  entity_key  TEXT NOT NULL,
  relation    TEXT NOT NULL,            -- liked|owned|subscribed
  desired_state   INTEGER NOT NULL,     -- 本地意图（操作合并：同 PK upsert 覆盖）
  last_remote_state INTEGER,            -- 远端快照
  sync_state  TEXT NOT NULL DEFAULT 'synced',  -- synced|pending|error
  retry_count INTEGER NOT NULL DEFAULT 0,
  last_sync_error TEXT,
  updated_at  INTEGER NOT NULL,
  PRIMARY KEY (entity_type, provider, entity_key, relation));

ALTER TABLE playlists ADD COLUMN provider   TEXT NOT NULL DEFAULT 'local';
ALTER TABLE playlists ADD COLUMN remote_id  TEXT;      -- owned=QQ dirid；subscribed=disstid
ALTER TABLE playlists ADD COLUMN relation   TEXT NOT NULL DEFAULT 'local';
ALTER TABLE playlists ADD COLUMN sync_state TEXT NOT NULL DEFAULT 'synced';
ALTER TABLE playlists ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE playlists ADD COLUMN last_sync_error TEXT;

CREATE TABLE playlist_ops (             -- owned 歌单曲目增删 outbox
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  playlist_id INTEGER NOT NULL,
  op TEXT NOT NULL,                     -- add|del
  song_id INTEGER,                      -- QQ numeric song id（NULL=未知待补）
  sync_state TEXT NOT NULL DEFAULT 'pending',
  retry_count INTEGER NOT NULL DEFAULT 0,
  last_error TEXT, updated_at INTEGER);

ALTER TABLE tracks ADD COLUMN qq_song_id INTEGER;   -- comment biz_id 映射
-- favorites 数据迁入 relations(track,qq|local,liked) 后 DROP TABLE favorites
```

冲突规则（reconcile 时）：`sync_state != 'synced'`（有 pending 意图）→ 本地 desired 胜，跳过；否则 QQ snapshot 胜。操作合并：同一 PK 的 `set_relation` upsert 即最后一次意图胜出（收藏→取消→再收藏 只发一次远端请求）。

### 3.2 SyncWorker（daemon 后台任务）

- 触发：`tokio::sync::Notify`——引擎处理写命令后 notify；另 30s 定时兜底。
- 循环：扫 `relations pending/error` + `playlists pending` + `playlist_ops pending` → 用 QqMusicClient + 当前凭证执行：
  - track/liked：`like_song/unlike_song((songid,1))`（songid 取 tracks.qq_song_id，NULL → error 留待 reconcile/播放补全）
  - playlist/subscribed：`fav_songlist/unfav_songlist(disstid)`
  - owned 歌单：`create/rename/delete(dirid)`；playlist_ops：`add_songs/del_songs(dirid,(songid,1))`
- 成功：置 synced + `last_remote_state=desired_state` + retry_count=0；失败：error + retry_count+1，重试间隔 `min(2^retry_count*10s, 10min)`。
- 无凭证：整体跳过（离线收藏合法，留 pending）。

### 3.3 daemon 命令（写走 daemon，读直读本地 DB）

`Request` 新增：`Favorite{source,key,title,desired}`、`PlaylistWrite{op: Create|Rename|Delete|AddTrack|RemoveTrack}`、`LibrarySync`（触发 reconcile）、`LibrarySyncStatus`。CLI `favorite add/remove`、`playlist create/rename/delete/add/remove-track` 全部改为 daemon 命令（停止 CLI 直写 DB）；`favorite list`/`playlist list|show`/`library history`/`sync-status` 直读 DB。

## 4. UserApi + 首次 reconcile

hmp-qqmusic-api 新增 `user.rs`（按上游 module 逐字移植参数）：

| Rust 方法 | module/method | 关键参数 |
|---|---|---|
| `get_created_songlist(uin)` | music.musicasset.PlaylistBaseRead / GetPlaylistByUin | `{"uin": str(uin)}` |
| `get_fav_song(euin,page,num)` | music.srfDissInfo.DissInfo / CgiGetDiss | dirid=201, song_begin/song_num（复用 GetSonglistDetailResponse + collect_paged） |
| `get_fav_songlist(euin,page,num)` | music.musicasset.PlaylistFavRead / CgiGetPlaylistFavInfo | offset/size |
| `get_fav_album(euin,page,num)` | music.musicasset.AlbumFavRead / CgiGetAlbumFavInfo | offset/size |
| `fav_songlist(id)` / `unfav_songlist(id)` | music.musicasset.PlaylistFavWrite / FavPlaylist / CancelFavPlaylist | v_playlistId 数组 |
| `get_homepage(euin)` | music.UnifiedHomepage.UnifiedHomepageSrv / GetHomepageHeader | uin |
| `get_vip_info()` | VipLogin.VipLoginInter / vip_login_base | require_login |

reconcile（`hmp library sync` / daemon 启动有凭证时）：拉 fav_song 全页 → relations(track,liked)；fav_songlist → relations(playlist,subscribed) + playlists(remote_id=disstid)；created_songlist → playlists(remote_id=dirid, owned)；fav_album → relations(album,liked)。**只 upsert 远端事实，不覆盖 pending**。

CLI：`library sync`（触发 + 轮询 sync-status 至空闲，60s 超时）、`library sync-status`、`library tracks --liked`、`library albums --liked`（直读 DB）；`account profile` / `account vip`（CLI 本地凭证直连 QQ，读操作）。

## 5. 统一 Playlist domain

`playlists` 表即统一视图：`hmp playlist list --scope all|local|owned|favorite`（默认 all）列 `TYPE`（local/qq-owned/qq-favorite）。权限：local 完全离线可写；owned 可写（本地提交 + playlist_ops outbox 同步）；subscribed 只读（仅 unfavorite 走 relations）。

## 6. CommentService

hmp-qqmusic-api 新增 `comment.rs`：`get_comment_count`、`get_hot_comments`、`get_new_comments`、`get_recommend_comments`（music.globalComment.CommentRead，PageNum 0 基 + LastCommentSeqNo 游标）、`add_comment`（含 reply_cmt_id）、`delete_comment`。biz_type=SONG=1、biz_sub_type=2。

daemon `Request::Comment{...}`（list/post/reply/delete）：**mid → tracks.qq_song_id → biz_id**（查不到报错提示先播放/同步）；读走内存 TTL cache（5min，`HashMap<(mid,sort), (Instant, page)>`），写直发 QQ 不缓存。CLI：`hmp comment list <mid> [--sort hot|new|recommend]`、`post <mid> <text>`、`reply <mid> <cm-id> <text>`、`delete <cm-id>`。

## 7. PlaybackEngine 显式状态机

- `EnginePhase { Idle, Resolving, Loading, Playing, Failed }` 进 `DaemonState`（CLI status 显示）。
- 换曲操作（play_source/navigate/QueueRemove 接替）前置 `phase_generation += 1`；`load_and_play` 全程持有本次 generation。
- 滞后事件防护：`PlaybackEnded` 到达时若 `phase == Loading`（新曲装载中，EOS 属旧曲）→ 忽略；`PlayerEvent::Error` 同理（装载结果由 load_and_play 决定）。否则正常 `on_ended`。
- phase 迁移：解析开始→Resolving；解析成功→Loading；`wait_current_applied` 完成→Playing；解析/装载失败→Failed（回滚后按旧曲恢复 Playing/Idle）。

## 实现顺序（每步独立 commit + 测试）

3 → 4 → 5（依赖 3/4 的 schema 与 API）→ 6 → 7。CI 不碰（独立 agent）。
