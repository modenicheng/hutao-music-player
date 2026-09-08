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
3. **播放路径创建的 local_files 行是二等行**（无 scan_root/generation）：daemon 关机期间
   删除的文件永不标记 missing，直到下次扫描。
4. **`delete_playlists_absent` FK 地雷**：subscribed 歌单一旦有曲目缓存就会 FK 失败
   （当前写路径拒绝 subscribed，暂不可达；缓存功能落地前必须先清理子行）。
5. proxy 多区间 Range 返回 416（RFC 建议 200 全量）；`stream_range_body` 对空流
   会挂起自定义 `Source` 实现者（内置实现不触发）。
6. decrypt 全文件读取用 `std::fs::read` 在 async 上下文（百毫秒级 worker 停顿，
   可 spawn_blocking）。
7. **顶层短命令别名缺失**：main.rs 的设计注释说「高频短命令保留为 alias」，但
   `hmp quality/loop/shuffle/history` 不存在（文档曾是这么写的，本轮已把文档对齐到
   `hmp player quality` 等）。是否补顶层别名是产品决策。
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

### 遗留

- FLAC 标签文件级乱码（`立春 - 薛凯琪.flac` 下载时双重编码，库内其余曲目正常）
  ——数据问题不修代码；如需处理应在下载链路做编码探测。
- `show-overlay` 无挂载组件（M6 播放页未移植，点击封面区暂无响应，已知缺口）。
- 运维提醒：协议演进后需重启 daemon；桌面自动拉起只认 current_exe 同目录与
  PATH 的 `hmp`，release 部署需同步重建。
