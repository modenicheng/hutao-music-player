# hutao-music-player vs 官方 Windows QQ音乐 API 能力审计报告

## 审计方法与限制（重要说明）

- 官方 Windows 端**不是 Electron 应用**，没有 app.asar。安装目录 `/mnt/c/Program Files (x86)/Tencent/QQMusic/` 是原生 C++ 客户端：QQMusic.exe（主进程）+ QQMusic.dll（13.7MB，UI/业务，部分 VMProtect 混淆）+ **QQMusic_Protocol.dll（协议层，唯一集中持有全部 music.* 模块字符串，144 个去重条目）** + QMNetwork.dll（u.y/c.y/ct.y/stat.y.qq.com 网关）+ WnsSDK.dll（WNS 长连接）。
- 尝试过 npx @electron/asar：无 asar 可解，方案弃用，改用二进制字符串提取（grep -aoE）。
- `musicu.fcg` 字面量不出现在任何 DLL 中（协议层封装在 WNS/私有传输内），但域名 u.y.qq.com 等存在；`music.*` 命名空间与本项目完全同构，可直接复用网关调用。
- 排行榜/发现页：QQMusic_Protocol.dll 中**没有任何 toplist/board 模块字符串**；WebkitCache（235MB）+ Log/qmbrowser 证明客户端内嵌浏览器加载 Web 页面（发现页/排行榜/会员页走内嵌 Web + 传统 bin/*.fcg CGI，如 fcg_vip_login.fcg、fcg_get_profile_homepage.fcg）。
- 中间产物已固化在 /tmp/qqmusic_audit/：modules_all.txt（官方144模块）、context_4domains.txt（模块+参数上下文）、methods_raw.txt（328个 method/回调名）。

## A. 本项目已实现模块清单（crates/hmp-qqmusic-api/src）

| 模块 | method | 所在文件 |
|---|---|---|
| music.search.SearchCgiService | DoSearchForQQMusicDesktop | protocol/cgi.rs |
| music.login.LoginServer | Login | login.rs |
| （VIP登录）VipLogin.VipLoginInter | vip_login_base | login.rs |
| music.vkey.GetVkey | UrlGetVkey | song.rs |
| music.vkey.GetEVkey | CgiGetEVkey | song.rs |
| music.pf_song_detail_svr | get_song_detail_yqq | song.rs |
| music.trackInfo.UniformRuleCtrl | CgiGetTrackInfo | song.rs |
| music.musichallSong.PlayLyricInfo | GetPlayLyricInfo | lyric.rs |
| music.srfDissInfo.DissInfo | CgiGetDiss | songlist.rs, user.rs |
| music.musicasset.PlaylistBaseRead | GetPlaylistByUin | user.rs |
| music.musicasset.PlaylistBaseWrite | AddPlaylist / DelPlaylist | songlist.rs |
| music.musicasset.PlaylistDetailWrite | AddSonglist / DelSonglist | songlist.rs |
| music.musicasset.PlaylistFavRead | CgiGetPlaylistFavInfo | user.rs |
| music.musicasset.PlaylistFavWrite | FavPlaylist / CancelFavPlaylist | user.rs |
| music.musicasset.AlbumFavRead | CgiGetAlbumFavInfo | user.rs |
| music.musicasset.AlbumFavWrite | （收藏/取消专辑） | album.rs |
| music.musichallAlbum.AlbumInfoServer | GetAlbumDetail | album.rs |
| music.musichallAlbum.AlbumSongList | GetAlbumSongList | album.rs |
| music.musichallAlbum.AlbumListServer | GetAlbumList | singer.rs |
| music.musichallSinger.SingerList | GetSingerList / GetSingerListIndex | singer.rs |
| music.musichallSinger.SingerInfoInter | GetSingerDetail / GetSingerSongList / GetSingerMvList | singer.rs |
| music.SimilarSingerSvr | GetSimilarSingerList | singer.rs |
| music.UnifiedHomepage.UnifiedHomepageSrv | GetHomepageHeader / GetHomepageTabDetail | singer.rs, user.rs |
| music.musicToplist.Toplist | GetAll / GetDetail | top.rs |
| music.recommend.RecommendFeed | GetRecommendFeed | recommend.rs |
| music.recommend.TrackRelationServer | GetRadarSong | recommend.rs |
| music.radioProxy.MbTrackRadioSvr | get_radio_track（guess 推荐，require_login） | recommend.rs |
| music.playlist.PlaylistSquare | GetRecommendFeed（广场 tab/列表） | recommend.rs |
| music.globalComment.CommentRead | GetNewCommentList / GetHotCommentList / GetRecCommentList / CmListUIVer | comment.rs |
| music.globalComment.CommentWriteServer | AddComment（另有 DelComment） | comment.rs |
| music.globalComment.CommentCountSrv | GetCmCount | comment.rs |
| 歌单“我喜欢”（dirid=201 经 CgiGetDiss） | like_song / unlike_song、get_fav_song | songlist.rs, user.rs |

注：本项目用 CgiGetDiss(dirid=201) + PlaylistFavWrite 实现“我喜欢”歌曲的读写；favor_system_* 未使用。

## B. 官方 Windows 端使用的模块清单（QQMusic_Protocol.dll，144 条字符串 ≈ 90 个端点）

按功能域分组（**粗体**=本项目未实现）：

- 登录/会话：music.login.LoginServer(.Login/.Logout/.Authorize)、**music.getSession.session.GetSession**、**music.oauthProxy.oauthProxyServer.GetLoginBuffer**
- 播放地址：music.vkey.GetVkey(.CgiGetTempVkey)、music.vkey.GetEVkey(.CgiGetHotVkey)、**music.vkey.GetDownUrl**、**music.vkey.GetEDownUrl(.CgiGetDownUrl/.CgiGetEDownUrl)**
- 歌曲信息：music.trackInfo.UniformRuleCtrl、**music.trackInfo.UniformConfig.GetToneColorInfo**、**music.musichallSong.PlayLyricInfo.GetPlayLyricInfo**（同名method）、**music.musichallSong.AudioEffectInfo.GetAEUserNum**
- 搜索：music.search.SearchCgiService.DoSearchForQQMusicDesktop、**music.smartboxCgi.SmartBoxCgi.GetSmartBoxResultForPc**、**tencent_musicsoso_hotkey.HotkeyService.GetHotkeyForQQMusicPC**
- 歌单：music.srfDissInfo.DissInfoForPc.uniform_get_Dissinfo、music.srfDissInfo.PlExtServer.getPlLmtInfo、music.musicasset.PlaylistBaseRead.GetPlaylistByUin、PlaylistBaseWrite(.AddPlaylist/.DelPlaylist/.EditPlaylist)、**PlaylistDetailRead.GetUniformSongDetailInfo**、PlaylistDetailWrite(.AddSonglist/.DelSonglist/.SeqSonglist)、PlaylistFavRead、**music.musicasset.PlaylistSeqCtrl.SeqPlaylist**
- 专辑：music.musichallAlbum.AlbumSongList.GetAlbumSongList、music.musicasset.AlbumFavRead.CgiGetAlbumFavInfo
- 歌手：**music.recommend.SingerRadioServer.GetSongList**（歌手电台）
- 电台/推荐：music.radioProxy.MbTrackRadioSvr.get_radio_track、music.recommend.TrackRelationServer(.GetRadarSong/.GetSimilarSongs)、**music.recommend.RecommendWidget.GetPCCommonEntryPoint/.SavePCCommonEntryPoint**、**music.prepushHotFile.HotFile.GetSonglist**、**music.platform.QMSongProxy.QMUserPlayTopInfo**
- **最近播放：music.musicasset.PlayRecentlyRead.GetPlayRecentlyInfo、PlayRecentlyWrite.ReportPlayRecentlyInfo/.DeletePlayRecentlyInfo**
- **收藏（红心）：music.favor_system_read.get_favor_list / get_favor_num、music.favor_system_write.do_favor**（DLL 中还出现了带笔误的 `music.music.favor_system_read`）
- MV/视频：**music.musicasset.MVFavRead.getMyFavMV、MVFavWrite.AddDelFavMV、music.stream.MvUrlProxy.GetHotMV、music.video.VideoData、music.VideoCDN.VideoCdnDispatch**
- 评论：music.globalComment.CommentReadServer(.GetHotCommentList/.GetNewCommentList)
- 关注/社交：**music.concern.ConcernSystem.cgi_get_all_concern_list、.cgi_concern_user**
- 播放上报：**music.richFlag.listening.ListeningMusicReport、music.redpower.RecTagger.SearchTagInfo**
- VIP/商业化：**music.vipcgi.VipBannerCgi.QueryVipBanner、VipCenterReddotCgi.QueryVipCenterReddot、AuditionTipsCgi.QueryAuditionTips、music.vip.CentralizedSvr.QueryBrevityMembershipItemList、vip.ExcSoundBenefitSvr(.GetVIPStartButton/.StartTrial)、vip.PlaylistVipRemindSvr.GetPlaylistVipRemind、vip.PayAlertSvr、vipsalecgi.PriceOfferCgi.GetBriefPriceOfferGroups(2)、vip.VIPSubscriptionPromoCgi.ListSubscriptionNotices、vipMusic.RemindViewSvr.GetRemindViewTips、music.lvz.VipIconUiShowSvr**
- 音效：**music.superSound.AudioManager(.GetAudioEffectByTrackId/.GetLMEffectByTrackId)、music.superSound.AudioEffectConfig、qmcpcom 的 HRTFRead/Write、music.mir.AIEffectSvr(.GetEffect/.SetEffect/.GetEntrance)、mir.MirInfoServer、mir.SheetMusicSvr（曲谱）**
- 免费模式/试听：**music.stream.TaskFreeListenServer.GetTaskFreeListenAuth、music.stream.TaskFreeListen.SetFreeMode、music.basicSvr.ResourceDownloadLimitSvr.GetDownloadPermission**
- AI/新功能：**music.aiAssistant.AiAssistantCtrlCenter(.AiAssist/.PollMessageChunk)、music.ai.ai_user_data_svr、music.ai_track_daily_svr、music.modio.ModioSong(.GetSongText/.GetSongTTS/.UpdateSongText)（ AI 歌词/伴唱）、music.lightUGCRoom.*（轻UGC房间）、music.yiflow.Nexus、music.playerStyle.LyricAnimationSvr.GetAnimation**
- 其他基础设施：**music.appconf.PCConfigSvr(.QueryAlertID/.QueryIcon)、music.msgcenter.RedDot.GetRedDots、music.audioCdnDispatch.cdnDispatch、music.shortUrl.sUrl.LongToShort、music.feedback.FeedbackBlack(.AddDislike/.CancelDislike/.GetDislikeList)（黑名单/不感兴趣）、music.gameCenter.MiniGameSvr.*、music.WeiyunMusic.*（微云网盘音频）、music.musichaptic.GetInfoServer.FcgGetTMHInfo（触感反馈）、music.appAccessToken.appAccessTokenServer.WmpfEncode**

## C. 差距清单（官方有、本项目无）——按功能域

### 收藏/我喜欢（红心）★ 最高价值
- `music.favor_system_read.get_favor_list`：获取红心歌曲列表。参数迹象：`fav_type`、`wid`；响应字段迹象：`pic_size`、`vec_userid`、`vec_fav_num`、`fav_num`。需登录。
- `music.favor_system_read.get_favor_num`：红心数量。同上参数。
- `music.favor_system_write.do_favor`：红心/取消红心歌曲。参数迹象：`fav_type`、`wid`、`vec_id`（批量）。需登录。
- 用途推测： favor_system_* 是跨端红心体系（fav_type 区分歌曲/专辑/MV 等对象类型）； 本项目现用 dirid=201“我喜欢”歌单模拟，与官方红心数据源不同步，接入后可与手机端红心互通。
- `music.feedback.FeedbackBlack.GetDislikeList/.AddDislike/.CancelDislike`：不感兴趣/黑名单。需登录。

### 最近播放 ★
- `music.musicasset.PlayRecentlyRead.GetPlayRecentlyInfo`：最近播放列表（CCommonCGI handler `_handler_OnGetPlayRecentlyInfo@CQQMPOthers`）。需登录。
- `music.musicasset.PlayRecentlyWrite.ReportPlayRecentlyInfo` / `.DeletePlayRecentlyInfo`：上报/删除最近播放。需登录。

### 主页推荐/推荐组件
- `music.recommend.RecommendWidget.GetPCCommonEntryPoint` / `.SavePCCommonEntryPoint`：PC 首页推荐入口位配置（取/存卡片入口）。匿名可试，预计带 uin 更丰富。
- `music.platform.QMSongProxy.QMUserPlayTopInfo`：当前播放歌曲的热度/打榜信息（播放页顶部互动条）。匿名。
- `music.prepushHotFile.HotFile.GetSonglist`：预推送热门歌曲列表（启动预热+热门榜数据源）。匿名。
- 本项目已有 RecommendFeed/电台，可补齐上述三个入口级能力。

### 发现页
- 官方**本地无发现页 CGI 模块**：发现页为内嵌浏览器 Web 页（WebkitCache 235MB、Log/qmbrowser、缓存中 bin/fcg_vip_login.fcg、fcg_get_profile_homepage.fcg、fcg_order_singer_getnum.fcg 等传统 CGI）。若本项目要做发现页，直接复用本项目已实现的 RecommendFeed + PlaylistSquare + Toplist 即可，无需新协议。

### 排行榜
- 官方 PC 端二进制内**无任何 music.musicToplist / Toplist / board 字符串** → 排行榜 UI 也走 Web（或移动端共享页面）。本项目实现的 `music.musicToplist.Toplist`（GetAll/GetDetail）是移动端/Web 端同源模块，反而是本项目比官方 PC 端“多”的能力，继续用即可。

### 歌手域
- `music.recommend.SingerRadioServer.GetSongList`：歌手电台。参数迹象：`secretUin`、`singerId`（响应迹象 `singerList`）。匿名可调（secretUin 可空）。

### 歌曲播放增强
- `music.vkey.GetDownUrl`/`GetEDownUrl`（.CgiGetDownUrl/.CgiGetEDownUrl/.CgiGetTempVkey/.CgiGetHotVkey）：下载专用/加密 vkey 与临时/热门 vkey。需登录（下载）。本项目仅有播放用 GetVkey/GetEVkey。
- `music.trackInfo.UniformConfig.GetToneColorInfo`：音色信息。匿名。
- `music.musichallSong.AudioEffectInfo.GetAEUserNum`：音效使用人数。匿名。
- `music.superSound.AudioManager.GetAudioEffectByTrackId/.GetLMEffectByTrackId` + `music.superSound.AudioEffectConfig`：按曲目取音效配置（银河音效）。匿名读，写需 VIP。
- `music.playerStyle.LyricAnimationSvr.GetAnimation`：歌词动画。匿名。
- `music.modio.ModioSong.GetSongText/.GetSongTTS/.UpdateSongText`：AI 伴唱歌词文本/TTS。部分需登录。
- `music.musichaptic.GetInfoServer.FcgGetTMHInfo`：触感反馈配置。匿名。

### 歌单补强
- `music.musicasset.PlaylistBaseWrite.EditPlaylist`：编辑歌单信息（改名/改简介/标签）。需登录。
- `music.musicasset.PlaylistDetailWrite.SeqSonglist`：歌单内歌曲排序。需登录。
- `music.musicasset.PlaylistSeqCtrl.SeqPlaylist`：歌单本身排序（我的音乐列表顺序）。需登录。
- `music.musicasset.PlaylistDetailRead.GetUniformSongDetailInfo`（参数迹象 `bPaged`）：歌单内歌曲统一详情（含试听/VIP状态）。匿名/带登录更全。
- `music.srfDissInfo.PlExtServer.getPlLmtInfo`：歌单限制信息（审核 censor_status 等）。匿名。

### MV/视频域（整域缺失）
- `music.musicasset.MVFavRead.getMyFavMV` / `MVFavWrite.AddDelFavMV`：收藏 MV。需登录。
- `music.stream.MvUrlProxy.GetHotMV`、`music.video.VideoData`、`music.VideoCDN.VideoCdnDispatch`：热门MV/视频数据/视频CDN调度。匿名。

### 社交域（整域缺失）
- `music.concern.ConcernSystem.cgi_get_all_concern_list` / `.cgi_concern_user`：关注列表/关注用户。需登录。参数/响应迹象：`concern_status`、`friend_status`、`iconurl`、`fans_num`、`is_myself`。

### 播放上报
- `music.richFlag.listening.ListeningMusicReport`：听歌上报（richFlag 状态栏联动）。需登录。
- `music.redpower.RecTagger.SearchTagInfo`：歌曲打榜标签。匿名。

### VIP/商业化域（整域缺失，均为读接口，多匿名可调）
- `music.vipcgi.VipBannerCgi.QueryVipBanner`、`VipCenterReddotCgi.QueryVipCenterReddot`、`AuditionTipsCgi.QueryAuditionTips`（试听提示）
- `music.vip.CentralizedSvr.QueryBrevityMembershipItemList`、`vip.ExcSoundBenefitSvr.GetVIPStartButton/.StartTrial`、`vip.PlaylistVipRemindSvr.GetPlaylistVipRemind`、`vipsalecgi.PriceOfferCgi.GetBriefPriceOfferGroups(2)`、`vip.VIPSubscriptionPromoCgi.ListSubscriptionNotices`、`music.lvz.VipIconUiShowSvr`
- `music.stream.TaskFreeListenServer.GetTaskFreeListenAuth`、`music.stream.TaskFreeListen.SetFreeMode`（免费听歌模式）

### 搜索补强
- `music.smartboxCgi.SmartBoxCgi.GetSmartBoxResultForPc`：搜索框联想（PC版）。匿名。
- `tencent_musicsoso_hotkey.HotkeyService.GetHotkeyForQQMusicPC`：热搜词。响应迹象：`ret_code`、`vec_hotkey`。匿名。

### AI/新形态域（整域缺失，优先级低）
- `music.aiAssistant.AiAssistantCtrlCenter.AiAssist/.PollMessageChunk`（AI助手）、`music.ai.ai_user_data_svr`、`music.ai_track_daily_svr`、`music.lightUGCRoom.*`（轻UGC听歌房间）、`music.yiflow.Nexus`、`music.mir.SheetMusicSvr`（曲谱）、`music.WeiyunMusic.*`（微云备份）、`music.gameCenter.MiniGameSvr.*`

### 会话/基础设施
- `music.getSession.session.GetSession`：会话保活。需登录态。
- `music.oauthProxy.oauthProxyServer.GetLoginBuffer`：OAuth 登录缓冲（第三方授权）。登录流程中。
- `music.appconf.PCConfigSvr.QueryAlertID/.QueryIcon`、`music.msgcenter.RedDot.GetRedDots`、`music.audioCdnDispatch.cdnDispatch`、`music.shortUrl.sUrl.LongToShort`、`music.basicSvr.ResourceDownloadLimitSvr.GetDownloadPermission`

## D. 四域详细接口情报

### D1. 收藏（红心歌曲）
官方端点（QQMusic_Protocol.dll 字符串相邻性证据，模块与 method 及参数为同一段 .rdata）：
```
模块: music.favor_system_read
  method: get_favor_list
  method: get_favor_num
模块: music.favor_system_write
  method: do_favor
请求参数字段: fav_type, wid, vec_id
响应字段迹象: pic_size, vec_userid, vec_fav_num, fav_num
```
- 推测请求形状：
  - get_favor_list: `{"module":"music.favor_system_read","method":"get_favor_list","param":{"fav_type":1,"wid":<uin>}}`（fav_type=1 疑为歌曲红心；vec_userid/vec_fav_num 指示响应为数组+计数结构）
  - get_favor_num: `{"param":{"fav_type":1,"wid":<uin>}}`
  - do_favor: `{"module":"music.favor_system_write","method":"do_favor","param":{"fav_type":1,"wid":<uin>,"vec_id":["0039MnYb0qxYhV"]}}`（vec_id 为批量歌曲 mid/ids）
- 登录需求：wid 表明必须登录（uin）。
- 接入建议：在 user.rs 增加 `get_fav_songs(fav_type)` 与 `do_favor(vec_id, is_favor)`；响应提取路径推测 `$.data.vec_userid[*]` / `$.data.fav_num`（需实测校准）。注意官方 DLL 有笔误模块名 `music.music.favor_system_read`，以无笔误版本为准。

### D2. 主页推荐
官方端点：
```
模块: music.recommend.RecommendWidget
  method: GetPCCommonEntryPoint   # 读取 PC 首页推荐入口位/卡片
  method: SavePCCommonEntryPoint  # 保存用户对入口位的自定义
模块: music.prepushHotFile.HotFile
  method: GetSonglist             # 预推送热门歌曲列表（热门推荐数据源）
模块: music.platform.QMSongProxy
  method: QMUserPlayTopInfo       # 播放页歌曲热度/互动信息
```
- GetPCCommonEntryPoint 预计 param 为空或 `{}`，响应为入口卡片配置数组（直接 JSON 渲染到首页）。
- GetSonglist 预计 `{"param":{}}` 或带 `{"tag_id":...}`；响应疑为 `$.data.songlist[*]`（与 toplist 响应结构同族）。
- 本项目可直接补充这三个只读接口；Save 类需登录。

### D3. 发现页
- 结论：官方 PC 发现页 = 内嵌 Web 页面，非 music.* 原生协议。证据：Protocol DLL 无 discover/homepage feed 模块（仅有 RecommendWidget 入口位配置）；WebkitCache 235MB 且含 bin/fcg_vip_login.fcg、fcg_get_profile_homepage.fcg、fcg_musiclist_getinfo_cp.fcg、fcg_list_songinfo_cp.fcg、fcg_order_singer_getnum.fcg 等经典 Web CGI；Log 下有 qmbrowser 目录。
- 接入建议：本项目“发现页”用已有能力拼装：RecommendFeed（个性化流）+ PlaylistSquare（歌单广场 tab）+ Toplist.GetAll（榜单入口）+ SingerRadio（歌手电台）。无需新增协议。

### D4. 排行榜
- 结论：官方 PC 端二进制无 toplist 模块（grep toplist/kingboard/boardid 均 0 命中），榜单 UI 走 Web。**本项目 top.rs 的 `music.musicToplist.Toplist`（GetAll/GetDetail）是可用且比官方 PC 端更直接的数据源**（移动端同源模块）。
- 官方端与榜单相关的本地能力：`music.prepushHotFile.HotFile.GetSonglist`（热门歌曲预热列表，可作“热歌”兜底）与 `music.platform.QMSongProxy.QMUserPlayTopInfo`（单曲在榜信息：播放页“打榜”条）。
- 若需完全对齐官方 PC 观感，可内嵌 Web 榜单页（y.qq.com 榜单页）；若要原生榜单，继续用 Toplist 模块即可。

## 汇总优先级建议
1. 收藏域 favor_system_read/write（跨端红心，体验核心）——需登录
2. 最近播放 PlayRecentlyRead/Write（三件套）——需登录
3. 歌单补强：EditPlaylist / SeqSonglist / SeqPlaylist / PlaylistDetailRead.GetUniformSongDetailInfo ——需登录
4. 搜索补强：SmartBox（联想）+ Hotkey（热搜）——匿名即可，成本低
5. GetSimilarSongs（相关歌曲，官方有本项目无）——匿名
6. SingerRadioServer.GetSongList（歌手电台）——匿名
7. MV/关注/VIP 提示/AI 域——按产品需要后排
