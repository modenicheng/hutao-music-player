# S8 · §21 内存有界性证伪——根因调查报告（只读静态分析）

- 工作树：`.worktrees/s8-mem-rca`（分支 `investigate/mem-regression`，基线含 `930b101`）
- 输入：`docs/verify/2026-10-23/REPORT.md` + `rss_curve.csv`（逐样本精读）、`docs/AUDIT.md` §20/§21/§23/§24、
  S6 编排脚本与素材生成器（`.worktrees/s6-verify/verify/scripts/mem_run.ps1`、`gen_media.py`）、
  hmp-desktop / hmp-daemon / hmp-core / hmp-storage 全部嫌疑源文件、Slint 1.17.1 渲染器源码
  （`i-slint-renderer-femtovg-1.17.1/images.rs`）。
- 方法：假设驱动 + 逐文件证据排除；量级核算；置信度三级（确定 / 高度可能 / 待真机复验）。
- 本阶段零代码改动。

---

## 0. 执行摘要

| 观测 | 根因结论 | 置信度 |
|---|---|---|
| ① 平台斜率 +9~11MB/min | 不是均匀泄漏：P2 全平、P3 一次性 PM 台阶 +69MB、P4 非均匀台阶+振荡（贡献几乎全部斜率）。代码侧真实缺陷（换曲全量队列重投影 × 90 行 × LRU 逐出后全尺寸 JPEG 重解码 churn + EqClock 30fps 全窗重绘）解释 P1 段爬升与高位平台；P3/P4 台阶的分配源在仓内代码路径中**穷尽排查未找到** | P1 机制：确定；P3/P4 台阶来源：待真机复验 |
| ② 双窗漂移 147MB | ①的累计：cover_cache 64MB 打满 + 解码 churn 的分配器滞留 + P3/P4 台阶 + P5 前段 | 确定（构成）/ 待复验（台阶部分） |
| ③ P4 静置 115s +127MB WS | **两进程在代码语义上均确证空闲**（播放已于 P2 内停止，见 §2.1；引擎事件驱动不推送、desktop 除 EqClock 33ms 属性自增外无定时分配）。但 CSV 显示 daemon 同窗口 WS 50→69MB、PM 13.7→25.7MB 同步增长且 P5 起播即回落 7MB——仓内任何路径都无法产生该行为。系统级内存压力/驱动层是首要嫌疑 | 未归因（需真机 VMMap/ETW 捕获）；仓内时间驱动面逐一排除：确定 |
| ④ P5 起播 20s PM +2.27GB 后进程死亡 | 应用侧该窗口的有界核算上限 ≈ 数百 MB 瞬态 churn（缓存已温），观测值超任何单窗口应用侧核算 8~40 倍；PM:WS 增量比 13.6:1（commit 不驻留）、稳定保持 ≥5s、无 WER 死亡——签名与 GL 驱动/WDDM 系统内存 commit 放大（纹理churn风暴 + 系统压力下换页）相容 | 高度可能（机制族）/ 待真机复验（归因） |

**产品缺陷（代码侧，确定级）**——即使非 P4/P5 直接元凶，也是必须修的有界性/性能缺陷：
D1 每次 `next`/自动切曲 → queue revision bump → **全量队列重投影 + 90 行逐行 cover_cache 装载**（`hmp-core/src/queue.rs:422-430` skip_next 递增 revision；`hmp-desktop/src/backend.rs:445-462` revision 变化即 `project_queue`；`player_bridge.rs:719-731` 整模型重建）。
D2 cover_cache miss = **全尺寸 1200×1200 JPEG 解码（~4.3MB RGB + 中间缓冲 ~6.5MB 触摸）只为产出 ≤256px 缩略**（`cover_cache.rs:120-129`）；80 曲 × (256 桶 20MB + 每换曲 512 桶 1MB) 工作集 > 64MB CAP → 循环访问 + LRU = 周期性打爆（经典 cyclic LRU worst case），每次换曲触发一批全尺寸重解码（P1 t=475s 的 WS 528/PM 763 单样本尖峰即首次打满的证据）。
D3 EqClock **无条件 33ms 全局 Timer**（`ui/primitives.slint:198-209`），播放期整窗 ~30fps 重绘 21 分钟（femtovg 全帧重绘），是驱动侧分配 churn 的持续放大器。

---

## 1. 素材与阶段驱动事实（从 S6 工件复核）

- 素材（`gen_media.py`）：80 首 WAV，**每首恰 ~10s**（16bit/mono/8kHz），互异 1200×1200 JPEG（PIL q70，约 200-500KB）内嵌于 WAV `id3 ` chunk。全尺寸解码 = 1200×1200×3 = 4.32MB（image 0.25.9 RGB8）+ zune-jpeg 中间面 ~2.2MB。
- 阶段驱动（`mem_run.ps1`）：P1 play + next/6s；P2 `queue add`×20 → **`queue clear`（无 --all）**循环；P3 `playlist add/remove`/2.5s；P4 无驱动；P5 同 P1。
- CLI 语义（`hmp-cli/src/main.rs:206-211`）：`queue clear` 默认 **保留当前曲不停播**（QueueClear{all:false}，`hmp-daemon/src/engine.rs:617-635` clear_pending）。

### 1.1 P4 时「播放是否还活着」——确证：已停

1. P2 首次 `queue clear` ≈19:18:24（tick 20 × ~1.35s）：清 pending，当前曲继续 ≤10s；
2. EOS → `on_ended`（`engine.rs:1215-1231`）→ `advance_on_eos` 返回 None（队列空 + LoopMode::None 默认，`hmp-core/src/queue.rs:105,407`）→ `enter_idle` → **播放永久停止**（P2 内的后续 `queue add` 只入队不续播，无任何播放命令直到 P5 首帧 `play`）。
3. ⇒ 19:18:35 至 19:27:58（P5 起）**播放全停**：引擎事件驱动不再 publish（`engine.rs:759-786`）；订阅通道静默；daemon 侧 SyncWorker 30s tick 无凭证即返回（`sync.rs:85,118-121`）、watcher 1s tick 空转（`watcher.rs:125`）、smtc/tray 无定时器；desktop 侧除 EqClock（暂停分支零重绘，`primitives.slint:190-191,228`）外无任何定时器（全 crate grep：bridge 刷新防抖/player_bridge 反馈/音量 Timer 均 SingleShot 且事件驱动）。
4. **这是 ③ 的悖论核心：两进程代码语义上空闲，却双双增长**（desktop +130MB WS/+85MB PM；daemon +19MB WS/+12MB PM；daemon PM 在 P5 首条 CLI 到达时回落 7MB）。

---

## 2. 四观测逐项分析

### 2.1 ① 平台斜率（P2+P3+P4）

分阶段斜率（CSV 端点，WS）：P2 0 MB/min（286→286 全平）；P3 首样本一次性 +9MB WS/+69MB **PM** 后 240s 全平；P4 非均匀（前 15s 平 → 台阶 → ±60MB 振荡爬至 +130MB）。即判据 3 的斜率几乎全部由 P3 台阶 + P4 爬升构成。

**已确定的部分（P1 段形态与高位平台的来源）——换曲链全量重投影 × cover_cache 逐出重解码 churn：**

换曲（next / EOS / play）每次执行：
- daemon：`queue.rs:422-430` skip_next **bump revision**（engine.rs:1300-1303 注释明示 skip_next/advance_on_eos 均递增）→ `backend.rs:445-462` revision 变化 → `project_queue()`（QueueList 全量 IPC + sqlite 批查）→ `player_bridge.rs:719-731` **整队列 VecModel 重建**，每行 `queue_cover` → `cover_cache::get_or_load(path, 256)`（`player_bridge.rs:1401-1412`）；当前曲另走 512 桶（`cover_for_track`，`player_bridge.rs:1428-1443`）+ `track_theme::apply_cover` 环境层管线。
- cover_cache miss 代价：`cover_cache.rs:120-129` `ImageReader::open().decode()` 全尺寸解码 1200×1200（~4.3MB 输出 + ~2.2MB 中间）→ `thumbnail(256)` → 256KB 入缓存。
- 工作集核算：80 封面 × 256 桶 = 20MB；每换曲插入 512 桶 1MB；播过 ~44 曲后总需求 > 64MB CAP → **循环访问序列（每次重建按同序触碰 80 个 256 桶条目）+ LRU = 周期性整体打爆**：每次换曲的 512 插入逐出 4 个尾部 256 条目 → 下次重建 miss 重解码 → 再插入再逐出…… 每次换曲的稳态 churn ≈ 10-40 次全尺寸解码 ≈ 50-250MB 瞬态分配。P1 的 PM 249→316（+67MB ≈ 缓存打满）与 t=475s 单样本 WS 528/PM 763 尖峰（首次打满、批解码风暴）与之精确吻合（REPORT 亦标注「LRU 打满逐出生效迹象」）。

**未归因部分：** P3 首样本 +69MB PM 台阶（WS 仅 +9MB——PM≫WS 签名，与 ④ 同族）与 P4 全段（见 2.3）。仓内代码对这两段的Idle状态分配为穷尽排除后的零。

### 2.2 ② 双窗漂移 147MB

P1 后半 274MB（缓存打满后的平台）→ P5 前 421MB：= P3 台阶（PM 侧重叠）+ P4 全段 +130MB + P5 起播重建温装载。构成与 ① 相同；§21 预测「RSS 有界于 64MB cover_cache + 模型字符串 + GPU 纹理」中的 64MB 部分成立（P1 PM 增量与之吻合），超出部分即未归因台阶 + churn 滞留。

### 2.3 ③ P4 静置爬升 +127MB——未归因（仓内排除完毕）

时间结构（CSV，5s 采样）：
- 19:25:58-19:26:08 三样本全平（293956/293956/293960）；
- **19:26:13（P4 起 15s）单样本台阶**：desktop WS +40MB/PM +64MB；**同一样本 daemon WS 50.1→52.6MB 开步**，19:26:19 再 +9.7MB、19:26:24 +4.3MB（daemon PM 13.7→25.7MB 持续爬）；
- 此后 desktop PM ±60MB 振荡（446→506→389→320→397→…→505→…→467，含 -69MB 单样本回落=真实释放/去提交），daemon 缓爬至 69.7/25.8MB；
- P5 首条 CLI（19:27:58 `status`+`play`）到达时 **daemon PM 立即回落 7MB**。

排查结论（逐一，证据）：
| 嫌疑 | 判定 | 证据 |
|---|---|---|
| 播放自动续跑驱动 | **排除** | §1.1 链条：P2 内播放已停；P2/P3 WS 全平佐证（若在播，每 10s 换曲应见 P4 同款振荡） |
| 引擎/订阅推送 | 排除 | engine.rs:759-786 publish 纯事件驱动；idle 无 tick |
| SyncWorker 30s | 排除 | sync.rs:118-121 无凭证即刻返回（隔离环境未登录） |
| watcher 1s | 排除 | watcher.rs:125 空集 drain；媒体目录无变更 |
| smtc / tray / mpris | 排除 | smtc.rs、tray/、无 Timer/interval |
| desktop 定时器面 | 排除 | 全 crate 仅 bridge 刷新防抖/player_bridge 反馈/音量三个 SingleShot Timer（事件驱动）；EqClock 33ms 仅属性自增、暂停分支零重绘（primitives.slint:186-236） |
| IPC 帧读取 | 排除 | backend.rs:602-622 `read_frame` len ≤ MAX_FRAME=1MB（ipc.rs:15）；且 idle 无帧 |
| 内存压力假象（WS 部分） | 部分成立（高度可能） | ±60MB WS 振荡形态与系统级 trim/refault 相容（REPORT 证词：P5 时点机器整体内存紧张；压力或于 P4 起始）。但 PM 台阶与 daemon PM 爬升是真实 commit，压力无法解释 commit 增长 |

**结论：** P4 的分配发生在已审计代码路径之外。首要候选（按可能性）：(a) GL 驱动/WDDM 系统内存 commit 增长（desktop 侧；与 ④ 同族机制，压力下更明显）；(b) Windows 堆的 decommit/recommit 周期放大瞬时分配；(c) 未审计的系统交互面（管道缓冲、SMTC WinRT 内部等）。**需真机 VMMap/ETW 捕获闭环**（§5 E2）。P4 的 +127MB 超过 cover_cache CAP 两倍这一指控本身成立，但其来源不是 cover_cache（idle 无装载路径——cache 仅在 get_or_load 调用时增长，而 P4 无任何调用源）。

### 2.4 ④ P5 起播 PM 单刻 +2.27GB——量级核算与排除

数据：19:28:13 PM 423,372KB → 19:28:18 PM 2,752,128KB（+2,328,756KB = **+2,274MB**），WS 404,660→576,324（**+168MB**，PM:WS ≈ 13.6:1）；19:28:23 PM 2,753,228（**稳定持有**，非瞬态）；随后进程消失，无 WER。此前 10s PM 处于下降段（496→483→423，P5 起播重建的瞬态正在释放）。

**应用侧该窗口（起播后 ~15-20s，`play` 队列替换 + 2-3 次 `next`/EOS）有界核算：**
- 队列重建 ×3-4 次：每次 90 行 `get_or_load(256)`——256 桶条目自 P2 起全部在缓存（无逐出源），实际 miss ≈ 每次 `next` 的 512 插入逐出的 ~4 条 → 每次重建 ~4-8 次全尺寸解码 ≈ 30-60MB churn；
- 当前曲管线 ×3：512 解码（若 miss ~6.5MB）+ `apply_cover`（to_rgba8 1MB + 480px 缓冲 0.92MB + sample 16KB + ambient **f64** 缓冲：data 480×480×4×8B=7.4MB + 每趟 blur tmp 7.4MB×6 趟，`track_theme.rs:617-635,697-721`）≈ 每换曲 ~25-55MB churn，全部瞬态；
- Slint 纹理：`i-slint-renderer-femtovg-1.17.1/images.rs:253,283-297` TextureCache 每渲染 pass drain 强引用计数=1 的纹理（delete_image）——纹理随应用侧 Image 引用走，有界 ~25MB（90×256KB + 512 + ambient）；
- IPC/协议：MAX_FRAME=1MB 封顶（backend.rs:610）。

⇒ 单窗口应用侧上限（最坏冷缓存全量风暴）≈ 90×6.5MB ≈ 580MB 瞬态 + ≤64MB 滞留；实际窗口（温缓存）≈ 100-250MB。**观测 +2,274MB 且稳定持有，超核算 8~40 倍。**

逐项排除 2.27GB 的仓内放大来源：
| 候选 | 判定 | 证据 |
|---|---|---|
| track_theme 全尺寸 to_rgba8 链 | 排除（作为 2.27GB 来源） | track_theme.rs:590-603 输入是 cover_cache 已降采样 ≤512 的 Image；`to_rgba8` ≈1MB；`APPLIED` 去重（761-795）同曲同主题零重算；全部缓冲 ≤8MB |
| 换曲/重播全尺寸解码 ×数百次 | 排除 | 每换曲实际 miss ≤8 条（温缓存）；20s 内换曲 ≤4 次；且均为瞬态非持有 |
| 内嵌封面 32MB 全量解码路径 | 排除（桌面侧不存在） | MAX_COVER_BYTES=32MB 只在 daemon 扫描/解析侧（hmp-storage/local.rs:71,101）；桌面经 covers/ 落盘文件读小图；daemon 全程 max WS 68MB 自证有界 |
| cover_cache 无界/绕过 | 排除 | CAP 64MB 解码后字节加权、逐出有测试（cover_cache.rs:29,103-117）；无绕过直装路径（queue_cover/cover_for_track/local_cover_image 全走 get_or_load） |
| IPC 巨帧/长度前缀错读 | 排除 | MAX_FRAME=1MB 双侧校验 |
| Slint 纹理缓存无界累积 | 排除 | 渲染器源码 drain 语义（上表）；纹理随 Image 引用释放 |
| 大 Vec/with_capacity 病态尺寸 | 排除 | desktop 全 crate 容量计算均有界（track_theme/cover_cache/backend 读帧 ≤1MB） |

**成立的首选解释（高度可能，机制族）：** commit-不驻留 + 稳定持有 + 无 WER 死亡的组合，与 **OpenGL/WDDM 驱动层系统内存 commit 放大**（Intel/NVIDIA/AMD UMD 的进程内分配计入 PrivateMemorySize64；系统内存压力下驻留被换出故 WS 低）相容；触发面为 P5 起播的**纹理 churn 风暴**（90 行封面 + 每换曲 512 封面/480 环境层新纹理上传 + EqClock 30fps 整窗重绘 ×新纹理）叠加已存在的系统内存压力（压力下驱动 staging/池再增长；极端时驱动/上下文重建一次性 re-commit GB 级）。次要候选：分配器 arena 在解码风暴下的段增长。**两者都需真机复验归因**（§5 E1），但无论归因何处，「2.27GB commit 暴增时桌面进程正在执行的产品行为 = 换曲全量重投影 + 全尺寸重解码 + 30fps 重绘」成立，削减这些行为即削减触发面。

P3 起点的 +69MB PM 台阶（WS 仅 +9MB）与 ④ 同签名（PM≫WS），提示同一放大机制在 P3 已间歇活动。

---

## 3. 嫌疑面总表（任务清单逐项）

| 嫌疑面（任务书） | 判定 | 关键证据（file:line） |
|---|---|---|
| track_theme.rs「单次有界解码」是否真有界 | **属实**（Applied 只存 key+16KB 采样；480px 缓冲；每换曲一次 to_rgba8 输入已 ≤512） | track_theme.rs:581-603, 741-750, 761-795；测试 1030-1058 |
| 换曲/重播每次全尺寸 to_rgba8 | **否**（输入即降采样图；全尺寸解码只存在于 cover_cache miss 路径=缺陷 D2） | track_theme.rs:590（`image.to_rgba8()` 输入为 512 桶 Image） |
| P5 2.27GB 来自封面解码放大 | **排除**（直接路径）；间接相关（churn 触发驱动/分配器放大） | §2.4 核算 |
| player_bridge spawn_cover_fetch（10Hz+退避） | 本运行无 QQ 远程封面（素材全本地），未触发；退避实现正确 | player_bridge.rs:942-946（仅 https 触发）、1128-1224 |
| queue_rows 预取回填 / 512/256 桶 | 回填仅 QQ 行；**有份（D1/D2 的载体）**：每 revision 全量重建 90 行 × get_or_load | player_bridge.rs:719-754, 1401-1412；backend.rs:445-462 |
| 盘缓存命中是否复用 cover_cache | 是，命中零重解码（clone 引用） | cover_cache.rs:66-75 |
| cover_cache LRU 真逐出/CAP 计口径/降采样全路径 | 逐出真实、权重=解码后字节、全路径降采样；**但 miss 的解码成本未降采样**（D2） | cover_cache.rs:61-117, 120-129 |
| 绕过 cache 的直装路径 | 无 | 全调用点走 get_or_load |
| backend/bridge 订阅循环、schedule_refresh 防抖、大模型重建 | 防抖 500ms 正确；重建为字符串模型（TrackRow 无封面），浏览代理 6min 平坦 41MB 自证有界；**有份（D1）**：revision→全量重投影在 backend.rs | bridge.rs:493-526；backend.rs:445-462 |
| Slint set_* 大模型旧侧滞留 | 未发现（VecModel 替换即释放；纹理随引用 drain） | images.rs drain 语义 |
| viewport/track-table 真机 view-h 回写链 | 已接线（app.slint:67-72 双向别名）；且非本事故路径（无滚动；全量渲染回退路径已被浏览代理证明平坦） | app.slint:67-72；REPORT B-浏览代理 |
| 时间驱动面（P4） | **全部排除**（§2.3 表）；EqClock 为唯一常开 Timer，暂停零重绘、零分配 | primitives.slint:186-236 |
| Slint 纹理/femtovg | 纹理缓存有界（drain）；**femtovg 全帧重绘 × EqClock 30fps 是持续 churn 放大器（D3）**；GB 级 commit 的最终载体候选=驱动层（待复验） | primitives.slint:203-208；images.rs:283-297 |

---

## 4. 修复建议（性价比排序）

> 原则：先消灭「换曲 = 全量重投影 + 全尺寸解码 + 30fps 全窗重绘」这三个放大器——它们既是确定的产品缺陷，也是 ④ 的触发面；P4 的未归因分配待 E2 捕获后再定向修。

**R1（首选，高性价比）：消灭换曲全量队列重投影。** 二选一：
- R1a（daemon 侧，一改全端受益）：`skip_next`/`advance_on_eos` **不再 bump revision**（光标移动不是结构变更；current 下标已在 summary 中随状态推送）。`hmp-core/src/queue.rs:422-430` + `hmp-daemon/src/engine.rs`（注意 engine.rs:1300-1303 注释的既有依赖：装载指纹含 revision——需同步改写槽键）。
- R1b（desktop 侧，改动面小）：`apply_event` 的 queue_rows 分支改为**重建模型用现有封面/占位、封面经既有异步回填通道逐行换图**（`update_queue_row_cover` 已存在，QQ 行回填机制扩展到本地 file:// 行），重建不再同步触发 ≤90 次 get_or_load。
- 预期：P1/P5 型 churn 中重投影次数 90+ → ≤ 数次；重建 CPU/分配峰值塌缩一个量级。
- 冲突面：R1a 动 hmp-core queue.rs + engine.rs（**daemon 核心，需协调者确认是否与在途 S1 冲突**）；R1b 仅动 `player_bridge.rs`/`track_covers.rs`（S1/S7 均不在此）。

**R2（次选，高性价比）：cover_cache miss 降本——预缩封面派生文件。** `hmp-storage/src/scan.rs::persist_cover` 落盘原图之外再写 `covers/<hash>.512.jpg`（或 .256）；`cover_cache.rs::decode_downsampled` 优先读派生文件 → miss 解码从 ~6.5MB 触摸降到 ≤0.5MB、CPU ~16x 降。LRU 打满后的周期性重解码风暴从 50-250MB churn 降到 ~5MB。
- 冲突面：`hmp-storage/scan.rs` + `daemon/local.rs`（§20 封面链路面，**可能与 S1 冲突，需协调者核对**）+ `cover_cache.rs`（本流独有）。

**R3（低改动）：EqClock 播放门控/降频。** `ui/primitives.slint:198-209` 的 Timer 由 `Player.playing` 门控启停（或 33ms→66ms）。暂停期行为不变（本就零重绘）；播放期整窗重绘 30fps→按需，21 分钟长跑的驱动 churn 大幅下降。
- 冲突面：`ui/primitives.slint` + `ui/app.slint`（EqClockHost 挂载处）——**与 S7（track-table.slint:301 / queue-drawer.slint:365 close-policy）无文件交集**。

**R4（低优先）：track_theme 环境层 f64 缓冲改 ping-pong 双缓冲 + f32。** 每换曲 churn ~55MB→~15MB。`track_theme.rs:617-635, 697-721`。无冲突。

**R5（必要，观测）：复验基建**（§5）。在修复落地前后各跑一次归因捕获，P4 台阶与 P5 GB 级 commit 才能从「高度可能」升级为「确定」。

---

## 5. 修复后复验实验设计

- **E1 P5 尖峰隔离复现**（归因 ④）：仅 P1+P5 协议（跳过 P2-P4），对照组三臂：① 原样 femtovg；② `SLINT_BACKEND=winit-software`（软件渲染，剥离 GL 驱动）；③ 修复后版本。各 10min，PM/WS 5s 采样 + GPU 共享/专用内存遥测。软件臂无尖峰 ⇒ 驱动放大实锤；修复臂无尖峰 ⇒ 触发面消除确认。
- **E2 P4 捕获**（归因 ③）：P4 前显式 `hmp stop` + `queue clear --all`（确定性停播），VMMap 每 10s 快照 diff（desktop+daemon）+ WPR heap/commit trace；若仍有台阶，VMMap 按「堆/私有数据/镜像」分类直接指认分配源。
- **E3 计数器插桩验证**（debug 构建）：`decode_downsampled` 调用数、cover_cache len/bytes、queue 重建次数、EqClock 重绘帧率——churn 场景 10min，断言：重建 ≤5 次、miss 解码 ≤10MB/次。
- **E4 回归 C3 全量**：S6 判据 1-4（max WS ≤600、双窗漂移 ≤24MB、平台斜率 ≤+0.5MB/min、无持续新高）× 21min 全协议；daemon 对照 ≤70MB。
- **护栏**：沿用 S6 隔离环境（独立管道/XDG、音量 0、真库只读比对）；E1/E2 各 ≤12min GUI 暴露，累计 ≤25min/日。

## 6. 需协调者裁决

1. **R1a vs R1b**：R1a 改 daemon revision 语义（影响所有订阅方与装载指纹键 engine.rs:1300-1303），收益全局；R1b 只动桌面、风险小但治标。是否采用 R1a？**S1 在途流涉及哪些文件，与 hmp-core/queue.rs、engine.rs、hmp-storage/scan.rs 是否冲突？**
2. **真机复验授权**：P4 台阶与 P5 2.27GB 的最终归因需要 E1/E2（各 ~12min GUI 暴露 + ETW/VMMap）。先批复验后修复，还是先落 R1-R3 再一并复验？（建议：R1b/R2/R3 无冲突可先行，E1/E2 与修复并行排期。）
3. **S7 无冲突确认**：本报告修复面（player_bridge/track_covers/cover_cache/scan.rs/primitives.slint/app.slint/track_theme.rs）与 S7 的 track-table.slint:301 / queue-drawer.slint:365 无文件交集；唯 scan.rs/engine.rs 待 S1 范围确认后动工。
