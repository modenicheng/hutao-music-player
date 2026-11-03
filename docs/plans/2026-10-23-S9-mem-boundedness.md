# S9 计划 · 内存有界性修复（R1b / R2 / R3）

- 工作树：`.worktrees/s9-mem-fixes`（分支 `fix/mem-boundedness`，基线 main `c9add84`）
- 权威输入：`docs/verify/2026-10-23/mem-rca-FINDINGS.md`（S8 根因调查）
- 背景：`docs/AUDIT.md` §21 / §24、`docs/verify/2026-10-23/REPORT.md`（S6 长跑 FAIL 判定）
- 本阶段：**只计划，不实施**。实施需协调者放行（所有权清单见 §6）。
- 协调者已裁定：**不做 R1a**（daemon revision 语义不动——skip_next/EOS 不 bump revision
  影响所有订阅方与装载指纹键，列为远期候选）。

---

## 0. 目标

消灭 S8 定性的三个「确定级产品缺陷」（换曲 = 全量重投影 + 全尺寸解码风暴 +
30fps 整窗重绘），使 §21 内存有界性判据在真机长跑复验中可判通过或可归因收窄：

| 项 | 一句话 | 主判指标（详见 §5 复验） |
|---|---|---|
| R1b | 换曲全量重投影保留（字符串/结构立即重建），封面装载改**异步逐行回填** | 队列行同步解码次数 90 → 0（限定口径：换曲的当前曲 `cover_for_track` 仍有 1 次 512 桶 miss，不在本指标内）；P1 段 t≈475s 打满尖峰（WS 519/PM 763）消失 |
| R2 | `persist_cover` 增写 ≤512px 派生文件；桌面 miss 优先读派生件 | 单次 miss 解码触摸 ~6.5MB → ~1.2MB（像素量 1.44M → 0.26M，≈5.5×） |
| R3 | EqClock Timer 加 `Player.playing` 门控 | 静止期 33ms Timer 唤醒 30 次/s → 0；播放期动画行为不变 |

非目标（明确不做）：P4 静置爬升与 P3 PM 台阶的未归因分量（S8 已穷尽排除仓内来源，
待 E2 VMMap/ETW 真机捕获）；R1a daemon revision 语义；R4 track_theme 双缓冲；
S7 在途文件（track-table.slint / queue-drawer.slint / context-menu.slint /
now-playing.slint）。

---

## 1. 现状证据（file:line，全部已在基线 c9add84 核实）

### 1.1 R1b —— 换曲全量重投影 × 逐行封面装载

链条（每次 `next`/EOS 换曲）：

1. `hmp-core/src/queue.rs:415-424`（`advance_on_eos` 非 Track 模式走 `skip_next`）+
   `queue.rs:422-430`（`skip_next` bump revision；`engine.rs:1300-1303` 注释明示）
   → revision +1。
2. `crates/hmp-desktop/src/backend.rs:445-462`：`StateChanged` 分支 revision 变化 →
   `project_queue().await`（`backend.rs:488-516`：QueueList 全量分页 IPC + sqlite
   批量元数据）→ `UiStateEvent.queue_rows = Some(Vec<QueueRowMeta>)`。
3. `crates/hmp-desktop/src/player_bridge.rs:718-731`（`apply_event` queue_rows 分支）：
   整队列 `VecModel` 重建；每行 `row_from_meta`
   （`player_bridge.rs:1363-1395`）→ `queue_cover`
   （`player_bridge.rs:1401-1412`）→ `cover_cache::get_or_load(path, 256)` ——
   **≤90 次同步解码装在同一个 UI 事件派发里**。
4. miss 代价：`crates/hmp-desktop/src/cover_cache.rs:120-129`
   `decode_downsampled` = 全尺寸解码（1200×1200 ≈ 4.32MB RGB + ~2.2MB 中间）→
   `thumbnail(256)`。
5. 放大机制（S8 §2.1 核算）：80 曲工作集（256 桶 20MB）+ 每换曲 512 桶 1MB 插入
   > 64MB CAP（`cover_cache.rs:29`）→ 循环访问序列 × LRU = 周期性整体打爆 →
   每次换曲 4-8 条全尺寸重解码（冷时最多 90×6.5MB ≈ 585MB 瞬态）。
6. 既有异步回填机制（复用对象，§23 遗产）：`update_queue_row_cover`
   （`player_bridge.rs:1227-1243`，按 mid 线性扫 + `set_row_data` 原地换图；
   mid 不在模型 → 自然 no-op）与 `track_covers::prefetch_tracks_with`
   （`track_covers.rs:66-113`，QQ 远程行预取回包回填）。
7. 单曲循环边界：`queue.rs:421-423` Track 模式 EOS 重播当前**不 bump revision**
   → 无重投影，R1b 不引入新路径。

### 1.2 R2 —— cover_cache miss 全尺寸解码

- 写侧唯一收口：`hmp-storage/src/scan.rs:36-51` `persist_cover`（内容哈希去重，
  tmp+rename 原子写，落 `<data_dir>/covers/<hash>.jpg`）。**五个**调用点全部经它：
  `hmp-daemon/src/content.rs:390`（CoverGet 远程下载）、
  `hmp-daemon/src/local.rs:188`（本地播放解析 `project_local_cover`）、
  `hmp-daemon/src/watcher.rs:176`（扫描）、
  `hmp-daemon/src/reconcile.rs:75`（歌单封面补抓）、
  `crates/hmp-cli/src/scan.rs:76`（CLI `hmp scan`——收口在 scan.rs 时 CLI 自动全覆盖是额外收益）。
- 读侧：`cover_cache.rs:57-59`（`get_or_load` 唯一入口）、`120-129`
  （miss = 无条件按原路径全尺寸解码）。调用方三处桶约定：队列行 256
  （`player_bridge.rs:1405`）、当前曲 512（`player_bridge.rs:1435`）、
  侧栏/详情/卡片 256（library_view，经 `local_cover_image`）。
- 现状：covers 目录只存原图一幅；S6 素材为互异 1200×1200 JPEG（200-500KB），
  桌面每次 miss 都付 ~6.5MB 触摸只为产出 ≤256px 缩略。

### 1.3 R3 —— EqClock 无条件 33ms Timer

- `crates/hmp-desktop/ui/primitives.slint:193-209`：`EqClock` global（`t` 属性）+
  `EqClockHost`（0×0 宿主，内嵌 `Timer { interval: 33ms; triggered => EqClock.t += 33ms }`），
  **无条件常跑**。
- 挂载：`ui/app.slint:54`（`EqClockHost { }`，全窗唯一实例）。
- 消费方仅 `Equalizer`（`primitives.slint:217-259`，`t` 三元绑定 +
  paused/frozen/offset 相位冻结机制）；两处实例化：
  `track-table.slint:180-184`、`queue-drawer.slint:252-256`，均为
  `paused: !Player.playing; visible: Player.queue-current-mid == row.mid`。
- 行为事实（S8 §2.3 已核实）：暂停/停止期 paused 三元分支**不注册对 EqClock.t 的
  依赖** → 零重绘（primitives.slint:190-191 注释自证）；但 Timer 本体仍 33ms 唤醒
  自增；**播放期**全部已实例化 Equalizer（含 `visible:false` 的非当前行）都走
  非暂停分支依赖 EqClock.t → 整窗 ~30fps 重绘（femtovg 全帧重绘），21 分钟长跑的
  驱动侧纹理 churn 持续放大器。
- 测试探针先例：`tests/hover_slider.rs:189-195` —— `mock_elapsed_time(100ms)` 后
  断言 `EqClock.t` 前进（且该测试已先行 `Player::set_playing(true)`，与本门控天然
  兼容）。Timer + mock 时间 = 本仓已验证的无头测试形态。

---

## 2. 设计

### 2.1 R1b —— 队列重建立即、封面异步逐行回填

**核心**：`apply_event` queue_rows 分支保留全量重建（字符串/结构、queue_meta、
SENT_META 兜底、QQ 远程预取全部不动），仅把**本地 file:// 行的封面装载**从
「同步 90 连发解码」改为「缓存命中即同步 + 未命中占位 + 异步分批回填」。

数据通路：

```
row_from_meta(meta, &mut pending)
  └ queue_cover(meta, &mut pending):
      ├ 本地 file:// → path
      │   ├ cover_cache::get_if_cached(path, 256) 命中 → 直接用（零解码、零闪烁）
      │   └ 未命中 → 占位图 + pending.push((mid, path))
      └ http(s) → 占位图（走既有 prefetch_tracks_with，不改）
apply_event 尾部：
  pending 非空 → schedule 换装 pending（clear + replace，在途批次自然作废）
回填驱动（UI 线程，slint::Timer repeating 16ms，批 ≤ BACKFILL_BATCH=6）：
  每 tick 处理 ≤6 条：
    image = cover_cache::get_or_load(path, 256)   // 真解码，LRU 记账照旧
    update_queue_row_cover(player, mid, image)    // mid 已不在 → no-op
  队列耗尽 / 新重建换装 → 旧条目随换装丢弃；耗尽即 timer 停表置 None
  （不留常开 Timer——R3 同纪律）
```

关键决策：

- **D1 驱动用 `slint::Timer` 而非 `invoke_from_event_loop` 链**：Timer 可被
  `i_slint_backend_testing::mock_elapsed_time` 无头驱动（hover_slider EqClock 探针
  即此形态），全链（重建→占位→tick 回填→行换图）可测；invoke 链在无事件循环时
  不可冲刷。批间隔 16ms（≈1 帧）保证批间让出渲染/输入；90 行冷启动 ≈ 15 tick ≈
  240ms 填满。过渡边界：T1 先行、T2 未合时，冷批解码可能超 16ms tick 预算——
  tick 串行处理，只延迟下拍，无正确性问题。
- **D2 缓存命中行保持同步装载**：`get_if_cached` 只查不装载（hit → clone；
  miss → None，不解码不记账，**探测不更新 stamp**——探测不是「使用」，重建高频
  探测不应固化 LRU 序，近因记账留给随后的 get_or_load 回填/当前曲路径）。热行
  （会话内常态）零行为变化、零闪烁；只有 LRU 逐出的冷行短暂占位 ≤240ms。
- **D3 世代语义由「队列换装 + 单一 Timer」吸收**：每次重建 `schedule` 直接
  换装整个 pending 队列（clear + replace），在途批次自然作废——Timer 回调永远
  服务当前队列，无 stale 回调竞态；next 连打/EOS 链快速重建时不做已淘汰行的
  无用解码。
- **D4 回填正确性不依赖 mid 唯一性**：`update_queue_row_cover` 命中首个 mid 行
  即返回——队列内同 mid 重复行第二行保持占位（§23 既有已知边界，不扩大也不修复）。
- **D5 backend.rs 零改动（对 S7 的论证）**：R1b 的全部改动落在
  `player_bridge.rs`（apply_event/row_from_meta/queue_cover + 新回填模块）与
  `cover_cache.rs`（get_if_cached）。backend.rs:445-462 的 revision 比对、
  project_queue、UiStateEvent 结构、dispatch_ui 均不变——「重投影立即（字符串/
  结构）」本就是 backend 侧现状，R1b 只改 UI 侧对投影结果的图片装载时机。
  **⇒ R1b 与 S7 在 backend.rs 无任何交叠。**

边界：

- 队列为空 / revision 归零重连：rows 空 → pending 空 → 不启 Timer，模型置空同现状。
- 单曲循环（LoopMode::Track）：不 bump revision（queue.rs:421-423）→ 无重建无回填。
- `queue clear`（保曲停播）：revision bump → 重建一次，pending 视行数；行为同上。
- 离线翻转：`apply_offline` 不触碰 queue 模型（现状保留）；在途回填 gen 由下一次
  queue_rows 分支 bump——离线路径不 bump，在途回填对旧模型继续换图，无害
  （mid 命中才写）。
- R2 落地后回填批的解码成本同步坍缩（每条 ~6.5MB → ~1.2MB），两项独立成立、
  组合见效。

### 2.2 R2 —— covers 派生缩略文件（写侧 daemon，读侧桌面）

**契约**（新增，双侧共享实现）：原图 `covers/<hash>.jpg` 之外增写
`covers/<hash>.512.jpg` —— 最长边 ≤512px、保比缩放、JPEG q80；
**后缀钉死 `.512.jpg`（互锚测试据此断言）**；
**原图最长边已 ≤512 时不出派生件**（QQ CDN 小封面天然免写，读侧直接用原图，
解码量本就 ≤0.8MB）。

写侧（`hmp-storage/src/scan.rs::persist_cover` 内，五个调用点自动全覆盖——
含 CLI `hmp scan`）：

```
persist_cover(cover: &[u8]):
  ……现有：原图 exists 检查 + tmp+rename 写 <hash>.jpg……
  派生（best-effort，失败只 debug! 日志、绝不 Err）：
    if <hash>.512.jpg 不存在:
      header 探尺寸（ImageReader::with_guessed_format + into_dimensions，不整解）
      max(w,h) > 512 才继续：
        image::load_from_memory(cover) → thumbnail(512,512)
        → JpegEncoder q80 编码 → tmp(".<hash>.512.jpg.tmp")+rename
  返回值/签名不变（仍返回原图 file:// URI——派生件对调用方不可见）
```

- 惰性迁移：旧库无派生件的原册，在下次扫描（watcher）/ 本地播放解析
  （local.rs）/ CoverGet / reconcile **首次触达该 hash** 时补写（persist_cover
  输入就是原图字节，无需重读盘）；≤512 小图因 header 短路永不补写、也永不为它
  付解码。一次性成本：每 >512 原图一次 ~6.5MB 解码 + 编码，daemon 侧瞬态
  （daemon 实测 max WS 68MB 有界）。
- 并发：tmp+rename 与原图同款（CLI scan 与 daemon 并发同 hash 时 rename 幂等，
  Windows rename 覆盖目标，内容相同）。
- 磁盘：仅 >512 原图增派生件，**绝对上界 ~≤100KB/张**（512px JPEG q80 实测
  量级；对照原图 200-500KB）；covers 目录本就是内容寻址只增不减（现状无 GC），
  派生件沿用同策略，不新增清理机制。

读侧（`crates/hmp-desktop/src/cover_cache.rs::decode_downsampled`）：

```
decode_downsampled(path, max_side):
  if max_side ≤ 512:
    derived = 同目录 <stem>.512<ext>      // hmp-core 共享助手拼名
    if derived 可解码 → thumbnail(max_side) → 返回   // ≤512 桶全程小图
  回退：按原路径解码（现状行为）——派生件缺失/损坏/非图全部自动回退
```

- 缓存 key 仍为 `(原路径, max_side)`：命中路径零变化；派生件只优化 miss 的解码
  成本，缓存内容与逐出语义不动。
- 双向兼容是设计属性：旧 daemon + 新桌面 → 派生件不存在 → 回退原图（即现状）；
  新 daemon + 旧桌面 → 派生件被无视（多余文件，无害）。两侧可独立回滚。
- 共享常量与拼名助手落 `hmp-core/src/media.rs`（CoverRef 旁；
  `pub const COVER_DERIVED_SIDE: u32 = 512;` + `pub fn derived_cover_name(&str) -> Option<String>`，
  派生名后缀钉死 `.512.jpg`），hmp-storage / hmp-desktop 均依赖 hmp-core，
  杜绝命名约定漂移（互锚测试双侧断言同名）。
  （hmp-core 不在禁碰清单内，禁碰的是其 queue.rs / engine.rs——本改动两文件皆不触；
  仍提请协调者确认与 S1 无文件级冲突，见 §7。）
- 已知画质取舍：512 桶展示 ≤336px 由派生件直接缩出（与今日原图直缩有极轻二次
  重采样差，JPEG q80 下视觉不可辨）；256 桶二次缩放同理。入册已知边界。

### 2.3 R3 —— EqClock 播放门控

**改动两处**：

1. `primitives.slint` `EqClockHost` 增 `in property <bool> running: true;`，
   Timer 增 `running: root.running;`（Slint Timer 内建属性；false→停表，
   true→从整间隔重启）。默认 true = 未接线时保持现状动画（防呆）。
2. `app.slint:54` 挂载改 `EqClockHost { running: Player.playing; }`。

暂停边界与恢复即时性：

- 暂停/停止/离线：`apply_daemon_state`（10Hz 推送）或 `apply_offline` 置
  `Player.playing=false` → Timer 停 → 静止期 33ms 唤醒归零。Equalizer 视觉由既有
  paused/frozen 机制冻结在暂停瞬间（`primitives.slint:228-236`），无变化。
- 恢复：playing=true → Timer 重启，首个 tick ≤33ms；`changed paused` 复算 offset，
  EQ 按 EqClock.t 绝对相位续摆——恢复即时性与今日完全一致（动画函数是绝对时间
  的纯函数，停表期间 t 不前进 = 暂停语义）。
- 装载中间态（Resolving/Loading）：playing 保持 true 则 EQ 继续摆（现状）；
  daemon 若瞬时报 Paused 则 EQ 短暂冻结（现状 paused 分支同款视觉），无新增风险。
- 播放期 30fps 是动画设计行为，保留：播放且当前行 EQ 可见时整窗 30fps 重绘
  不变（33ms→66ms 降频列为复验不达标时的调节旋钮，不在本次默认改动内）。
- 加固（T3c，**已裁定批入**，同在 primitives.slint 内、不触 S7 文件）：Equalizer
  内部把有效暂停改为 `paused || !self.visible`——非当前行实例（visible 由宿主绑定为
  false）不再注册 EqClock.t 依赖，当前行滚出虚拟窗口时停止无效动画求值。
  若 `self.visible` 自读在 1.17 有绑定语义问题则回退仅做 Timer 门控（决策留实施
  时验证，风险 ≤ 一处回退）。**不做** track-table/queue-drawer 的 per-row
  `paused` 收紧（S7 在途文件；EQ 不可见但抽屉关闭/行滚出时播放期仍可能整窗
  重绘——入册已知边界，**已裁定不塞 S7**，留 S9 后续独立小改）。

---

## 3. 任务分解（文件清单 + 验收标准）

> 顺序建议：T1（R1b）→ T2（R2）→ T3（R3），相互无阻塞依赖；R2 改 daemon 链，
> 完成后须按 AUDIT §18/§22 纪律重建**所有**链接 hmp-daemon 的二进制
> （hmp.exe 与 hmp-desktop.exe 同指纹）并 `hmp quit` 杀旧实测。

### T1（R1b）异步逐行回填

| # | 文件 | 改动 | 验收 |
|---|---|---|---|
| T1a | `crates/hmp-desktop/src/cover_cache.rs` | 增 `pub fn get_if_cached(path, max_side) -> Option<Image>`（只查不装载不记账、**探测不更新 stamp**） | 单测：hit 克隆 / miss None 且 len、misses 计数不变 / 探测不刷新 LRU stamp / 后续 get_or_load 仍按 miss 装载 |
| T1b | `crates/hmp-desktop/src/player_bridge.rs` | `queue_cover` 拆缓存探测与 pending 收集；`row_from_meta` 增 pending 出参；`apply_event` queue_rows 分支接回填调度；新增回填模块（thread_local `(VecDeque, Option<Timer>)` 换装语义 + batch 处理函数，见 §2.1 D3）；pending 计数 tracing::debug（换曲一次重建的 miss 解码数可从日志读出，供复验 E3 口径） | 单测 + 集成测试见 §4；clippy/fmt 干净 |
| T1c | `crates/hmp-desktop/src/backend.rs` | **零改动**（论证见 §2.1 D5） | git diff 为空 |

### T2（R2）派生缩略文件

| # | 文件 | 改动 | 验收 |
|---|---|---|---|
| T2a | `crates/hmp-core/src/media.rs` | `COVER_DERIVED_SIDE` + `derived_cover_name()` | 单测：拼名（有/无扩展名）、常量 = 512 |
| T2b | `hmp-storage/src/scan.rs` | `persist_cover` 增派生写（header 短路 + best-effort） | 单测见 §4；签名与返回值不变（四调用点零改动） |
| T2c | `hmp-storage/Cargo.toml` | 增 `image = { workspace = true }`（若 workspace jpeg 特性未含编码器则补 feature——编译期即验） | cargo build 过；无新增非 workspace 依赖 |
| T2d | `crates/hmp-desktop/src/cover_cache.rs` | `decode_downsampled` 派生优先 + 回退链 | 单测见 §4 |
| T2e | （重建纪律） | hmp.exe + hmp-desktop.exe 同批 release 重建、`hmp quit` 后实测：新封面落盘出现 `<hash>.512.jpg`；旧封面首次播放/扫描后补出 | 真机/隔离端点实测记录 |

### T3（R3）EqClock 门控

| # | 文件 | 改动 | 验收 |
|---|---|---|---|
| T3a | `crates/hmp-desktop/ui/primitives.slint` | `EqClockHost` 增 `running` 属性 + Timer.running；**同步改写 186-192 模块注释**——删除「因此时钟常跑即可，无需播放门控」自证矛盾句，改述门控语义（留着即与实现矛盾） | 门控测试见 §4 |
| T3b | `crates/hmp-desktop/ui/app.slint` | `EqClockHost { running: Player.playing; }` | 同上 |
| T3c | `primitives.slint` | Equalizer 内部 `paused \|\| !self.visible` 自门控（已裁定批入） | 可见性语义回归（当前行切换时 EQ 冻结/复活正确）；失败即回退 T3a/b |

---

## 4. 测试计划

### R1b —— 回填正确性与竞态（player_bridge.rs 内嵌 tests + cover_cache 单测；
AppWindow 形态沿用 `bridge.rs:1570-1571` 先例：init_no_event_loop + AppWindow::new，
同二进制多 AppWindow 测试加 `#[serial]`）

1. `get_if_cached` 三态（T1a 验收，cover_cache 单测，含 cache_stats 断言不装载）。
2. **重建即占位 + 命中即同步**：AppWindow + 调 `apply_event`（mod tests 可见私有
   fn）投 queue_rows（temp 目录真 PNG 封面，先 get_or_load 预热一条）：
   热行立即非占位（尺寸断言区分——文件缩略宽高比 vs 占位方形 256×256）、
   冷行占位。
3. **分批回填全链**：`mock_elapsed_time(16ms × N)` 逐 tick 推进后，全部冷行
   依序换为真图；批大小边界（恰 6/7 条）正确断批。
4. **换装竞态（§2.1 D3）**：回填中途再投一次 queue_rows（schedule 换装 pending）
   → 旧代剩余条目随换装丢弃、不再解码（以 cache miss 计数差断言）；
   新代 pending 只含新代未命中行；被淘汰 mid 不在新模型 → set_row_data
   不命中、无 panic。
5. **mid 消失 no-op**：pending 中 mid 已不在新模型 → set_row_data 不命中、无 panic。
6. **空队列/单曲循环**：rows 空 → 不启 Timer（t 推进无副作用）；
   Track 循环不 bump revision 属 daemon 行为，桌面侧以「同 revision 不重建」
   （backend.rs:447 既有语义）为前置，不在桌面测试范围。
7. （可选，时间盒 ≤ 半天）live_daemon.rs 形态扩展：真 daemon + 带封面素材，
   断言 next 后 240ms 内队列行封面非占位——若 harness 成本超时盒则以 2-5 +
   §5 复验覆盖，不硬撑。

### R2 —— 派生文件回退链

1. **写侧**（hmp-storage 单测，沿 persist_cover 既有隔离数据目录模式）：
   a. 新 1200×900 PNG/JPEG 封面 → 原图 + `<hash>.512.jpg` 双落盘，派生件解码
      尺寸 ≤512 且保比；b. 二次调用同字节 → 双文件不重写（mtime/内容不变）；
   c. ≤512 小封面 → **无**派生件（header 短路）；d. 垃圾字节 → 原图照写、
   无派生件、**不 Err**（现状签名保持）；e. 派生 tmp 残留不影响下次成功。
2. **读侧**（cover_cache 单测）：a. 原图 1200×900（4:3）+ 伪派生 400×100（4:1，
   故意异宽高比作来源标记）→ get_or_load(256) 得 256×64 ⇒ 证走派生；
   b. 派生缺失 → 256×192（原图回退）；c. 派生损坏字节 → 回退原图；
   d. 512 桶从派生直出（512×≤512）；e. 缓存命中路径与派生无关（二次调用零解码，
   hits+1）。
3. **契约一致**：hmp-core 拼名单测与上述 a/b 用例互为锚（写侧产名 = 读侧拼名）。

### R3 —— 门控行为（tests/eq_clock.rs 新文件，单测试单进程纪律；
形态 = svg_icon/drag 系：init_no_event_loop + AppWindow::new + mock_elapsed_time）

1. `playing=false` → mock 200ms → `EqClock.t` 不动（0ms）。
2. `playing=true` → mock 100ms → t 前进 >0（与 hover_slider.rs:189-195 现有正例
   互补，该测试因已置 playing 仍绿，无需改）。
3. 暂停→恢复：t 单调不回退、恢复后继续前进；再次暂停即冻结（门控往返无粘死）。
4. （若 T3c 落地）可见性自门控：设 queue-current-mid 指向行 A → mock 推进 →
   EQ 相位前进；current 切走 → 冻结；切回 → 复活（绝对相位连续）。

### 全仓回归（三项合入后统一跑，逐 crate）

`cargo fmt --all -- --check`；`cargo clippy --workspace --all-targets --all-features -- -D warnings`；
`cargo test --workspace`（USAGE §9 覆盖面；live_daemon 系按其既有前置）。

---

## 5. 修复后复验设计（E1）

**基建**：S6 脚本**已入库**（main `e16c76d`，`docs/verify/2026-10-23/scripts/`，
与 s6 worktree 版本逐字节同）→ 实施阶段自该处复制入本 worktree `verify/`
（mem_run.ps1 / gen_media.py / analyze_mem.py / env-s6.ps1），素材重新生成
（80×10s 近静音 WAV + 互异 1200×1200 JPEG）。护栏沿用 S6：隔离管道/独立 XDG
数据目录、user_volume=0 断言、真库只读前后比对、按 PID 收尾三查、GUI 两臂各
~21min（已获批）；**看门狗继承**：mem_run.ps1 自带 28min 看门狗，复制件原样
继承——任一臂超时由看门狗收杀，不另造退出逻辑。

**臂设计（E1，已裁定：两臂同日、软件渲染臂延后）**：

| 臂 | 构建 | 渲染 | 目的 |
|---|---|---|---|
| E1-fix-gl | 修复版（R1b+R2+R3） | femtovg（默认） | 主判据：C3 全协议 21min |
| E1-base | 基线 c9add84 | femtovg | 同日同机复跑基线，消除 S6 当日系统内存压力混杂，得同Session 对照曲线 |
| E1-fix-sw（延后） | 修复版 | `SLINT_BACKEND=winit-software` | 剥离 GL 驱动：若本臂无 P5 尖峰而 E1-fix-gl 有 → 驱动放大实锤、触发面未除尽（升级回炉）；两臂皆无 → S9 修复确认 |

**判据（沿用 REPORT C3 口径 + S8 预期数字）**：

| # | 判据 | 基线（S6 实测） | 修复生效阈值 |
|---|---|---|---|
| A（主） | P5 起播 20s 无 GB 级 PM commit 且进程存活到 P5 结束 | PM 单刻 +2.27GB → 2688MB，进程死亡 | 全程无 PM 样本 >800MB；P5 25/25 样本齐 |
| B | P1 段无 LRU 打满解码风暴尖峰 | t≈475s 单样本 WS 519/PM 763 | P1 全程无样本 WS >400MB / PM >500MB |
| C | P2 段平台斜率（结构性 churn 代理指标） | ≈0（全平） | ≤ +0.5MB/min（R1b/R2 后换曲重投影 churn 归零的间接证据；配合 T1b decode 计数日志：**稳态重建（除会话首重建——冷装载属预期）**每重建 miss 解码 ≤8 条且单条 ≤1.2MB 量级） |
| D | C3 判据 1/4 | max WS 566.7（余量 33）；判据 4 破 | max WS ≤600 保持且余量 ≥200MB；「无持续新高」以 P4 结束 vs P2 末比。**归因路由**：D 破且呈 F 同签名（PM≫WS 台阶/振荡）→ 走 E2 捕获归因，不算回炉失败 |
| E | daemon 对照 | max WS 68.3 | ≤75MB（较 70 放宽：R2 后 daemon 首触每 hash 一次派生解码瞬态，68MB 基线不可外推；**迁移期豁免**——全部 >512 旧封面补齐派生后回落观察） |
| F | 平台总斜率（P2+P3+P4） | +9.2~+11.1MB/min | **诚实预期**：可归因分量（P2 重投影/解码 churn）清零，P3 台阶与 P4 爬升主分量 S8 已判定未归因（系统/驱动族）——若斜率降至 ≤+0.5 属超预期收获（说明未归因分量与纹理/驱动 churn 耦合且被消除）；若仍 +6~+9 且 E1-fix-sw 同型 → 非渲染驱动路径，按 S8 §5 E2 立项 VMMap/ETW 捕获，**不算 S9 失败但如实归档** |

E3 计数器口径（并入判据 C）：T1b 的 tracing::debug 计数（每次重建的 miss 解码
条数/字节）在长跑日志侧统计——修复版预期每重建 miss ≤8 条 × ~1.2MB，
对照今日冷启动 90×6.5MB / 稳态 4-8×6.5MB。

---

## 6. 所有权与禁碰对照

- 本流独占（实施等协调者放行）：`crates/hmp-desktop/src/player_bridge.rs`、
  `cover_cache.rs`、`ui/primitives.slint`、`ui/app.slint`（仅 EqClockHost 挂载行）、
  `hmp-storage/src/scan.rs`（封面持久化处）、`crates/hmp-core/src/media.rs`
  （新增常量/助手，**新申请**，见 §7 裁决 1）、`hmp-storage/Cargo.toml`、相关测试
  与 `tests/eq_clock.rs`。
- `backend.rs`：R1b 零改动（§2.1 D5 论证）——S7 交叠自然消解。
- 禁碰遵守：engine.rs、queue.rs（R1a 不做）；transport/serve/server（R2 的
  persist 在 hmp-storage scan.rs，不在 server 侧，无需申请）；now-playing.slint、
  track-table/queue-drawer/context-menu（S7 在途）；docs/**（本 PLAN.md 除外）。
- 不 push；不动主工作区与其他 worktree；实施前如需构建 check 限 `-j 4`。

## 7. 需协调者裁决

1. **R2 写侧落点与 hmp-core 小改**：persist_cover 落 `hmp-storage/src/scan.rs` +
   共享契约落 `hmp-core/src/media.rs`。S8 曾警告 scan.rs「可能与 S1 冲突」——请
   确认 S1 范围。若 scan.rs 冲突：备选方案 = 派生写改在 daemon 侧包装
   （content.rs/local.rs/watcher.rs/reconcile.rs 四点各包一层；损失面：CLI scan
   （scan.rs:76）不受益、四处重复，次选）；若 hmp-core 冲突：备选 = 两侧各自
   常量 + §4 契约互锚测试。
2. **[已裁定] E1 臂次与 GUI 预算**：E1-fix-gl + E1-base 两臂同日、各 ~21min
   （GUI 预算已批）；E1-fix-sw 软件渲染臂延后执行。
3. **[已裁定] 播放期重绘残留**：不塞 S7——track-table.slint:181 /
   queue-drawer.slint:253 的 per-row paused 收紧维持「已知边界」记录，留 S9
   后续独立小改（不影响本计划主判据）。
4. **[已解决] S6 verify 脚本归档**：脚本已由协调者归档入库（main `e16c76d`，
   `docs/verify/2026-10-23/scripts/`，含 mem_run.ps1 的 28min 看门狗）——E1
   从该处复制入 worktree `verify/`（看门狗随复制件继承），无归档请求。

## 8. 风险与回滚

| 项 | 风险 | 缓解 | 回滚 |
|---|---|---|---|
| R1b | 冷行占位→真图 ≤240ms 渐进（今日同步直出）；Timer/换装逻辑 bug 致行漏填或错填 | 热行同步零变化；漏填上限 = 占位（非错图）；§4 竞态测试 | revert player_bridge.rs + cover_cache.rs（自包含，无协议/格式面） |
| R1b | mock 时间驱动 Timer 的无头测试形态失效（slint 行为差异） | hover_slider EqClock 探针为既有实证；若失效退 §4-7 集测 + 复验 | 测试形态调整，产品代码不受影响 |
| R2 | 派生解码失败/磁盘满 | best-effort：失败只日志，读侧自动回退原图 | 写侧单独 revert 即停增派生件；存量派生件无害可留 |
| R2 | daemon 首触 >512 旧封面的一次性解码延迟（播放路径 local.rs:188） | 每_hash 仅一次；header 短路小图；daemon 68MB 有界实证 | 同上 |
| R2 | 磁盘增长 | 仅 >512 原图增派生件，绝对上界 ~≤100KB/张；复验记录 covers 目录前后体积入账 | 删派生件即恢复（读侧回退链） |
| R3 | 门控误伤：不该停时停（EQ 冻结在播放中） | playing 是 10Hz 推送单一出口（apply_daemon_state:809），暂停语义与 EQ paused 同源；§4 门控测试 | revert primitives.slint + app.slint 两文件（无状态、无格式） |
| R3 | Timer.running 属性 1.17 语义差异（如需显式 restart） | 编译期 + §4 测试即验 | 同上 |
| 全局 | 陈旧 daemon/二进制错位（AUDIT §18/§22 高频事故） | R2 改 daemon 链后同批重建 hmp.exe + hmp-desktop.exe、`hmp quit` 实测 | — |
| 全局 | 修复后 P5 仍有 GB 级 commit | E1 双渲染臂判别：驱动放大 → 触发面未除尽，升级回炉（候选：R3 降频 66ms、纹理预算、S8 E2 捕获） | 不硬宣成功，如实归档 |
