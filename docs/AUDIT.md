# 后端审计报告（滚动更新；2026-09-03 始于 Linux 侧，§18 起含 Windows 专题）

> 全程自主推进的会话记录。决策原则：**明确 bug 直接修**；需要产品/语义决策的只记录不动手。
> 基线：`5ffe34f`（工作树含上次会话遗留的 QR/NodeToken 成品改动）。

## 0. 基线状态

- `cargo test --workspace`：全绿（约 508 个测试）。
- `cargo clippy -D warnings`：**HEAD 即失败** —— `hmp-smtc` 的 `CoverSource`/`classify_cover_source`
  只被 `#[cfg(windows)]` 模块消费，Linux 构建下是死代码。
- 未提交改动 5 文件：QR 原生分辨率渲染 + `get_homepage` 补 `NodeToken`（含回归测试与移植文档），
  验证通过后单独提交（`5d70b60`）。

## 1. 本轮提交

| commit | 内容 |
|---|---|
| `5d70b60` | 上次会话遗留：NodeToken 修复 + QR 原生分辨率渲染 |
| `2921aaa` | smtc Windows-only 死代码门控 + e2e 文档 lint |
| `ada9551` | 全部用户可见提示英文化（含 i18n 后的 RG 残留修复与回归测试） |
| `9abf660` | media 层 7 项 bug 修复 |
| `5c68862` | storage 层 9 项 bug 修复 |
| `642fe7f` | 大文件测试拆分 + 去重 + 文档对齐 |

## 2. 已修复 bug

### 引擎（hmp-daemon/engine.rs）
- **RG 残留**（与 c7082fc 同类）：`QueueClear --all` / `QueueRemove` 清到空两条 Idle 路径
  不清 `current_rg_db`/`rg_factor` → status 显示已消失曲目的增益，且此后 SetVolume 被旧增益污染。
  收敛到 `enter_idle()`，含回归测试。
- `QueueRemove` 用 `snapshot()`（O(n) 克隆）做单点判断 → `current_idx()`。

### 播放驱动（hmp-player + hmp-media）
- **解密缓存永不命中**（高危）：cache_key 哈希完整 URL，而 QQ `purl` 的 `guid`/`vkey`
  每次请求都变 → 每次播放都新增一个 30–100MB 孤儿文件。改为只哈希 URL path + ekey。
- **缓存容量驱逐从未被调用**：`evict_if_needed` 是死代码，`HMP_DECRYPT_CACHE_MIB` 从未生效；
  pid 后缀的 `.tmp` 残留永不清扫。写缓存成功后调用驱逐 + 清扫陈旧 tmp。
- **m4a 永远解密失败**：ISO-BMFF 的 `ftyp` 在 offset 4（前 4 字节是 box size），
  魔数检查只认 offset 0。
- **`bytes=-N` 后缀区间返回 416**（RFC 7233 违例）：mp4 demuxer 探测尾部 moov 常用，已支持。
- **最终缓存文件非原子写**：崩溃留下「头 8 字节合法」的截断文件，缓存命中检查只看魔数
  → 永久命中坏文件。改为 `.part` + rename。
- **proxy accept 循环一遇错就退出**：瞬时错误（ECONNABORTED/EMFILE）让 listener 永久消失，
  表现为播放中所有取流 connection refused。改为退避重试，连续 64 次失败才放弃。
- **CDN 请求无任何超时**：reqwest 默认无超时，连接被黑洞时取流永久挂起并占死
  Semaphore 许可。统一 `cdn_client()`（connect 10s + read 30s）。
- 请求头解析要求 `": "`：静默丢弃合法的 `Range:bytes=0-99`（RFC 7230 允许无空格）。

### 存储（hmp-storage + daemon local.rs + CLI scan）
- **无 `busy_timeout`**（高危）：文档声称 CLI/daemon 跨进程 WAL 并发，但写锁互斥且
  默认 0 等待 → daemon 写库瞬间 `hmp scan` 等直接 "database is locked" 失败。设 5s。
- **缺索引**（O(N²) 扫描）：`local_files.fingerprint`（每文件/每监听事件全表扫）与
  `playlist_tracks.playlist_id`（每行 COUNT(*) 全扫）→ 迁移 v4。
- **指纹复用偷行**：`cp -p` 的两份相同文件（同 mtime）在扫描时来回「偷」同一行；
  且目标 tracks 行已存在时 `UPDATE source_key` 撞 UNIQUE 中止整轮扫描。加守卫回退全新插入。
- **空标题入库**：无标签文件 `title: ""` 覆盖文件名回退 → 列表空白行。空串视为缺失。
- **MP3 码率单位错**：lofty 返回 kbps，阈值 `>= 300_000` 恒假 → 320k 全被标成 `Mp3_128`。
- **配置文件损坏被静默覆写**：parse 失败 → 默认值 → 下次 save() 整体抹掉用户配置
  （含 replaygain 开关）。load 时 warn；save 前把解析失败的旧文件备份为 `config.toml.bak`。
- **凭证文件非原子写**：崩溃留截断 JSON，且所有调用方 `.ok().flatten()` 静默登出。
  tmp+rename（0600）+ 损坏时 warn 并按未登录处理。
- **封面缓存竞态**：并发写同一 hash 交错留下半张封面且被 `exists()` 去重永久缓存。tmp+rename。
- **CLI 扫描单文件失败中止整轮**：`?` 传播跳过 `finish_scan`（missing 标记失效）。跳过并计数。
- **播放路径插入的 local_files 行不复位 missing**：`ON CONFLICT(path)` 补 `missing = 0`
  与 `track_id = excluded.track_id`。

### 其他
- `should_refresh` 依赖服务端中文文案判超时：已加英文容错（`timeout`/`timed out`）；
  长期建议按错误码判定（见 §4）。
- **洗牌种子固定**：`QueueCore::new` 的 XorShift 种子是常量 → 每次重启对同一歌单生成
  完全相同的「随机」顺序。改为时间播种（确定性测试走 `set_seed`）。

## 3. 卫生 / 重构

- `engine.rs` 3593→1081 行、`db.rs` 3009→2046 行：测试模块经 `#[path]` 拆到
  `engine_tests.rs` / `db_tests.rs`（模块树不变，`use super::*` 语义不变）。
- hmp-cli 两处重复的 Howard Hinnant 民用历法实现收敛到 `timefmt.rs`。
- 清掉空根目录 `src/`、过期注释（"与 CLI play.rs 一致，复制"）。

## 4. 已知问题（未修，需决策/低优先）

1. **QMC1/V1 footer 误报**（media/decrypt）：尾 4 字节 LE 恰落 1..=1024 时被当成 V1
   密钥区，静默截掉最后 `4+key_size` 字节音频（约 1/400k 概率但按文件确定）。
   修法需验证提取的 key 能解出已知魔数，涉及流式与两条下载路径，建议单独立项。
2. **`list_favorites` INNER JOIN**：reconcile-only 的 QQ 收藏（未播放/未缓存过）不显示
   ——首次登录后「QQ 上几百首收藏，列表只有十几首」的观感。需要决定：LEFT JOIN +
   占位标题，或 reconcile 时补建 tracks 行。
   **2026-10-02 大幅缓解**：`reconcile_playlist_tracks`（§17）为歌单曲目建了 tracks 行，
   收藏曲目随之获得真实元数据；彻底关闭仍需 LEFT JOIN 决策。
   **2026-11-25 复核（§30.1 / §30.4）**：「我喜欢」不全、红心全灭的现行主因是
   `reconcile_remove_absent` 的分片 `NOT IN`（远端快照 >500 条即全清），不是 INNER JOIN——
   真库当前没有「有关系无曲目」的孤儿行；INNER JOIN 议题独立，仍未决策。
3. **播放路径创建的 local_files 行是二等行**（无 scan_root/generation）：daemon 关机期间
   删除的文件永不标记 missing，直到下次扫描。
   **2026-11-28 已修根内文件**（§31）：`add_local_file` 归入所在扫描根并带当前代际，
   `finish_scan` 的 missing 收尾从此覆盖播放行；扫描根之外的播放行仍无根可归
   （没有 watcher 覆盖，属预期）。
4. ~~**`delete_playlists_absent` FK 地雷**：subscribed 歌单一旦有曲目缓存就会 FK 失败~~
   **已拆除**（2026-10-02）：歌单曲目缓存随 `reconcile_playlist_tracks` 落地（§17），
   `purge_tracks_of_absent_playlists` 在删除歌单前按同一谓词清理子行（含 subscribed），
   wiremock 集成测试覆盖远端取消收藏整行删除不炸 FK。
5. ~~proxy 多区间 Range 返回 416；`stream_range_body` 对空流会挂起~~
   **已随回环代理整体删除而失效**（2026-09-30 进程内随机访问解密源取代，载体不存在）。
6. ~~decrypt 全文件读取用 `std::fs::read` 在 async 上下文~~ **已失效**
   （流式改造后无 async 上下文全文件读取路径）。
7. **顶层短命令别名**：已补齐（main.rs 有 Pause/Resume/Next/Prev/Stop/Seek/Volume
   顶层别名；quality/loop/shuffle/history 维持子命令，与 USAGE 一致）。
8. 桌面端（hmp-desktop）UI 文案仍中文：独立产品面，未动。
9. 登录超时判定按消息文案（已双语容错）；QQ 服务端改协议时可考虑按 code 判定。

## 5. 功能扩展机会（只列不实现，待决策）

已移植但**无任何前端入口**的 API 模块（hmp-qqmusic-api 是独立发布 crate，移植成果
沉睡在库里）：

| 模块 | 能力 | 建议入口 |
|---|---|---|
| `lyric.rs` | ~~歌词~~ **已落地**（ed1dad3：CLI `hmp lyric` + daemon 三级缓存 + 播放页接入） | — |
| `top.rs` | ~~排行榜分类/详情~~ **已落地**（ff8050e：`hmp top` + 排行榜页） | — |
| `singer.rs` | 歌手主页/歌曲/专辑/MV/相似 | `hmp artist <mid>` 浏览 |
| `recommend.rs` | ~~首页 Feed/雷达/推荐歌单/新歌/猜你喜欢~~ **已落地**（ff8050e：`hmp discover`/`hmp guess` + 发现页） | — |
| `album.rs` | 专辑详情/新碟（播放路径已用） | `hmp album <mid>` 查看曲目列表 |

其他方向（按价值排序）：

1. **MPRIS Seeked 信号与 playlist 桥接完善**、GNOME AppIndicator 兼容验证；
2. **系统桌面通知**（换曲/错误，Linux notify-rust / Windows toast）；
3. **定时停止 / sleep timer**（daemon 层小改动）；
4. **gapless 播放**：preload 目前只消解 resolve 延迟，未做解码器无缝拼接；
5. **音频输出设备选择**（config `[audio]` 已有骨架，CPAL 枚举设备）；
6. **本地库去重工具**（同指纹多路径聚合展示）；
7. **HTTP 遥控 API**（局域网遥控，socket 协议已有，套 HTTP 壳即可）；
8. **Scrobble**（last.fm/libre.fm）。

## 6. 过度设计审视

总体判断：**核心复杂度都有事故背书，不算过度**。具体：

- **事务式换曲**（save_state/restore_state + 装载成功才提交）：代码量可观，但
  P0（空队列洗牌越界 panic）/P1（误确认旧曲）回归测试证明必要。保留。
- **PreloadSlot 双键防乱序**：写槽 `(revision, gen)` 字典序 + 消费只看 gen+id，
  注释解释了为何消费不含 revision（否则永不命中）。已由测试覆盖。保留。
- **SessionMirror 脏检查 + 节流写盘**：位置 tick 100ms 一次，无脏检查会疯狂写盘。合理。
- **QueueCore 规范顺序 + 播放顺序双数组**：shuffle 下 Previous 回到「真正刚播过的」
  的唯一干净实现。合理。
- **XorShift 自实现**：避免 rand 依赖，20 行。合理（本轮已补时间播种）。
- 真正可质疑的一处：**FakeDriver/FakeResolver 测试脚手架**（engine_tests.rs 约 400 行
  fake 代码）体量超过部分被测逻辑，但换来 97 个引擎测试，性价比可接受，不拆。
- `hmp-storage` 引入 `tracing` 依赖（本轮为配置/凭证损坏告警），与全仓一致，非负担。

## 7. 验证

- `cargo test --workspace`：**511 passed / 0 failed**（新增 RG 回归、timefmt、后缀区间、
  ftyp offset-4、cache key 查询串稳定性等测试）。
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`：0 错误。
- `cargo fmt --all -- --check`：干净。

## 8. M8 后端缺口记录（Slint 桌面端接入真实后端；2026-09-07 复核；同日接口补齐轮落地）

> 背景：hmp-desktop 正从 mock-first（页面数据全部来自 mock.rs、播放由 PlayerHost 模拟，
> `crates/hmp-desktop/src/main.rs:1-3`）转向接真实后端（hmp-daemon，Unix socket IPC）。
> 解耦原则：UI 只发意图、daemon 单一状态源、UI 不做 HTTP/不碰凭证。
> 以下每条均先读代码证实/证伪再记录；"不缺"的如实说明机制。
>
> **状态更新（2026-09-07 接口补齐轮）**：除注明外，下述缺口已全部落地——
> IPC 新增 `QueuePlayAt / Search / LyricGet / AccountStatus / QualityGet / QualitySet /
> CoverGet` 七个请求与 `Event::LibraryChanged` 事件（crates/hmp-core/src/ipc.rs）；
> daemon 侧 `content.rs` 统一出网（搜索/歌词/账号/封面，TTL cache + 封面落
> `covers/<hash>.jpg`）；`track_meta_batch` 扩列 duration/cover_uri；`PlaybackState`
> 增加 `user_volume` 原值。真机冒烟 13/13 通过（QQMusicDownloads 本地队列 +
> QQ 网络搜索/歌词）。仍开放的：#5 下载/已购域（产品决策）、#2 的内容详情
> 半边（推荐/榜单/歌手页，随内容页里程碑）、#6 昵称拉取受 QQ 服务端
> business code 10000 影响（CLI `hmp account profile` 同失败；AccountStatus
> 诚实回退 `QQ {uin}`）。原条目保留作历史记录。


1. **IPC 无媒体库读请求（读库 = 客户端直读 sqlite）——契约已成立，非临时方案**。
   `Request` 枚举（crates/hmp-core/src/ipc.rs:111-183）只有播放/队列/收藏写/歌单写/
   评论/LibrarySync/Status/Subscribe 等，没有任何媒体库查询；连 `QueueList` 的注释都
   写明"元数据投影在客户端侧经媒体库批量查询"（ipc.rs:127-133）。CLI 是现行事实契约：
   `open_library()` 直连 `library.sqlite3`（crates/hmp-cli/src/library.rs:18-22，模块
   文档"直读本地 DB + daemon reconcile 触发"）。边界评估：schema 耦合是编译期共享
   hmp-storage crate（同仓同版本，无跨仓漂移）；跨进程 WAL 并发已有 busy_timeout=5s
   （crates/hmp-storage/src/db.rs:282-284，本轮审计已修）。同仓的 Slint 桌面可沿用
   直读（crates/hmp-desktop/Cargo.toml:20 已依赖 hmp-storage），代价是 UI 侧要自管
   "读库放后台线程、变更后重查"的纪律——刷新时机依赖第 9 条的变更事件。
   建议：维持直读契约并写进 PORTING 文档，不为读路径新增 IPC。

2. **IPC 无搜索与内容详情接口——已关闭（内容域里程碑落地）**。Search/Discover/Top/歌手/推荐等内容读面已全部经 daemon content.rs 统一出网（ff8050e 接 IPC+CLI+桌面 UI、9b90e81 内容页升级），`hmp search`/`hmp account` 直连旁路同步收编。以下为当时的分析存档：
   **2026-11-25 更正（§30.4）**：并未收编——`hmp search`/`hmp account`/`hmp login` 仍直接创建
   `QqMusicClient` 出网并直读凭证（`crates/hmp-cli/src/search.rs:10`、`account.rs`、`login.rs`）。
   桌面导航已含 home/discover/top/top-detail/search/playlist/album/artist 页
   （crates/hmp-desktop/ui/nav.slint:4-22），全部只有 mock 数据。daemon 只消费
   SongApi/SonglistApi/AlbumApi/CommentApi 与 reconcile 用的 UserApi
   （crates/hmp-daemon/src/player.rs:16、sync.rs:193、reconcile.rs:10）；搜索/榜单/
   歌手/推荐模块无任何 daemon 消费（§5 表所列休眠能力）。现行旁路：`hmp search`
   直连 QqMusicClient（crates/hmp-cli/src/search.rs:7-8）、`hmp account` 直连 UserApi
   （crates/hmp-cli/src/account.rs:8、41）——绕过 daemon，多客户端各自出网、凭证读取
   分散（桌面技术上也能直连，Cargo.toml:19 已依赖 hmp-qqmusic-api，但违反解耦原则）。
   建议：IPC 增加一组读请求（Search / 内容详情），由 daemon 持凭证统一出网并享受其
   内存缓存（评论已有同类先例，ipc.rs:156-158）；这是 M8 最大的一块新协议面，需
   产品决策排期。

3. **IPC 无歌词读取——已关闭（ed1dad3）**。`Request::LyricGet` + daemon 本地三级缓存 + CLI `hmp lyric` 已全部落地，播放页歌词走同一链路。以下为当时的分析存档：
   `Request` 无歌词变体；QQ 歌词 API（lyric.rs）目前唯一
   消费者是桌面老 AppCore 的进程内直连（crates/hmp-desktop/src/app.rs:20-23、
   796-821 `LyricApi::get_lyric`），而 AppCore 已随旧 UI 契约废弃
   （crates/hmp-desktop/src/bridge.rs:2；PORTING.md:41"app.rs 暂不动，M8 重接"）；
   CLI 无 `hmp lyrics`（§5 表已列为休眠能力）。M6 歌词页接线时无 IPC 可用。实现细节：
   QQ 歌词请求需要 `song_type`（app.rs:809），daemon 的 `ResolvedTrack` 未透出该字段
   （crates/hmp-daemon/src/player.rs:93-104）——走 daemon 取词则 daemon 内部需自行补齐
   （resolve 时本就拿到了 detail.track，player.rs:260-268）。建议：新增
   `Request::LyricGet { mid }` 由 daemon 出网；UI 侧 lyrics.rs 的 LRC 解析可复用。

4. **封面链路：本地不缺，QQ 曲目缺本地产物——部分成立**。
   - 本地曲目机制完整：嵌入封面提取后持久化为 `<data_dir>/covers/<hash>.jpg` 并以
     `file://` URI 进 tracks 行（crates/hmp-storage/src/scan.rs:34-51；
     crates/hmp-daemon/src/local.rs:167-176；回归测试 local.rs:429-441），UI 可直接
     消费，不缺。
   - QQ 曲目无任何本地产物：resolve 时构造远程 `https://y.gtimg.cn/...{pmid}.jpg`
     直接放进 `Track.cover`（crates/hmp-daemon/src/player.rs:288-293）；MPRIS 的
     `mpris:artUrl` 原样透传该 https URL（crates/hmp-mpris/src/metadata.rs:66-71），
     **没有**"下载到本地缓存再给 file://"——MPRIS 能显示是因为客户端自己拉网。
     daemon 全仓无 QQ 封面下载/缓存代码；桌面现状封面是确定性 SVG 占位生成
     （crates/hmp-desktop/src/covers.rs:1-8、183-197）。UI 禁 HTTP 原则下 QQ 封面
     到不了 UI。建议：daemon 复用 persist_cover 的目录契约
     （`<data_dir>/covers/<hash>.jpg`，scan.rs:4）下载 QQ 封面，把 file:// URI 随
     状态/详情下发；MPRIS 顺带改用 file://。

5. **下载库/已购库无后端域——成立**。storage 建表只有 tracks/local_files/favorites/
   playlists/playlist_tracks/play_events/track_artists/scan_roots/relations/
   playlist_ops（crates/hmp-storage/src/db.rs:223-261、1909、1916、2005、2018），
   daemon 无相关接口；downloads/purchased 目前只是 Vue 移植来的 mock 概念
   （crates/hmp-desktop/src/mock.rs:697、747；ui/downloads-page.slint、
   ui/purchased-page.slint）。若要落地，最小域需要：
   ① `download_tasks`（track_id/目标音质/状态/本地路径/进度）与 `purchases`
   （购买记录映射）两表；
   ② 下载执行器：复用 resolve 的取流+解密链，落盘为可离线播放的本地文件并登记
   tracks/local_files（注意 §4.4 的 FK 地雷：subscribed 歌单曲目缓存前置清理）；
   ③ 已购记录只能靠同步——hmp-qqmusic-api 目前没有"已购列表"API 模块，需先确认
   上游接口存在与否。建议：M8 先诚实空态，不动后端；列入 §5 功能扩展候选。

6. **登录态/账号信息无 IPC 读——成立**。`Request` 无账号状态查询；daemon 的
   `has_credential()` 仅供内部前置校验（crates/hmp-daemon/src/player.rs:191-197），
   未暴露。现行旁路：CLI `hmp account profile/vip` 直读凭证文件 + 直连 UserApi
   （crates/hmp-cli/src/account.rs:7-18、41）；老 AppCore 是桌面进程内直读 keyring
   （app.rs:433-441、496-506）——正是"UI 不碰凭证"原则要杜绝的形态。UI 只能从
   写/播放操作的 `IpcErrorCode::NotLoggedIn` 失败间接推断登录态
   （ipc.rs:347-348；crates/hmp-daemon/src/server.rs:364-370、447），无法驱动设置页
   账号面板/未登录引导（settings-account 路由已存在，nav.slint:21）。建议：新增
   `Request::AccountStatus`（is_logged_in + 昵称/uin + vip 摘要，daemon 读凭证 +
   UserApi）；登录流程本身（QR 轮询写凭证）是否搬进 daemon 是独立决策。

7. **音质偏好无 IPC 写，两处存储不收敛——成立**。UI 音质选择写桌面本地
   `~/.config/hmp/desktop-ui.json`（crates/hmp-desktop/src/prefs.rs:12-13、40-46；
   bridge.rs:262-274；0=标准…3=Hi-Res 四档枚举）；daemon 每次解析曲目时读
   `hmp_storage::Config::load().quality.chain()`（crates/hmp-daemon/src/player.rs:
   299-301）——即 config.toml 的 `[quality]`（CLI `hmp quality` 直写该文件，
   crates/hmp-cli/src/quality.rs:22-49；语义是 auto/fixed+回退链）。两处互不感知：
   UI 选"无损"，daemon 照旧按 config.toml 档位解析。巧合是 daemon 逐曲重读 config，
   理论上 UI 直接写 config.toml 即生效，但这违反"UI 只发意图"且与 CLI 写路径竞态。
   建议：IPC 增加 `Request::SetQualityPreference`（复用 QualityPref 语义），由
   daemon 落 config.toml；桌面 prefs 的 quality 降级为纯展示态或删除。

8. **队列"跳到第 N 首播放"无命令面——成立（底层能力已在）**。`Request`
   （ipc.rs:111-183）与 `PlayerCommand`（crates/hmp-core/src/player.rs:161-185）均无
   PlayAt/跳转变体；但 QueueCore 已有 `set_current(index)`（crates/hmp-core/src/
   queue.rs:275，含 clamp）+ 引擎 `load_and_play`（crates/hmp-daemon/src/engine.rs:
   795），`set_current` 目前只在 PlayNext 插入路径内部使用（engine.rs:683）。老
   AppCore 的 `PlayQueueIndex`（app.rs:161-163、690-697）证明该语义确有 UI 需求；
   UI 队列抽屉点歌目前只能绕道（整单重放或用 QueueRemove 曲解语义）。建议：加
   `Request::QueuePlayAt(usize)`，引擎侧复用 QueueRemove 当前曲的事务式装载模式
   （engine.rs:357-399：先装载成功再提交，失败回滚）。

9. **媒体库变更无推送事件——成立**。`Event` 仅 `StateChanged(DaemonState)`
   （ipc.rs:209-213），`publish()` 只由播放状态 watch、队列命令、引擎事件驱动
   （engine.rs:326-468、474-495）；本地监听（crates/hmp-daemon/src/watcher.rs）与
   QQ reconcile（sync.rs）全程不接触 state_tx（两文件 grep 无 publish/state_tx）。
   桌面库页是启动时一次性静态装载（bridge.rs:58-198；main.rs:22），扫描/同步完成后
   UI 永不更新。建议：Event 增加轻量 `LibraryChanged`（或 StateChanged 附带库代际），
   watcher 批处理落库后与 reconcile 完成处各触发一次；UI 收到后重查 sqlite
   （依赖第 1 条的直读契约）。

10. **daemon 生命周期与桌面共存——已解决（本轮同步重构）**。
    `spawn_detached` 原固定用 `std::env::current_exe()` 以 setsid 拉起"自己"，
    桌面二进制不是 hmp：直接复用会把 hmp-desktop 自身再 spawn 一遍。
    M8 接线轮已把 detach 点参数化：`spawn_detached_exe(exe, args)`
    （crates/hmp-daemon/src/serve.rs，`spawn_detached` 变 current_exe 薄包装），
    桌面 `connect_or_spawn` 定位 hmp 二进制（current_exe 同目录 → PATH）后
    `hmp serve --background` 拉起；flock 单实例与 socket 就绪轮询
    （client.rs:84-95）原样复用。hmp-desktop 已依赖 hmp-daemon crate。

11. **`track_meta_batch` 投影过窄，QQ 队列行缺时长/封面——成立**。
    队列元数据投影依赖的 `track_meta_batch`（crates/hmp-storage/src/db.rs，
    CLI/桌面共用）只回标题/歌手/专辑；daemon 侧 `cache_stubs` 落库的
    duration_ms/cover_uri 列没有读出口。表现为：队列抽屉里 CLI 搜来播的 QQ
    曲目时长显示 0:00、封面只能程序化占位（crates/hmp-desktop/src/backend.rs
    队列投影回退路径）。建议：`track_meta_batch` 扩列返回 duration/cover_uri，
    桌面/CLI 投影同步受益。

12. **音量语义：状态里的 volume 含 ReplayGain 补偿——成立**。
    引擎 SetVolume 把用户音量乘以 RG 因子后下发的即 `PlaybackState.volume`
    （engine.rs `set_volume(user × rg_factor)`；`DaemonState.replaygain_db`
    是原始标签值）。开 RG 的本地曲目下，UI 音量滑杆读到/回设的是补偿后值，
    与用户设定有静默偏差。建议：PlaybackState 增加 `user_volume` 原值
    （或 RG 因子），UI/MPRIS 展示与回设都用原值。

### 汇总

| # | 缺口 | 影响 | 建议最小解法 | 优先级 |
|---|---|---|---|---|
| 1 | IPC 无媒体库读（客户端直读 sqlite） | 非缺口：CLI 事实契约，schema/WAL 边界已可控 | 维持直读并文档化；刷新触发靠 #9 | —（契约确认） |
| 2 | IPC 无搜索/内容详情 | M4 内容页无数据源；CLI 旁路直连、请求分散 | ~~新增 Search/ContentDetail 读请求~~ **搜索已落地**（`Request::Search` + 桌面搜索页）；内容详情（推荐/榜单/歌手）随内容页里程碑 | 高（半闭合） |
| 3 | IPC 无歌词读取 | M6 歌词页无数据源 | **已落地**：`Request::LyricGet`，daemon 经详情自查 song_type，TTL cache；M6 页面接线待页面移植 | 已闭合 |
| 4 | QQ 封面无本地产物 | UI 无法显示 QQ 封面 | **已落地**：`Request::CoverGet` 下载进 `covers/<hash>.jpg`（复用 persist_cover 去重）回 `file://`；当前曲异步换真图（mid 复核防串台）；MPRIS 维持远程 URL（其客户端自行拉网，行为不变） | 已闭合 |
| 5 | 下载库/已购库无后端域 | downloads/purchased 页只能是 mock/空态 | 域设计列入 §5 候选（产品决策，未动） | 低（开放） |
| 6 | 登录态/账号无 IPC 读 | 账号面板/未登录引导无数据 | **已落地**：`Request::AccountStatus`（TTL cache；昵称失败回退 `QQ {uin}`；当前 QQ 服务端对 get_homepage 返 10000，CLI 同失败，非回归） | 已闭合 |
| 7 | 音质偏好两处存储不收敛 | UI 选择对播放不生效 | **已落地**：`Request::QualityGet/Set`，daemon 落 config.toml（非法别名拒绝）；桌面选择即写 IPC，启动从 daemon 同步（auto/未知保持现选） | 已闭合 |
| 8 | 队列无 PlayAt | 队列点歌语义缺失 | **已落地**：`Request::QueuePlayAt(usize)` 事务式跳播（越界报错/当前曲仅 ensure-play/失败回滚）；抽屉点行直连 | 已闭合 |
| 9 | 库变更无推送事件 | 库页静态快照永不更新 | **已落地**：`Event::LibraryChanged`，watcher 批处理/sync reconcile/收藏/歌单写各触发；桌面收到后重查 sqlite + 重放当前详情路由 | 已闭合 |
| 10 | spawn_detached 绑定 current_exe | 桌面无法拉起 daemon | exe 路径参数化（spawn_detached_exe） | 已解决（前轮） |
| 11 | track_meta_batch 缺 duration/cover 投影 | 队列行 QQ 曲目 0:00、程序化封面 | **已落地**：TrackMeta 扩列两字段；桌面队列投影直接消费（本地行封面读盘，QQ 行占位、当前曲走 CoverGet） | 已闭合 |
| 12 | 状态 volume 含 RG 补偿 | UI 音量滑杆与用户设定静默偏差 | **已落地**：`PlaybackState.user_volume` 原值随状态发布（serde default 兼容旧帧）；桌面滑杆读原值 | 已闭合 |

## §14 播放操作链路审计（2026-09-08，"无法播放、按钮失灵"专项）

### 现场根因（非代码缺陷也计入）

1. **运行中的 daemon 是过期二进制**：真机常驻的 `hmp serve` 为 09-07 16:38
   release 构建（≈`25301ad`），不含 `9fd5727` 输出设备候选序修复——引擎
   装载后设备打不开，卡在「Engine 自称 Playing、进度冻结在恢复点、
   State 卡 Loading」的假播放态。重启到当前构建后恢复正常。
   教训：桌面 `connect_or_spawn` 会复用任何已占用 socket 的旧 daemon，
   协议演进时旧 daemon 对新请求回 BadRequest（见 §14.2 修复的静默吞错）。

2. **媒体库垃圾行**：`local:/mnt/d/QQMusicDownloads`（目录）与 4 条已不存在的
   /tmp 测试曲被 upsert 进真库（目录行为 `File::open(dir)` 在 Linux 成功、
   解码 0 字节挂死 → 装载 5s 超时的唯一出口）。已手工清理；根治见 §14.1-3。

### 代码修复（本轮落地）

1. **PlayList 协议缺口（上一曲/下一曲永久禁用的根因）**：桌面
   `on_play_tracks(tracks, start)` 签名收整列表，实际只发点击行的单曲
   `Request::Play`，daemon `queue.replace([单曲])` 把队列清成 1 首——
   can_go_previous/next 恒 false、抽屉恒 1 行。修复：
   - `Request::PlayList { ids, start }`（hmp-core ipc）；
   - engine `play_list`：事务式（装载起播曲成功才 `queue.replace(ids, start)`），
     stub 解析两路——库内 id 批量投影（免逐文件 read_meta，大列表不卡引擎）、
     未命中走解析器；凭证门与 `Play(Track)` 同口径（列表含 QQ 曲目才要求登录）；
   - 桌面整表发送 + `SENT_META` 显示层 overlay（库外 QQ 曲目队列行标题不回退 mid）。
2. **命令静默吞错（"点了没反应"的主因）**：全部命令回调 `let _ = request()`
   丢弃结果。修复：`request()` 冷启动窗口重试（NotFound/Refused ×4×250ms，
   覆盖订阅循环拉起 daemon 的窗口）；拒绝/传输失败 → `Player.feedback` 反馈条
   （4s 自动消退，NotLoggedIn 给人话文案）；daemon `last_error` 与离线翻转同样
   浮出（去重防 10Hz 推送刷屏）。
3. **目录/缺失路径源头拒绝**：`LocalSourceResolver` 的 `local_stub`/`resolve_local`
   加 `is_file` 守卫（目录不再入队/入库），列表解析返回空 → 确定性失败。
4. **装载失败即时反馈**：`wait_current_applied` 订阅驱动事件，同代
   `PlayerEvent::Error` 立即失败（此前只能等 5s 超时，坏文件的「点了没反应」
   窗口从 5s 收到即时）；超时兜底保留（驱动静默卡死场景）。
5. **音量 0 回跳**：`apply_daemon_state` 弃用「user_volume==0 视为未设置回退
   RG 补偿值」的 hack（静音是合法值），恒用引擎发布的 user_volume 原值。
6. **UI 线程阻塞隐患**：`LibraryChanged` 刷新从「UI 线程同步全量读库」改为
   500ms 防抖 + 后台线程读快照/详情，UI 线程只做模型落地。

### 验证

- 单测：engine PlayList 三例（整表+下标/库快路径绕过解析器/装载失败保持旧队列）、
  local 目录守卫两例、desktop overlay/文案两例；workspace 全绿。
- 真机：`examples/playlist_smoke.rs`（12 首整表入队、下标起播、进度真实推进、
  上一曲可用）；`tests/live_daemon.rs`（#[ignore] E2E：订阅落地 → toggle/next
  → daemon 生效 → 推送回写 → play-tracks 整表替换 + 队列投影无 mid 回退）。

### 追加修复（2026-09-08 下午，"播放历史/播放列表"二次报障）

1. **现场复发确认（§14.1 同款）**：真机 daemon 仍为 09-07 16:38 旧构建，
   桌面端（14:23 新构建）发 `PlayList` → 旧 daemon 反序列化拒绝
   `unknown variant 'PlayList'`，错误浮出条如实展示（§14.2 生效的证明）。
   处置：重建 release 三件套 + `hmp quit` + 重拉 daemon（桌面订阅循环
   只连不拉起，新 daemon 起后被自动接管）。
2. **RecentPlay 播放键投影缺口**：`recent_plays` 不带 source/source_key，
   历史页 QQ 行 mid 只能置空 → `resolve_id_stubs` 走解析器必败、整单拒绝
   （"半截列表不提交"语义下，一行空 id 拖垮整个历史列表回放）。修复：
   - `RecentPlay` 增列 `source`/`source_key`（JOIN 已有，随手投影）；
   - 桌面 `recent_rows` 按 source 分流：本地行直接用 source_key（去掉
     `local_path` 回查），QQ 行经 `track_meta_batch` 补全（与我喜欢页同口径）；
   - 发送端剔除空 mid 行并重映射起播下标（防御库外残行）。
3. **真机端到端**：真实历史 12 首（全 local）整表 PlayList → Ok、
   起播首行、队列 12 行；空列表 PlayList → 协议接受（不再 unknown variant）。
   `立春 - 薛凯琪` 标题乱码为 §14 遗留已录数据问题，非回归。
4. **最近播放改 LRU 视图（产品决策，推翻"按会话流水展示"）**：同曲重复播放
   在历史页产生重复行、当前曲高亮也随之多份。新增 `recent_tracks`（storage）：
   `ROW_NUMBER() OVER (PARTITION BY track_id ORDER BY started_at DESC)` 取每曲
   最近一次会话，一曲一行、再播置顶、limit 截断（LRU 语义）；桌面历史页
   切换到该视图（页头"共 N 首"= 去重后曲目数）。`recent_plays` 会话流水
   保留给 CLI `history`（审计/收听时长视角，不裁剪）。真库验证：51 条流水
   → 12 首各一行。
5. **我喜欢页移除"最近播放预览"区块（产品决策）**：`recent-preview` 属性、
   桥接取前 5 逻辑随之删除，最近播放只保留整页入口（侧栏导航）。hover_slider
   集成测试此前隐式依赖预览区真数据行充当内容页可命中行——改播种合成
   `liked` 行（与真库解耦，同文件侧栏播种先例）。

### 遗留

- FLAC 标签文件级乱码（`立春 - 薛凯琪.flac` 下载时双重编码，库内其余曲目正常）
  ——数据问题不修代码；如需处理应在下载链路做编码探测。
- ~~`show-overlay` 无挂载组件~~ 已接线（player-bar → player_bridge，M6 播放页收口）。
- 运维提醒：协议演进后需重启 daemon（DaemonState 新增字段已带 `#[serde(default)]`，
  旧 daemon 快照可解码；unknown variant 类破坏性变更仍需重启）；桌面自动拉起只认
  current_exe 同目录与 PATH 的 `hmp`，release 部署需同步重建。

## §15 播放页（M6）初移植——歌词页 & 评论（2026-09-08）

`now-playing.slint`（NowPlayingBody.vue + PlayerOverlay.vue 移植）：
全屏 overlay（`Player.overlay-visible` 条件挂载，隐藏即卸载；PlayerBar 封面
点击 `show-overlay` / 收起键 / ESC 关闭），环境层渐变+封面低透明放大，
首屏舞台定高一屏（左封面+信息随其下：标题+裸心形喜欢 mock/歌手/专辑 |
右整列歌词），上滑抵达评论区，底部控制台常驻（刻度进度条：每 10% 一根
刻度、四分之一加高、拖拽时间气泡；左音质+音量、中 48px 主播放键、右评论/
队列跳转键）。歌词跟随：原型 LyricsPane 的临界阻尼弹簧（ω=12 半隐式欧拉
dt=16ms）以 16ms Timer 在 Slint 层复刻；景深亮度按行中心↔视口中心距离
衰减；用户接管（按行/滚轮）暂停 3s；点行 `seek-ms` 精确跳转。

数据管线（player_bridge）：换曲检测（PREV_MID）→ QQ 曲 `LyricGet` →
`lyrics.rs::parse_lrc` → `NowPlaying.lyrics`（代际 +1 触发弹簧直达）；
`active-line` 由状态推送按 position 折算（拖拽 pinned 期间不折算）；评论
`CommentList`（hot/new，(mid,sort) key 去重，overlay 打开即装载）；本地曲
诚实空态（歌词"暂无歌词"、评论就绪空）。`Player` 增 `album`/`seek-ms`。

**与原型的已知偏差**（数据/引擎差异）：无逐字扫色（daemon 只投影行级
LRC，QRC 词级时间轴未透出）；景深无 blur（Slint 无滤镜），焦点行以字号
强调替代；评论扁平投影（无楼中楼/展开回复动画/吸顶节头）；头像=首字
占位、喜欢/点赞=mock 态（换曲回落）、shuffle/repeat=UI mock；跳评论为
瞬时跳转（原型 smooth scroll）。

**验证**：workspace 全绿；真数据帧验证（QQ mid：LRC 1013B+翻译 350B、
评论 360 条）；shot-headless 扩展 `--overlay/--seed/--wheel/--wait` 出图
验收（首屏弹簧焦点行居中+景深衰减；评论区居中列+节头 tab+发丝线）。
注：testing backend 不泵跨线程 invoke，无头实例收不到 daemon 推送——
数据管线以原始帧单独验证，布局以 --seed 出图验收。

### Slint 新坑（本轮入册）

1. 顶层 `const` 不存在 → 组件 property 替代；
2. `if`/`for` 块内的元素 id 对外层不可见——引用它的属性/Timer 必须
   同步下沉到该作用域（歌词弹簧因此挂在舞台节点）；
3. `@linear-gradient` stop 只接受 float（0..1），px 不行；
4. `%` 不是取模（`Math.mod`）；字符串拼接用 `"\{expr}"` 模板；
5. 函数体内未声明标识符直接赋值解析错（局部量用 let，或内联表达式）；
6. 负 delta 滚轮 = 向下滚。

## §16 播放与 IPC 深度审计（2026-10-01，三路子代理专项：播放链路 / IPC 生命周期 / 文档交叉核对）

**零号发现（环境级）**：真机常驻 daemon 跑的是 d3298a3 时代（21:31 构建）二进制，
aeb2437（明文无 Range 回退）提交于其后 22:36、**从未进入任何可执行文件**——重构
提交后的播放实测全部发生在陈旧进程上。纪律：改播放/IPC 链后必须重建 + `hmp quit`
再实测；`tasklist` 核对进程二进制 mtime（PORTING 排查口诀的 daemon 常驻版）。

**本轮修复（5 项，全部红→绿回归）**：

1. **DecryptReader 窗口内 seek 恰落 `fetched_until` 双向永久挂起**（探针实锤）：
   Seek in-window 分支单调推进 `consumed_until` + notify；read 饥饿兜底 notify。
   经 rodio `try_seek` 阻塞反馈可瘫痪整个音频驱动（hmp-media/stream/reader.rs）。
2. **`wait_current_applied` 忽略 `load_gen` 假 ACK**：同曲重载时旧装载同 id 立即
   ACK、失败无回滚——改双条件（gen+id）；FakeDriver 系补置 load_gen（hmp-daemon/engine.rs）。
3. **装载期暂停被覆盖**：completion 分支硬编码 `sink.play()`——Pause 在 Loading 态
   记录意图、completion 保持 Paused（hmp-player/core.rs）。
4. **DaemonState 新增字段无 serde default**（E2E 实锤：旧 daemon 存活时新客户端
   全部 `protocol error: missing field`；桌面订阅解码失败静默 continue → UI 永久冻结）：
   四字段补 default + ipc.rs 头部立 wire 兼容约定 + 桌面解码失败断线重连 + CLI
   附 `hmp quit` 重试提示（hmp-core/ipc.rs、hmp-desktop/backend.rs、hmp-cli/client.rs）。
5. **`hmp quit` 在 daemon 未运行时先 spawn 再杀**（慢盘下留孤儿 daemon）：
   `connect_existing` 纯连接，端点无监听 = 幂等成功（hmp-cli/main.rs）。

**开放项（2026-11-04 复核：五项已修，仅余三条）**：

- ~~IPC：accept 循环一错即停摆无自愈（server.rs break）~~ **已修**（server.rs accept
  退避自愈，代码注明「自愈（AUDIT §16 #1）」，25ms×2ⁿ 封顶 1s）；单实例管道 + 1s BUSY
  重试预算在连接风暴下可能耗尽（实测裸连 97% BUSY）——**仍开放**（Windows 面）；
  **2026-11-25 更正（§30.4）**：已落地——`PIPE_STANDBY = 4` 待命实例池 + `CONNECT_BUDGET = 5s`
  （`crates/hmp-daemon/src/transport.rs:65,92`）；
  ~~客户端读响应无超时~~ **已修**（cli/client.rs 读响应 30s 硬超时）。
- 播放：~~中途读错误被 rodio 视为 EOS → 静默跳歌无 Error 态~~ **已修**（core.rs 读错误
  Error 态上抛）；~~`try_seek` 阻塞命令循环最长 30s~~ **已修**（后台化 + 10s ACK
  deadline）；~~音质回退链不覆盖解码期失败~~（**2026-10-02 关闭**，见 §17：engine 降档
  循环覆盖装载与解码期失败，Timeout 不降档为有意边界）；预解析期无总超时——**仍开放**。
  **2026-11-25 更正（§30.4）**：已落地——`PRELOAD_RESOLVE_TIMEOUT = 15s`
  （`crates/hmp-daemon/src/engine.rs:30,1510`）。
- ~~引擎命令循环内联 await 解析（歌单/专辑秒级）→ 播放控制命令排队，UI 感知「点了
  没反应几秒」~~ **已修**（resolve 走通道投递，播放控制不再被解析排队）。
- 其余见 §4（V1 footer 误报待立项、local_files 二等行、桌面端 UI 文案；INNER JOIN
  收藏观感已由 §17 大幅缓解、彻底关闭待 LEFT JOIN 决策）
  与 PORTING.md（设备热插拔不迁移、逐字歌词、P9 喜欢键）。

## §17 远程播放报错归因 + 歌单曲目缓存落地（2026-10-02，双代理并行修复）

1. **远程曲目播放失败被误报为「音质不存在」**：逐档取流探针实锤——存量凭证连免 VIP
   的 MP3_128 也 `104003`、免登录试听 RS02 正常 → 凭证被服务端判过期（`hmp auth`
   Expiry: expired），**音质 map 本身无误**（SongFileType 常量 ↔ `quality_to_file_type`
   ↔ `quality_from_file_type` ↔ `available_from_sizes` 三表逐一核对自洽；`models::File`
   补建模 `size_hires`）。旧代码全链失败后返回 `QualityUnavailable("")`（取流响应缺
   midurlinfo 时消息恒空，诊断黑洞）。修复：全链鉴权类错误码（104003 等，`is_auth_
   result_code`）归因为 `NotLoggedIn`（可操作文案）；混合失败才逐档带标签报
   `QualityUnavailable`；engine 降档重试守卫前移到装载前（复现已失败档/达
   `MAX_LOAD_DEGRADES`=8 不再装载同一份坏数据）；降档中解析失败（链耗尽）必回滚旧曲
   （真实驱动 `perform_load` 先 `sink.clear()`，不回滚=静音，含回归测试）。
   **用户侧动作：`hmp login` 重新扫码恢复远程播放。** 风险：VIP-only 曲 + 有效非 VIP
   凭证同码型，也会报 NotLoggedIn（上游码型无法区分，比空文案更可操作）。
2. **歌单曲目数量恒 0**：reconcile 自始只写 playlists 行、从未写 playlist_tracks
   （§4.4 所述「缓存功能未落地」缺口，非回归；API 层 live 核验零故障）。落地第四腿
   `reconcile_playlist_tracks`：逐歌单 `CgiGetDiss` 快照差集合入（远端增/删/元数据
   COALESCE 只升不降；页上限 20，超限视为不完整放弃落库防误删）；**owned 行
   remote_id 存 dirid 而详情接口只认 tid** → listing 就地建 dirid→tid 映射（免二次
   出网）；「我喜欢」（remote_id=201）特判走 `get_fav_song`；pending 意图在场本地
   胜出零出网。§4.4 FK 地雷随之拆除，§4.2 INNER JOIN 收藏观感被顺带大幅缓解。
   live 验证：22/22 歌单落库 4566 条链接，抽样三个歌单曲目数与远端 `total_song_num`
   全等。已知边界：远端重排序不回写本地 position；>2000 首歌单本轮放弃（用户最大
   1591 首，无实际影响）。

## §18 陈旧 daemon 自动重启 + 本地封面三断点 + 侧栏圆角（2026-10-02，用户三报并行修复）

1. **「无法播放远端音源，报 no available audio quality: result=104003」＝陈旧常驻
   daemon 再度咬人**：报错文案逐字是 5c5429e 之前的旧格式（新代码全档鉴权失败只
   可能报 `not logged in or credentials expired` 或带档位前缀的聚合串，e2e 锚定）
   ——daemon 是脱离会话的长驻进程（单实例锁），重建只换了 hmp-desktop.exe
   （18:42），daemon 实际跑的 `target/release/hmp.exe` 还是 10-01 产物（进程
   18:54 才启动，二进制是旧的）。侧栏「已登录」只是本地存在性检查
   （`account_status` 600s 缓存，不验服务端有效性），与 104003 并不矛盾。逐档
   探针复核：存量凭证全档 104003、RS02 正常 → 服务端会话仍失效，用户侧尚未重新
   扫码。修复（收口而非再绕）：①hmp-daemon 新增 build.rs 构建指纹（`src/**.rs`
   + `Cargo.toml` 的 FNV-1a，自带实现跨工具链稳定；无 rerun-if 指令 → 包内变更
   即重跑，桌面端依赖 daemon crate 必然连带重链、两侧取值恒同源），经
   `DaemonState.backend_build`（serde default 兼容旧 daemon 缺字段 = 空串）随
   状态推送；②桌面 `connect_or_spawn` 就绪后以 `Request::Status` 短连接核对指
   纹，不一致（= 运行中 daemon 是旧代码）→ `Quit` 优雅退出 + 磁盘二进制重新拉
   起 + 复核一次；仍不一致 → 新 `BackendError::StaleBackend`（磁盘二进制本身
   过旧，报错点名「重新构建/重装」）。自愈闭环：新桌面 × 旧 daemon 进程 → 自动
   换新；旧磁盘二进制 → 明确报错，不再「看着是新的跑着是旧的」。
   **用户侧动作不变：远程播放真正恢复仍需 `hmp login` 重新扫码（本次会话实锤
   凭证仍未续）。**
2. **本地曲目播放无封面（三断点叠加，非单因）**：①桌面 `cover_for_track` 裸
   `strip_prefix("file://")`——规范形态 `file:///C:/...` 被解析成 `/C:/...`
   （Windows 读不到，covers.rs 注释早有记载的根因之一）→ 改用现成的
   `covers::file_uri_to_path` 双形态解析，加载失败补 debug 日志（此前静默）；②
   storage `read_meta` 内嵌封面 2MB 上限把 3000px JPEG（3-6MB 常见）整张静默滤
   成「无封面」（LocalMeta 注释还写着「前 2MB」，与行为相悖）→ 上限抬到 32MB，
   并补 primary tag 之外的全 tag 兜底（歌词提取同款 primary → 全 tag 语义）；
   ③数据侧：8224023（2026-09-26）写路径归一化之前，旧 daemon 以
   `local:\\?\C:\...` verbatim 键入库，归一化后同文件另建规范键行 → verbatim
   行沦为永久重复（cover_uri 恒 NULL 的库页双卡；v5 `merge_ghost_local_tracks`
   只认「无 local_files 的意图幽灵」，带子行的 verbatim 扫描行永远漏网）→ v7
   迁移 `merge_noncanonical_local_tracks`：规范行在场 → relations/歌单链接/
   **播放历史（play_events，FK 无 CASCADE，verbatim 行由播放创建几乎必然带子
   行——本迁移实机首跑即因漏搬它 DELETE 失败回滚、daemon 回退内存库，教训：
   动 tracks 行先盘 REFERENCES 子表）**搬迁后删行（含 local_files 子行）；缺席
   → `source_key` 与 `local_files.path` 就地改键；路径不存在跳过等下次（幂等，
   含两分支回归测试）。附：covers/ 目录
   里两个被 DB 引用的本地封面文件丢失属历史数据损伤（全仓无 eviction 代码路
   径，疑 2026-09-09 双数据目录事故遗毒）；`resolve_local` 每次解析都重提取重
   落盘、scan 全量 `set_track_cover`，重播/重扫即自愈，不加代码。
3. **侧栏歌单封面直角**：`border-radius: Theme.radius-sm` 早就设了，缺的是
   `clip: true`——Slint 圆角只裁自身背景、不裁子 Image。宽窄两栏两处封面按
   queue-drawer 缩略图同款模板补齐 clip + `background: Theme.muted` +
   `image-fit: ImageFit.cover`。

## §19 优雅退出全链路广播（2026-10-02，用户报「托盘退出后 desktop 还在运行」）

**现象**：托盘「退出」（或 `hmp quit` / SIGINT）后 daemon 正常退出，但桌面窗口
仍开着——订阅流只是断流（EOF），桌面端落入既有离线路径（窗口保持、每 2s 重
连、播放操作全部 no-op），成为离线僵尸；两进程间没有优雅关闭消息，退出只对
daemon 单方面生效。

**根因两点**：①协议层没有「daemon 正在优雅退出」的事件——EOF 既表示优雅退出
也表示崩溃，客户端无从区分，只能按「可能恢复」走重连；②桌面订阅循环的拉起
资格按 `last_revision.is_none()` 判定，daemon 在 Subscribe 与首帧之间的窗口内
退出（EOF 竞态）会被误判为首连，把用户刚退出的 daemon 复活——即「tray 退出但
daemon 依然运行」的另一个形态。

**修复**（发送方 + 接收方 + 时序三段收口）：
1. 协议：`Event::Shutdown` 新变体（hmp-core ipc.rs；serde unknown variant 对旧
   客户端 = 解码失败断线重连，升级窗口内退化为既有行为，wire 兼容不变）。
2. daemon server：订阅连接 select 臂监听引擎 `terminated`（sticky watch，与
   state/library 两臂同形；先 borrow 再 changed 覆盖宽限窗口内新到订阅的即时
   触发）→ 推 `Event::Shutdown` 后断开。`watch::changed` 取消安全，臂被抢占
   后下一轮重建 future 不丢通知。未订阅短连接不受影响（响应/EOF 收尾）。
3. daemon serve：`term_wait` 后新增 1s 订阅广播宽限（`SUBSCRIBER_SHUTDOWN_GRACE`）
   ——编排任务与各连接任务并发调度，进程退出会先杀掉连接任务，不留窗口客户端
   只见 EOF。回环链路一帧极小，1s 足够且不强制。
4. 桌面 backend：收到 `Event::Shutdown` → `slint::quit_event_loop()` 整窗退出
   （可跨线程调用；runtime 随 main 结束销毁，订阅任务一并终止），不再重连；
   拉起资格改 `ever_connected`（本进程曾成功连上即永不拉起），EOF 竞态复活
   daemon 的窗口就此关闭。
5. 语义边界：崩溃/kill -9 等非优雅死亡没有 Shutdown 事件，桌面端仍走离线重连
   （不自动拉活，与「用户显式退出后不得复活」的既有契约一致）；陈旧构建重启
   流程（§18）中桌面端自己发 Quit 时无订阅在场（首连阶段），不会误触发自杀；
   若另有订阅中的旧桌面实例，会随旧 daemon 一起退出（单实例卫生，可接受）。

**回归**：`quit_broadcasts_shutdown_to_subscribers`（server.rs：订阅 → 短连接
Quit → 断言 Shutdown 帧先于 EOF，容忍其前的中间状态推送）+
`event_shutdown_roundtrips_through_frame`（ipc.rs 序列化往返）。
EOF 语义不变：非优雅死亡依旧无事件。

## §20 封面链路四断点总修 + 列表封面预取（2026-10-03，用户报「大部分歌曲封面显示不出来」+「打开列表就应请求封面并缓存」）

**量化证据**（实机库直查）：4172 首 QQ 曲目中 4167 首 `tracks.cover_uri IS NULL`
（99.9%），covers/ 目录仅 37 个文件——封面链路实质上只对「恰好播放过的曲 +
发现页歌单卡」工作，列表页全线占位。

**四个断点**（上游到 UI）：
1. **入库丢封面**：reconcile `qq_track_row`/`repair_stub_metadata` 手握
   `song.album.pmid`（API 模型早有该字段）却写 `cover_uri: None`，注释还写着
   「远程 URL 由 CoverGet 域守卫链路接管」——域守卫只是校验器，不生产 URL。
   只有「恰好播放过」的曲经播放路径（player.rs T002 模板）落过 URL → 5/4172。
   → 两处 upsert 带 `cover_url_from_pmid`（T002R300x300M000 模板，与播放路径
   同款）；空 pmid → None（COALESCE 保留既有）。
2. **投影丢列**：桌面 `song_row_from_qq` 把 `track_meta_batch` 读出的
   `cover_uri` 无条件置空（注释「http 封面 UI 禁网 → 程序化占位」）——把
   「禁直连」错当「禁用」，库里明明有 rebind 回写的 file:// 产物也看不到。
   → SongRow 拆双列：`cover_uri`（file:// 直读盘）/ `remote_cover`（http，
   预取输入，UI 不直连）。
3. **无盘级缓存**：CoverGet 每次请求都出网下载，persist_cover 只按内容去重
   文件，URL→文件映射只在 tracks.cover_uri 一个地方（还是 NULL）。
   → v8 迁移 `cover_cache(url PK, file_uri, fetched_at)`：CoverGet 命中索引
   零出网直接回本地产物（URL 内嵌专辑 pmid，pmid 变则 URL 变，按 URL 缓存即
   内容寻址，无需 TTL）；下载成功双写索引 + `rebind_cover_url`。
4. **无预取**：只有 discover 歌单卡与当前播放曲有异步补图，歌曲行列表
   （歌单详情/我喜欢/榜单/猜你喜欢/最近播放）从不发 CoverGet。
   → 新模块 `track_covers`：列表落地即按 (mid, url) 清单批量预取（信号量
   8 并发；进程内 mid|url 去重，失败不重试——下次进页面盘缓存毫秒回）；
   URL→Image 旁路缓存让刷新重建模型立即拿到图（discover 刷新回退占位的
   已知缺陷不再复现）；回包广播全部 8 个曲目列表模型（同曲多列表在途竞态
   一并覆盖）。挂接点：apply_snapshot（我喜欢+最近播放）、歌单/专辑/歌手
   详情、discover 新歌/榜单/猜你喜欢。

**配套防御**：upsert 的 cover_uri 分支加 CASE——已 rebind 的 `file://` 行
不得被下一轮 reconcile 携带的远程 URL 打回（否则投影退化回占位还要再走一轮
CoverGet）；file:// 对 file:// 仍放行（扫描重提取换图可生效）。

**回归**：storage `cover_cache_roundtrip` /
`upsert_cover_remote_url_never_demotes_local_file`；daemon wiremock
`reconcile_tracks_carry_cover_url_and_rebind_survives_resync`（pmid 落库 +
空 pmid 不落 + rebind 存活重放）；desktop `qq_row_splits_cover_into_local_and_remote`、
`targets_from_rows_keeps_qq_remote_only`、`all_slots_are_distinct_and_complete`。

**已知边界**：存量库的 URL 回填靠 daemon 首轮 reconcile 全量 upsert（4566
条歌单链接，分钟级）；远端换封面（同 pmid 换图）不会自动失效盘缓存——QQ
封面 pmid 与图片内容绑定，实际不发生；排行榜分类页头图、歌手照片仍无数据源。
## §21 桌面前端内存治理（2026-10-03，btop 实测桌面 RSS 2.4G vs 播放核心 18M）

**现象**：hmp-desktop 常驻内存 2.4GB 且随浏览单调增长不回落；同链路的 hmp
播放核心仅 18MB——增长源全部在 UI 进程。

**根因四项**（三路只读子代理审计 + 逐点代码核实）：
1. **四套「只进不出」的 `HashMap<String, slint::Image>` 线程本地缓存**钉住解码
   位图：covers.rs 占位封面（按 seed）、library_view.rs 本地封面（按 URI）、
   player_bridge.rs 磁盘封面（按路径）、online_covers.rs 发现页封面。关键放大
   器：`Image::load_from_path` 本走 Slint 内部 5MB 权重 LRU，但应用层强引用
   把解码位图永久钉住，LRU 形同虚设；且 URI/路径/seed 三套 key 互不相通，同一
   封面文件最多被解码并钉住两份。
2. **占位封面按曲目 mid 播种**：2026-09-30 改版后同类占位像素逐字节相同
   （covers.rs 测试自己断言），但每曲仍按 seed 各存一份 256×256 RGBA
   （256KB/条）——1 万行收藏 ≈ 2.5GB，单项即可解释观测值。
3. **`TrackRow.cover` 是纯死重**：`ui/track-table.slint` 全文无 image 元素，
   12+ 个表格模型（收藏/最近/本地/下载/已购/搜索/榜单/队列）每行背一张全尺寸
   封面，唯一消费方是 queue-drawer 36×36 缩略——为一张 36px 图付 8-36MB/行。
4. **零虚拟化**：全部列表 `for` repeater 全量实例化（4566 行 × 15-20 元素）。

**修复四项**（linked worktree fix/desktop-mem-2.4g，四子代理契约并行 + 主代理
集成；隔离自并行改封面的另一会话线）：
1. **占位封面缓存按图标类型收口**（covers.rs）：key = `icon_path_for(seed)` 三
   类静态串，条目封顶 3；`cover_image(seed)` 签名不变。测试 `cover_image_cache
   _is_capped_by_icon_kind`。
2. **统一 cover_cache（新 src/cover_cache.rs）**：字节加权 LRU，key=(路径，
   max_side 桶)，CAP 64MB 超限逐最久未用；装载即 `image` 解码 → thumbnail
   降采样 → rgba8——缓存的与交给 UI 的都是小图。三处旧缓存删除并路由：
   library_view（256）、player_bridge（队列行 256 / 当前曲 512）、online_covers
   （256）；`local_cover_image` 对外签名不变（bridge 侧栏/详情/卡片调用点自动
   受益）。LRU 逐出/命中/降采样 ×4 测试。
3. **TrackRow 剥 cover + QueueRow 分型**（data.slint/stores.slint）：表格行不
   背位图；队列行走 QueueRow（TrackRow 全字段 + cover，消费方仅 queue-drawer）；
   `play-tracks([TrackRow],int)` 契约不变；搜索/发现行与 `to_track_row` 删封面
   构造，bridge 侧栏/详情/卡片封面保留。to_track_row 的 mid 播种 256KB/曲 复
   制路径就此消失。
4. **曲目表窗口化虚拟化**（新 ui/viewport.slint 全局 + track-table 行窗口）：
   顶层 Scroll 把滚动状态经 property 声明处双向别名 + changed/init 命令式回写
   进 Viewport 全局（y/view-h/origin-y）；TrackTable 只实例化可见窗
   （ceil(view-h/pitch)+2×8 overscan）行，行按绝对索引绝对定位
   （y=(window-start+offset)×pitch），占位高度=N×pitch 不变（滚动条几何不变）；
   view-h≤0（无头/未回写）退化全量渲染。**集成实测踩三坑**（已录 PORTING 坑位）：
   ①非布局父级容器必须显式钉 width+height（显式 height 不进 preferred 链 +
   repeater 无固有尺寸 → HoverGroup 被 layout 压成 0 高 + clip → 整表不可见
   不可点，take_snapshot 截图定位）；②语句级全局绑定/别名均 parse error，唯
   property 声明处双向别名合法；③for 整数即模型、无 `0..N` 区间、for 体内
   `row` 不可作属性名。
   附：**track_theme 单次有界解码**（Applied 摘除全尺寸封面常驻，只留 64×64
   采样 ≤16KB；sample/ambient 共享一次 to_rgba8→480px 有界缓冲，换曲瞬态
   双份全图拷贝消失）。
5. **hover × 虚拟滚动专项验证**（用户点名风险面）：testing backend + software
   renderer 探针实测——滚动后 hover 上报正常；**被 hover 行被滚出窗口卸载时
   HoverBus 自清**（HoverItem 退出自校验在实例几何跳变时自然触发 leave，比旧
   行为的高亮滞留更干净）；微动指针恢复上报；跨窗口边界的块飞行/黏滞形变与
   旧行为等价。drag 回归（thumb 跳转深滚后点行索引差 >20 行 + 1:1 拖拽比例）
   全绿。

**回归**：lib 66 测试全绿（含 cover_cache LRU×4、covers 封顶、track_theme 有界
解码）；drag/hover_slider/hover_stretch/svg_icon 集成测试全绿；clippy
`--all-targets` 零警告；`cargo fmt --check` 干净。内存水位验证留待真机长跑
（预期：RSS 有界于 64MB cover_cache + 模型字符串 + GPU 纹理，不再随浏览单调
增长）。

**已知边界**：滚动中 hover 块位置仍是上报时刻快照（旧行为即如此，鼠标微动即
刷新）；队列行 cover 取 256 桶（36px 缩略余量充足）；占位图三类共享同一位图
句柄（Slint 纹理单份）；远端 URL 封面仍走程序化占位（UI 零 HTTP 原则不变）。

**与 §20（封面链路四断点）的语义合并**：两线同日并行，track_covers 预取的
UI 侧模型回填（`apply_cover` 广播 8 个表格模型行 + `COVER_IMAGES`/
`IMAGE_CACHE` 两个无界线程本地位图缓存）与本节 ②「表格行不背图」正面冲突，
合并时摘除回填保留预取——预取的价值（daemon 盘缓存预热 + rebind_cover_url
库内升级）不变，播放/队列抽屉/详情页随后的封面读取经 cover_cache 直读盘；
签名去 `ui` 参数（纯 IPC 派发）。另有：共享 target 目录被两 worktree 交替
构建产出「幽灵编译错误」（daemon 直查绿、作依赖编译红，冷跑独立 target 即
消失）——多 worktree 共享 CARGO_TARGET_DIR 不可靠，与 §20 前科同类。

## §22 「前端又不能播放音频」——§20 后陈旧 hmp.exe 未重建 + StaleBackend 无限 churn（2026-10-03，用户报）

**现象**：桌面前端完全无法播放（用户口径「前端又不能播放音频了」）。此前 §16
零号发现、§18 已两度实锤「陈旧常驻 daemon」，本轮为同一陷阱的第三次复发，但
形态升级：**磁盘上的二进制本身是旧的**，重启 daemon 解决不了。

**根因链**（实锤证据）：
1. §20（8f9d5b9，10-03 02:29）改了 hmp-daemon 三个源文件（content/reconcile/
   server），但 release `hmp.exe` 停留在 10-02 21:57（edc304f 之后 8 分钟构建，
   早于 §20）；`hmp-desktop.exe` 却在 10-03 10:44 重建——**桌面新、磁盘后端旧**
   的错位组合。推测成因：只跑了 `cargo build --release -p hmp-desktop`（或等价
   单包构建），§20 备注里「重建 release daemon+desktop」的后半句没落地。
2. 指纹实锤：桌面 exe 内嵌 `BUILD_CODE=5c2c5827f3fd9a2c`（HEAD 构建），陈旧
   daemon 按其源码必然报另一指纹 → `verify_backend_generation` Quit+重拉（同一
   个旧 exe）→ 复核仍不一致 → `StaleBackend` → 订阅循环按离线降级，前端整体
   不可播。§21 修复期只有桌面端被重建，指纹握手正确拦下了这次错位——机制
   工作正常，但暴露出两个次生缺陷。
3. **次生缺陷 A（本轮修复）**：`connect_or_spawn` 失败后订阅循环每 2s 重试，
   每轮都对陈旧 daemon 执行 Quit+重拉——磁盘二进制不换就无限 churn，且把
   CLI/托盘本可用的 daemon 反复杀起。修复：`verify_backend_generation` 记账
   「重启后仍不一致」时的磁盘二进制形态（mtime+len，`STALE_BINARY_SEEN`），
   磁盘未变则快速失败不再 Quit（daemon 存活，CLI 可用）；重建二进制（stamp
   变化）自动恢复完整重启核对（自愈，无需重启桌面端）。纯决策逻辑
   `restart_worthwhile` 单测钉死。
4. **次生缺陷 B（本轮修复）**：离线原因从未到过用户眼前——toast 恒为「播放
   服务未连接」，`StaleBackend` 的可操作文案（「请重新构建/安装 hmp 后端二进
   制」）只进日志。修复：`UiStateEvent` 增 `offline_reason`（仅 StaleBackend/
   NoBackendBinary/SpawnTimeout 三类可操作错误携带，IO/协议断连沿用笼统提示
   防 OS 文案刷屏），离线翻转的一次性 toast 直接展示原因与修复指引。

**验证**：`hmp status` 实测新 daemon `backend_build=5c2c5827f3fd9a2c` 与桌面端
一致；本地曲（tone.wav）Playing 位置推进零错误；远程 QQ 曲以 Flac 全档播放
（`hmp auth` 的「Expiry: expired」为陈旧标志，实际取流健康——与 §17 的
104003 失效签名不同，勿混淆）。hmp-desktop lib 测试全绿（新增 3 例）。

**教训**：①「重建」必须落到所有链接 hmp-daemon 的二进制（hmp.exe 与
hmp-desktop 同源同指纹），单包构建是本仓库的高频事故源；②握手拦下错位只是
兜底，UI 必须把拦下的事实说给用户（本次 toast 修复后，同类事故用户可自救）；
③离线 churn 与静默原因叠加，把一次「重建一下」的环境事故放大成「前端坏了」。

## §23 远端封面显示链路复审（2026-10-04，用户报「远端封面无法被正确显示」）

**实机量化**（%LOCALAPPDATA%/hmp）：tracks 表 3935/4172 QQ 曲目已带 T002 模板
远程 URL（§20 断点①修复生效），但 `cover_cache` 表 **0 行**、covers/ 目录
38 个文件最后落盘 10-03 10:44（早于含修复的 release hmp.exe 11:44 构建）——
**新 daemon 运行期间零封面下载**。直连探针实测 y.gtimg.cn 封面 URL 下载
200/25KB 真 JPEG；手动拉起 HEAD daemon 后 IPC `CoverGet` 78ms 下载落盘、
二次请求 510µs 盘缓存命中、`rebind_cover_url` 一条 URL 命中 62 行同封面曲目
——**daemon 侧下载/落盘/盘缓存/rebind 全链路健康，断点在桌面端消费侧**。

**两个断点**（上溯到 UI）：
1. **队列抽屉非当前曲 QQ 行封面永不补图**：队列行封面在投影时固化
   （`row_from_meta` → `queue_cover`，远程 URL 落程序化占位），daemon
   `rebind_cover_url` 刻意不改媒体库代际（封面补齐不触发整页重查），队列
   模型只在 `queue.revision` 变化时重建——预取（track_covers）回包又全部
   丢弃（§21 语义合并时摘除回填）。组合结果：只有当前曲有
   `spawn_cover_fetch` 原地换图，抽屉里其余 QQ 曲**整场会话占位**。
   → `track_covers::prefetch_tracks_with`（带 `OnCover` 回包回调）：队列
   落地（apply_event queue_rows 分支）对远程封面行发起预取，回包经
   `invoke_from_event_loop` 落 256 桶 → `update_queue_row_cover` 原地换图。
   预取/回填全走 daemon 盘缓存与 rebind，去重/并发语义不变。
2. **三处进程级死去重「失败永不重试」**：`REQUESTED` 记账在发起时插入、
   从不释放——冷启动窗口（daemon 未就绪 / StaleBackend 恢复中 / 网络瞬断）
   内的请求全部失败且**永久在账**，该图整个会话占位（与 §22 的离线窗口
   正面叠加：StaleBackend 期间列表落地 → 预取全灭 → daemon 恢复后 UI 仍
   不再问）。→ 改「失败出账」：`track_covers`/`online_covers` 失败即移除
   记账（触发源=进页面/队列重建，低频自然重试）；`spawn_cover_fetch` 由
   10Hz 推送驱动，另加指数退避（2s 起步 ×2 封顶 60s，序列取自存储的时长
   而非时刻差——重试总发生在上次时刻过期后，按时刻差翻倍会恒停最小值），
   成功清除退避。三处触发纪律记录在各自模块 doc。

**回归**：desktop lib 73 测试全绿（新增 `failed_request_releases_dedup_key`、
`cover_retry_backoff_doubles_and_caps`）；clippy `--all-targets` 零警告。
daemon 侧实测见上（探针法：起 HEAD daemon + 直发 CoverGet，观察首下/命中
/rebind 行数）。

**已知边界**：队列行预取失败的重试依赖下次队列重建（换曲/加歌/重开会话）
——不换曲的静止队列不自动重试（当前曲除外，10Hz 退避重试兜底）；歌手照片、
榜单头图仍无数据源（§20 已知边界不变）；队列行回填图 256 桶（≤256KB/行）
与 §21「QueueRow 背 cover」契约一致，内存有界。

## §24 桌面验收补账——右键菜单视觉验收 + §21 内存长跑证伪（2026-10-23，S6 流；发现与独立审计双代理验证）

1. **右键菜单视觉验收（A 项）**：testing backend + software renderer 截 12 张（浅/暗 × 曲目表/队列/歌单上下文 × hover/禁用/钳制变体），9 PASS——面板几何（216 定宽/高度公式）、分隔线、禁用 opacity、hover 暖色、底右缘钳制逐张对照 context-menu.slint 内嵌规格实证；3 张（picker 系）转为缺陷证据。
2. **缺陷（高）：右键菜单「加入播放列表…」二级 picker 不可达**——track-table.slint:301 / queue-drawer.slint:365 的 PopupWindow 未设 close-policy，slint 1.17.1 默认 close-on-click（PORTING.md:188 原记载写反——缺陷漏网的认知根源，已更正）使弹层内任意完整点击（含禁用行）即关菜单。行为级证据：press-only/release-only 菜单存活、full-click 必关、picker.png≡picker-local.png（md5 同）。**修复：S7 流（close-on-click-outside）。**
3. **缺陷（严重）：§21 内存有界性证伪（B 项 FAIL）**——隔离真机 21 分钟长跑（音量 0、独立管道/独立数据目录、守护进程对照正常 max WS 68MB）：WS 平台斜率 +9.2~+11.1MB/min（对口径不敏感）、双窗漂移 147MB、P4 静置 115s 无驱动爬升 +127MB（超 cover_cache 64MB CAP 两倍）、P5 重播起播 20s 内 PM 单刻暴增 +2.27GB 后桌面进程死亡（死亡机制不单独归因，暴增本身系进程自身分配行为）。独立审计从 CSV 重算全部数字精确重现、混杂排除（外部内存压力不可能造成 commit 增长；封面缓存冷填充不可解释静置期爬升）。**根因调查立项 S8（开放项最高优先级）。** 附：全量模式浏览代理 6min WS 平坦（41MB）——§21 根因 3（表格剥图）与模型重建方向有效；窗口化虚拟化无法无头代理（Viewport.y 在 window-start 公式自相消，方法学边界入册 PORTING）。
4. 产物：docs/verify/2026-10-23/（REPORT、RSS 曲线 PNG+CSV、12 张截图、真机眼验 runbook）；QA 基建 examples/shot-menu.rs 入库。

## §25 内存有界性根因与修复闭环（2026-10-23/24，S8 调查 + S9 修复 + E1 双臂复验，双代理验证）

1. **根因（S8 只读调查，docs/verify/2026-10-23/mem-rca-FINDINGS.md）**：①平台斜率与双窗漂移 = 确定缺陷链「换曲/自动切曲 → 全量队列重投影 + 90 行 cover_cache(256) 装载 × miss 全尺寸 1200² 解码（6.5MB 触摸/条）> 64MB CAP → LRU 循环打爆」；③P4 静置爬升仓内穷尽排除（引擎/订阅/SyncWorker/watcher/定时器面全静默、daemon 同步增长）→ 系统态依赖；④P5 PM +2.27GB 超 app 侧核算 8~40 倍、PM:WS=13.6:1 签名 → GL/WDDM commit 放大相容。
2. **修复（S9）**：R1b 队列封面异步逐行回填（16ms×6/tick、换装作废、热行 get_if_cached 同步零闪烁、探测不更新 LRU stamp）；R2 派生封面 `<hash>.512.jpg`（persist_cover 五调用点含 CLI scan 写侧、读侧三级回退、互锚 hmp-core::media::derived_cover_name、双向兼容独立回滚）；R3 EqClock 播放门控 + Equalizer paused||!visible 自门控（暂停冻结经绝对相位机制数学等价）。
3. **E1 复验（docs/verify/2026-10-24-mem-e1/，同日同机双臂各 23.5min）**：fix 臂四判据全 PASS（max WS 246MB、斜率 -0.02MB/min、P5 36/36 存活、daemon 50MB 含迁移期、派生件 90 个 ≈23KB/张）；**基线臂当日亦有界（250MB/+0.166）——S6 失效签名跨日非平稳，主分量系系统态（当日内存压力）**，与 S8 排除论证闭环；修复全指标 ≤ 基线，机制级消灭由测试钉死（重建派发内解码 0 次/批恰 6/换装作废/门控往返）。真库护栏 94 文件前后零变化。
4. **新登记**：①`hmp quit` 后桌面挂死成不可杀僵尸（两臂均有、基线同——退出路径死锁，**开放项 S10 候选**）；②backend_build 指纹只哈希 hmp-daemon/src/**（R2 落 hmp-storage 不入指纹，E1 两臂同指纹实锤）——依赖级陈旧检测失明，纪律防线不变；③E3 回填计数日志未落盘（stdout 未重定向，口径缺口）；④R1a（daemon revision 不 bump）留作远期候选；⑤播放期不可见行 EQ 求值残留（打磨项）。

## §26 quit 后桌面僵尸挂死——根因与修复（2026-10-24，S10 流；调查四阶段+审计六发续接双代理验证）

1. **根因（cdb 符号化栈实锤，置信度实锤级）**：非 GL。`ExitProcess → LdrShutdownProcess → TLS/FLS 析构 → thread_local RefCell<slint::Timer> drop（bridge.rs schedule_refresh 回调闭包捕获 Arc<BackendRuntime>，退出时为最后引用）→ BackendRuntime::drop → tokio Runtime::drop → BlockingPool::shutdown 在 oneshot 上死等已被 ExitProcess 杀死的 blocking 线程 → loader 关停期 WaitOnAddress 永久挂起（TerminateProcess 亦无法清出）`。GL 理论（初审通过）被 software 对照臂证伪——无 igd* 驱动模块仍复现僵尸且两臂逐帧同栈；「不可杀」的本质是 loader 关停期的线程困死。
2. **修复（T7）**：`BackendRuntime` 持 `&'static Runtime`（Box::leak，进程同寿，任何退出路径不再触发 shutdown 握手）+ 契约单测（句柄消亡后在途任务继续，旧实现必红）。forget(ui)/KeepWindowShown 方案随前提证伪 revert；2s 用户态看门狗保留为纯纵深（对 loader 期挂死无效，如实注释）。
3. **验收（9 臂真机，隔离端点）**：基线（a5a0236）quit×2+software×1 = **3/3 ZOMBIE**（3 分钟短周期即复现）；fix2 quit 2/2 + X 关窗 1/1 = **全部 1.1s CLEAN_EXIT**（自然 drop 与 GL 销毁照常——基线臂自证该段无罪）。GL 后端实测 WGL（M1 预测成立）。
4. **方法论遗产**：①对照臂设计直接推翻了双代理一致认可的假设——「为佐证而设的对照臂」可能是最有价值的实验；②cdb 在 Windows Kits 默认路径（前任「缺席」误判贻误一轮）；③配额驱逐链下「增量落盘+续接代理」模式六发接力完成审计。

## §27 全列表窗口化虚拟化 + 队列封面按窗回填（2026-10-25，用户报「列表全卡（含 release）+ 桌面 4.3G 内存」；S12 流）

**双症状同源定案**：
1. **列表卡顿**：§21 只虚拟化了 TrackTable，其余列表仍 `for` 全量实例化——播放列表抽屉（队列可上千行 × ~25 元素/行，且面板 `visible=false` 仍参与布局全量驻留）、封面卡网格（发现页/库页/歌手/专辑/已购共用 CoverCardGrid）、榜单卡网格（30+ 组）、侧栏歌单区（QQ 收藏歌单数百行 × 带图）。
2. **内存爆炸复现（§24 B 项 4.3G 的机制实锤）**：1591 首 × 逐 add 全量队列重建压力下，未门控构建（S9 全量在）实测 Private 331MB → **3.7GB**（队列封面全量装载潮 + 行持位图 1591×~256KB + 逐 add 模型换装 churn，重建期平台 331MB、装载潮完成后冲顶）；仅加「抽屉关闭门控」的中间构建同压力 218MB 平坦。E1 修复臂有界与此爆炸并存不矛盾——爆炸需「大队列 + 封面装载」共同触发，E1 双臂脚本未覆盖该路径。

**虚拟化推广（第一批四文件，配方入册 PORTING「窗口化虚拟化推广模式」）**：queue-drawer 队列行窗（同文件命名 Scroll 变体）、page-parts CoverCardGrid 按行窗（Viewport 变体，发现页/库页/歌手/专辑/已购六页自动受益）、top-page 榜单卡组内行窗、sidebar 歌单区宽窄两形态行窗。统一铁律：容器高度只依赖模型长度（防滚动偏移绑定成环）、行按绝对索引绝对定位（offset 不可作下标）、view-h≤0 退化全量渲染（drag.rs 深滚守护仍有效）。**有意不虚拟化**：评论区（20 条/页有界 + 行高变）、歌词（行高变 + 弹簧跟随依赖逐行几何上报）、右键 picker/搜索歌手卡/扫描根/音质档（有界小列表）；sidebar 组收起动画期间不做 group-open 门控（防收起动画闪空）。

**队列封面按窗回填（R1c，R1b 的按窗化）**：新 `QueueWindow` 全局（queue-drawer 行窗口容器经双向别名 + changed/init 发布 start/count；**start/count 两个 changed 都要挂**——队列装载时 start 恒 0 而 count 0→N，单挂 start 漏发，测试实锤）；Rust `schedule_queue_window_covers` 只对窗内行做 `get_if_cached` 探测（miss → 回填 Timer 分批装载）+ 远端预取（track_covers 在账去重）；排程统一走 changed → `Player.queue-window` 回调（**Slint changed 延迟到事件循环泵执行，重建当拍读全局必旧值**——回调下一拍必到）；抽屉关闭期零解码零预取。行持位图 1591 → ~30（×256KB），解码潮与远端预取同量级收窄。

**验证**：`cargo clippy --workspace --all-targets -D warnings` 零警告；hmp-desktop 112 测试全绿（含 drag.rs 深滚守护；新增 `queue_rebuild_defers_covers_until_drawer_opens` 按窗门控测试）；无头截图 library/discover 退化路径布局不变；真机压力实测（1591 逐 add + 12min 采样，本机非隔离）：启动瞬态 WS 250MB → 回落 **194MB WS / 218MB Private 死平（斜率 ≈0）**，队列 1592 首全程驻留、抽屉关闭态。

**已知边界**：①逐 add 全量重建的模型换装 churn 仍 O(N²)（压力路径专属；按窗后实测无碍，合并式重建留作候选）；②打开抽屉首拍窗内 ~30 行解码（毫秒级补齐，测试钉死）；③重建换装与 changed 发布间 ≤1 tick 的陈旧在途解码（mid 守卫 no-op，E3 口径 +≤6/次）；④歌词/评论区不虚拟化的理由如上，若日后评论改多页加载需重估。

**另案登记**：真机验证中发现 `hmp play playlist:<本地id>` 对 qq-owned 歌单远端详情拉取恒报 "playlist is empty"（#16/#17 两条均复现，本地库 1591 首完整、登录态正常）——内容域 fetch 链路疑似 QQ 侧接口/风控变化，未列入本流处理。

## §28 当前曲封面 10Hz 覆盖互踩 + 取色同键吞重算（2026-10-25，用户报「本地有缓存但底部播放栏恒占位」）

**根因（双层，同源于 M8 全量投影未随封面异步化演进）**：`apply_daemon_state` 在每次状态推送（~10Hz）全量重设 now-playing 元数据。① `cover_for_track` 对 https 远程封面每帧回写程序化占位，把 `spawn_cover_fetch` 回包落地的真图**下一帧覆盖回占位**——QQ 曲目 `playback.current.cover.url` 恒为 daemon resolve 时的 https 模板 URL（player.rs 从 pmid 现拼，不经 rebind），故 QQ 曲播放条/播放页大图恒占位（本地 file:// 曲每帧重算命中 LRU 结果幂等，未暴露）；② 取色键占位态与真图态同为 `mid|url`，真图回包的 `apply_cover` 被键去重**静默吞掉**——真图取色/环境层从未生效，恒为占位中性色。

**修复（键控幂等同步，废弃每帧重设）**：元数据层（标题/歌手/专辑/封面）按 `PREV_MID` 换曲门控，10Hz 推送只逐帧更新真正逐帧变化的传输/进度态；音质徽章按 `(mid, tier)` 门控（同曲重装载降档可翻新）。封面经 `now_playing_cover`（纯函数）产出 `(封面键, 真图?)` 四形态键：`file:{path}` 本地直读 / `real:{mid}|{url}` 会话已取回（新 `RESOLVED_COVERS` 备忘 url→盘路径——换曲重进/重装载时 REQUESTED 在账不会重发，靠此直读盘出真图）/ `pending:{mid}|{url}` 待取回 / `prog:{mid}` 占位；`sync_now_playing_cover` 键不变不动 `Player.cover`（异步真图不再被覆盖）。取色键随封面键走：`pending:` → `real:` 必然异键，回包重算自然触发（顺修②）。`spawn_cover_fetch` 调用点保持逐推送（即退避重试引擎，退避窗口内纯记账查询）。

**验证**：clippy -D warnings 零警告；hmp-desktop 103 lib 测试全绿（新增 `now_playing_cover_key_semantics` 钉四形态键 + 桶填充尺寸契约）；daemon 侧实探（命名管道直发 CoverGet）确认盘缓存毫秒回，问题全在桌面端覆盖互踩。附带发现：image 0.25 `thumbnail` 为**桶填充语义（可放大）**，`decode_512_bucket_serves_derived_directly` 既有测试已钉死（400×100 → 512×128），非回归，测试按此口径断言。

## §29 桌面端 Slint 1.18 升级回归 + M7 收口（2026-11-25，用户报「继续完善 UI/交互逻辑」）

**零号发现（依赖轮遗留，静默三周）**：S11（2026-11-04）把 Slint 从 1.17 升到 1.18.1 后，`.slint` 里 `timer.restart()` 对**从未启动过**的 `Timer { running: false }` 变成空操作（编译为 `slint::Timer::restart()`，要求此前 `start()` 过；1.17 行为不同）。本仓六处「running: false + restart()」全部失效，且无头截图与既有测试都看不出来：

| 位置 | 失效行为 |
|---|---|
| now-playing `page-anim` | 评论跳转键无效（插值定时器从不启动） |
| now-playing `take-over-timer` | 歌词手动滚动后再也不恢复自动跟随 |
| queue-drawer `unmount-timer` | 抽屉关闭后永不卸载（常驻挂载，队列可滚时拦走内容区右缘点击/拖拽） |
| app `overlay-unmount-timer` | 播放页关闭后永不卸载（整棵树常驻，歌词跟随 16ms 定时器被 10Hz 状态推送反复拉起——抵消 S9 的静置零唤醒） |
| Scroll `hide-timer` | 滚动条拇指不再自动隐藏 |
| VolumeControl（借 EqClock 当时钟，S9 R3 起 EqClock 仅播放期走表） | 暂停/未播放时悬停后移开，弹层永远不收 |

**修法**：统一改成版本无关写法 `if (t.running) { t.restart(); } else { t.running = true; }` + `triggered` 内 `self.stop()`；音量宽限改真正的 Timer。回归测试 `crates/hmp-desktop/tests/soft_timers.rs`（队列/播放页软卸载两轮、评论跳转双向、歌词跟随恢复）、`volume_control.rs`。**纪律**：依赖轮升级 Slint 后必须真机过一遍定时器驱动的行为（软卸载、自动隐藏、插值滚动），这类失效不产生编译错误也不掉测试。

**M7（PORTING 清单里唯一未勾项）收口**：键盘语义、焦点可达、减少动效、窄窗断点——设计入册 DESIGN.md v0.14，实现与机制事实入册 PORTING.md「M7 键盘 / 焦点 / 减动效」与「Slint 机制坑位（1.18 增补）」。同轮修掉的既有缺陷：根作用域未关 `focus-on-click` 致键盘焦点离开后首次点击被吞；ESC 收口写在控件的兄弟作用域上；`TextLink` 无首选宽度；歌手链接命中区越界；节头链接文案与动作不符；路由切换不动滚动位置；「新建歌单」无动作；音量弹层缺 Vue 的钉住语义。

**验证**：`cargo fmt --all -- --check` 干净；`cargo clippy --workspace --all-targets --all-features -- -D warnings` 零警告；`cargo test -p hmp-desktop` 105 单测 + 19 集成二进制全绿（新增 9 个）；全仓逐 crate 见本轮提交说明；窄窗 683×800 十一页 + 播放页 + 抽屉无头截图核对。

**开放项（新）**：
- 播放页 overlay 打开时无模态焦点陷阱：Tab 仍会经过被遮盖的主界面控件。
- 弹层内部（右键菜单 / 音质选择）方向键导航未做。
- 侧栏歌单行只有行窗内的才是 Tab 停靠点。
- 歌单重命名/删除仍只有 CLI（`hmp playlist rename/delete`）；UI 的右键菜单是曲目语义，歌单级菜单需新面板。
- 窄窗设置行描述文字截断（T8，既有）。

## §30 架构与曲库两问审计（2026-11-25，用户问「前后端分离是否合理、以后扩展性如何」+「本地库与 QQ 库的结合、音频整理与艺人/专辑逻辑很奇怪」；八路只读子代理专项 + 主代理真库取证）

**范围与方法**：基线 `e24e843`（main，工作树干净），审计阶段全程只读、未运行 cargo、未改代码。拆成八路只读分工并行：IPC 契约与边界矩阵 / crate 分层与进程拓扑 / 桌面前端职责 / 曲库存储模型 / 曲库入库同步与播放 / QQ 侧实体建模 / 曲库 UI 呈现 / 文档与演进史。其中五路返回（桌面、存储、入库同步、UI、文档史），三路（IPC、分层、QQ 实体）触发模型用量上限中断、未产出，其影响结论的事实由主代理补读核实。**凡标了位置且未注明「未复核」的结论，均由主代理回代码或回真实数据核对过**；子代理独有且主代理未复核的条目标「未复核」。真库取证：把 `library.sqlite3` 连同 `-wal` 拷到 `/tmp` 查副本（daemon 常驻、WAL 未落盘；不拷 `-shm`；不读账号表），用完删除，运行中的库未被触碰。

**结论速览**

| 问 | 判定 | 评分 |
|---|---|---|
| 前后端分离 | 进程划分合理且应保留（关窗续播、系统媒体控制、CLI 遥控都需要常驻 daemon）；不合理在接缝：曲库数据不走 IPC 而由前端直连 SQLite、协议无版本号、QQ 是隐含的默认命名空间 | 3 / 5 |
| 本地库 × QQ 库 | 地基正确（两种曲目同表、歌单可混排），但模型只到「曲目行」为止：艺人/专辑是曲目上的字符串，QQ 的歌手 mid、专辑 mid 入库即丢，两库之间没有任何关联 | 2 / 5 |

**决策口径**（沿用本报告开篇原则）：明确 bug（§30.1、§30.2.4、§30.3.3）归修复轮（§30.7）；需要产品语义决策的（§30.6）只记录不动手。

### 30.1 P0：reconcile 分片 `NOT IN` 清空全部喜欢状态

- **现象**（真库）：「我喜欢」866 首，每轮 reconcile 结束后 `relations` 里 865 行 `track/liked` 被置 `desired=0`，仅 1 行因处于 `error` 态未被触碰。
- **根因**：`crates/hmp-storage/src/db.rs:1472-1485` `reconcile_remove_absent` 为绕开「SQLite 变量上限」把 `present_keys` 按 500 分片，每片执行 `UPDATE … WHERE … AND entity_key NOT IN (本片)`——片 1 清掉片 2 的行、片 2 再清掉片 1 的行，远端快照超过 500 条即该 provider 全清。`delete_playlists_absent`（`db.rs:1504-1514`）同形，收藏歌单超过 500 个时整类删除。调用点：`crates/hmp-daemon/src/reconcile.rs:203`（track liked）、`:253` 与 `:258`（playlist subscribed）、`:604`（album liked）。既有单测只覆盖 0 或 1 个 key（`db_tests.rs:29-103`）。
- **复现**：真库副本上按原样跑两条 UPDATE——866 →（片 1，500 key）500 →（片 2，366 key）**0**；正确语义（整体 `NOT EXISTS`）保持 866。据入库同步分工的 git 考，该写法自 `1cfc622`（2026-08-09）起存在（未复核）。
- **用户可见**（按代码推断，未截图）：桌面红心取 `relation_desired`（`crates/hmp-desktop/src/player_bridge.rs:262-279`）全灭；「我喜欢」页取 `list_favorites`（`library_view.rs:207-243` → `db.rs:1278-1296` → `ui/library-page.slint:105-108`）只剩 error 态 1 行；侧栏「我喜欢」歌单走 `playlist_tracks` 仍 866 首——同一个「我喜欢」两处数量不同。对已收藏曲目点红心会向 QQ 多发一次 `like_song`；真库中《追风之翼 The Wind Chaser》（`002tqi6Z1Lszqc`）正卡在 `desired=1 / remote=0 / error`、`retry_count=27`、`QQ Music API error 1101` 的退避重试里（每 10 分钟一次、无终态）——是否由此引起未证实。
- **修法**：整体 `NOT EXISTS`（临时表或 `json_each`），或改成「先取本地 synced 行、在 Rust 侧按 present 集合求差、只对缺席 key 正向更新」；两处同改，补 501+ key 回归测试。注释「SQLite 变量上限 999」已过时：内置 3.53.2（libsqlite3-sys 0.38.2）的默认上限是 32766。
- **同一函数的相邻脆弱点**：`present_keys` 为空即全量清理（`db.rs:1461-1470`、`1497-1502`），而 `UserApi::get_fav_song`（`crates/hmp-qqmusic-api/src/user.rs:229`）在响应缺 `data` 时 `unwrap_or(json!({}))` 解析成空列表而不是报错，响应结构体字段又全是 `#[serde(default)]`、`data.code` 不检查——QQ 风控或截断造成的「成功但空」会清空本地 QQ 收藏并删掉订阅歌单行（下次成功可恢复）。
- **自愈**：代码修复后不需要手工修数据——下一轮 reconcile 的第一阶段（`reconcile_relation(..., true)`）会把在场曲目重新置 `desired=1`。

### 30.2 问题 2：本地库 × QQ 库

#### 30.2.1 根因三层

| 层 | 现状 | 证据 |
|---|---|---|
| 身份 | QQ 的 `Song` 模型带 `singer[].mid` 与 `album.mid`（`crates/hmp-qqmusic-api/src/models.rs:227-309`），三条入库路径却只留名字：歌手拼成一个字符串、专辑只存名；艺人/专辑没有任何表（`hmp_core::Album`/`Playlist` 是生产零引用的空壳） | `reconcile.rs:540-558` `qq_track_row`；`crates/hmp-daemon/src/engine.rs:1662-1682` `track_row`；`engine.rs:1685-1702` `stub_row`；`crates/hmp-core/src/media.rs:205-216` `TrackStub { artists: Vec<String>, album: Option<String> }` |
| 视图 | 曲库浏览的五个查询全部 `JOIN local_files … source='local'`，只覆盖本地文件；专辑按名分组、专辑歌手取 `MAX(artist)`；`album_artist`、`disc_number` 落库但零读取 | `db.rs:1025-1087` `library_tracks`；`:1090-1116` `library_albums`；`:1119-1135` `library_artists`；`:1138-1159` `local_tracks_by_album`（`:1143` 排序不含碟号）；`:1162-1188` `local_tracks_by_artist`；桌面详情页同口径 `crates/hmp-desktop/src/library_view.rs:489-602` |
| 关联 | 本地文件与 QQ 曲目之间没有任何关联表或匹配落库；唯一交集是歌词兜底，匹配结果用完即弃 | `crates/hmp-daemon/src/content.rs:237-288` `local_lyric_with_fallback` / `search_lyric_mid`；全仓无 link 表 |

#### 30.2.2 真库取证（4121 首曲目 = QQ 4109 + 本地 12；23 个歌单；`user_version = 8`）

| 事实 | 数据 |
|---|---|
| 曲目身份 | QQ = 裸 songmid；本地 = `local:<canonical 绝对路径>`（移动即换身份）；`playback_state.json` 里的队列是 866 个裸字符串 |
| 扫描根 | `/mnt/d/QQMusicDownloads`（QQ 音乐下载目录）——12 个本地文件里有 9 个在 QQ 侧有同名曲目（如本地 #32《希望有羽毛和翅膀》↔ QQ #54），两边各一条、喜欢与播放次数各记各的 |
| 艺人 | QQ 曲目 758/4109 的 `artist` 含逗号（`", "` 拼串）；`track_artists` 仅 12 行且全是本地行（每首一行未拆分的标签原文）；本地 #31 艺人 = `张韶涵/HOYO-MiX`、#32 = `知更鸟/HOYO-MiX/Chevy`；同一首歌 QQ 侧写作 `知更鸟, HOYO-MiX, Chevy` |
| 专辑 | 1569 个专辑名对应 1650 个（专辑, 艺人串）组合——如「原神-闪耀的群星6 The Stellar Moments Vol. 6」对应 5 个艺人串；专辑 mid 全库未存 |
| `album_artist` / `year` | `album_artist` 4121 行零填充；本地 `year` 12 行全空（这批文件本身没有 `DATE` 标签） |
| 曲库页实际呈现（按查询回放） | 专辑视图 10 张（8 张只有一首）；「China Philharmonic Orchestra (K2HD)」显示歌手「鲍元恺」（实为吕其明、鲍元恺各一首）。歌手视图 8 个，含两个合成串与一个乱码串；HOYO-MiX 计 4 首（实际 6 首） |
| 喜欢 | `relations` 866 行（865 行 `desired=0`，见 §30.1）；「我喜欢」歌单行 `remote_id=201`（owned）866 首 |
| 歌单 | QQ 22 个（owned 10 + subscribed 12）+ 本地 1 个「测试歌单」 |
| 测试残留 | `tracks` id 20/36/38 为合成行（key 为空串、`1`、`so001`，标题同 key） |
| 源文件损坏 | 本地 #33 的标签本身就是坏的（title/artist/album 为「`U+FF00` + 字节」形态，`metaflac` 原始字节 `ef bf a7 …`），下载器写入所致，不是 HMP 解码问题；HMP 无修正手段（无标签编辑、无手动关联 QQ 曲目） |
| 缓存 | 歌词盘缓存只按 QQ mid（`<mid>.lrc`）；封面内容寻址落 `<data_dir>/covers`（598 文件 21M），`cover_cache` 表只覆盖 QQ URL |

#### 30.2.3 具体表现

1. **多艺人三种写法**：播放路径 `track_row` 用 `" / "`（`crates/hmp-core/src/media.rs:256-262` `artist_names`），列表缓存 `stub_row` 与 reconcile 的 `qq_track_row` 用 `", "`，本地行是标签原文（`crates/hmp-storage/src/local.rs:78-84,108`：逐帧写入 `track_artists`，不做字符串拆分；lofty 0.25.4 的 ID3v2.4 只按 `\0` 切多值，`A/B`、`A; B`、`feat.` 整体算一个艺人）。同一行谁后写谁赢（`db.rs:337` 的 `COALESCE`）。
2. **专辑按名字分、歌手取字典序最大者**（`db.rs:1092-1101`）；多碟曲序交错（`db.rs:1143`）；无「未知专辑/未知艺人」桶（`db.rs:1094`）；同名专辑共用封面映射（`library_view.rs:197-205`，未复核）。
3. **点艺人/专辑是死路或错路**：播放条桥接只写展示名、丢掉 `ArtistRef.id`/`AlbumRef.id`（`player_bridge.rs:1101-1112`），`ui/now-playing.slint:525,533` 以展示名导航；艺人页先拿整串精确匹配 `track_artists`，必空后退「最长包含匹配」（`library_view.rs:524-546`）——点「知更鸟 / HOYO-MiX / Chevy」落到 HOYO-MiX 的本地页，QQ 曲目的歌手直接「未找到」；库内 QQ 行的 `artist_mid`/`album_mid` 填的也是展示名（`library_view.rs:748-750`，两路分工一致，未复核）；`Nav.param` 一个字符串挤了四个命名空间（数字 DB id、展示名、`qq:`+mid、榜单 id；`ui/nav.slint:24-36`）。
4. **「我喜欢」有两份表示**：`relations(track, qq, liked)` 与 owned 歌单行（`reconcile.rs:36` `FAV_SONG_DIRID = "201"`）；侧栏「自建歌单」不过滤该行（`library_view.rs:369-387`，未复核）。
5. **本地曲目进不了 QQ 自建歌单**：`crates/hmp-daemon/src/server.rs:385-387` 拒绝，但 `InvalidQuery` 被统一映射成「remote QQ playlists are read-only…」（`server.rs:456-459`）；UI 侧预判禁用并注「不支持本地曲」（`ui/context-menu.slint:154-156`）。HMP 自建歌单（`relation='local'`）永不上云（`playlists_pending` 过滤、`sync_playlist` 的 create 分支不可达，未复核）。
6. **喜欢/取消喜欢只有播放页一个入口**：右键菜单只有「播放 / 下一首播放 / 加入队列 / 加入播放列表… / 从队列移除 / 从歌单移除 / 查看歌手 / 查看专辑」（`ui/context-menu.slint:167-244`）。
7. **列表看不出本地还是 QQ、搜索不搜本地**：`TrackRow.source` 有值但 UI 不渲染；搜索只发 `Request::Search`（未复核）。
8. **桌面端没有专辑/歌手/流派浏览入口**，「音乐库」是一张按标题码点序的平表；排序全是 SQLite BINARY 序（`db.rs:1068,1126,1172`；`ui/nav.slint:4-22`，未复核）。
9. **歌单三套互斥语义**：本地歌单可混排但不上云；owned 拒本地曲；subscribed 只读。远端已删的自建歌单本地不删（`reconcile.rs:258` 只清 subscribed）。

#### 30.2.4 确定性缺陷

修复轮范围内的条目：

| # | 缺陷 | 位置 |
|---|---|---|
| D1 | 分片 `NOT IN` 全清（§30.1） | `db.rs:1472-1485`、`1504-1514` |
| D2 | 空快照被当成权威空；软失败被解析成空列表 | `db.rs:1461-1470`；`user.rs:229` |
| D3 | 移动或改名本地文件后「喜欢」失联：指纹命中时只改 `tracks.source_key`，`relations.entity_key` 仍是旧路径，而 `list_favorites` 按 `source_key = entity_key` 内连接 | `db.rs:767-779` 对 `:1278-1284` |
| D4 | 带标准日期标签的本地文件年份恒空：读的是 `ItemKey::Year`，而 lofty 0.25 把 `DATE`/`TDRC`/`©day`/`ICRD` 映射为 `RecordingDate`（`f6c073c` 依赖轮引入）；即使读对键，`"2020-05-01".parse::<i64>()` 也会失败 | `local.rs:123-125`；lofty `src/tag/item.rs:206,307,368,420-421` |
| D5 | 歌单曲目补抓没有游标：预算 24、每轮从第 0 行开始，注释里的「下一轮续跑」并未实现；QQ 歌单超过 24 个后排在后面的永远补不到曲目（真库 22 个） | `reconcile.rs:30`、`:384-396` |
| D6 | 批量 upsert 无条件 `title = excluded.title`（单条版有「标题等于 key 时不覆盖」的保护），而 stub 的标题就是 mid → 入队未播的库内 QQ 曲目标题被打回 mid，靠 `repair_stub_metadata` 自愈 | `db.rs:499-506` 对 `:331-335` |
| D7 | 「最近播放」不随播放更新：引擎写 `play_events` 后不发 `LibraryChanged`（bump 点只有 `server.rs:319,441,449` 与 `sync.rs:90,97,113`），导航到该页也不重读。上限 12 条与 `format_stamp` 的 UTC 口径是有意取舍（与 `hmp history` 输出一致、不引入时区依赖，`library_view.rs:709-718`），不算缺陷 | `engine.rs:1239-1254`；`library_view.rs:28`；`bridge.rs:796-837` |
| D8 | 任何库刷新都重置音乐库的文件夹过滤：`apply_snapshot` 无条件 `set_local_tracks(全部)`，不重放当前选中的文件夹 | `bridge.rs:660-668` 对 `:698-727` |
| D9 | 本地封面只认内嵌图，不探测目录封面（全仓没有 `folder.jpg`/`cover.jpg` 探测） | `local.rs:87-102` |
| D10 | 本地专辑曲序忽略碟号；专辑歌手不使用 `album_artist` | `db.rs:1143`、`:1092` |

只记录、不在修复轮的条目：

- **身份就是路径**：播放过但未扫描的文件会永久入库且没有 `scan_root_id`/generation（即 §4.3 的二等行）；非 UTF-8 文件名经 `Path::display()` 有损转写（未复核）；生产代码没有任何 `DELETE FROM tracks/local_files`，也没有「移除扫描根」。
- **reconcile 没有定时**：只在 daemon 启动、`LibrarySync`、扫码登录成功时触发；手机上的收藏变化要等重启或手动 sync。
- **「我喜欢」只写关系不建曲目行**（`reconcile.rs:186-195`，未复核）——即 §4.2 的 INNER JOIN 议题；真库当前 0 行孤儿。
- **`playlists.remote_id` 一列两义**：owned 存 dirid、subscribed 存 disstid，tid 不落库（`db.rs:149-150`）；`resolve_source_ids_impl` 把 `PlayRequest::Playlist(id)` 的 id 原样当远端歌单号去取详情（`crates/hmp-daemon/src/player.rs:524-536`）。这是 §27 另案「`hmp play playlist:<本地主键>` 恒报 empty」的模型侧线索，根因仍未证实（见 §30.8）。
- **缺失文件治理**：`lf.missing` 读出后未消费，`SongRow` 没有 missing 字段（未复核）；`PlayList` 快路径不看 missing。
- **阻塞 I/O 持库锁**：watcher 持库锁期间读指纹、标签、封面；`resolve_local`/`local_stub` 在 async 里同步读盘（未复核）。
- **元数据要客户端回传**：`LyricGet`/`Favorite`/`AddTrack` 都要求前端提供 title，CLI 传的是 `title: id`（未复核）。
- **SyncWorker 空转**：每 30 秒先读密钥环再查 pending（`sync.rs:85,119-121`）。
- **指纹**：`DefaultHasher` 不保证跨 std 版本稳定，只取前 1MB（未复核）；没有「标签解析版本号」，修了 D4 这类解析规则后存量行不会自动刷新。
- **`sync_relation` 不检查 provider**：本地行若意外 pending 会永久 error 重试——生产路径不可达（daemon 对本地来源立即 `mark_relation_synced`，`server.rs:306-312`；`LibraryDb::add_favorite` 仅测试使用）。
- **约束与清理**：`playlist_tracks` 没有 UNIQUE；`play_events` 不裁剪；封面/歌词盘缓存不淘汰；内嵌 PNG 也存成 `.jpg`（未复核）。

#### 30.2.5 目标模型（建议，待 §30.6 决策）

缺的是「一首歌」与「它的一个音源」之间的中间层：

```text
tracks(id)                                    一首歌（保留现有整数主键）
track_sources(track_id, provider, key, …)     QQ: mid + song_id    本地: 路径 / 指纹
artists(id, name, sort_key)   + artist_keys(artist_id, provider, provider_key)
albums(id, title, year, album_artist_id) + album_keys(album_id, provider, provider_key)
track_artists(track_id, artist_id, position)   现有表把 artist 文本换成 artist_id
```

- 本地文件与 QQ 曲目挂在同一首歌下即「关联」，不需要独立的 link 表（自动匹配的来源与置信度记在 `track_sources` 行上）。
- 喜欢、歌单、播放历史都挂在 `tracks.id` 上，两源自动共享；同时根治 D3 与「我喜欢两份」。
- 播放时选一个可用音源（本地优先、失败回退 QQ）；歌词、封面、评论经关联取 mid。
- 艺人页、专辑页可以同时列出本地与 QQ 的内容。
- 匹配器不必从零做：`search_lyric_mid` + `pick_best_song` 已有「标题 + 歌手检索 → 评分 → mid」，补时长与专辑即可；结果落库，另加手动「关联到 QQ 曲目」。

#### 30.2.6 分阶段迁移（每步只加列、可回滚；人日为粗估）

| 阶段 | 内容 | 粗估 |
|---|---|---|
| 0 止血 | §30.2.4 的 D1–D10 与 §30.3.3 的桌面缺陷，各补回归测试 | 2–3 人日 |
| 1 停止丢信息 | 入库保存专辑 mid 与歌手列表（含 mid），统一分隔符，专辑按（专辑名, 专辑歌手）分组，`tag_version` 触发存量重扫 | 3–5 人日 |
| 2 实体化 | `artists`/`albums` 表与 key 表，`track_artists.artist_id`；桌面改用 mid 导航 | 5–8 人日 |
| 3 曲目与音源拆分 | `track_sources`，自动关联 + 手动关联，本地优先，集合化的「我喜欢」 | 5–8 人日 |
| 4 读路径收编 | 见 §30.3.5 | 随问题 1 |

每一步都动 schema：按 AGENTS.md 重建全部链接 daemon 的二进制并 `hmp quit` 实测。

### 30.3 问题 1：前后端分离

#### 30.3.1 判定与保留项

保留：常驻 daemon 独占播放引擎、SQLite、MPRIS/SMTC、托盘（MPRIS 是进程内适配器，直接复用 `Request`/`DaemonState`：`crates/hmp-daemon/src/mpris.rs:6-17`）；播放状态单一来源；UI 只发意图（桌面端 0 个文件引用 `hmp_qqmusic_api`，不碰凭证）；`relations` 表兼作 durable outbox（本地先提交、pending 胜、先推后拉、指数退避 `min(2^n·10s, 10min)`）；迁移逐级事务 + `user_version`；扫描的 mtime+size 增量、指纹移动复用、`missing` 标记不删行；封面内容寻址 + 派生缩略图。

#### 30.3.2 接缝问题

| # | 问题 | 证据 |
|---|---|---|
| A1 | **曲库数据面不走 IPC**：`Request` 的 37 个变体里没有任何曲库读请求，只有 `LibrarySync` 触发与无载荷的 `LibraryChanged`；前端直连 SQLite。这是成文决定（库内核 spec §3.3「写走 daemon，读直读本地 DB」+ 本报告 §8.1「契约已成立，不为读路径新增 IPC」），但与 PROJECT §10.3「UI 不直接执行 SQLite」冲突 | `crates/hmp-core/src/ipc.rs:114-285`、`:344-355`；桌面 3 处开库（`library_view.rs:132,152`、`backend.rs:621` 队列投影、`player_bridge.rs:267` 心形回读）；CLI 10 个文件引用 `hmp_storage` |
| A2 | **「只读直库」并不只读**：`LibraryDb::open` 会建目录、设 WAL、跑迁移；桌面 `load_data` 早于 daemon 指纹核对。`library_view.rs:5`「daemon 是唯一写方」不成立 | `db.rs:285-297`；`crates/hmp-desktop/src/main.rs:27`；`backend.rs:182-219`（未复核） |
| A3 | **写方不止 daemon**：`hmp scan` 由 CLI 直写；引擎播放路径的 `upsert_track`/`record_play_start` 与 `resolve_local` 也写库 | `crates/hmp-cli/src/scan.rs:107-110`（未复核）；`engine.rs:1239-1254` |
| A4 | **没有协议版本号**：兼容只靠「新字段一律 `serde(default)`」加构建指纹；新增枚举变体对旧 daemon 仍是破坏性变更；单帧上限 1 MiB，`PlayList` 把整表 id 塞进一帧（约万首本地曲目时临界的估算未实测） | `ipc.rs:1-6`、`:15`；`player_bridge.rs:522-565`（未复核）。变体数 10（08-08）→ 37（10-23），`ipc.rs` 204 → 1346 行（文档史分工） |
| A5 | **状态推送没有增量**：每次 `state_rx.changed()` 都克隆整包 `DaemonState`；队列的 `revision` 在换曲、切循环、切随机时也自增，桌面每换一曲整包重拉并重建队列 | `crates/hmp-daemon/src/server.rs:202-205`；`crates/hmp-core/src/queue.rs:349,381,389,458`；`backend.rs:508-515`（注释称仅结构变化才重拉） |
| A6 | **QQ 是隐含的默认命名空间**：`TrackProvider::from_id` 非空 `local:` 前缀才判本地，否则一律 QQ；`source` 是自由字符串；`PlayRequest::Track` 与 `::Local` 语义重叠；`TrackRef` 生产零引用 | `ipc.rs:19-30`、`:49-110`、`:150-159`、`:408-417` |
| A7 | **`hmp-core` 混装**：领域模型、播放命令、队列逻辑（984 行）、带 QQ 形态的内容 DTO（`SearchSong`/`TopSongDto`/`CommentPage`/`LoginQr*`/`RemoteArtistDetail`…）同处一个「不得依赖具体 QQ 接口字段」的 crate | `crates/hmp-core/src/lib.rs`、`ipc.rs:439-760`；与 PROJECT §5.2 不符 |
| A8 | **前端为几个符号链接整个 daemon crate**：桌面只用 `BUILD_CODE`、`server::socket_path`、`serve::spawn_detached_exe`、`transport::IpcStream`、`IpcListener`；另有 `reqwest`、`hmp-qqmusic-api`、`tokio-util`、`tracing-subscriber` 四个依赖在 src 里零引用 | `backend.rs:16,129,138,158,161,190,206,273,724,813,846`；`crates/hmp-desktop/Cargo.toml`。CLI 用 6 个符号（含 `run_foreground`/`run_background`）——`hmp serve` 就跑在 CLI 进程里，属固有依赖 |
| A9 | **CLI 自己出网**：`hmp search`/`account`/`login` 直接创建 `QqMusicClient`（并直读凭证），绕过 daemon | `crates/hmp-cli/src/search.rs:10`、`account.rs`、`login.rs` |
| A10 | **依赖倒置**：`hmp-storage` 依赖整个 `hmp-qqmusic-api` 只为再导出 `Credential` 类型；`hmp-media` 依赖 `hmp-storage` 只为 `cache_dir`；`hmp-qqmusic-api` 又 dev-depend `hmp-storage` | `crates/hmp-storage/src/credential.rs:4`；`crates/hmp-media/src`（2 处 `hmp_storage::cache_dir`） |
| A11 | **构建指纹范围过宽**：哈希集合含 daemon 并不链接的 `hmp-cli`、`hmp-desktop` | `crates/hmp-daemon/build.rs:34-44` |

#### 30.3.3 桌面端缺陷

| # | 缺陷 | 位置 |
|---|---|---|
| U1 | 歌词一次失败后本会话永不重试：`REQUESTED` 只 insert 不 remove；文档注释「换曲再回来时自然重试」与实现相反 | `player_bridge.rs:1152-1157` |
| U2 | 封面「失败出账 + 指数退避」有两处从未生效：`REQUESTED`/`COVER_RETRY` 是 `thread_local`，入账在 UI 线程、出账与退避记账在 tokio 工作线程（操作的是另一个线程的空集合）。`online_covers.rs:16-19` 已改全局 Mutex；§23 称修了三处，实收一处（据桌面分工的 git 考：`ba07870` 引入、`97e3d8f` 只修一处，未复核） | `player_bridge.rs:1321-1325`、`:1418-1425`、`:1472-1478`；`track_covers.rs:31-34,105` |
| U3 | 桌面端不初始化 tracing subscriber，`warn`/`error` 运行时全是空操作 | `crates/hmp-desktop/src/main.rs`（全工作区只有 `crates/hmp-cli/src/main.rs:365-370` 初始化） |
| U4 | 音质徽章显示的是前端推算值：文案取 `Quality.tier-label(Quality.effective(Player.track-max-tier))`，而 `track-max-tier` 就是本次实际档；daemon 给的 `track-quality` 无人读，`Track.available_qualities` 未使用；降档后弹层把更高档位标成「本曲未提供」 | `ui/quality-badge.slint:30,79,103,122`；`ui/stores.slint:32,34`；`player_bridge.rs:1115-1129` |
| U5 | 删除歌单曲目按界面下标、没有 key 校验（快照过期即删错行，owned 歌单还会同步到远端） | `menu_bridge.rs:192-203` → `server.rs:408-433` → `db.rs:2016-2022` |
| U6 | QQ 曲的歌手/专辑链接用展示名导航（见 §30.2.3 第 3 条） | `player_bridge.rs:1101-1112`；`ui/now-playing.slint:525,533` |
| U7 | 库页数据层全量、无分页：每次快照对每个本地文件 `fs::metadata`、重建 7 个模型；首帧与导航在 UI 线程同步读库 | `library_view.rs:158-189,334`；`bridge.rs:565,608-742,753`（未复核） |
| U8 | 图片解码在 UI 线程（PROJECT §10.4 明文禁止）；SQLite 与 stat 在 async 任务里同步执行 | `cover_cache.rs:139-167`；`bridge.rs:72-95,591-602`；`backend.rs:574`（未复核） |
| U9 | 离线/陈旧构建的修复指引只显示 4 秒，没有持久横幅与重连入口 | `backend.rs:249-260`；`player_bridge.rs:60,798-801`（未复核） |
| U10 | 没有无头 view-model：编排长在 Slint Global 上，约 29 个模块级可变全局；更换 GUI 框架时约 7500 行非测试代码里约 2800 行可复用 | `player_bridge.rs:73-1419`、`bridge.rs:53-575` 等（计数未复核） |
| U11 | 状态流核心没有自动化测试；15 个集成测试经 `bridge::load_data` 直读真实用户库、无 XDG 隔离；唯一 E2E `tests/live_daemon.rs` 为 `#[ignore]` | `crates/hmp-desktop/tests/`（未复核） |
| U12 | stringly-typed：`Favorite`/`AddTrack` 的 `source`、`sort`（`"hot"`/`"new"`）、`QualitySet.mode` 别名串、`Response::Cover` 两种 `file://` 形态、未找到态靠 `message.contains("not found")` 判定 | `ipc.rs:150-179,408-417`；`remote_details.rs:85-87`（未复核） |
| U13 | 在线内容全是首窗：`TopDetailGet { num: 100, page: 1 }`，`discover_has_more`/`search_count` 置位后无人读；`GuessGet` 处理有两份 | `bridge.rs:298-333,1326-1443`（未复核） |
| U14 | 重复实现：队列元数据投影（`backend.rs:619` 对 `crates/hmp-cli/src/commands.rs:365`）、`provider_of`、时间格式化、帧读写与超时——CLI 与桌面各一份，没有共享客户端（请求超时补了两次） | 未复核 |
| U15 | 界面层：右键菜单不按上下文裁剪（远端歌单页也显示「从歌单移除」，点击静默返回）；「我喜欢」页歌单网格不换行；扫描/添加文件夹按钮永久禁用（IPC 无扫描请求）；下载/已购是永久空页 | `ui/track-table.slint:272-273`、`menu_bridge.rs:189-191`、`ui/library-page.slint:148-160`、`bridge.rs:732-741`（未复核） |

#### 30.3.4 扩展性

| 场景 | 现状 | 主要阻力 |
|---|---|---|
| 再接一个前端（脚本、移动端、网页遥控） | 勉强可行 | 协议只有代码没有 spec；没有版本号；读不到曲库数据（必须直连 SQLite）；`MAX_FRAME` 1 MiB |
| 再接一个在线音源 | 很难 | QQ 是默认命名空间；QQ 调用散在 daemon 多个模块（content/server/player/engine/sync/reconcile/login/comment）；没有 provider trait |
| 纯本地离线 | 基本能用 | 本地源已免登录；曲库浏览很弱 |
| 加一个新领域功能 | 散弹式修改 | 请求与响应各加变体 + daemon 处理 + CLI/桌面各一份桥接；仅桌面消费的端点约 14 个（计数未复核） |
| 更换 GUI 框架（PROJECT §3.2 声称保留） | 成本高 | 见 U10 |

#### 30.3.5 目标形态与建议（按性价比排序）

1. **协议加版本握手**：连接时交换 `Hello { proto, build, caps }`，不兼容就明确报错；`serde(default)` 只用于加字段，弃用走版本。
2. **前端连接改真只读**：单独的 `open_read_only()`（不迁移、不建目录），迁移只由 daemon 执行。
3. **扫描收进 daemon**：`Request::LibraryScan` + 进度事件，CLI 变薄客户端，daemon 成为唯一写方；`LibraryChanged { scope, generation }`。
4. **抽 `hmp-client` 小 crate**（传输、socket 路径、拉起、指纹），桌面不再链接 daemon crate；清掉零引用依赖。
5. **拆 `hmp-core`**：线协议与版本 / 领域模型 / 队列逻辑分离。
6. **音源显式化**：`{ provider, key }` 取代前缀判断；daemon 内用 `MusicProvider` trait 收拢 QQ 与本地的解析、歌词、封面、实体页。
7. **曲库读路径分两步**：先保持直读（只读化即可），出现第三个前端或全量重查成为性能问题后，再加带分页的读请求。

需要改的决定只有一条：「读直连 SQLite」从「契约」降级为「临时方案」。其余都是在现有骨架上补接缝，不需要推倒重来。

### 30.4 对既有登记的纠错

- **§4.2 `list_favorites` INNER JOIN**：「我喜欢」不全、红心全灭的现行主因是 §30.1，不是 INNER JOIN——真库当前没有「有关系无曲目」的孤儿行。两个议题相互独立，INNER JOIN 仍未决策。
- **§8.2**「`hmp search`/`hmp account` 直连旁路同步收编」不实（见 A9）。
- **§16 开放项**里两条「仍开放」实际早已落地：BUSY 预算 → `PIPE_STANDBY = 4` + `CONNECT_BUDGET = 5s`（`crates/hmp-daemon/src/transport.rs:65,92`）；预解析总超时 → `PRELOAD_RESOLVE_TIMEOUT = 15s`（`engine.rs:30,1510`）。真正剩余的只有 §4 残余。
- **§5 歌手/专辑两行**「无任何前端入口」已过时：桌面远端歌手/专辑页已随 S1 落地；CLI 仍没有 `hmp artist`/`hmp album`（未复核）。
- **§27 另案**（owned 歌单远端播放恒 empty）根因仍未证实，文档内存在互斥归因（§27「疑似 QQ 侧接口/风控变化」对 USAGE §4.1「dirid≠disstid」）；模型侧线索见 §30.2.4。
- **PROJECT.md 漂移**（文档史分工，未逐条复核）：§4.1 架构图仍是单体（Slint UI → AppCore）；§5.2 称桌面端是「主程序」并负责单实例与通知；§9.4 的 MPRIS 注册方与「CLI 无队列语义」不符；§11 写 `hmp.db` 并列出从未创建的 `artists`/`albums`/`schema_migrations`/`lyric_cache` 表（实际是 `library.sqlite3` 加 `user_version`）；§2.3「多音源聚合」非目标与 08-08 加入的本地 provider 没有调和；§2.1「不希望依赖额外守护进程」与 §8.6 自相矛盾。
- **DESIGN.md** 仍是 Web 时代的契约（「Web 前端的唯一设计基准」、进程内 HTTP 服务）；**AUDIT.md 没有 §9–§13**，「§4.4」一词两义。
- **`library_view.rs:5-14` 模块文档**（「daemon 是唯一写方」「启动时静态快照」）与 **PORTING.md:182,320** 已过时。

### 30.5 演进史要点（为什么长成这样；文档史分工）

- 08-06 PROJECT.md 定稿时是单体（进程内 AppCore），本地库不在项目章程里；08-08 为 CLI 遥控设计并当天落地 daemon（`Request` 10 个变体），同日 SQLite 媒体库 v1 带 provider 模型与 `hmp scan`——本地库是当时审计清单的副产物。
- 08-09 单日 76 个提交定下绝大多数基础决定（`album TEXT`/`artist TEXT`、按名聚合、owned 存 dirid、「写走 daemon、读直读本地 DB」），全部出自计划文档的「决策定案」，而不是产品需求。
- 09-07 起桌面端作为「又一个适配器」接入，spec 明写的「后端不感知具体前端」「删掉任何一个不碰后端一行」随后失效：封面下载、扫码登录、音质写入、远端详情都是桌面专属请求。
- 已不成立的前提：daemon 独占 QQ API 与凭证（A9）、写走 daemon（A3）、「SQLite 主要是缓存」（它现在是本地意图与本地歌单的事实源）。

### 30.6 待决策（产品语义，只记录不动手）

1. 「我喜欢」是否只保留 `relations` 一份（倾向是），不再展示 `remote_id = 201` 的 owned 歌单行。
2. 同一首歌两边都有时的播放偏好：本地优先（倾向）还是 QQ 优先。
3. QQ 自建歌单是否允许加入本地曲目（仅本地附加、不上云）。
4. 「读直连 SQLite」是否降级为临时方案（§30.3.5 第 7 条）。
5. 本地艺人的拆分规则（`/`、`;`、`，`、`feat.`、`&`）与是否保留原串。
6. 空快照的清理策略（连续两轮一致才清，还是首轮为空即跳过）。
7. 同步错误是否设终态（`error` 行当前每 10 分钟无限重试）。
8. 最近播放的上限（现为 12）与是否分页。
9. 既有遗留（从未决策）：§4.1 V1 footer、§4.8 桌面端文案语言、§4.9 登录超时按文案判定、下载/已购域、HTTP 遥控与「非目标」的矛盾。

### 30.7 修复轮（2026-11-25 晚起）

范围限于明确 bug，不含架构重构（§30.3.5）与产品决策（§30.6）。三路子代理按 crate 所有权切分，各自使用独立 worktree 与 target（AGENTS.md「多 worktree 各自持有 target/」），不提交、不触碰运行中的 daemon；完成后由主代理合入主工作树，跑全仓逐 crate 验证，重建全部二进制并真机实测。

| 分工 | 文件所有权 | 条目 |
|---|---|---|
| 存储 | `crates/hmp-storage/src/`（`db.rs`、`local.rs`、`db_tests.rs`） | D1、D3、D4、D6、D9、D10 |
| daemon | `crates/hmp-daemon/src/`、`crates/hmp-core/src/ipc.rs`、`crates/hmp-qqmusic-api/src/user.rs`、`crates/hmp-desktop/src/menu_bridge.rs` | D2、D5、U5、歌单写错误文案 |
| 桌面 | `crates/hmp-desktop/`（`player_bridge.rs`、`track_covers.rs`、`bridge.rs`、`library_view.rs`、`main.rs`、`ui/`） | U1、U2、U3、U6、D7、D8 |

**任务书**：`docs/plans/2026-11-25-F1-fix-storage.md`、`2026-11-25-F2-fix-daemon.md`、`2026-11-25-F3-fix-desktop.md`（含逐项的修法要求、回归测试要求与各自的验证命令）。

**基线**（HEAD `e24e843`，三个全新 worktree 各自独立 `target/`，未合入任何修复）：clippy 零警告；测试全绿——hmp-storage 73、hmp-core 72、hmp-daemon 253、hmp-qqmusic-api 251、hmp-desktop 123。明细与原始输出见 `docs/verify/2026-11-25-fix-round/`。

**顺带发现（修复轮期间，主代理直接修）——Linux MPRIS 不在任何按文档命令构建的二进制里**：`hmp-daemon/mpris` 是可选 feature（`crates/hmp-daemon/src/serve.rs:195-201,230` 的 `#[cfg(feature = "mpris")]`）。它本是默认 feature（58a0964 引入 daemon 时 `default = ["tray", "mpris"]`），94c2c95（08-24）改成 `default = []`，此后没有任何依赖方打开它（60718b3 又清掉了 desktop 对 hmp-mpris 的直依赖）：`hmp-cli` 只开 `tray`、`hmp-desktop` 不开任何 feature——`cargo tree -p hmp-cli -p hmp-desktop -e features -i hmp-daemon` 只解析出 `default` 与 `tray`。实测旧发布二进制 `target/release/hmp` 与 `hmp-desktop` 里 `org.mpris.MediaPlayer2.hmp` 的字符串均为 0 个（托盘 ksni 相关 389 个），`playerctl -l` 里没有 hmp。AGENTS.md 的 clippy 用 `--all-features`，每次都过，掩盖了「检查的不是发布的那份」。

修法：`crates/hmp-cli/Cargo.toml` 增加 `[target.'cfg(not(windows))'.dependencies] hmp-daemon = { path = "../hmp-daemon", features = ["mpris"] }`——daemon 跑在 `hmp serve`（本 crate）进程里，系统媒体控制必须在这里打开；Windows 走 SMTC（`cfg(windows)` 模块，不受 feature 门控），零影响。`cargo tree` 确认 `hmp` 现带 `tray` + `mpris`，`cargo check -p hmp-cli --locked --offline` 通过，`Cargo.lock` 未变。真机验证（重启 daemon 后 `playerctl -l` 应出现 hmp）待补。

**结果（2026-11-25 夜）**：三路全部交回并合入，按缺陷拆成原子提交（每个单独过 clippy 与该 crate 的测试）；主代理在审阅时另补 4 处。合入态（`4842a1f`）全仓验证全过：`cargo fmt --check`、clippy（all-targets, all-features）、`cargo test --workspace`、`cargo doc --no-deps`（`RUSTDOCFLAGS=-D warnings`，与 CI `test` job 末步同款）。真机实测见本节末。

| 提交 | 内容 | 测试增量 |
|---|---|---|
| `d68e37b` | `hmp-cli` 打开 `hmp-daemon/mpris`（上文顺带发现） | — |
| `b986a40` | D1（P0）缺席清理整体排除 | hmp-storage +3 |
| `714b301` | D6 批量 upsert 标题保护 | +1 |
| `562304a` | D3 移动/改名后「喜欢」搬到新键 | +2 |
| `fdd7ed7` | D10 专辑曲序按碟号、专辑歌手优先 album_artist | +1 |
| `3b7ce68` | D4 年份读 RecordingDate | +3 |
| `18c8ccd` | D9 无内嵌图时取目录封面 | +2 |
| `76fd813` | 上项的测试适配不区分大小写的文件系统（主代理） | — |
| `1c56c34` | D2 接口软失败报错 / 空快照闸门 / 收口卡死的 error 行；D5 补抓游标 | hmp-qqmusic-api +4，hmp-daemon +10 |
| `1fecaf7` | 半份快照不定缺席（主代理，见下） | hmp-daemon +2 |
| `4687686` | 歌单写拒绝按因区分；U5 移除曲目带 `expect_key` 校验 | hmp-core +1，hmp-daemon +2 |
| `ee1c8ff` | U1 歌词重试、U2 封面账本进程级 | hmp-desktop +3 |
| `0d5c813` | 封面按内容识别格式（主代理，见下） | +1 |
| `5816981` | U6 播放页歌手/专辑按 mid 导航 | +1 |
| `542caed` | D7 进「最近播放」页时后台重读 | +2 |
| `6da7c8c` | D8 文件夹过滤在库刷新后保持 | +1 |
| `4842a1f` | U3 启动日志落盘 | +1 |

合计：hmp-storage 73 → 85，hmp-core 72 → 73，hmp-daemon 253 → 267（lib 166 → 180；逐包编译时不带托盘 feature，全仓一起编译时另有 1 条门控测试），hmp-qqmusic-api 251 → 255，hmp-desktop 123 → 132（lib 105 → 113、`main.rs` 单测 +1、18 个集成二进制不变）。

**主代理补的四处**：

- **半份快照不定缺席**（`1fecaf7`）：代理为「收口 error 行」引入了「快照是否取全」的判定，却只用在了收口上。分页拉到一半某页软失败返回空页（`hasmore=1` 但列表为空）时 `present` 只剩前几页，缺席清理照样执行——与 §30.1 同一类症状，订阅歌单那处还会连带删本地歌单行与曲目缓存。现在三个快照源都要求取全才定缺席。
- **封面按内容识别格式**（`0d5c813`）：存储代理报告 `persist_cover` 一律落 `<hash>.jpg`，而 `image::ImageReader::open` 只按扩展名选解码器；PNG 原图最长边 ≤512 时没有派生缩略件可走，直解必败。内嵌 PNG 早就受影响，D9 修完后 `cover.png` / `Folder.png` 也会踩到。读侧加 `with_guessed_format()`，已缓存的错标旧封面无需重扫。去掉嗅探那一行时回归测试即红。
- **目录封面测试在 Windows 上必败**（`76fd813`）：同一目录里先后写 `cover.jpg` 与 `Cover.jpg` 并断言前者优先，NTFS 上这是同一个文件。改为先探测文件系统是否区分大小写。
- **`cargo doc -D warnings` 报错**：`resume_playlist_tracks` 的文档链接到私有项，并入 `1fecaf7`。CI 的 `test` job 末步就是这条命令，本地四项检查里加上它才提前发现。

**任务书与代理报告的更正、偏差**（续做时以下文为准）：

- 内置 SQLite 是 3.53.2（libsqlite3-sys 0.38.2），任务书里写的 3.54.1 有误；上限 32766 属实。
- **U1 的前提有误**：`spawn_lyric_fetch` 原本只在换曲时调用，只加闸门的话「5 秒后重试」永远不会触发。代理把入口挪到了每次状态推送（词表未就绪或账本有失败记录时才调）。后果两条：无词曲目每 60 秒左右重拉一次直到换曲，没有终态（daemon 对空词页有 300 秒负缓存，只是本机 IPC，不出网——保留）；换曲重进会重新拉词（此前重进显示的是上一曲的词表）。
- **1a 的业务码没做**：`get_detail` 自己也不查内层 `data.code`，任务书说的「既有口径」在接口层不存在。`data` 在场、`data.code` 非 0、列表为空的响应仍是 `Ok(空)`，只靠 1b 的闸门兜底（见 §30.8）。
- **1b 的副作用**：reconcile 没有定时，远端真的清空「我喜欢」或全部订阅后，本地要等第二次触发（重启 daemon 或手动 sync）才合拢。闸门只加在三处，`apply_playlist_track_snapshot` 的曲目差集没加。
- **D5 的语义变化**：预算按「处理过的行数」计，被跳过或抓取失败的行也占位；失败的歌单本遍内不重试；游标是 `list_playlists()` 的行下标，本遍中途歌单增删可能让个别行被跳过或重复一次，下一遍自愈。
- **歌单写错误码**：四种拒绝从 `Internal` 改为 `BadRequest`；存储层的 `QueryReturnedNoRows` 回 `PlaylistNotFound`。
- daemon 代理改了所有权清单外的 `crates/hmp-daemon/tests/reconcile_playlist_tracks.rs`：`reconcile_user_library` 签名变了，7 处调用点必须跟着改；其中一条测试断言的正是「空快照当轮删除」的旧行为，改成三轮。
- 存储代理在任务书没写死处的取法：目录封面按「主名 → 大小写变体 → 扩展名」，首个命中的文件超限就当没有（不退而取后面的候选），同名目录跳过；年份在 `RecordingDate` 读不出 4 位数字时也退 `Year`（比任务书略宽）。

**任务书外发现（本轮未修，建议另开）**：

- `persist_cover` 一律落 `<hash>.jpg`：读侧已按内容解码，盘上文件名仍然错标。
- `read_meta` 的标题/歌手/年份只读 primary tag：WAV 只有 RIFF INFO 标签时读不到，D4 对这种文件不生效。
- hmp-desktop 集成测试不隔离数据目录：带 runtime 的用例只要走到库读取就会只读投影真实库（既有问题，`navigation_scroll.rs` 因 D7 多一处；§31 `419c59c` 已修）。
- 目录封面在扫描读标签时生效：之后才放进目录的封面文件要等重新扫描才会被读到。
- 内嵌 PNG 封面 >512px 走派生缩略件不受影响，≤512px 的此前一直不显示（`0d5c813` 一并修好）。

验证产物在 `docs/verify/2026-11-25-fix-round/`：`baseline.md`（基线小结）、`prewarm.txt` / `baseline-tests.txt`（基线原始输出）、`fixround-verify.txt`（修复轮合入态四项检查；其中 `doc` 一段是修文档链接之前的失败记录，修后重跑通过）、`main-desktop-tests.txt` / `main-daemon-on-storage.txt`（主工作树里桌面补丁、存储补丁各自单独的验证）、`cover-sniff-fixed.txt`（封面嗅探那条回归测试的验证）。三路代理的原样补丁没有归档——内容都在上面的原子提交里。

**依赖整体升级与 CI（同夜；用户要求「依赖尽可能往新了搞，允许改代码适配」）**

- **CI 为什么红**：`main` 上最近三次 push（`a1d47a4`、`20aad3f`、`e24e843`）只有 `deny` job 挂，其余五个 job 都过。三条 RUSTSEC：`libsqlite3-sys 0.38.2`（内置 SQLite 3.53.2，CVE-2026-119375 要 3.54.0 以上）、`url 2.5.8`（要 ≥ 2.5.9）、`cookie 0.18.2`（要 ≥ 0.18.3）。`rusqlite 0.40` 线把 `libsqlite3-sys` 钉在 `^0.38`，升不动，只能跨小版本升。
- **做了什么**（`7de09b5`）：直接依赖全部升到最新——Slint 1.18.1 → 1.19.1（连带 `i-slint-*`）、rusqlite 0.40.2 → 0.43.0（`libsqlite3-sys 0.40.0`，内置 SQLite 3.54.1）、notify 8.2.2 → 9.0.0、resvg 0.48.1 → 0.50.0、tray-icon 0.26.1 → 0.27.0、serial_test 3.5.0 → 4.0.1；其余兼容范围内的一并 `cargo update`（锁文件 221 个包变动：199 升、8 新增、14 移除，`proc-macro-error`、`winapi` 等老依赖出图）。本来就是最新的没动（rodio 0.23.0、image 0.25.10、windows 0.62.2、windows-sys 0.61.2 等）；symphonia 0.5.5 是 rodio 的传递依赖，最新 0.6.1 升不了。三条通告全部靠真实升级解决，没有新增 ignore；`deny.toml` 里 RUSTSEC-2024-0370（proc-macro-error）的 ignore 随它出图而删。
- **MSRV**：Slint 1.19 要 Rust 1.95，也是 `cargo metadata` 聚合出来的全图顶点 → `rust-version`、`clippy.toml`、CI 的 msrv job 一起 1.92 → 1.95。
- **代码适配**：rusqlite / notify / tray-icon / resvg / serial_test 跨大版本零代码改动，全仓编译只挂一处——`build.rs` 的 `SLINT_ENABLE_EXPERIMENTAL_FEATURES` 从 `1` 改成 `interfaces`（1.19 起「全部打开」是错误，必须点名）。
- **A/B 截图查出的行为变化**（`shot-headless` 离屏，14 个画面升级前后逐像素比；对比图在 `shots/`）：
  - **真回归，已修**（`4cc4038`）：歌词进页不直达当前行。歌词行靠 `changed y / height / is-active` 向跟随逻辑上报位置；播放页在当前行已定时才挂载的话，行带着最终几何创建，这三个回调一次都不触发，舞台一直等不到上报，页面停在词表顶部，要等下一句开始才跟上。1.18 里行先落 0 再被布局赋值，所以碰巧会触发。修法是舞台在等待上报期间每拍递增 `report-probe`、行上主动上报；回归测试 `tests/lyric_follow.rs` 取像素判断当前句是否居中，换回未修的界面文件即失败。
  - **投影与多余绑定**（`d461805`）：1.19 的投影跟随填充与边框的透明度，并在编译期提示——封面那层矩形没有背景，`drop-shadow` 一行都不生效，补 `background: Theme.muted`；队列行容器是布局子元素，显式 `x: 0` 多余，删掉。
  - **核对过不是回归**：曲目表与队列的窗口化虚拟化仍生效（探针实测：视口高 648，200 行队列只渲染 25 行）；其余页面的像素差在 1.5%–3.1%，全在文字边缘与细边框上，位置没变。
  - **属于修好的**：播放页标题按设计折成两行（1.18 里没折、直接省略）；带翻译的歌词行在 1.18 里有一部分的翻译没渲染出来。
  - **渲染器层面的差异，不代表真机**：1.19 的软件渲染器才支持投影、圆角裁切跟随 `border-radius`、圆角抗锯齿改进——队列抽屉面板（8.8%）、播放设置页（6.6%）、封面圆角的差别都来自这里。离屏截图用的是软件渲染器，真实应用用的是 GL 渲染器。
- **方法上的坑**（下次升级照做）：
  - `shot-headless` 会建真实 runtime 去连 daemon；新构建的客户端连上旧 daemon 会判「后端是陈旧构建」并把它退出重启。离屏截图必须隔离：空的 HOME / XDG 目录、假 `HMP_IPC_ENDPOINT`、PATH 里没有 `hmp`（找不到后端二进制就进离线模式，连不上也拉不起）。
  - 测试里要取像素得用带软件渲染器的 testing backend（`set_platform(TestingBackend::new(TestingBackendOptions { renderer_name: Some("software") … }))`）；`init_no_event_loop()` 的缺省后端不实现 `take_snapshot`。
  - 无曲目时播放页是空态、不渲染歌词列，歌词相关测试要先 `Player.has-track = true`。
  - 升级前的「对照」用旧 worktree 留着旧锁文件构建，两份截图逐像素比；只看编译和测试发现不了这类回归。
- **验证**：fmt、clippy（all-targets, all-features；`clippy.toml` 的 msrv 同步 1.95 后没有激活新 lint）、全仓测试 888 条（+1 歌词回归）、`cargo doc -D warnings`、`cargo deny` 四项 ok。推送前另按 CI `test` job 的原命令（单线程 + nocapture）彩排了一遍。
- **CI 结果**：`e24e843..f495997` 共 21 个提交推送后，六个 job 全绿（[run 45633681415](https://github.com/modenicheng/hutao-music-player/actions/runs/45633681415)）——`main` 自 10 月底以来首次整轮通过，`deny` job 不再红。

**真机实测（待补）**：新的发布版二进制已按文档命令重建（`cargo build --release --locked -p hmp-desktop -p hmp-cli`；`target/release/hmp` 里 `org.mpris.MediaPlayer2.hmp` 从 0 处变为 3 处，两个二进制都带 SQLite 3.54.1）。重建时用户正在播放，没有重启 daemon，下面三条都还没验：

1. 重启桌面端（新桌面会自动把旧 daemon 换掉）后 `playerctl -l` 应出现 `hmp`，`playerctl -p hmp status` / `metadata` 能读到播放态。注意反过来不成立：旧桌面连上新 daemon 会判「后端是陈旧构建」，重启一次后端后停在 `StaleBackend`；新 `hmp` 命令连旧 daemon 也会把它退出重启。
2. 被清掉的喜欢状态是否自愈：重启后 `hmp library sync` 触发一轮 reconcile，拷库副本确认 `relations` 里 `entity_type='track' AND provider='qq' AND relation='liked' AND desired_state=1` 的行回到约 866，且之后不再被清。
3. Slint 1.19 在真机 GL 渲染器下的样子（封面投影、标题两行、圆角与文字抗锯齿）与歌词进页直达、最近播放进页重读、文件夹过滤保持、播放页歌手/专辑跳转——离屏 A/B 只覆盖软件渲染器，这几条要人眼看。

### 30.8 未核实 / 存疑

- 所有界面表现都是按代码推断的，没有截图，也没有运行验证。（§30.7 末的 Slint 1.19 升级另有离屏 A/B，见下。）
- U1/U2 的线程语义依据代码文本与 tokio 多线程 runtime 语义，未运行复现。
- `hmp play playlist:<本地主键>` 对 owned 歌单报 empty 的根因未证实（QQ 实体分工未返回）；QQ 同曲不同专辑版本是否对应不同 songmid 也未核实。
- A4 的帧大小、U7/U8 的性能量级都是估算。
- 真库中 error 行（1101）的成因与 1101 的语义未证实。
- QQ `CgiGetDiss` 的内层 `data.code` 非 0（歌单不存在时 -100006）目前被解析成「成功但 0 首」，只靠空快照闸门兜底（§30.7）。是否改成直接拒绝，取决于「从没喜欢过任何歌的账号」是否也返回非 0 码——没有实测，也不能拿真实账号去试。
- 修复与升级的真机表现见 §30.7 末的真机实测记录。
- Slint 1.19 在真机 GL 渲染器下的样子没有实测：离屏 A/B 用的是软件渲染器，而 1.18 的软件渲染器根本不画投影，所以「升级前」那一半截图里的面板本来就没有投影，不能拿来推断真机前后差异。真机上需要人眼确认的：封面投影、标题两行、圆角与文字抗锯齿。
- 标「未复核」的条目来自子代理报告，主代理没有逐条回代码核对。

## §31 修复轮补遗（2026-11-28，承接 §30.7）

§30.7 之后逐条处理「任务书外发现」与 §4 遗留。每项单独提交、各自带回归测试；合入态
验证：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
`cargo test --workspace` 全绿（hmp-storage 85 → 88）。

| 提交 | 内容 | 回归测试 |
|---|---|---|
| `46df596` | 登录二维码 PNG 落盘目录可注入：测试不再写真实 `data_dir`（此前在只读 HOME / 沙箱下 `cancel_removes_qr_file` 必红） | 断言路径落在注入目录 |
| `07aa438` | **§4.3**：`add_local_file` 把播放入库的行归入所在扫描根并带当前代际；`finish_scan` 的 missing 收尾从此覆盖播放行 | `playback_ingested_file_joins_scan_root_and_is_marked_missing` |
| `843cc57` | 封面缓存扩展名按内容魔数（此前一律 `.jpg`，PNG/WebP 被错标；读侧早已按内容解码） | `persist_cover_extension_follows_content` |
| `57f3550` | `read_meta` 逐字段按 primary → 全部 tag 兜底；纯 RIFF `LIST/INFO` 的 WAV 不再丢标题/歌手/专辑/年份 | `read_meta_reads_riff_info_without_id3`（去掉兜底即红） |
| `419c59c` | 桌面集成测试不再打开真实库：新增 `load_data_with_snapshot` / `load_data_offline` 与 `bind_offline`，13 个只装载空壳的 UI 用例改用 `load_data_offline`，`navigation_scroll` / `remote_navigation` 用 `bind_offline`；`load_data` / `bind` 生产语义不变 | `injected_snapshot_and_offline_bind_never_touch_data_dir`（hmp-desktop +1） |

**桌面集成测试的库隔离（`419c59c` 细节）**：

- 生产启动仍是 `bridge::load_data`（`library_view::load_snapshot` 直读 `data_dir`）与
  `bridge::bind`；新增入口只供测试/离线注入，避免把「读真实库」当成启动副作用。
- 13 个原本调用 `bridge::load_data` 的 UI 用例改用 `load_data_offline`（空快照）；`navigation_scroll`
  新增空快照装载并改用 `bind_offline`（导航到本地详情 / 「最近播放」时不再读库）；
  `remote_navigation` 同样走 `bind_offline`；bridge 的 `load_detail` 单测改传入 `offline = true`，
  不再因本机存在真实库而偶然打开它。
- `live_daemon` 是例外：它的 E2E 断言（本地曲目整表 play-tracks）需要真实媒体库，且本就
  `#[ignore]`，保持 `load_data`。
- 回归测试把 `XDG_DATA_HOME` 一次性指向临时目录，在临时库里播种「磁盘行 / 磁盘专辑」，
  然后断言注入快照与离线绑定看不到这些内容、且主库 / WAL / SHM 的文件指纹不变。这个
  `set_var` 只在本测试二进制（单用例、且在任何 UI / runtime / 线程创建之前）调用一次，
  没有把进程级环境隔离扩散到全套测试，避免 Rust 2024 `set_var` 的线程竞态。
- 真机验证：`cargo test -p hmp-desktop` 前后 `~/.local/share/hmp/library.sqlite3` 的
  size/mtime 不变（1445888 / 1795688714），且没有生成 `-wal` / `-shm`。

**仍未修**：

- **§4.1 QMC1/V1 footer 误报**。现有 magic-mismatch 重试只兜住「剥离后解不出魔数」的大剥离；
  key_size 很小时（尾部 4 字节恰为小整数）剥离后仍解得出 `fLaC` / MP3 同步字，音频尾部被
  静默截断。可靠修法要在下载与流式两条路径上用候选 key 解密首块并验魔数，且需要真实
  V1/QMC1 样本做正反向测试——没有样本前动手有把真 V1 判死的风险，保持开放。
- **目录封面在扫描后才放进目录**的，要等下次扫描才可见（§30.7 任务书外发现）。
