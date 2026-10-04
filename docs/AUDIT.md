# 后端审计报告（2026-09-03，Linux 侧）

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
3. **播放路径创建的 local_files 行是二等行**（无 scan_root/generation）：daemon 关机期间
   删除的文件永不标记 missing，直到下次扫描。
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
| `lyric.rs` | 歌词（仅桌面端接入，CLI 无 `hmp lyrics`） | CLI `hmp lyrics <mid>` / status 内嵌 |
| `top.rs` | 排行榜分类/详情 | `hmp top` / `hmp play top:<idx>` |
| `singer.rs` | 歌手主页/歌曲/专辑/MV/相似 | `hmp artist <mid>` 浏览 |
| `recommend.rs` | 首页 Feed/雷达/推荐歌单/新歌/猜你喜欢 | `hmp recommend` / `hmp play radar` |
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

2. **IPC 无搜索与内容详情接口——成立，M4 内容页无数据源**。
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

3. **IPC 无歌词读取——成立**。`Request` 无歌词变体；QQ 歌词 API（lyric.rs）目前唯一
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

**开放项（按优先级，未修）**：

- IPC：accept 循环一错即停摆无自愈（server.rs break）；单实例管道 + 1s BUSY 重试
  预算在连接风暴下可能耗尽（实测裸连 97% BUSY）；客户端读响应无超时。
- 播放：中途读错误被 rodio 视为 EOS → 静默跳歌无 Error 态；`try_seek` 阻塞命令循环
  最长 30s；~~音质回退链不覆盖解码期失败~~（**2026-10-02 关闭**，见 §17：engine 降档
  循环覆盖装载与解码期失败，Timeout 不降档为有意边界）；预解析期无总超时。
- 引擎命令循环内联 await 解析（歌单/专辑秒级）→ 播放控制命令排队，UI 感知「点了
  没反应几秒」。
- 其余见 §4（footer 误报流式路径残余、INNER JOIN 收藏残余、local_files 二等行）
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
