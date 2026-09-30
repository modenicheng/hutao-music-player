# QQMusicApi 移植跟踪文档（QQMUSIC_PORTING.md）

> 本文档逐模块记录 Python 参考实现与 Rust 移植的对应关系、已移植/未移植接口、
> fixture、已知差异与 Live 测试结果（docs/PROJECT.md §24）。
> 移植过程中任何新增/删除的模块映射都必须在此登记。

## 参考实现

| 项目 | 值 |
| --- | --- |
| 仓库 | https://github.com/L-1124/QQMusicApi |
| 固定 commit | `108617ffe80abefec6358717b9f4d3677550db10`（2026 主线 `main`） |
| 许可证 | GPL-3.0-or-later（与 HMP 一致） |
| 语言/依赖 | Python 3.10+；niquests、pydantic、orjson、anyio、cryptography |

> 参考源通过 `scripts/fetch-python-ref.sh` 按需拉取到 `.deps/qqmusic-api-python/`
> （已被 .gitignore 排除），不通过 submodule 引入，避免双重提交/子模块指针/CI 成本
> （决策记录见 docs/PROJECT.md §5 讨论）。

## 模块映射

| Python 源文件 | Rust 目标模块 | 状态 | 备注 |
| --- | --- | --- | --- |
| `qqmusic_api/core/request.py` | `protocol/cgi.rs` | ✅ 已移植 | CgiRequest 描述符、错误码映射、批量信封解包 |
| `qqmusic_api/core/api_context.py` | `protocol/comm.rs` | ✅ 已移植 | comm 构造、Cookie 注入、UA |
| `qqmusic_api/core/versioning.py` | `protocol/comm.rs` | ✅ 已移植 | Platform、VersionProfile、g_tk |
| `qqmusic_api/algorithms/sign.py` | `protocol/sign.rs` | ✅ 已移植 | hash33、zzc_sign |
| `qqmusic_api/core/client.py` | `client.rs` | 🔶 部分 | musicu 请求入口 + HTTP 请求（登录用）；无全局凭证状态、无 Android 会话/限流 |
| `qqmusic_api/core/exceptions.py` | `error.rs` | ✅ 已移植 | 错误分类（§12 适配），含登录域错误 |
| `qqmusic_api/models/request.py` | `credential.rs` | ✅ 已移植 | Credential（脱敏 Debug，无全局持有，含登录响应解析） |
| `qqmusic_api/modules/login.py` | `login.rs` | 🔶 部分 | QQ 扫码完整链路 + refresh/check_expired/logout；微信/手机扫码待移植 |
| `qqmusic_api/modules/login_utils.py` | `login.rs` | 🔶 部分 | PollInterval + wait_qrcode_login（轮询/去重/退避/取消）；无 PhoneLoginSession |
| `qqmusic_api/modules/song.py` | `song.rs` | ✅ 已移植 | 详情/批量查询/播放 URL + 加密取流（GetEVkey）；凭证解耦 |
| `qqmusic_api/modules/lyric.py` | `lyric.rs` | ✅ 已移植 | 歌词（自动 QRC 解密） |
| `qqmusic_api/models/base.py` | `models.rs` | ✅ 已移植 | Song/Singer/Album/File/Pay/MV + SongList |
| `qqmusic_api/modules/songlist.py` | `songlist.rs` | ✅ 已移植 | 歌单详情（免登录）+ 创建/删除/加歌/收藏（需登录） |
| `qqmusic_api/modules/album.py` | `album.rs` | ✅ 已移植 | 专辑详情/歌曲/新碟（免登录）+ 收藏/取消收藏（需登录） |
| `qqmusic_api/modules/singer.py` | `singer.rs` | ✅ 已移植 | 歌手列表/索引/主页(Android)/Tab/歌曲/专辑/MV/相似/简介 |
| `qqmusic_api/modules/user.py` | `user.rs` | ✅ 已移植 | 主页头部/VIP/自建与收藏歌单/收藏专辑/我喜欢（凭证解耦；`get_homepage` 额外携带 `NodeToken`，见[已知差异]） |
| `qqmusic_api/modules/top.py` | `top.rs` | ✅ 已移植 | 排行榜分类/详情 |
| `qqmusic_api/modules/recommend.py` | `recommend.rs` | ✅ 已移植 | 首页 Feed/雷达/推荐歌单/新歌（免登录）；猜你喜欢（需登录） |
| `qqmusic_api/algorithms/__init__.py` | `algorithms/qrc.rs` | ✅ 已移植 | qrc_decrypt（3DES + zlib） |
| `qqmusic_api/algorithms/tripledes.py` | `algorithms/tripledes.rs` | ✅ 已移植 | 自定义 3DES 变体（PC-2 偏移） |
| `qqmusic_api/modules/song.py` | （待移植） | ⬜ 未移植 | 阶段 C |
| `qqmusic_api/modules/lyric.py` | （待移植） | ⬜ 未移植 | 阶段 C |
| `qqmusic_api/modules/songlist.py` | （待移植） | ⬜ 未移植 | 阶段 D |
| `qqmusic_api/utils/device.py` | （待移植） | ⬜ 未移植 | 仅 Android 平台需要 |
| `qqmusic_api/utils/qimei.py` | （待移植） | ⬜ 未移植 | 仅 Android 平台需要 |
| `qqmusic_api/utils/mqtt.py` | — | ⬜ 不移植 | HMP 非目标功能 |
| `qqmusic_api/core/pagination.py` | `pagination.rs` | 🔶 部分 | 策略层（PagerStrategy/AsyncPager）未移植；折叠为 `Page`/`PagedView`/`Paged` 统一分页原语 + 三条 has_more 归一规则（2026-09-29 重设计，见[统一分页重设计]） |
| （无上游对应；独立实现） | `algorithms/qmc2` | ✅ 已移植 | QMC2 解密：TEA-CBC、ekey 派生（EncV1/EncV2）、map/RC4 流密码、STag/QTag 尾部检测 |
| （无上游对应；独立实现） | `crates/hmp-media` | ✅ 已移植 | 加密流下载→解密→XDG 缓存→file URI（CLI/桌面共用；含 proxy：回环 Range 解密代理） |

## 已移植接口

### 基础请求层（阶段 A，docs/PROJECT.md §6.6）

- [x] `hash33(s, h=0)` → `crates/hmp-qqmusic-api::protocol::sign::hash33`
- [x] `zzc_sign(payload)` → `protocol::sign::zzc_sign`
- [x] `VersionPolicy.get_g_tk(credential)` → `protocol::comm::g_tk`
- [x] `Platform`（ANDROID/DESKTOP/WEB）→ `protocol::comm::Platform`
- [x] `VersionPolicy.build_comm(...)`（WEB 平台）→ `protocol::comm::build_web_comm`
- [x] `ApiContext.build_api_kwargs(...)` → `client::QqMusicClient::musicu_request`
- [x] `Client._unwrap_cgi_batch(...)` → `protocol::cgi::unwrap_cgi_batch`
- [x] `CgiRequest._parse_response(...)` → `protocol::cgi::CgiRequest::parse_response`
- [x] Cookie 注入（`uin`/`qqmusic_uin`/`qm_keyst`/`qqmusic_key`）
- [x] 日志脱敏（Cookie、music key 不落日志）

### 登录（阶段 B，docs/PROJECT.md §6.5）

- [x] `LoginApi.get_qrcode(QQ)` → `login::LoginApi::get_qrcode`（ptqrshow → Set-Cookie qrsig + PNG）
- [x] `LoginApi.check_qrcode` → `login::LoginApi::check_qrcode`（ptqrlogin → ptuiCB 解析 → 事件）
- [x] `LoginApi._authorize_qq_qr` → `login::LoginApi::authorize_qq_qr`（check_sig → p_skey →
      oauth authorize → code → QQLogin CGI）
- [x] `LoginApi.refresh_credential` → `login::LoginApi::refresh_credential`（Login CGI，
      按 login_type 分支，错误包装 CredentialRefresh）
- [x] `LoginApi.check_expired` → `login::LoginApi::check_expired`（profile homepage fcg）
- [x] `LoginApi.logout` → `login::LoginApi::logout`（Logout CGI）
- [x] `QRCodeLoginSession.wait_qrcode_login` → `login::LoginApi::wait_qrcode_login`
      （轮询/去重/指数退避/超时/取消 CancellationToken）
- [x] `QR`/`QRCodeLoginEvents`/`QRLoginResult`/`PollInterval` → `login` 模块同名类型
- [ ] `LoginApi.get_qrcode(WX/MOBILE)` + `check_qrcode` 对应分支 —— 待移植
- [ ] `PhoneLoginSession`（短信验证码登录）—— 待移植

### 歌曲与歌词（阶段 C，docs/PROJECT.md §6.6）

- [x] `SongApi.get_detail` → `song::SongApi::get_detail`（`music.pf_song_detail_svr`，Web 平台）
- [x] `SongApi.query_song` → `song::SongApi::query_song`（`CgiGetTrackInfo` 批量）
- [x] `SongApi.get_song_urls` → `song::SongApi::get_song_urls`（`UrlGetVkey` 取流，含 guid/filename 拼接）
- [x] `GetSongUrlsResponse.build_urls` → `song::GetSongUrlsResponse::build_urls`（sip + purl 拼接）
- [x] `SongFileType`/`SpecialSongFileType` → `song::SongFileType` 常量
- [x] `LyricApi.get_lyric` → `lyric::LyricApi::get_lyric`（含 QRC 自动解密）
- [x] `qrc_decrypt` → `algorithms::qrc_decrypt`（自定义 3DES-ECB + zlib）
- [x] `tripledes.py` → `algorithms::tripledes`（含 PC-2 偏移 Bug 的自定义变体）
- [x] `Song`/`Singer`/`Album`/`File`/`Pay`/`MV` → `models` 模块

### 取流实测记录（2026-08-06）

- 免登录：`RS02`（试听）返回 `purl`+`vkey`；`M500`/`C400` 等完整音质返回 `104003`（无权限，需登录态）；
- 完整音质需调用方传入 `credential`（`str_musicid` 注入 `uin` 参数）。

### 加密取流（阶段 D 补充，2026-08-06）

- [x] `EncryptedSongFileType` → `song::SongFileType` 合并：`is_encrypted=true` 的常量
  （`FLAC`=`F0M0.mflac`、`MASTER`=`AIM0.mflac`、`VINYL`=`V0M0.mflac`、`OGG_*`=`O8M*/O6M0/O4M0.mgg`、
  `ATMOS_*`=`Q0M0/Q0M1/Q0M3/D0M4`、`DTS_X`=`DTM3.mmp4`、`NAC`=`TLM1.mnac`）；
- [x] `get_song_urls` 按 `file_type.is_encrypted` 自动切换 `music.vkey.GetEVkey`/`CgiGetEVkey`；
- [x] 响应 `ekey` 字段已解析（供播放器解密加密文件）；
- 实测：免登录 FLAC 取流返回 `101404`（需登录）/`104003`（无权限）；VIP 登录后调用
  `get_song_urls(&[info], SongFileType::FLAC, Some(&credential))` 取流；
- 明文高音质变体（`F000`/`AI00` 等）服务端已停发，故不提供（上游保留但不可用）；
- 上游普通组高音质常量（`MASTER`/`FLAC`/`OGG_*` 等）与加密组同名，Rust 合并为单一
  `SongFileType`（高音质统一为加密版本）；
- **待播放器阶段**：`.mflac`/`.mgg` 解密播放（上游仓库无解密算法，需社区方案如 unlock-music）。
- **实测记录（2026-08-08）**：`CgiGetEVkey` 返回 `ekey` 后解密播放链路已接线（Task 3/4），QMC2 解密播放完整闭环已验证。加密流播放链路：CLI/桌面 → 本地回环解密代理（http://127.0.0.1:随机端口）→ Range 按需解密 → Rodio 流式播放。

### 歌单/专辑/歌手/排行榜/推荐（阶段 D，docs/PROJECT.md §6.6）

- [x] `SonglistApi.get_detail` → `songlist::SonglistApi::get_detail`（`CgiGetDiss`）
- [x] `SonglistApi.create/delete/add_songs/del_songs/like_song/unlike_song` → 同签名（需登录，凭证解耦）
- [x] `AlbumApi.get_detail/get_song/get_new_album` → `album::AlbumApi`（`GetAlbumDetail`/`GetAlbumSongList`/`get_new_album_info`）
- [x] `AlbumApi.fav_album/del_fav_album` → 需登录
- [x] `SingerApi` 全部 9 接口 → `singer::SingerApi`；`get_info`/`get_tab_detail` 用 Android comm（ct=11/cv=14090008）
- [x] `TopApi.get_category/get_detail` → `top::TopApi`（`GetAll`/`GetDetail`）
- [x] `RecommendApi.get_home_feed/get_radar_recommend/get_recommend_songlist/get_recommend_newsong` → `recommend::RecommendApi`
- [x] `RecommendApi.get_guess_recommend` → 需登录（免登录实测 1000）

### 阶段 D 实测记录（2026-08-06）

- 歌单/专辑/歌手/榜单/推荐分类全部免登录可用；「猜你喜欢」需登录态；
- `GetSingerDetail`（歌手简介）布尔参数必须以 0/1 整数编码，JSON `true` 返回 10006（上游直接传 Python bool 属上游缺陷，移植已修正）；
- ~~`GetSingerDetail` 传 `ex_singer`/`group_singer` 等扩展参数时 10006，最小参数（`singer_mids` + `pic`）可用~~
  （2026-09-29 修订：0/1 整数编码下**全开参数亦可用**，当时的 10006 实为 JSON bool 编码所致）；
- 歌手歌曲/专辑接口服务端可能忽略 `number` 参数（请求 5 返回 30）；
- `GetRecommendFeed` 免登录返回的 `cover`/`creator` 全为 null（提取逻辑由合成测试覆盖；
  2026-09-29 复验：封面/创建者位于 `List[*].Playlist.basic` 下，提取层级已修复，不再恒空）。

### 用户库实测记录（2026-08-10）

- `GetHomepageHeader`（用户主页）**必须携带 `NodeToken` 参数**（值任意，官方网页端传
  `Date.now().toString()` 毫秒时间戳字符串）；缺省时服务端返回业务错误码 10000 且 `data` 为空壳。
  上游 Python 库缺省该参数（属上游缺陷），Rust 移植已在 `get_homepage` 补上（见[已知差异]）；
- 该 CGI 另存在按 IP 的滚动限流：短时间大量请求（约 20+ 次）后无论参数是否正确均返回 10000，
  静置一段时间后恢复（2026-08-10 实测）；
- 歌手主页（`SingerMid` 路径）不受 `NodeToken` 影响，免登录可用；
- `get_vip_info`/`get_created_songlist`/`get_fav_song`/`get_fav_songlist`/`get_fav_album`/
  `GetProfileReport`（音乐基因）均可用，`GetProfileReport` 可作主页昵称/头像的备用来源。

### 全量二次核验（2026-09-29，逐 API 现场核验 + 修复 + 回归）

> 四域并行现场核验全部 pub API（约 115 次真机请求，间隔 ≥1.1s，未触发限流）；
> 新增离线回归 65 例，`cargo test -p hmp-qqmusic-api` 231 例全绿。逐域记录：

- **通用结论**：写操作/会话级响应的判定字段（`result`/`v_failedPlaylistId`/`SubCode`/`AddedCmId` 等）
  均在**内层 `data`**，不在 musicu 子响应顶层；serde 区分大小写，服务端 CamelCase 键必须显式
  alias，且同一服务家族内大小写可能不一致（`AddComment.SubCode` vs `DelComment.Subcode`）；
- **用户/歌单域**：`GetPlaylistByUin` 必须数字 uin（加密 uin → 80030）；`PlaylistFavRead` 参数键
  `uin`、`AlbumFavRead` 参数键 `euin`（均传加密 uin），两者列表键均为 `v_list`（原别名缺失致解析
  0 条，已修复）；`FavPlaylist` 不能收藏自建歌单与 201 目录（result=80184）；写闭环（建临时歌单 →
  加/删歌 → 收藏/取消 → 删除）全通，删除后 dirid 可被服务端复用，唯一标识用 tid；`CgiGetDiss`
  对不存在歌单返回 data.code=-100006；`AddSonglist`/`DelSonglist` 的 CGI 80092 按上游语义映射
  false（补 allow_error_codes）；like_song 对部分歌曲返回 80105（语义未明）；
- **评论域**：`AddComment` 新评论 ID 键为内层 `data.AddedCmId`（原读顶层 commentId 恒空）；
  `DelComment` 判定键为内层 `data.Subcode`（原读顶层 `SubCode` 恒 false）；评论发表→删除闭环
  现场验证可用（创建即删）；
- **推荐域**：`PlaylistSquare/GetRecommendFeed` 封面/创建者在 `Playlist.basic` 下（提取层级修复）；
  「猜你喜欢」需登录确认（免登录 1000）；
- **歌曲/歌词/歌手域**：`get_song_detail_yqq` 的公司/流派/简介/语言/发布时间在
  `data.info.<字段>.content`（需内层提取，原解析恒空，已修复）；试听 `TRY` 免登录取流 Range 206
  可读；**取流文件名必须用 `media_mid`**（doubled song-mid 服务端照常签发 vkey 但 CDN 404）；
  加密 `FLAC` VIP 登录返回 ekey 且可流式读取；`qrc=true` 解密结果为 QRC XML（非 LRC）；
  `GetSingerDetail` 0/1 编码下全开参数可用（修订 2026-08-06 记录）；歌手 9 接口全部免登录可用；
- **登录/凭证域**：`ptqrshow` 免登录可用（PNG + qrsig）；`ptqrlogin` 携带伪造 qrsig 返回
  **HTTP 403**（无效二维码以 HTTP 错误而非 ptuiCB 事件表达）；`wait_qrcode_login` 取消语义修复
  （取消可中断睡眠与进行中轮询）；`refresh_credential` 的 `musicid` 须按数值下发、loginType 须
  还原至 `comm.tmeLoginType`、内层 data.code 需二次校验；`Credential` serde 补齐服务端形状
  （`musicid`/`musickey`/`loginType` int 形状、必填字段 default）与 keyring 形状双向往返；
  `refresh_credential`/`logout`/`authorize_qq_qr` 为会话级写操作仅做离线验证；
- **用户主页服务端异常**：`GetHomepageHeader` 自 2026-09-29 观测起对合法参数整体返回 10000
  空壳（加密/数字 uin、有无 NodeToken 一致；参数校验仍在跑，数字型 NodeToken 触发 10006；上游
  Python 同受影响）。昵称/头像改用 `GetProfileReport`（新增 `UserApi::get_music_gene`，上游
  `get_music_gene` 补齐移植），CLI/daemon 数据源已切换、homepage 保留为兜底。

### 统一分页重设计（2026-09-29，crate 层 Page/PagedView + 签名迁移 + live 复验）

> 15+ 分页接口原为松散 `(page: i64, num: i64)` 参数、has_more 形态各异（`hasmore: i64`/
> total 推算/缺失）；统一为 `src/pagination.rs` 的 `Page`（1 基页号 + 页大小，构造钳制
> page≥1、num 1..=100）+ `Paged::paged(page) -> PagedView`（items 借用切片/total/
> has_more/`next_page()`）。各接口由 Page 派生 wire 参数（`sin`/`begin`/`song_begin`/
> `offset`/`start`/`From` = `page.offset()`；`num`/`song_num`/`size`/`count`/`Size`/
> `PageSize` = `page.num`；评论 `PageNum = page-1`）。has_more 三条归一规则：①显式
> `hasmore`/`HasMore` 以服务端为准（收藏歌单/收藏专辑/歌单详情/评论/推荐歌单）；②无显式
> 字段但有总数（歌手列表/歌曲/专辑/MV/新碟/榜单详情）按 `total > offset + len` 推算；
> ③两者皆缺按 `items.len() == page.num` 保守推算（`PagedView::conservative`，当前无此
> 形态的响应类型，保留为公开 helper）。迁移与 wire 派生由 `tests/regression_pagination.rs`
> 离线锚定（20 例）；daemon/cli/desktop 调用点仅做签名适配。

- **live page=2 复验**（`examples/live_pagination.rs`，7 次请求，间隔 ≥1.1s）：
  - `singer.get_songs_list`（周杰伦 mid=0025NhlN2yWrP4）：请求 `number=5` 服务端返回
    **30 条**（确认忽略 number，2026-08-06 记录复现）；**`begin` 偏移被服务端尊重**——
    page=2/num=5 首条（青花瓷）== page=1 第 6 条，无跳变；按实际返回条数推进
    （page=2/num=30）与首页无重叠；`total_num=1012` → total 推算 has_more 成立。
    **结论：该族接口（含 get_album_list/get_mv_list）翻页必须按 `items.len()` 推进，
    不能假设返回 `page.num` 条**；
  - `top.get_detail`（飙升榜 62）：`totalNum=100`；page=2（offset=5）与 page=1 无重叠，
    total 推算 has_more 正确；
  - `user.get_fav_songlist`（登录态）：total=12，page=1（10 条）`hasmore=1`、page=2
    （2 条）`hasmore=0`——显式判定与实际剩余条数一致，服务端字段可靠；
  - 歌手索引（`get_singer_list_index`）维持 2026-09-29 记录：服务端固定每页 80 条并忽略
    页大小，`Page::num` 仅参与 `sin` 偏移计算，续页推进以 `items.len()` 为准（与上游
    `MultiFieldContinuationStrategy` 的 `sin + len >= total` 终止语义一致）。

## 尚未移植（2026-09-29 全量扫描重写）

> 对上游 `modules/`、`models/`、`utils/`、`core/`、`algorithms/` 全量清点后重写本节；
> 此前版本所列「登录/播放 URL/歌词/歌单/专辑/歌手/推荐/写操作」均已随阶段 B-E 完成，已移除。

### 缺口：上游有、Rust 未移植

**整个模块未移植：**

| 上游模块 | 内容 | 规模 | HMP 取舍 |
| --- | --- | --- | --- |
| `modules/search.py` | `get_hotkey`（热搜词）/ `complete`（搜索联想）/ `general_search` / `search_by_type`（+`SearchType` 10 值枚举） | 4 方法 + 模型 | **建议移植（P1）**：hotkey/complete 免登录可用性高；`general_search` 2026-08-06 实测「需登录态」——当时无登录态，现已有，需登录复验；移植后 daemon 歌词兜底与搜索页受益。需 `utils/common.get_searchID` |
| `modules/mv.py` | `get_detail` / `get_mv_urls` / `get_mv_list` | 3 方法 | 播放地址维持「用户明确暂不需要」；`get_detail`/`get_mv_list` 元数据随需要再议 |
| `modules/private_message.py` | 私信域（会话/消息/发送/删除/配置等 15 方法） | 15 方法 | 建议登记**不移植**（HMP 非目标功能；此前未在本文档登记，补录） |
| `modules/helper.py` + `helper_utils.py` | 云盘上传（`InitUpload`/`FinishUpload` + COS 分片 `UploadFileSession`） | 2 方法 + 会话类 | 建议登记**不移植**（HMP 无上传需求；补录） |

**已移植模块内的方法缺口：**

| 上游模块 | 缺失方法 | 备注 |
| --- | --- | --- |
| `song.py`（Rust 3/13） | `get_cdn_dispatch` / `get_similar_song` / `get_labels` / `get_related_songlist` / `get_related_mv` / `get_other_version` / `get_producer` / `get_sheet` / `has_sheet` / `get_fav_num` | 相似歌/相关歌单/相关 MV/其他版本对发现页/相关推荐 UI 有直接价值（P1）；其余随需要 |
| `user.py`（Rust 9/13） | `get_follow_singers` / `get_fans` / `get_friend` / `get_follow_user` / `get_fav_mv` / `get_dislike_list` | 关注/粉丝/好友域，均需登录态；Rust 另有上游没有的 `fav_songlist`/`unfav_songlist`（本地增强） |
| `lyric.py`（Rust 1/5） | `get_singing_annotations_info`（助唱标注）/ `get_multi_style_trans_lyric`（多风格翻译）/ `is_ai_dict_exists` / `get_ai_dict` | 桌面端逐字/翻译歌词增强时需要；`singingAnnotationsTs` 字段同样未含 |
| `comment.py`（Rust 6/7） | `get_moment_comments`（Moment/动态评论，`SongTsComment`） | — |
| `login.py`（LoginApi 8 方法 Rust 有 6）+ `login_utils.py` | 微信扫码（`QRLoginType::Wechat` 分支）、手机客户端扫码（`Mobile` 分支 + `checking_mobile_qrcode`）、`PhoneLoginSession`（`send_authcode`/`phone_authorize` 短信登录）、`iter_events` 事件流 API 形态 | 手机扫码依赖 MQTT（已决策不移植）；微信需 open.weixin.qq.com 页面解析；`iter_events` 为 API 形态差异（Rust 仅阻塞式 `wait_qrcode_login`，`PollInterval` 逻辑已内含） |

**类型/常量缺口：**

- `song.rs` `SongFileType` 缺 `SpecialSongFileType` 尾部 12 个常量：`MULTI`(O601)/`PIANO`(AI01)/`BAYIN`(AI02)/`GUZHENG`(AI03)/`QUDI`(AI04)/`HULUSI`(AI05)/`SUONA`(AI06)/`SHOUDIE`(AI07)/`GUITAR`(AI08)/`DRUMS`(AI09)/`KAZOO`(A200)/`THERAPY`(AA01)（AI 演奏/疗愈音色组，纯增量，随取流需要补）；
- `RingSongFileType` 整组缺（`RING_128`(R500)/`RING_96`(R400)/`RING_48`(R200) 彩铃类型）；
- 伴随模型缺口：`models/search.py`（Hotkey/Complete/GeneralSearch 响应）、`models/mv.py`、`models/private_message.py`、`models/user.py` 的 relation/dislike/fav_mv 类型。

### 有意不移植（决策记录）

- **Android 平台会话**：`utils/device.py`（设备指纹）/ `utils/qimei.py`（QIMEI）/ `utils/mqtt.py` —— HMP 目标为 Linux/Windows 桌面，WEB 平台覆盖目标场景；上游 `search.py` 的 `DoSearchForQQMusicMobile`、`login.py` 的手机扫码均依赖此层；
- **分页策略层**：`core/pagination.py`（Offset/Page/MultiFieldContinuation 策略）—— 策略层不移植；语义折叠为 `src/pagination.rs` 的三条 has_more 归一规则（显式字段 / total 推算 / 满页保守推算），窗口统一为 `Page`（见[统一分页重设计]）；
- **上游普通组明文高音质常量**（`F000`/`AI00`/`Q000` 等）—— 服务端已停发，Rust 高音质统一为加密版本；
- **`MV 播放地址**（`get_mv_urls`）—— 用户明确暂不需要。

## Fixture

### 目录约定

- `crates/hmp-qqmusic-api/tests/fixtures/`：随 crate 发布的解析测试 fixture（离线、CI 默认运行）
- `fixtures/qqmusic/`：仓库级差分测试原始录制（Python/Rust 对比，本地运行）

### 现有 fixture

| 文件 | 来源 | 用途 |
| --- | --- | --- |
| `tests/fixtures/search/quick_song.json` | Live 录制（免登录 smartbox） | quick_search 解析测试 |
| `tests/fixtures/song/detail_by_id.json` | Live 录制（song_id=186016） | get_detail 解析测试 |
| `tests/fixtures/song/urls_try.json` | Live 录制（RS02 试听） | get_song_urls 解析测试 |
| `tests/fixtures/lyric/encrypted.json` | Live 录制（crypt=1） | QRC 解密 + get_lyric 解析测试 |

### 搜索接口实测记录（2026-08-06）

- `music.search.SearchCgiService/DoSearchForQQMusicDesktop`：返回空列表（旧方法，已失效）；
- `music.search.SearchCgiService/DoSearchForQQMusicMobile`：需 Android 平台参数，WEB comm 下歌曲为空；
- `music.adaptor.SearchAdaptor/do_search_v2`（general_search）：需登录态，免登录下无歌曲数据；
- `c.y.qq.com/splcloud/fcgi-bin/smartbox_new.fcg`（quick_search）：**免登录可用**，返回歌曲/专辑/歌手/MV，
  阶段 A 采用此入口。

## 已知差异

| 项 | Python 参考 | Rust 移植 | 说明 |
| --- | --- | --- | --- |
| HTTP 客户端 | niquests（multiplexed、令牌桶限流） | reqwest | 限流由 HMP 应用层控制 |
| Android 平台 | 完整支持（QIMEI/设备会话） | 不移植 | HMP 面向 Linux 桌面 |
| 响应模型 | pydantic BaseModel | serde（DTO 起步允许 `serde_json::Value`） | 稳定后逐步强类型化 |
| 布尔参数 | `bool_to_int` 自动转换 | 显式 int 转换 | 保持可读性 |
| **`get_homepage` 参数** | `{"uin": euin, "IsQueryTabDetail": 1}` | **额外携带 `NodeToken`**（当前毫秒时间戳字符串） | 上游缺省该参数时服务端返回 10000 空壳（2026-08-10 实测）；官方网页端 share/profile_v2 亦发送 `NodeToken: Date.now().toString()`。2026-09-29 起该 CGI 对合法参数整体返回 10000 空壳（服务端异常，上游同受影响），取昵称/头像改用 `get_music_gene`（`GetProfileReport`） |
| **凭证模型** | client 持有全局 `credential`，方法可选覆盖 | **无全局凭证状态** | 请求级传入；仅显式 `refresh_credential`；调用方管理多凭证 |

## 设计决策记录

### 凭证解耦（2026-08-06，docs/PROJECT.md §6.4）

- 客户端不持有全局凭证，无自动轮换/定时刷新；
- 需要登录态的请求由调用方传入 `Option<&Credential>`；
- 刷新仅通过显式接口 `refresh_credential(&Credential) -> Credential`（阶段 B 实现）；
- 调用方负责 keyring 存储与过期判断，客户端返回业务错误码供调用方决策。

### 统一分页原语（2026-09-29，`src/pagination.rs`）

- `Page { page, num }`（1 基页号 + 页大小，构造钳制 1..=100）：全部分页窗口
  接口以 `Page` 为单一入参，各 CGI 自行派生 wire 参数（`song_begin`/`sin`/
  `offset`/`PageNum` 等）；`Page::offset() = (page-1)*num`；
- `PagedView<'a, T>`（`items`/`total`/`has_more`/`page` + `next_page()`）经
  `Paged::paged(page)` trait 统一访问；13 个分页响应类型已实现；
- **has_more 三条归一规则**（实现处 docstring 必须注明）：① 服务端显式
  `hasmore`/`HasMore` 字段的以服务端为准（收藏歌单/专辑/歌单详情/评论/
  推荐歌单）；② 无显式字段但有总数的按 `total > offset + len` 推算（歌手
  列表/专辑/榜单）；③ 两者皆缺按 `len == num` 保守推算；
- 服务端分页坑（实测）：`GetSingerListIndex` 固定 80/页忽略 num；歌手歌曲/
  专辑/榜单列表服务端可能忽略 `number`（请求 5 返回 30）——**翻页推进以
  返回条数为准，不能假设返回 `page.num` 条**；
- 评论三读接口（热评/新评/推荐评）改为返回完整 `CommentListResponse` 信封
  （总数 + 显式 `HasMore`），上游对应行为；调用方经 `paged()` 取视图；
- 消费链路：daemon IPC `CommentList` 携带 `page`/`num`（serde default 兼容
  旧帧），`CommentPage` 增加 `has_more`/`page`；CLI 全局 `--json` 直接序列化
  分页 DTO 供 agent 程序化翻页。

### 进程内随机访问解密源（2026-09-30，替换回环 HTTP 代理）

- **动机**：2026-08-08 为 GStreamer（souphttpsrc 只认 HTTP Range）引入的
  `127.0.0.1` 回环解密代理，在 gst 弃用（08-24 换 rodio）后纯属同进程内的
  TCP/reqwest/stream-download 临时文件自我开销，且是系统代理劫持事故面
  （2026-09-29/30 两次：`cdn_client` 与 `LOOPBACK_CLIENT` 被迫 `no_proxy`）。
  QMC2 密钥流按绝对偏移寻址，天然随机访问——直接实现
  `hmp_core::MediaStreamSource`（`len()` + `open() -> Read+Seek reader`），
  `LoadRequest.stream` 优先于 uri，加密/明文统一（明文 = IdentityCipher）。
- **reader 协议坑**（`hmp-media/src/stream/reader.rs`）：生产者（async 任务）
  被区间请求头阶段的 epoch 变化"取消"时必须回到主循环重判（`continue 'main`），
  绝不能当作退出——否则消费者在新偏移 condvar 死锁，且只在 seek 恰落窗口间
  才偶发；完全消费的 chunk 不能立即出队（回看保留窗口），统一
  `end + retain <= pos` 修剪；生产者绝不跨 `.await` 持 std MutexGuard；
  Rust 2024 `gen` 是保留字（用 epoch）；`future::select` 双方需 Unpin
  （`Box::pin`），输家原样归还是"假唤醒不产生重复请求"的关键。
- **rodio 0.21 DecoderBuilder 要求 `R: Read + Seek + Send + Sync`**，而
  `Box<dyn MediaStream>` 只有 Send → hmp-player 侧 `SyncStream(Mutex<...>)`
  适配器桥接 Sync（无 unsafe）；decoder 构建仍必须在 `spawn_blocking`
  （2026-09-29 LIFO 死锁纪律不变，仅措辞更新）。
- **tee 边播边缓存**（取代解析即后台全量回填）：武装时机在 `open()` 而非
  `prepare_media`（同源多次 open 防双写同一 tmp）；chunk offset < high-water
  （探测期回读）跳过不 detach，> high-water（前向 seek 跳洞）永久 detach；
  drop 后后台补齐 `[high-water, len)` 单区间再转正（收尾含驱逐扫描，也在
  后台做，不卡解码线程）；键空间不变（path|ekey），二次播放
  `cached_playable_uri` 命中语义不变。**语义变化**：G2 预解析但未播放的
  曲目不再回填缓存。
- **明文 + 无 Range CDN 的回退**：内嵌 ekey 提取失败后转 `fill_plain_at`
  明文全量落缓存（先内嵌后明文双探测；明文侧多一次全量下载，仅此罕见
  回退路径发生）。
- **RC4 段密钥流缓存**（`qmc2/cipher.rs`）：`QmcRc4Cipher` 内
  `Mutex<Option<(seg_id, [u8;0x1400])>>` 容量 1 段；等价性依据"丢弃
  seg_key+in_seg 步后取 N 字节 ≡ 丢弃 seg_key 步生成整段后取 [in_seg..]"；
  `encode_other_segment` 的单段不变量（`in_seg + len <= 0x1400`）由 decrypt
  四段式结构保证（有 debug_assert 钉住），勿引入绕过分段的调用路径。
- **测试基建坑**：`#[tokio::test]`（current_thread）下消费者阻塞读会饿死同
  runtime 的生产者 → 阻塞读一律 `spawn_blocking`；tokio runtime drop 会等
  永久阻塞的 spawn_blocking 线程 → 测试用源必须在 drop 时解除阻塞；XDG
  隔离锁 panic 毒化用 `unwrap_or_else(PoisonError::into_inner)`；本 crate
  （hmp-qqmusic-api）生产代码 deny unwrap/expect，锁中毒走恢复不 panic；
  wiremock 判别尾部探测时小于 0x40 的文件须用 `end == total-1` 判据。

## Live 测试

> 需要真实账号与网络，默认忽略；Live 测试不使用个人 Cookie 提交公共 CI。

```bash
cargo test --features live-tests -- --ignored
```

环境变量：`HMP_QQMUSIC_COOKIE`、`HMP_LIVE_TEST_TRACK_ID`（不写入仓库）。

## 上游变化记录

| 日期 | commit | 变化 | Rust 侧影响 |
| --- | --- | --- | --- |
| 2026-08-06 | `108617f` | 基线（首次移植） | — |
| 2026-08-10 | 上游未修复 | `GetHomepageHeader`（用户主页）服务端要求 `NodeToken` 参数，上游缺省 → 返回 10000 空壳 | `user::UserApi::get_homepage` 补 `NodeToken`（毫秒时间戳字符串）；新增回归测试 |
| 2026-09-29 | 上游未修复 | `GetHomepageHeader` 对合法参数整体返回 10000 空壳（服务端异常）；`GetProfileReport` 可用 | 新增 `user::UserApi::get_music_gene`（上游 `get_music_gene` 补齐移植）；CLI/daemon 昵称数据源切换，homepage 保留兜底 |
| 2026-09-29 | 移植修复 7+ 项 | 全量二次核验：收藏歌单/专辑 `v_list` 别名、fav 写判定内层 `data`、评论 `AddedCmId`/`Subcode`、推荐歌单 `Playlist.basic` 封面层级、歌曲详情 `info.<字段>.content` 内层提取、`wait_qrcode_login` 取消语义、`Credential` 服务端形状解析（int `musicid`/int `loginType`/必填 default）、`refresh_credential` musicid 数值下发 + 内层 code 校验 | 各域 `tests/regression_*.rs` 共 65 例离线回归锚定（见[全量二次核验]） |
| 2026-09-29 | 移植重构（分页重设计） | 新增 `src/pagination.rs`：`Page`/`PagedView`/`Paged` 统一分页原语 + has_more 三条归一规则；15 个分页接口 `(page, num)` 松散参数 → `Page`（songlist `get_detail` 的 num/page 双参数归一为 Page 派生 `song_begin`/`song_num`） | daemon/cli/desktop 调用点仅签名适配；离线回归 `tests/regression_pagination.rs`（20 例）+ live 探针 `examples/live_pagination.rs`（见[统一分页重设计]） |
