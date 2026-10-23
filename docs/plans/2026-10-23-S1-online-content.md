# S1「在线内容动线」实施计划（D2 / C6 / T5）

> 工作流：feat/ui-online-content @ cd4a75e。本计划自包含，供独立评审后实施。
> 所有不带仓前缀的相对路径均以 worktree 根 `D:\coding\hutao-music-player\.worktrees\s1-online-content` 为基准。

## 修订记录

- **已按评审修订（2026-10-23，结论「通过但需小改」）**：
  - **M1**：发现页封面旁路表键统一经单一规范化辅助函数（登记 `register_playlist_covers` 与匹配 `apply_playlist_cover` 两侧同一函数，消除 `qq:` 前缀引入后的键失配回归面）；T4 验收增单测「推荐卡 mid 前缀 ↔ 旁路表键一致」。
  - **M2**：本地投影路径同步维护新 `*-state`（apply_detail 命中分支 → 就绪、None 分支 → 未找到，bridge.rs 各分支逐一落位）；T4 单测清单增「本地/远端交替导航后状态正确」。
  - **M3**：详情页状态机码位对齐仓内既有约定 **0=初始 / 1=加载中 / 2=就绪 / 3=未找到 / 4=失败**（top-page.slint:133-146、search-page.slint:3 同款），删除原「与 discover/top 码位一致」的错误暗示，各分支按新码位写明。
  - **M4**：tab 计数徽章直接绑定模型长度（`search-albums.length` 等，零桥接成本），废弃「search-count 语义改当前 tab」方案；T7 触碰清单同步更新。
  - **O1（采纳）**：state=1 落点照抄 `spawn_discover_load` 先例——UI 线程被调时**直接** `Data::get(&ui).set_*_state(1)`（勿套 `invoke_from_event_loop`，TestingBackend 下会挂起 mock 泵，bridge.rs:154-156/232-234 注释在案）；异步回包再走 invoke。
  - **O2（采纳）**：桌面侧补 TestingBackend 集成测试——驱动 on_navigate A→B + 迟到回包丢弃（tests/context_menu.rs 基建）。
  - **O3（采纳）**：T3 wiremock 挂载名以 api 层源码（singer.rs 等）的请求构造字段为准照抄，不凭记忆写端点名；验收注明「按 api 层请求构造字段匹配」。
  - **O4（采纳）**：R1 探针失败的降级源确认为 `PlaylistSquare`（recommend.rs `get_recommend_songlist`），保留为降级路径。
  - §⑧ 四项裁决问题已由评审批复：C6 按 smartbox MVP；B1/B2 裁剪（B3 已并入 T8）；tests/remote_details.rs 允许；首窗 100 首策略接受。

---

## ① 目标与范围

打通桌面端「浏览 → 深入」动线，闭合三处已登记缺口：

| 缺口 | 一句话目标 | 来源 |
|---|---|---|
| **D2** | 远端内容卡（发现页/首页推荐歌单卡；后续已购专辑卡）点击后进入**远端详情**（daemon 拉取、只读展示），不再落「未找到」空态 | UI-AUDIT §五 D2、§七覆盖缺口 |
| **C6** | 搜索结果补齐设计稿的**专辑 / 歌手 tab**（数据核实结论：smartbox 本就返回专辑/歌手，缺口纯在桌面消费侧，无需换数据源） | UI-AUDIT §五 C6、§七 |
| **T5** | 排行榜卡封面：daemon 已回 `picurl`，桌面补齐 TopCardData 字段 + UI 映射 + 在线封面管线接线 | UI-AUDIT §五 T5 |

**不在本轮范围**（明确排除）：
- 已购域数据源（QQ 开放接口无此域，已购页维持诚实空态；本轮使 `Route.album` 具备远端参数能力，未来已购卡直接复用）；
- 远端详情页的写操作（收藏歌单/收藏专辑/加入歌单——只读展示，D2 口径）；
- 远端详情翻页 UI（固定首窗，见 §③ 设计决策 D-4）；
- 逐字歌词、评论增强、MV、相似歌手等 singer.rs 其余沉睡接口；
- 换用 `DoSearchForQQMusicMobile` / 移植 `general_search`（见 §⑧ 待裁决 Q1，本计划建议不换）。

### 加分项（可裁剪，独立分节）

| # | 内容 | 依赖 | 成本估计 | 建议 |
|---|---|---|---|---|
| **B1** | 榜单详情页头图（`TopSummary.head_pic_url` 已解析未透传；top-detail 页头加背景图，经 CoverGet） | T9 | 小（协议 1 字段 + 页头布局） | **默认裁剪**：Vue 基准 TopDetailView.vue 亦无头图（仅 PageHeader+表格），DESIGN §3.2 未单列头图；留 AUDIT §20/§23 已知边界不动 |
| **B2** | 歌手简介（`SingerApi::get_desc`，歌单/歌手详情页头部简介折叠区） | T7 | 小（一次出网 + 简介折叠 UI） | 视工期：协议字段本轮预铺（`desc` serde default 空串），UI 不做 |
| **B3** | 榜单卡「N 首」角标（`TopSummaryDto.total_num` 透传 + TopCard 角标） | T9 | 极小 | **并入 T9 核心**：DESIGN §3.2 榜单卡含曲数信息，daemon 一行透传，卡片渲染顺带 |

---

## ② 现状与根因（证据：文件:行号）

### D2 —— 远端卡点击必落空态

数据流断点链（以发现页歌单卡为例）：

1. 卡片导航参数 = 远端 diss 数字 id：
   - `crates/hmp-desktop/src/bridge.rs:136-148` `discover_playlist_card` 把 `DiscoverPlaylist.id`（远端歌单数字 id）直接填进 `CoverCardData.mid`；
   - `crates/hmp-desktop/ui/discover-page.slint:90-92`、`ui/home-page.slint:126-128` `card-clicked(mid) => Nav.navigate(Route.playlist, mid)`。
2. 详情装载只认**本地库**：
   - `crates/hmp-desktop/src/bridge.rs:684-697` `load_detail`：`Route::Playlist` → `param.parse::<i64>()` → `library_view::playlist_detail`（查本地 `playlists` 表）；`Route::Album/Artist` → 按展示名查本地投影；
   - `crates/hmp-desktop/src/library_view.rs:436-446` `playlist_detail_from` 只在 `list_playlists()` 里找行——远端推荐歌单不在库内 → `None`。
3. UI 落诚实空态：
   - `crates/hmp-desktop/ui/playlist-page.slint:21-26`「未找到歌单（可能已删除）」；`ui/album-page.slint:19-24`「本地媒体库中没有这张专辑」；`ui/artist-page.slint:19-21` 同。
4. 根因记录在案：`crates/hmp-desktop/PORTING.md` M4.5——「详情页参数约定：歌单 = DB id；专辑/歌手 = 展示名（远端 mid 要等内容接口）」「QQ-only 落空态」。

已购专辑侧：`ui/purchased-page.slint:101-103` 同样 `Nav.navigate(Route.album, mid)`，但 `bridge.rs:657-661` 恒置空模型（无已购域）——卡片当前不渲染，属「机制就绪、数据源缺席」。

### C6 —— 搜索只有歌曲表

**核实结论（本计划关键新事实）：smartbox 数据源完整含专辑/歌手，断点纯在桌面消费侧。**

- 协议层早已建模三组：`crates/hmp-qqmusic-api/src/protocol/search.rs:46-54` `QuickSearch { songs, albums, singers }`；fixture 实证 `tests/fixtures/search/quick_song.json` 含 `album` 2 条（mid+name+pic+singer）与 `singer` 2 条（mid+name+pic）。
- daemon 已投影：`crates/hmp-daemon/src/content.rs:107-145` `search()` 把 `result.albums/singers` 映射进 `SearchPage.albums/singers`；`crates/hmp-core/src/ipc.rs:454-477` `SearchPage` 三组齐备、随 `Response::Search` 出线。
- 桌面丢弃：`crates/hmp-desktop/src/bridge.rs:1000-1006` 只消费 `page.songs`；`ui/search-page.slint:148-169` 只渲染 TrackTable；文件头（search-page.slint:1-3）「smartbox 窄投影无专辑/时长」的备注被 UI-AUDIT §五 C6 误读为「数据源无专辑/歌手」——实际只是**歌曲行**无专辑/时长列。
- 设计基准：`apps/hmp-tauri/src/views/SearchView.vue:60-166` 三 tab（歌曲/专辑/歌手）+ 每 tab 计数徽章 + 专辑 CoverCard 网格 + 歌手圆形头像卡；DESIGN.md §3.2「大搜索框（自动聚焦）+ 三 tab（歌曲/专辑/歌手）」。

上游换源评估（QQMUSIC_PORTING.md「搜索接口实测记录」）：`DoSearchForQQMusicDesktop` 上游已失效（返回空）；`DoSearchForQQMusicMobile` 需 Android 平台参数（device/qimei 层，§有意不移植）；`general_search` 需登录态且未移植。**结论：smartbox 足以支撑 tab MVP，不换源。**

### T5 —— 排行榜卡封面未渲染

- daemon 已回封面：`crates/hmp-daemon/src/content.rs:443` `picurl: t.front_pic_url` → `hmp_core::TopSummaryDto.picurl`（ipc.rs:538）。
- 桌面三连丢：
  - `crates/hmp-desktop/ui/data.slint:95-103` `TopCardData` 无 cover/picurl 字段；
  - `crates/hmp-desktop/src/bridge.rs:324-346` `spawn_top_category_load` 构造 `TopCardData` 时丢弃 `t.picurl`；
  - `crates/hmp-desktop/ui/top-page.slint:12-88` `TopCard` 纯文本卡（名 + 更新时间 + 预览 3 行），无任何 Image。
- 设计基准：`apps/hmp-tauri/src/views/TopView.vue:38-46` 封面图（4:3、radius-lg、阴影、hover 浮起）+「N 首」角标 + 名 + 更新时间；DESIGN.md §3.2「榜单卡网格（封面 + 名 + 更新时间 + 前 3 预览）」。
- AUDIT §23:784-785 已知边界「歌手照片、榜单头图仍无数据源」——歌手照片随本计划 D2c 落地而闭合；榜单头图留在 B1。

### 封面管线现状（复用面）

- daemon：`content.rs:28,32-55` 域守卫（y.gtimg.cn / qpic.y.qq.com / music-file.y.qq.com，http→https）+ `cover_url_from_pmid`（T002R300x300 模板）；`server.rs:621-669` `CoverGet` 分支已带 v8 盘缓存索引优先 + `rebind_cover_url` 回写。
- 桌面：`src/track_covers.rs`（列表预取，失败出账、回包回调）；`src/online_covers.rs`（发现页歌单卡换图：side-table id→url + CoverGet + `set_row_data` 原地换图，失败出账）；`src/cover_cache.rs`（字节加权 LRU 64MB，256/512 桶，§21 纪律：**禁止新手写 Image 缓存**）。

---

## ③ 设计方案

### 总体架构（不变量）

- **UI 零 HTTP**（PORTING.md/§21）：一切远端封面经 `Request::CoverGet`（daemon 盘缓存）换 `file://` 本地产物，UI 侧只经 `cover_cache` 装载降采样。
- **daemon 统一出网**：新增三个只读详情读接口，形态对齐既有 `DiscoverGet/TopDetailGet`（免登录、`ContentService` 委托、wire 兼容纪律：响应结构体新字段一律 `#[serde(default)]`，ipc.rs:1-6 头部约定）。
- **本地投影不动**：本地歌单（DB id）/专辑/歌手（展示名）装载路径原样保留；远端走参数前缀分流，互不污染。

### 设计决策

**D-1 导航参数前缀 `qq:`（远端实体寻址）**
- `Route.playlist` 参数：本地 = 十进制 DB id（现状）；远端 = `qq:<disstid>`。
- `Route.album` 参数：本地 = 专辑展示名（现状）；远端 = `qq:<albummid>`。
- `Route.artist` 参数：本地 = 歌手展示串（现状）；远端 = `qq:<singermid>`。
- 理由：本地参数空间（数字 id / 展示名）与 `qq:` 前缀天然不冲突；显示名理论上可叫「qq:xxx」属极端构造，可接受（本地查找先行命中即短路，见下）。`qq:` 同时是**曲目行内链接**的远端寻址记号（见 D-5）。
- 分流点：`bridge.rs::load_detail / apply_route`——`param.strip_prefix("qq:")` 命中即走远端装载（异步 + 状态机）；未命中走现有本地同步路径。`schedule_refresh`（LibraryChanged 重放，bridge.rs:493-526）远端参数返回 `DetailData::None` → `apply_detail` no-op（bridge.rs:792），远端页面数据不被库变更误清。

**D-2 远端详情页复用现有三页 + 状态机扩展**
- 不新增 Route、不新增页面文件；`playlist-page/album-page/artist-page` 既有布局（hero 封面 + PageHeader meta + TrackTable / 专辑网格）直接承载远端数据。
- `stores.slint` 三页各加 `playlist-state / album-state / artist-state`（int，**0=初始 / 1=加载中 / 2=就绪 / 3=未找到 / 4=失败**——与 top-page.slint:133-146、search-page.slint:3 的仓内约定对齐）；`*-found` 布尔保留并由本地路径同步维护（页面渲染改读 state）。
- **本地路径同步维护 state（M2）**：`apply_detail` 本地命中分支（bridge.rs:706/729/759）置 state=2；本地 None 分支（bridge.rs:728/758/791）置 state=3。本地/远端交替导航后状态必须正确——防「远端失败 state=4 → 返回本地歌单仍渲染失败文案」。
- 装载函数放**新模块 `crates/hmp-desktop/src/remote_details.rs`**（bridge.rs 已 1300 行，不再膨胀）：每页一个 `spawn_*_load(ui, runtime, param)`（**UI 线程被调，直接 `Data::get(ui).set_*_state(1)`**，O1——勿套 invoke，TestingBackend 下会挂 mock 泵）；`backend::request` 回来后 `invoke_from_event_loop` 落模型；未找到（3，服务端 not-found）与失败（4，离线/网络错误）分态。
- **竞态防护**：三页各一个代数计数器（对齐 `SEARCH_GENERATION` 先例，bridge.rs:976-980）——导航 A→B 时 A 的迟到回包按代数丢弃，防止旧数据覆盖新页（D3 搜索乱序同类问题）。

**D-3 daemon 数据域（content.rs，全部免登录）**
- 复用先例：`reconcile.rs:428-479 fetch_playlist_songs` 的 `CgiGetDiss` 分页消费。三个新读函数按 reconcile 模式做成**自由函数**（`pub async fn remote_playlist_detail(client: &QqMusicClient, id: i64) -> Result<RemotePlaylistDetail, String>` 等），`ContentService` 方法委托——便于 wiremock 集成测试直接注入 mock client（对齐 reconcile_playlist_tracks.rs:243-249 `client_for` 范式）。
- `remote_playlist_detail(id)`：`SonglistApi::get_detail(id, 0, Page::new(1,100), true, false, false)`。`resp.code != 0`（如 -100006 歌单不存在）→ `Err("playlist not found")`；投影 `info.list`（title/picurl/listen_num）+ `info.creator.nick` + `resp.songs`（复用 `project_song`）。
- `remote_album_detail(mid)`：`AlbumApi::get_detail(mid)` + `get_song(mid, Page::new(1,100))`。封面 = `cover_url_from_pmid(basicInfo.pmid)`；年份取 `time_public` 前 4；singers join `" / "`；company/genre/language/desc 透传（无则空串）。
- `remote_artist_detail(mid)`：`SingerApi::get_info(mid)`（Android comm 由 api 层内聚）取 name/singer_pic + `get_songs_list(mid, Page::new(1,50))`（`total_num` 作 song_total）+ `get_album_list(mid, Page::new(1,30))`（`AlbumBrief`→`RemoteAlbumBrief`，pmid→模板 URL）。**歌手照片就来自 get_info 的 `singer_pic`**——§20/§23「歌手照片无数据源」边界随之闭合；URL 域外（非 COVER_HOSTS）时 UI 回退程序化占位。
- 服务端忽略 `number` 的既有坑（QQMUSIC_PORTING live 记录）：has_more/total 一律按响应实际条数与 total 推算，不假设返回 `page.num` 条。

**D-4 首窗策略（不翻页）**：歌单/专辑固定拉前 100 首、歌手热歌 50 / 专辑 30，对齐 top-detail「num=100 固定 + meta 用实际条数」先例（bridge.rs:242-246,271-273）。响应带 `total`/`has_more`，UI meta 诚实标注「共 N 首（展示前 100）」；「播放全部」= 可见列表整表入队（`PlayList`，既有 `play-tracks` 契约，player_bridge.rs:378-421 不动）。翻页 UI 留后续。

**D-5 曲目行远端链接**：`DiscoverNewSong` 增 `album_mid`/`artist_mid` 两字段（serde default 空串），daemon `project_song`（content.rs:623-643）从 `Song.album.mid` / `Song.singer[0].mid` 填充；桌面 `discover_track_row`/远端详情行投影时填 `qq:<mid>` 进 `TrackRow.artist-mid/album-mid`——TrackTable 既有链接（track-table.slint:233-236,255-265）零改动即可跳远端详情，「浏览→深入」闭环从卡片延伸到行内。本地行行为不变（artist-mid=展示名）。

**D-6 C6 tab 结构**：
- 协议：`SearchAlbum`/`SearchSinger` 各增 `pic`（serde default 空串），daemon `search()`（content.rs:127-143）透传 `QuickAlbum.pic`/`QuickSinger.pic`（fixture 实证为 y.gtimg.cn 完整 URL，域守卫兼容）。
- 桌面：`stores.slint` 增 `search-tab`（0=歌曲 1=专辑 2=歌手）、`search-albums: [CoverCardData]`、`search-artists: [ArtistCardData]`（新 struct：`mid/name/cover: image`，圆形头像卡对齐 Vue SearchView.vue:150-162、273-296）。
- `search-page.slint`：tab 胶囊行 + 三内容区互斥；**tab 计数徽章直接绑定模型长度（`Data.search-results.length` / `Data.search-albums.length` / `Data.search-artists.length`，M4——零桥接成本）**；歌曲区现状不动（`search-count` 总数行保留现有语义不改）；专辑区 `CoverCardGrid`；歌手区圆形卡网格（同 CoverCardGrid 的「按容器宽算列数+绝对定位」模式，卡宽固定 104px 级）；当前 tab 空时「该分类下没有相关结果」；总 state=3 判定改为**三组全空**。
- 导航：专辑卡 → `Route.album` + `qq:<mid>`；歌手卡 → `Route.artist` + `qq:<mid>`（即 C6 → D2 的动线衔接）。
- 封面：专辑卡/歌手卡先程序化占位（`cover_image("album:{mid}")` / `("artist:{mid}")`，与 Vue 同 seed），再经 CoverGet 换真图（online_covers 泛化，见 D-7）。

**D-7 封面管线接线（T5 + D2 + C6 共用）**：
- **M1 键规范化**：`online_covers.rs` 增单一辅助函数 `playlist_card_key(cover_mid: &str) -> String`（实现为：无前缀的裸数字 id → `qq:{id}` 归一；已带前缀原样返回）——`register_playlist_covers`（登记侧，bridge.rs 落模型处随卡片 mid 同源构造）与 `apply_playlist_cover`（匹配侧，online_covers.rs:96）**两侧必须同走该函数**，杜绝「卡片 mid 加了前缀、旁路表键没加」的封面静默回归。discover 投影处卡片 mid 与登记键同帧同源派生。
- `online_covers.rs` 泛化：保留现有 discover 歌单路径（键归一经上述函数收口），抽出私有核心 `fetch_cover(ui_weak, runtime, key, url, apply: impl Fn(slint::Image))`（CoverGet + cover_cache 256 桶 + 失败出账），新增：
  - `register_top_covers / refresh_top_covers`：top-groups 嵌套模型（外层 VecModel<TopGroupData> → 内层 tops VecModel）原地换 `TopCardData.cover`；
  - 搜索专辑/歌手卡换图：`refresh_search_covers`（数据落地时模型已在，URL 由 Rust 侧随投影闭包持有，无需 side-table——搜索响应与模型落地同线程同帧，直接在落地循环里 spawn 即可，去重键 `search-album|{mid}|{url}` 同款）；
  - 远端详情 hero 图：`remote_details.rs` 落地时对 `picurl/photo_url` 调同一 `fetch_cover` 核心，回包校验页参数未变（代数守卫）后 set 属性（256 桶，hero 176px 消费）；歌手页专辑网格卡（≤30 张）逐卡同 discover 模式换图。
- 全部走既有 `CoverGet` → daemon 盘缓存（v8 cover_cache 索引）→ `rebind_cover_url`，无任何新直连。

**D-8 T5 卡片形态**：`TopCardData` 增 `cover: image` 与 `total-num: int`（daemon `top_category` 透传 `t.total_num`，TopSummary 已有该字段 top.rs:77-79）；`TopCard` 布局 = 封面（4:3、radius-lg、clip、`Theme.muted` 底、drop-shadow，右下「N 首」角标）+ 名 + 更新时间 + 预览 3 行（**保留现有 preview 消费**，对齐 DESIGN §3.2 的完整描述；Vue 实现无预览属其自身裁剪）。占位 = `cover_image("top:{id}")`；网格容器高度公式随新卡高调整（top-page.slint:160-180）。

### 协议面汇总（hmp-core/src/ipc.rs）

```rust
// Request（枚举尾部、Quit 之前追加）
PlaylistDetailGet { id: i64 },     // 远端歌单详情（免登录，只读）
AlbumDetailGet   { mid: String },  // 远端专辑详情（免登录，只读）
ArtistDetailGet  { mid: String },  // 远端歌手详情（免登录，只读）

// Response（Guess 之后追加）
PlaylistDetail(RemotePlaylistDetail),
AlbumDetail(RemoteAlbumDetail),
ArtistDetail(RemoteArtistDetail),

// DTO（GuessPage 附近追加；全部字段 serde(default)）
RemotePlaylistDetail { id, name, creator, picurl, listen_num,
                       tracks: Vec<DiscoverNewSong>, total, has_more }
RemoteAlbumDetail    { mid, name, singer, singer_mid, company, genre, language,
                       desc, time_public, picurl,
                       tracks: Vec<DiscoverNewSong>, total, has_more }
RemoteArtistDetail   { mid, name, photo_url, song_total, album_total,
                       tracks: Vec<DiscoverNewSong>,
                       albums: Vec<RemoteAlbumBrief>, desc }   // desc 默认空（B2 预铺）
RemoteAlbumBrief     { mid, name, picurl, year_text, track_count }

// 既有结构体扩展（全部 serde(default)，旧 daemon 帧可解码）
DiscoverNewSong += { album_mid: String, artist_mid: String }
SearchAlbum     += { pic: String }
SearchSinger    += { pic: String }
TopSummaryDto   += { total_num: i64 }        // B3：N 首角标
```

帧大小核算：100 曲 RemotePlaylistDetail ≈ 25KB JSON，远低于 `MAX_FRAME` 1MB（ipc.rs:15）。

### daemon server 分发（server.rs，共享风险文件，最小化）

在 `GuessGet` 分支（server.rs:723-747）之后**整段追加三个 match 臂**，形态照抄 `TopDetailGet`（server.rs:707-722）：`handle.content` Some → 调 service 方法 → `Response::*`；None → `Err(Internal, "content service unavailable")`。不触碰该文件任何其他区域。

---

## ④ 任务分解

> 纪律：构建 `cargo check -j 4 -p <crate>`；测试 `cargo test -j 1 -p <crate>` 逐 crate 串行；不设 CARGO_TARGET_DIR、不 cargo clean、不 push。每任务独立可评审提交（实施阶段获准后）。

### T1 协议层（hmp-core）

**触及**：`crates/hmp-core/src/ipc.rs`（Request/Response 枚举 + DTO + 测试）。
**内容**：§③ 协议面汇总全部条目；wire 兼容测试。
**验收**：
- `cargo test -j 1 -p hmp-core` 全绿；
- 新增：三 Request/Response 变体 + 四 DTO 帧往返测试；
- 旧形状容忍测试 ×4：不含 `pic` 的 SearchPage、不含 `album_mid/artist_mid` 的 DiscoverNewSong、不含 `total_num` 的 TopSummaryDto、省缺新字段的三个 Detail DTO → 解码为默认值（对齐 `daemon_state_tolerates_missing_fields_from_older_daemon` 先例，ipc.rs:880-901）。

### T2 daemon 内容服务（content.rs）

**触及**：`crates/hmp-daemon/src/content.rs`（自由函数 + ContentService 委托方法 + 单测）；`crates/hmp-daemon/src/server.rs`（仅三个分发臂，见 §⑦）。
**内容**：§③ D-3 三个自由函数 + 委托方法；`top_category` 补 `total_num` 透传；`search()` 补 `pic` 透传；`project_song` 补 `album_mid/artist_mid`。
**验收**：
- `cargo check -j 4 -p hmp-daemon` 零 error；
- content.rs 纯投影单测：code=-100006 → not-found Err；专辑 pmid → T002 模板 URL；singers join；歌手 photo_url 透传；空 pmid → 空串。
- server.rs 改动 = 且仅 = 三个 match 臂（§⑦ 清单核对）。

### T3 daemon wiremock 集成测试

**触及**：新文件 `crates/hmp-daemon/tests/remote_details.rs`（daemon/tests 归 daemon crate——tests/ 目录不在「src/server.rs」禁碰清单内，且与并行流无交叠；如评审认为 tests/ 目录有共享风险，可降级为 content.rs 内 #[cfg(test)] wiremock，实施时按裁决）。
**内容**：照 `reconcile_playlist_tracks.rs` 范式：`client_for(server.uri())` + musicu 信封 mount。**挂载匹配字段以 api 层源码的请求构造为准照抄（O3）**——`CgiGetDiss` 按 `disstid` 参数匹配（songlist.rs:176-189）、`GetAlbumDetail/GetAlbumSongList` 按 `albumMid`/`begin/num`（album.rs:229-280）、`GetHomepageHeader` 按 `SingerMid`（singer.rs:938-953，Android comm）、歌手歌曲/专辑列表按各自 param JSON，不凭记忆写端点名：
- `CgiGetDiss`：正常页（dirinfo + songlist + total_song_num + hasmore=1 → 100 窗口 + has_more）与 -100006（→ Err）；
- `GetAlbumDetail` + `GetAlbumSongList`：basicInfo/company/singer.singerList/songList[*].songInfo 解析投影；
- `GetHomepageHeader`（Android comm，按 `SingerMid` 参数匹配）+ 歌手歌曲/专辑列表：name/singer_pic/total/albums 投影。
**验收**：`cargo test -j 1 -p hmp-daemon` 全绿（新文件 + 存量回归无破）；**匹配器按 api 层请求构造字段断言**（O3）；Windows 坑位遵守：wiremock + tokio::test 常规形态（无 spawn_blocking 阻塞读、无 XDG 写）。

### T4 桌面远端装载骨架 + 歌单详情（D2a）

**首步（R1 真机探针，实施前置）**：起隔离 daemon（`XDG_DATA_HOME`/`XDG_CACHE_HOME` 临时目录 + `HMP_IPC_ENDPOINT` 隔离，勿碰用户真库/常驻实例），拉 DiscoverGet 取推荐歌单 id，逐个直发 `CgiGetDiss` 核对 id↔disstid 兼容性；失败则改用 `PlaylistSquare`（recommend.rs `get_recommend_songlist` 同源）降级并如实记录（O4）。

**触及**：新 `crates/hmp-desktop/src/remote_details.rs`；`crates/hmp-desktop/src/bridge.rs`（`discover_playlist_card` mid 加 `qq:` 前缀、`load_detail/apply_route` 分流、apply_detail 本地分支维护 state（M2）、`schedule_refresh` no-op 分支）；`crates/hmp-desktop/src/lib.rs`（mod 注册）；`ui/stores.slint`（playlist-state）；`ui/playlist-page.slint`（loading/未找到/失败态 + meta 扩展）；`src/online_covers.rs`（`playlist_card_key` 规范化（M1）+ `fetch_cover` 核心抽取）。
**内容**：D-1/D-2/D-7 骨架 + 歌单装载全链：状态机、TrackRow 投影（含 D-5 `qq:` 行内链接）、meta（共 N 首 + 创建者 + 超窗标注）、hero 封面占位→换图、播放全部。
**验收**：
- `cargo test -j 1 -p hmp-desktop` 全绿（存量 73+ 无回归）；新增单测：param 前缀分流（qq: vs 本地 id/展示名）、远端行投影（album_mid/artist_mid 前缀）、代数守卫迟到丢弃（纯函数抽取）、**「推荐卡 mid 前缀 ↔ 旁路表键一致」（M1）**、**「本地/远端交替导航后状态正确」（M2：远端失败 state=4 → 返回本地歌单 state=2 正常渲染）**。
- TestingBackend 集成测试（O2）：on_navigate A→B + 迟到回包丢弃。
- 离线 headless：`--route playlist --param qq:123` 呈 loading→失败态（不落「未找到」）。
- 真机（获准后）：发现页点任意推荐卡 → 详情出曲表 + 封面 + 播放全部可播；`hmp quit` 后点击 → 失败态文案。

### T5 桌面远端专辑详情（D2b）

**触及**：`remote_details.rs`、`ui/stores.slint`（album-state）、`ui/album-page.slint`（loading/失败态 + meta：年份/公司/语种/共 N 首）。
**验收**：单测（投影：time_public→年份、singer_mid 链接）+ 真机：搜索 tab（T6 后）或任一远端行专辑链接进入 → 曲表 + 封面；本地专辑路径回归（展示名参数照旧）。

### T6 桌面远端歌手详情（D2c，含歌手照片）

**触及**：`remote_details.rs`、`ui/stores.slint`（artist-state）、`ui/artist-page.slint`（loading/失败态 + hero 真照片 + meta 统计 + 专辑网格 `qq:` 导航 + 网格卡封面换图）。
**验收**：单测 + 真机：搜索「周杰伦」→ 歌手 tab → 卡片 → 详情页照片/热歌/专辑网格，网格卡再点进专辑详情（二级深入）。

### T7 C6 搜索三 tab

**触及**：`ui/search-page.slint`（tab 行 + 三内容区，**计数徽章直接绑定模型长度**）、`ui/data.slint`（ArtistCardData）、`ui/stores.slint`（search-tab/search-albums/search-artists）、`src/bridge.rs`（on_search 落三组模型 + state=3 判定改三组全空；`search-count` 保持现有总数语义不动，M4）、`src/online_covers.rs`（搜索卡换图）。
**依赖**：T5/T6（tab 导航的落点）。
**验收**：单测（专辑/歌手卡投影、三组全空判定）；真机：三 tab 切换 + 计数徽章 + 卡片封面 + 点击深入；空关键词/搜索中弱化等既有行为不回归（UI-AUDIT D5 语义）。

### T8 T5 排行榜卡封面

**触及**：`ui/data.slint`（TopCardData + cover/total-num）、`src/bridge.rs`（spawn_top_category_load 投影 + 占位）、`ui/top-page.slint`（TopCard 新布局 + 网格高度公式）、`src/online_covers.rs`（register/refresh_top_covers 嵌套模型换图）。
**验收**：单测（TopCardData 投影含 picurl→占位）；真机：排行榜页全部卡出封面（占位→真图渐进）；headless 离线呈程序化占位卡（布局不塌）；drag/hover 集成测试无回归（top 页无表格虚拟化参与，风险低）。

### T9 收尾

**触及**：`crates/hmp-desktop/PORTING.md`（M4.5 参数约定修订：`qq:` 前缀 + 远端详情装载；如遇新 Slint 坑位随手登记）。
**内容**：全量回归（见 §⑤ 清单）+ 真机走查脚本化清单（§⑤ 末尾）。
**验收**：`cargo test -j 1 -p hmp-core -p hmp-daemon -p hmp-desktop` 全绿；clippy `--all-targets` 零警告（三个 crate）；`cargo fmt --check` 干净；PORTING.md 更新。

### 加分项任务（可裁剪）

- **B1 榜单头图**：`TopDetailPage` 增 head_picurl（serde default）→ top-detail 页头背景图（CoverGet + 256 桶）。默认不做。
- **B2 歌手简介**：`remote_artist_detail` 增调 `get_desc`（多一次出网）填 `desc` → artist 页简介折叠区。默认不做（字段已预铺）。

---

## ⑤ 测试计划

| 层 | 手段 | 覆盖点 |
|---|---|---|
| 协议 | hmp-core 单测（帧往返 + 旧形状容忍） | T1 验收列出的 4+3 例 |
| daemon 纯逻辑 | content.rs 单测 | 投影/错误码映射/URL 模板 |
| daemon 集成 | tests/remote_details.rs wiremock（零真实出网） | 三域 CGI 信封解析 + 分页窗口 + not-found |
| 桌面纯逻辑 | remote_details/bridge 单测 | 前缀分流、投影、代数守卫、三组空判定 |
| 桌面集成 | 存量 drag/hover_slider/hover_stretch/svg_icon/context_menu 全量回归 | 布局/交互无破 |
| 视觉 | `shot-headless`（离线态：loading/失败/占位卡）+ `shot --route ... --param qq:...`（真机宿主） | 远端三页 + 搜索 tab + top 卡 |

**Windows / 生命周期坑位（明示）**：
- wiremock MockServer 在 Windows tokio 下常规可用（reconcile_playlist_tracks.rs 即 Windows 开发机产物）；测试内不写 XDG 目录（新自由函数无盘副作用）。
- hmp-daemon 改动 → BUILD_CODE 指纹变化：真机验证前必须**同时重建 `hmp.exe` 与 `hmp-desktop.exe`**（§22 教训：单包构建是错位事故源），并 `hmp quit` 清陈旧常驻进程。
- 多 worktree 并行：绝不共享/设置 CARGO_TARGET_DIR（§21 幽灵编译错误前科）；测试一律 `-j 1` 逐 crate，禁全仓并发。
- 集成测试若需步进布局（testing backend），沿用 mock_elapsed_time 推进纪律（PORTING.md 坑位）。

**真机走查清单（实施末期）**：登录态下①发现卡→歌单详情→播放全部→行内专辑链接；②搜索「周杰伦」→三 tab→歌手卡→歌手页→专辑网格→专辑页；③排行榜封面渲染 + 换榜残留检查（T6 审计先例）；④断网重试文案；⑤`hmp quit` 后冷启动不复活、远端页失败态。

---

## ⑥ 风险与回滚

| # | 风险 | 缓解 | 回滚 |
|---|---|---|---|
| R1 | **推荐歌单 id ↔ disstid 兼容性未实证**（GetRecommendFeed 的 playlist.id 理论同 tid 空间，但无交叉验证记录） | T4 实施第一步加真机探针（隔离 daemon：XDG + `HMP_IPC_ENDPOINT` 隔离，勿碰用户真库/实例；对推荐卡 id 直发 CgiGetDiss 核对）；若不符：**降级源 `PlaylistSquare`（recommend.rs `get_recommend_songlist`，O4 已确认存在）**或卡片只读降级（仍优于「未找到」） | 单任务粒度回退 |
| R2 | singer `get_info` Android comm 在 daemon client 下异常（api 层已封装，风险低） | T3 wiremock 按 SingerMid 参数匹配验证请求形态；真机走查兜底 | T6 独立回退，照片回退程序化占位 |
| R3 | `qq:` 前缀与展示名碰撞（专辑恰好叫 "qq:…"） | 本地查找先行：`load_detail` 先试本地投影，miss 且有前缀才走远端（顺序保证本地事实优先） | 前缀方案整体可换（仅 bridge 分流点 + 卡片 mid 构造两处） |
| R4 | 迟到回包覆盖新页（快速导航） | 每页代数计数器（D-2），单测钉死 | — |
| R5 | smartbox 各组条目数少（quick-match 端点，fixture：4 歌/2 专辑/2 歌手） | 属数据源上界而非缺陷；tab 徽章如实计数；升级路径见 §⑧ Q1 | tab 独立可回退 |
| R6 | TopCard 布局改动破坏 top 页 drag/hover 回归 | top 页无 TrackTable 虚拟化参与；网格高度公式单测 + headless 截图核对 | T8 独立回退（去掉 cover 字段消费即回纯文本卡） |
| R7 | 并行流冲突（server.rs / ipc.rs） | §⑦ 边界清单 + 合并前 rebase 复核 | 改动均为纯追加，冲突面极小 |
| R8 | 内存回归（新封面消费点） | 全部经 cover_cache 256 桶 + online_covers 去重；禁止新 Image 缓存（§21 纪律）；QA 时观察 RSS 有界 | — |

整体回滚性：所有改动**纯增量**（新 Request/Response 变体、新函数、新模块、页面状态机扩展）；本地路径与既有 UI 行为在无 `qq:` 参数时逐字节不变。极端回滚 = revert 对应任务提交，协议新增变体无人发送即无副作用。

---

## ⑦ 与并行流的边界确认

**共享风险文件改动点清单（精确到锚点）**：

1. `crates/hmp-daemon/src/server.rs` —— **仅**在 `Ok(Request::GuessGet { page })` 分支结束（现文件 747 行 `}` 之后）与 `Ok(Request::QualityGet)`（749 行）之间插入三个 match 臂（`PlaylistDetailGet` / `AlbumDetailGet` / `ArtistDetailGet`），形态复制 TopDetailGet 臂。不触碰 accept 循环、订阅臂、客户端超时等其他区域。
2. `crates/hmp-core/src/ipc.rs` —— 四处纯追加：
   - Request 枚举：`GuessGet` 之后、`Quit` 之前（现 261-267 行区间）追加 3 变体；
   - Response 枚举：`Guess(GuessPage)`（312 行）之后追加 3 变体；
   - DTO 区：`GuessPage`（599 行）之后追加 4 个新结构体；
   - 既有结构体行内加 serde(default) 字段：`DiscoverNewSong`（498-511）、`SearchAlbum`（455-461）、`SearchSinger`（464-469）、`TopSummaryDto`（526-541）。
   预计与「客户端超时区域」零重叠；若并行流也在 ipc.rs 尾部追加变体，合并为纯文本级 append 冲突。
3. `crates/hmp-daemon/tests/` 新文件 `remote_details.rs` —— 新路径无交叠（并行流拥有 engine_tests.rs，位于 src/）。若评审认定 tests/ 需避让，降级方案见 T3。

**独占文件**（无协调需求）：`crates/hmp-qqmusic-api/**`（本轮预计**零改动**——沉睡模块已够用，不移植新接口）、`crates/hmp-daemon/src/content.rs`、`crates/hmp-desktop/**`（含 PORTING.md；禁碰项 player_bridge.rs / now-playing.slint 不触及——远端封面不经当前曲路径）。

**禁碰承诺**：engine.rs、engine_tests.rs、transport.rs、serve.rs、hmp-player/**、hmp-media/**、player_bridge.rs、now-playing.slint、.github/**、README.md、docs/**、AGENTS.md 全部不动。

---

## ⑧ 待协调者裁决的问题（已由评审批复，存档）

> 评审批复（2026-10-23）：Q1 按 smartbox MVP 确认；Q2 B1/B2 裁剪、B3 并入 T8；Q3 tests/remote_details.rs 允许；Q4 首窗 100 首接受。原问题清单存档如下。

1. **C6 数据源决策（建议：不换源）**——核实表明 smartbox 已返回专辑/歌手（协议/fixture/daemon 投影三处在案），缺口纯在桌面丢弃；`DoSearchForQQMusicMobile` 依赖有意不移植的 Android 平台层、`DoSearchForQQMusicDesktop` 上游已失效、`general_search` 需登录且未移植。若产品要求更全的分页搜索结果，`general_search` 移植应单独立项（含登录态 live 复验），不属本流。**请确认按 smartbox 做 tab MVP。**
2. **加分项取舍**——B1 榜单头图：建议裁剪（Vue 基准亦无）；B2 歌手简介：建议裁剪或视 T4-T8 余量；B3「N 首」角标已并入 T8 核心。**请确认。**
3. **T3 测试落点**——`crates/hmp-daemon/tests/remote_details.rs` 新文件是否允许（并行流未见该目录占用；如需绝对避让可改 content.rs 内 cfg(test) wiremock，代价是测试体量受限）。**请确认。**
4. **首窗 100 首策略（D-4）**——远端歌单/专辑不提供翻页，超窗仅展示前 100 + 诚实标注。**请确认可接受。**
