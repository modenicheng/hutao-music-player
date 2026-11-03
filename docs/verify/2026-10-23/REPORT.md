# S6 验收补账报告（A 右键菜单视觉验收 / B §21 内存治理真机长跑）

- 工作树：`D:\coding\hutao-music-player\.worktrees\s6-verify`（分支 `chore/verify-baseline`，基线 `cd4a75e`）
- 执行期：2026-10-23（接棒前任代理；本报告由接棒代理完成，含中断前产物复核）
- 验证对象：main 已入库状态 `cd4a75e`（不依赖并行流）
- 产物根：worktree 内 `verify/`；产品树唯一新增 = `crates/hmp-desktop/examples/shot-menu.rs`（独立提交，QA 用途）

---

## A. 右键菜单视觉验收（A1 headless 截图矩阵 + A2 真机 runbook）

### A1 执行方式

- 宿主：`crates/hmp-desktop/examples/shot-menu.rs`（testing backend + software renderer +
  `take_snapshot`；mock 时间 16ms 分步；中性 PointerMoved 热身 + 预热渲染先例
  `examples/shot-headless.rs:314-317`；右键手法同 `crates/hmp-desktop/tests/context_menu.rs`）。
- 1280×800 共 12 张（11 计划项中 #5 拆浅/暗两张），输出 `verify/shots/menu/`，
  裁剪版 `_crops/`（`verify/scripts/check_shots.py` 生成并数值自查）。
- 面板矩形经像素级标定（白游程片段分析 + lib-light↔clamp-bottom 差分定位），非目测。

### A1 数值自查结论（check_shots.py 输出摘要）

| # | 场景 | 面板几何 | 底色(surface-3) | 行数/公式高 | 结论 |
|---|------|---------|-----------------|-------------|------|
| 1 | lib-light | (640,344) 216×248 | (250,250,250)→白 ✓ | 6 行=248 ✓ | PASS |
| 2 | lib-dark | 同上 | (62,60,57)→(58,56,53)±容差 ✓ | 6 行 ✓ | PASS |
| 3 | lib-hover | 同上 | 同 #1 | 6 行 ✓ | PASS（hover 暖色偏移见下） |
| 4 | lib-enabled-nav | 同上 | ✓ | 6 行 ✓ | PASS（导航行启用见下） |
| 5a | queue-light | (1064,144) 216×248 | ✓ | 6 行=248 ✓ | PASS |
| 5b | queue-dark | 同上 | 暗色 ✓ | 6 行 ✓ | PASS |
| 6 | playlist | (640,304) 216×286 | ✓ | 7 行=286 ✓ | PASS |
| 7 | picker（QQ 行） | — | — | — | **缺陷 #1 阻断**（弹层已关，证据） |
| 8 | picker（本地行） | — | — | — | **缺陷 #1 阻断** |
| 9 | clamp-bottom | (640,552) 216×248，底=窗底800 | ✓ | 6 行 ✓ | PASS（上收钳制） |
| 10 | clamp-right | (1064,344) 216×248，右=窗缘1280 | ✓ | 6 行 ✓ | PASS（水平钳制） |
| 11 | picker-empty | — | — | — | **缺陷 #1 阻断** |

深入核对（lib-light 基准）：

- 分隔线实测 y=466/507，理论 466/505（row2 后与 row3 后）→ 吻合（±2px 抗锯齿带）。
- 行文字存在度（std）：启用行 35.5/46.7/40.9/53.0 vs 禁用行（查看歌手/查看专辑）19.5/20.8
  → 禁用 0.45 opacity 灰度显著 ✓。
- hover：item1 无 (241,240,240) → 有 (230,217,216)，delta=(-10,-22,-23)，B 降幅>R 降幅
  → ctx-accent-soft 暖色高亮方向正确 ✓（浅暗两主题分别经 #1/#2 验证底色无对比度反转）。
- 导航启用（--nav-mids）：row4/5 std 36.1/37.2 ≈ 启用行水平（vs 禁用 19.5/20.8）→ mid 判据接线正确 ✓。
- queue 条目集（数值+视觉双核）：播放/下一首播放/加入队列 ‖ 从队列移除（**无**「加入播放列表…」）
  ‖ 查看歌手/查看专辑（禁用）→ 分隔线 266/307 = row2 后与 row3 后 ✓。
- playlist 条目集：7 行 = library + 从歌单移除（row4，std 35.3 启用态）✓。
- 软件渲染器「三不渲染」豁免（不判缺陷）：阴影不画（12px 阴影缺席）、clip 圆角忽略、纹理最近邻。

### 缺陷 #1（产品，发现于 A1）：右键菜单「加入播放列表…」二级 picker 面板不可达

- **现象**：点击主菜单「加入播放列表…」后弹层整体关闭，二级歌单选择面板不出现；
  三上下文（library/queue/playlist）× 两主题均如此；picker 空态「还没有自建歌单」同样不可见。
- **根因**：`crates/hmp-desktop/ui/track-table.slint:301` 与 `crates/hmp-desktop/ui/queue-drawer.slint:365`
  的 `row-menu := PopupWindow` 均未设置 `close-policy`。slint 默认策略为 close-on-click
  （builtins.slint 文档："By default, a PopupWindow closes when the user clicks"），
  `i-slint-core` window.rs 关闭条件 `mouse_inside && released && had_popup_on_press` 对
  **弹层内**完整点击即关闭。「加入播放列表…」行的 `activated` 只置 `picker-mode = true`
  （`ui/context-menu.slint`），框架随即关闭弹层。其余条目因显式 `dismissed()`（等价关闭）
  症状被掩盖，唯 picker 受害。
- **证据链**（`verify/shots/menu/_dbg/`）：
  1. `click3-press.png`：仅 press → 弹层存活（主菜单完整）；
  2. `click3-release.png`：仅 release → 弹层存活；
  3. `click3-both.png`：完整 click → 弹层关闭；
  4. `click4-disabled.png`：点击**禁用行**（TouchArea.enabled=false，activated 不可达）完整
     click → 弹层同样关闭 —— 排除 activated/dismissed 驱动，纯框架策略行为；
  5. 前任 `picker.png` 与 `picker-local.png` md5 相同（不同 --rows-source 产出同一张
     「菜单已关」背景图），三张 picker 截图目标区域每列仅 3 种颜色（纯背景，无文字）。
- **影响面**：桌面端右键菜单的「加入播放列表」功能完全不可用（真机 femtovg/winit 后端
  预计同样成立——关闭逻辑在 i-slint-core 层，与渲染后端无关；A2 runbook 第 8 节留真机复核点）。
- **修复方向（S6 不修，交协调者）**：两处宿主 PopupWindow 补
  `close-policy: close-on-click-outside;`（dismissed 显式关闭路径不受影响；ESC 语义两策略等价）。

### A2 交付

`verify/RUNBOOK-右键菜单.md`：真机眼验 runbook（10 节 + 回传格式），含缺陷 #1 真机复核
重点节、两处【变更·可回滚】检查点（一次性测试歌单）、关闭语义/滚动跟随/锚行滚出自动关/
设置持久化等 headless 外交互面。

---

## B. §21 内存治理真机长跑

### 执行参数与启动断言（verify/data/mem_run.log）

- 隔离环境（XDG 三变量 + `HMP_IPC_ENDPOINT=\\.\pipe\hmp-s6-verify`，`verify/scripts/env-s6.ps1`）；
  release 同批构建（hmp.exe 14:32:49 / hmp-desktop.exe 14:35:23，`backend_build=fad267e93c4cb73b`）。
- 启动序列断言全过：daemon pid=15456 → **音量门 user_volume=0 PASS** → 指纹自检 PASS →
  桌面 pid=16516（真机 GUI）→ P0-P5 按 3/7/4/4/2/3 分钟执行，5s 采样落
  `verify/data/rss_curve.csv`（desktop 主序列 + daemon 对照各 242/— 行）。

### 结果（analyze_mem.py，30s 中位数降采样，主序列 WS）

| 阶段 | 段内 WS 形态 |
|------|--------------|
| P0 静置 | 236MB 平稳 |
| P1 播放/切曲(~70次) | 221→260 爬升，t=480s 瞬态尖峰 WS 519/PM 731 后回落 ~280（LRU 打满逐出生效迹象） |
| P2 队列 churn | 279MB 全平 |
| P3 库 churn | WS 平 287，PM 台阶 +65MB(308→373) 后持稳 |
| P4 静置 | **WS 无驱动爬升 287→414MB（+127MB/2min）**，PM 373→462 |
| P5 复扫 | 起播后 12s 内 PM 单刻 **+2.27GB（413→2688MB）**，WS 395→567；**随后进程消失（崩溃）**，P5 仅得 25s 数据 |

### 判据（C3 修订版）

- 判据1 绝对上限：max WS = **566.7MB ≤ 600 → 满足**（余量仅 33MB，且受崩溃截断）
- 判据2 双窗漂移：P1 后半 274.3MB vs P5(截断) 421.6MB，|Δ|=**147.3MB > 24 → 破**
- 判据3 平台斜率（P2+P3+P4）：**+11.1MB/min > +0.5 → 破**（P4 静置爬升主导）
- 判据4 无持续新高：后期 max 421.6 > 前期 max 287.0 + 24 → **破**
- **判定矩阵：FAIL（max ∈ [300,600) 且判据 2/3/4 破）**
- **崩叠加权**：P5 起播 20s 内 PrivateMemorySize64 暴增 2.27GB 至 2.69GB 后进程死亡
  （19:28:18–19:28:28；Windows 事件日志无 WER 记录，死亡机制无法单独归因——
  当时机器整体内存紧张，但 **2.27GB 单刻 commit 暴增本身是实测产品行为**）。
  P5 判据 2 的窗口数据因此截断，判定以 3/4（数据完备）+ 崩沉事实为据。
- 曲线：`verify/data/rss_curve.png`；原始数据 `verify/data/rss_curve.csv`；
  daemon 对照 max WS 68.3MB（有界，符合 ~20-70MB 预期）。

### 执行侧事故记录（如实）

- mem_run.ps1 第 5 次启动方成功（前四次：PS 5.1 对无 BOM UTF-8 中文脚本按 GBK 误读致
  注释吞行解析错 ×2、TimeSpan.AddSeconds 在 .NET Framework 不存在 ×1、env 包同编码问题致
  全局变量空 ×1——均已修复：脚本重存 UTF-8 BOM + AddSeconds 换 Add(FromSeconds)）。
- 桌面崩溃后原 daemon(15456) 亦于 30s 内退出（原因未明，疑似同因系统内存压力）；
  P5 后续 CLI 自动 connect_or_spawn 重生 daemon(66592) 并继续空转。
- 脚本因一次 CLI 管道调用挂起未自然收尾 → Bash 侧硬停 + 手动收尾：
  `hmp quit`（隔离端点）+ 按记录 PID 复核（desktop 16516 / daemon 15456 / 66592 全消失）
  + 残留 powershell 按命令行匹配精确杀（42304）——无按映像名误杀。

### B-浏览代理（C1 修订执行结果）

- **判别门 FAIL → 按规则降级为「只覆盖根因 3 + 模型重建」**。结构性原因（本次新实证，
  修正 C1 的可行性假设）：`ui/track-table.slint` 窗口化公式
  `window-start = (visible-top − table-top)/row-pitch`，其中
  `visible-top = −Viewport.y`、`table-top = absolute-position.y − Viewport.origin-y − Viewport.y`
  —— **Viewport.y 两项相消**，窗口位置实际键于表格真实 `absolute-position`（仅真实
  Flickable 滚动可改变；testing backend 无 Rust API 驱动）。故「Rust 直驱 Viewport 全局
  切进窗口化路径」在此架构下不可达；深滚判别门正确识别（deep hit=true，view-h=631/y=-183680
  均已生效写入全局）。窗口化虚拟化（根因 4）**无代理覆盖**，其真实覆盖仍以真机路径为准
  （而真机主实验 FAIL——见上）。
- `--browse-force` 全量渲染模式跑 6 分钟（8 路由 × 4500 行全程滚动 × 每轮全量重灌，
  48 pass；软件渲染器、无 GPU 纹理、无 daemon 推送）：数据 `verify/data/rss_browse_proxy.csv`
  （72 样本），**max WS 41.1MB / PM 20.8MB，全程平坦无爬升**——4500 行纯文本行
  （TrackRow 已剥封面）+ 48 次全量模型重灌在软件渲染栈下有界：根因 3（行不背图）+
  模型重建的佐证成立（佐证口径：无 30s 窗斜率可算——单相曲线恒定，斜率≈0）。
  曲线图蓝线见 `verify/data/rss_curve.png`。

---

## 收尾与护栏

- 真库护栏（只读 stat 前后比对）：`%LOCALAPPDATA%\hmp\library.sqlite3`
  前后均 mtime 2026-10-04 20:59:18 / 1,277,952 字节（T2 期间与 B 长跑后各核一次）→ **隔离无渗漏**。
- 收尾三查：① `hmp quit`（隔离端点）已执行；② 记录 PID（desktop 16516、daemon 15456/66592、
  browse 代理进程）逐一 tasklist 复核消失；③ 启动前后按映像名快照对比——无本流残留进程
  （主工作区另一并行流的 cargo test/rustc 进程不属于本流，未触碰）。
- 全程未碰用户实例（默认管道 `\\.\pipe\hmp`）与用户真库；声音纪律：素材 -50dBFS 近静音 +
  播放前 `user_volume=0` 断言 PASS 后才放行播放阶段；GUI 窗口暴露约 21 分钟（19:07:45–19:28:28，获批 ≤25 分钟内）。
- 未 push；产品树唯一新增 `crates/hmp-desktop/examples/shot-menu.rs`（独立提交
  **`177da0b`**，QA 用途注明于提交信息）；PLAN.md 未动（协调者合并时处理）。

---

## 发现的产品缺陷清单

| # | 严重度 | 摘要 | 位置 | 状态 |
|---|--------|------|------|------|
| 1 | 高（功能不可用） | 右键菜单二级 picker 面板不可达（PopupWindow 默认 close-on-click，弹层内任意完整点击即关闭） | crates/hmp-desktop/ui/track-table.slint:301, crates/hmp-desktop/ui/queue-drawer.slint:365 | 已记录不修（证据链完备） |
| 2 | 严重（§21 有界性证伪） | 桌面端 P4 静置期无驱动 WS 爬升 +127MB/2min；P5 重播 20s 内 PM 暴增 2.27GB 至 2.69GB 后进程崩溃；判据 2/3/4 全破 → 判定 FAIL | 桌面前端（复现路径：播放/切曲 churn → 队列/歌单 churn → 静置 → 再次播放；素材 80 曲互异 1200px 内嵌封面近静音 WAV） | 已记录不修（曲线+CSV 证据） |
| 3 | 低（验证基建注记，非产品） | 窗口化虚拟化在 testing backend 下不可经 Viewport 全局直驱（Viewport.y 在 window-start 公式中自相消，键于 absolute-position）——修正 C1 假设，窗口化覆盖只能真机 | crates/hmp-desktop/ui/track-table.slint:32-50 | 记录（影响后续无头验证方法学） |
