# hmp-tauri (Vue) → hmp-desktop (Slint) 分模块移植

基准：`apps/hmp-tauri`（DESIGN.md v0.13，胡桃木设计语言）。
目标：`crates/hmp-desktop` —— 把 Vue 前端按模块移植成 Slint UI。
旧 Slint 原型（2026-08 初，`#ff2d55` accent 的另一套 Apple Music 风格）整体废弃，文件随模块移植逐个重写/删除。

## 概念映射

| Vue (hmp-tauri) | Slint (hmp-desktop) |
|---|---|
| vue-router（routes.ts，hash 路由） | `Nav` global：`Route` 枚举 + `param` + `navigate/back` 回调，历史栈在 Rust |
| player.ts `PlayerController`（provide/inject） | `Player` global：状态属性 + 命令回调，Rust 侧权威写入（模拟桥，M8 换真实后端） |
| qualityStore / themeStore | `Quality` / `AppTheme` global（持久化见下） |
| index.css 设计 token（亮/暗两套 CSS 变量） | `theme.slint` 的 `Theme` global（亮暗双值三元绑定；`Palette.color-scheme` 作系统偏好代理） |
| `--track-*` 曲目层（v0.3 只注入播放页 overlay） | `TrackPalette` global：`in-out` 默认绑定品牌回退值，播放页取色模块（M6）运行时覆写 |
| AppIcon（currentColor 内联 SVG） | `AppIcon`：`Image { colorize: }`，SVG 资产拷贝到 `ui/assets/icons/`（slint 需 `features=["svg"]`） |
| HoverGroup 滑动高亮 | `HoverGroup`/`HoverItem`：元素 `absolute-position`（窗口系）差值 → 组内坐标，`HoverBus` global 中转；animate x/y/width/height 140ms cubic-bezier(0.22,1,0.36,1) + opacity 240ms；indicator 层同几何滑动 |
| Scroll 自绘滚动条（900ms 自动隐藏） | `Flickable` + 覆盖层 thumb + `Timer` 自动隐藏 + `hide-scrollbar`（thumb 不渲染） |
| Equalizer（CSS 缩放动画 + 负 delay） | 三根 Rectangle `animate height { iteration-count: -1; direction: alternate; delay: -0.3s 步进 }` |
| mock-data.ts（FNV-1a 种子表 → 工厂） | `src/mock.rs` 逐函数移植（同一 hashSeed，输出与 TS 版一致） |
| covers.ts（程序化 SVG data-URL） | `src/covers.rs`：生成同一 SVG 串 → resvg 栅格化 → `slint::Image`（按 seed 缓存） |
| localStorage（quality/volume/theme） | `~/.config/hmp/desktop-ui.json`（serde_json） |
| 纯文字链接 `.text-link`（hover 提亮禁下划线） | `TextLink` 组件（color 过渡） |
| 数字 tabular-nums | 暂无内建等宽数字开关：接受默认字形（M8 后评估自绘或字体特性） |

## 模块清单与状态

- [x] **M0 基础**：theme tokens / icons+AppIcon / 数据结构(data.slint) / Nav / Player+Quality+AppTheme globals / Button+IconButton+TextLink / HoverGroup / Scroll / Equalizer
- [x] **M1 侧栏**：账户面板、主导航 8 项（exact 语义）、我的歌单宽窄双形态（宽=分组折叠、窄=两级滑动）、页脚（收起/设置/主题循环）、单一滑动高亮
- [x] **M2 播放条**：长进度条（hover 增粗、点击/拖拽 seek）、封面+歌名、QualityBadge(向上弹层)、控制排、VolumeControl 竖向弹层（静音切换）、队列键；模拟播放桥（250ms Timer 推进、自动下一曲、上一曲 3s 规则）；QueueDrawer 主界面态（遮罩+定高+清空+空态+移除）
- [x] **M3 库页**：TrackTable / CoverCard / SectionHeader / PageHeader / FactRow / 我喜欢页（hero 卡+喜欢列表+最近预览+歌单网格）/ 最近播放 / 音乐库（监视文件夹点击过滤）/ 下载（落盘明细）/ 已购（单曲表+专辑网格+页头金额聚合）
- [ ] **M4 内容页**：首页 / 发现 / 排行榜+详情 / 搜索 / 歌单 / 专辑 / 歌手
- [ ] **M5 设置**：总览三分类卡 + 常规（主题三选真实生效）/ 播放（音质四档+默认音量）/ 账号（只读+禁用纪律）
- [ ] **M6 播放页**：全屏 overlay（slide-bottom）、RulerProgress 刻度条、控制台、OKLab 取色（TrackPalette 覆写）、歌词（弹簧跟随+景深+逐字扫色+无滚动条）、评论区（编辑部式重排）、队列抽屉 themed 态
- [ ] **M7 收尾**：ESC/键盘语义、焦点可达、reduced-motion 免动画、窄窗（⅓ 宽 683px）断点核对
- [~] **M8 数据接线**：mock → daemon IPC（2026-09-07 完成首轮：播放/队列/收藏歌单/最近/本地库/侧栏歌单真数据，详见下方 M8 接线记录）；内容页（M4）与歌词（M6）等待后端补接口（docs/AUDIT.md §8）

## 已定工程决策

1. **mock-first**：与 Vue 版同纪律 —— 接口形状即契约（types.ts），mock 数据确定性（同一 FNV-1a 种子），诚实空态/禁用。真实数据接线是 M8。
2. **app.rs（AppCore）暂不动**：真实后端编排原样保留（编译不引用，无 dead_code 警告），M8 重接；`demo.rs`/旧 `bridge.rs`/旧 `bridge_tests.rs` 随旧 UI 契约删除。
3. **模拟播放桥在 Rust**：`slint::Timer` 250ms 快照推进 position，队列/seek/音量/自动下一曲全实现 —— 对应 Vue 的 BrowserPlayerBridge。**（M8 已由 daemon IPC 真桥替换，见下方接线记录；player_host.rs → player_bridge.rs。）**
4. **封面用 resvg 运行时栅格化**（slint 同款依赖，256px 缓存 `HashMap<seed, Image>`），保证与 Vue 版同一套确定性封面。
5. **TrackTable 歌手列简化**：歌手串整体一个链接（→ 主歌手页）；多人合唱逐人分链留到 M4 歌手页落地时评估（Slint struct 无嵌套数组，逐人分链需拆模型）。
6. **横向滚动**（direction=all）暂缺，内容页封面横排用到时补。
7. 旧 UI 的字符串页面标识（`current-page: "library"`）废弃，新 Nav 用枚举 + param（专辑/歌单/榜单 mid）。

## M8 接线记录（2026-09-07，首轮）

**架构**：沿 docs/PROJECT.md §8.6 解耦设计 —— 桌面 UI 是 daemon 的又一个适配器。
`src/backend.rs`：Unix socket 客户端（长度前缀 JSON 帧，`hmp_core::ipc`），连接失败仿 CLI `connect_or_spawn` 拉起 `hmp serve --background`（`spawn_detached_exe` 定位 current_exe 同目录/PATH 上的 hmp 二进制；flock 单实例仍归 daemon）；彻底失败 → 离线诚实降级（全空态，命令 no-op，daemon 退出不自动复活——`hmp quit` 语义优先）。订阅长连接收 `Event::StateChanged`，快照经 `invoke_from_event_loop` 回 UI 线程（推送 ~10Hz）。队列重建只认 `QueueSummary.revision`。媒体库读 = 直读 `library.sqlite3`（`src/library_view.rs`，CLI 同契约）。

**真数据**：我喜欢 / 最近播放（真实时间戳文案）/ 音乐库（扫描根分组统计）/ 歌单（relation 分流，副标题 "N 首"）/ 侧栏歌单区（Data.sidebar-*，色对按 id 哈希确定性装饰）/ 播放条与队列（DaemonState → Player 单向映射，命令 → Request）。

**仍 mock**：下载/已购两页（后端无此域，AUDIT §8.5）。

**已知偏差（M8 新增）**：
- 音质文案由 `actual_quality` 映射，无采样率（"FLAC" 而非 "FLAC · 44.1kHz"，诚实）；
- 最近播放时间戳为 UTC（桌面无 chrono/本地时区源，与 CLI `history` 口径一致，见 library_view::format_stamp）；
- 本地曲目键归一化（`canonical_local_key`，hmp-storage v5 迁移合并幽灵行）修复了收藏/歌单写路径与扫描器键格式分裂导致的"我喜欢显示裸键、0:00"问题；
- QQ 曲目封面 http → 程序化占位（UI 禁 HTTP，封面代理缺口 AUDIT §8.4）；本地曲目 `file://` 封面走真图；
- 队列点歌 = `Play(该曲 id)`，整队被该单曲替换（无 PlayAt IPC，AUDIT §8.8）；
- 队列行 QQ 曲目时长 0:00（`track_meta_batch` 无时长投影，AUDIT §8.11）；
- 客户端音质偏好暂不生效（与 daemon config.toml 两处存储，AUDIT §8.7）；音量以 daemon 推送为准（含 RG 补偿语义，AUDIT §8.12）；
- 歌手/专辑 mid 真数据行为空串（媒体库不存远端 mid），点击跳空参详情页（M4 落地时收敛）；
- 库页是启动静态快照（无 LibraryChanged 事件，AUDIT §8.9）。

**后端缺口全集**：docs/AUDIT.md §8（12 条 + 汇总表）。

## 已知偏差（相对 Vue 版）

- 数字等宽（tabular-nums）暂无 Slint 内建开关，接受默认字形；
- 歌手列整体一个链接（→ 主歌手页），多人合唱逐人分链待 M4 评估；
- 文件大小公式修正了 TS 版的量纲 bug（`(kbps*1000*ms)/8` → `kbps*ms/8`，旧式单曲显示 ~26 GB）；
- 窗口标题栏 CJK 方框是 niri 装饰条字体问题，与应用无关；
- **无窗口关闭按钮**：`no-frame: true`（绕 CSD 字体问题）且未自绘窗口控制，关窗走 WM；要补自绘按钮留 M4+ 壳层打磨。

## Slint 机制坑位（源码确证，2026-09-06）

- **has-hover 由命中的最顶层 TouchArea 独占**（i-slint-core input.rs：TouchArea 对 Moved 返回 EventAccepted，中止向更低兄弟的遍历；只沿祖先链传播）。行级 hover 传感必须把 TouchArea 做成内容的**祖先**（包住 @children），垫底兄弟永远收不到。行内更深的按钮仍各自拿 grab，不抢行 hover。
- **animate 只在绑定值变化时触发**（properties_animations.rs 的 ShouldStart 状态由原绑定脏标记驱动），恒定绑定永不动画；**负 delay 被当 `<=0` 即无延迟**，无 CSS 式错相。持续循环动画要用 Timer 驱动公式求值（Equalizer 的 EqClock 方案：33ms 共享时钟 + 余弦高度 + `paused ? frozen : t-offset` 三元暂停冻结——惰性分支不注册依赖，暂停零重绘）。
- **global 不能有子元素**（Timer 挂不进 global）；EqClock global + app.slint 挂 EqClockHost 组件驱动。
- **Flickable 滚动 = 隐藏 viewport 元素的 x/y（与 viewport-x/y 双向绑定）**，故 `absolute-position`（map_to_window 沿祖先几何累加）含滚动偏移且依赖是活的——集成测试的"上报矩形包含指针"不变量在滚动后 24/24 通过。
- **debug vs release 渲染差距巨大**：debug 全场景重绘 ~30fps、release ~200fps（FemtoVG+OpenGL，`SLINT_DEBUG_PERFORMANCE="refresh_full_speed,console"` 实测）。日常视觉验收用 `cargo run --release -p hmp-desktop`；持续动画（EQ）把重绘压到 30fps 采样。
- 集成测试基建：dev-dep `i-slint-backend-testing`（`init_no_event_loop` 进程级单例 → 一个测试二进制一个 #[test]）+ 公开 `Window::dispatch_event` 合成指针（`slint::platform::{WindowEvent, PointerEventButton}`）+ `mock_elapsed_time` 驱动 Timer；global 要在 app.slint `export {}` 后 Rust 侧才可读。
- **布局显式 `alignment` 会让 stretch 失效**（视觉验收发现的坑，2026-09-06）：HorizontalLayout/VerticalLayout 一旦写了 `alignment: start/end/...`，剩余空间按对齐分配、所有子元素取首选宽，`horizontal-stretch` 不再参与 —— "标题 stretch:1 + 右侧动作区"的 space-between 写法必须**不写 alignment**。曾导致 PageHeader 动作区/SectionHeader"更多"/hero 播放全部全部挤在标题旁。
- **逐行 stretch 列宽会漂移**：Slint 没有跨行共享的 CSS grid fr，每行自己的布局按"该行内容首选宽"分配 stretch 余量 → 短文本行的列位置肉眼可见地左右跳动（曲目表专辑列）。修法：按根宽算定宽（TrackTable 的 `grid-available × 1.4/2.2 / 0.8/2.2`；封面卡网格 `Theme.grid-card-width` 对应 Vue `repeat(auto-fill, minmax(9rem,1fr))`）。
- **舍入对齐**：TS `Math.round` 在 slint 侧用 `Math.round`、Rust 侧用 `f64::round`；别顺手写成 floor/div_ceil（时长"2 小时 5/6 分钟"、"m:ss" 都栽过）。
- 视觉 QA 基建：`examples/shot.rs`（`cargo run --release -p hmp-desktop --example shot -- <route> [--theme dark] [--playing] [--queue] [--hover X,Y] [--bus-hover X,Y,W,H]`，playing 走 `invoke_play_tracks` 让 PlayerHost 接管快照）+ `/tmp/hmp-qa2/dshot.sh`（niri 按 PID 找窗口 → focus-workspace/focus-window → `screenshot-window`，焦点校验+尺寸校验+重试）；Vue 对照用 `/tmp/hmp-qa/vshot2.mjs`（1420，视口 1018×1228 dpr=1.25，走真实主题循环切暗色）。winit 下 `dispatch_event(PointerMoved)` 不驱动 hover（testing backend 才行），hover 视觉用 `--bus-hover` 或真实指针。

## 构建与启动（M8 起）

- 构建：`cargo build --release`（桌面启动会自动拉起 daemon，需要 `hmp` 二进制在 PATH 或与桌面二进制同目录）。
- 测试：`cargo test -p hmp-desktop`（mock/nav/format 纯逻辑单测 + backend/library_view 投影单测）。
- 启动：`cargo run --release -p hmp-desktop --bin hmp-desktop`（niri 下逻辑目标 1024×1152 半宽 / 2048×1152 全宽）。
  - 启动即连接 daemon socket（`$XDG_RUNTIME_DIR/hmp.sock`），连不上自动 `hmp serve --background`（CLI 同款）；失败降级离线（全空态、命令无效）。
  - 库页数据 = 启动时直读 `$XDG_DATA_HOME/hmp/library.sqlite3` 的静态快照：先 `hmp scan ~/Music` 入库本地曲目，QQ 侧 `hmp login` + `hmp library sync`。
  - 播放/收藏/歌单写全走 daemon（与 CLI/MPRIS 同一状态源）；`hmp quit` 后 UI 呈离线空态，不自动复活后端。
- 视觉 QA：`cargo run --release -p hmp-desktop --example shot -- <route> [--theme dark] [--queue] [--playing]`（真实应用宿主，route 如 library/recent/local/downloads/purchased）+ `/tmp/hmp-qa2/dshot2.sh <name> <args…>`（niri 截图；daemon 需已在跑，播放态用 `hmp play` 预置）。
