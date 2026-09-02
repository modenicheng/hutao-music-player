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
