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
| `qqmusic_api/modules/song.py` | `song.rs` | ✅ 已移植 | 详情/批量查询/播放 URL（取流）；凭证解耦 |
| `qqmusic_api/modules/lyric.py` | `lyric.rs` | ✅ 已移植 | 歌词（自动 QRC 解密） |
| `qqmusic_api/models/base.py` | `models.rs` | ✅ 已移植 | Song/Singer/Album/File/Pay/MV + SongList |
| `qqmusic_api/modules/songlist.py` | `songlist.rs` | ✅ 已移植 | 歌单详情（免登录）+ 创建/删除/加歌/收藏（需登录） |
| `qqmusic_api/modules/album.py` | `album.rs` | ✅ 已移植 | 专辑详情/歌曲/新碟（免登录）+ 收藏/取消收藏（需登录） |
| `qqmusic_api/modules/singer.py` | `singer.rs` | ✅ 已移植 | 歌手列表/索引/主页(Android)/Tab/歌曲/专辑/MV/相似/简介 |
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
- `GetSingerDetail` 传 `ex_singer`/`group_singer` 等扩展参数时 10006，最小参数（`singer_mids` + `pic`）可用；
- 歌手歌曲/专辑接口服务端可能忽略 `number` 参数（请求 5 返回 30）；
- `GetRecommendFeed` 免登录返回的 `cover`/`creator` 全为 null（提取逻辑由合成测试覆盖）。

## 尚未移植

- 登录（QQ 二维码 / 微信扫码 / 微信换取登录态）—— 阶段 B
- 播放 URL 获取 —— 阶段 C
- 歌词 —— 阶段 C
- 歌单 / 专辑 / 歌手 / 每日推荐 —— 阶段 D
- 微信扫码登录 / 手机客户端扫码（MQTT）—— 微信需 open.weixin.qq.com 页面解析，手机端依赖 MQTT
- 短信验证码登录（`PhoneLoginSession`）—— 待移植
- 加密音质取流（`GetEVkey`/`CgiGetEVkey`）—— 待移植
- MV 播放地址（`modules/mv.py`）—— 待移植
- 写操作（收藏、歌单管理）—— 阶段 E
- Android 平台会话（`ensure_session`/QIMEI/设备指纹）—— HMP 目标为 Linux 桌面，暂不移植

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
| **凭证模型** | client 持有全局 `credential`，方法可选覆盖 | **无全局凭证状态** | 请求级传入；仅显式 `refresh_credential`；调用方管理多凭证 |

## 设计决策记录

### 凭证解耦（2026-08-06，docs/PROJECT.md §6.4）

- 客户端不持有全局凭证，无自动轮换/定时刷新；
- 需要登录态的请求由调用方传入 `Option<&Credential>`；
- 刷新仅通过显式接口 `refresh_credential(&Credential) -> Credential`（阶段 B 实现）；
- 调用方负责 keyring 存储与过期判断，客户端返回业务错误码供调用方决策。

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
