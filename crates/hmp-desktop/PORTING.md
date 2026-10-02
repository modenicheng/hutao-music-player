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
| `--track-*` 曲目层（v0.3 只注入播放页 overlay） | `TrackPalette` global（stores.slint）：默认值=品牌胡桃木（与 Theme.track-* 同源），`src/track_theme.rs` OKLab 取色运行时覆写；`ctx-*` 按上下文解析（overlay=专辑色/主界面=品牌），`Theme.track-*` 恒为静态品牌 |
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
- [x] **M4 内容页（本地投影轮，2026-09-07）**：歌单详情（playlist_tracks + 本地/QQ 元数据补全，头部 N 首+总时长）/ 专辑详情（library_albums 组行 + local_tracks_by_album，show-album=false）/ 歌手详情（track_artists 命中 + 展示串最长包含归一 + 曲目专辑聚合网格）；CoverCardGrid 换行网格；TrackTable 歌手/专辑链接接通（本地投影参数=展示名，空参禁用）。首页/发现/榜单/搜索仍为诚实占位（等 daemon 内容读接口，AUDIT §8.2）
- [x] **M5 设置**：总览三分类卡 + 常规（主题三选真实生效，Theme.set-mode）/ 播放（音质四档 Quality.select + 默认音量 HSlider→Player.set-volume 双写）/ 账号（账号读接口未接入 AUDIT §8.6：资料卡诚实占位 + 退出登录禁用 + FactRow "—"）
- [x] **M6 播放页（2026-09-09 收口，余一项后端阻塞）**：全屏 overlay（slide-bottom 入/退场 + 软卸载）、RulerProgress 刻度条、控制台、OKLab 取色（TrackPalette global 覆写，见下方 M6 收口记录）、歌词（弹簧跟随+景深+无滚动条）、评论区（编辑部式重排 + 112px 固定过渡带）、队列抽屉 themed 态；**未做**：逐字扫色（QRC 词级时间轴未投影，AUDIT §14）
- [ ] **M7 收尾**：ESC/键盘语义、焦点可达、reduced-motion 免动画、窄窗（⅓ 宽 683px）断点核对
- [~] **M8 数据接线**：mock → daemon IPC（2026-09-07 完成首轮：播放/队列/收藏歌单/最近/本地库/侧栏歌单真数据，详见下方 M8 接线记录）；内容页/歌词已接入（ff8050e 内容域 + M6 歌词页）；**下载/已购两页后端无对应域，诚实空态**（2026-09-29 删除 mock.rs，AUDIT §8.5）

## 已定工程决策

1. **mock-first**：与 Vue 版同纪律 —— 接口形状即契约（types.ts），mock 数据确定性（同一 FNV-1a 种子），诚实空态/禁用。真实数据接线是 M8。
2. **app.rs（AppCore）暂不动**：真实后端编排原样保留（编译不引用，无 dead_code 警告），M8 重接；`demo.rs`/旧 `bridge.rs`/旧 `bridge_tests.rs` 随旧 UI 契约删除。
3. **模拟播放桥在 Rust**：`slint::Timer` 250ms 快照推进 position，队列/seek/音量/自动下一曲全实现 —— 对应 Vue 的 BrowserPlayerBridge。**（M8 已由 daemon IPC 真桥替换，见下方接线记录；player_host.rs → player_bridge.rs。）**
4. **封面用 resvg 运行时栅格化**（slint 同款依赖，256px 缓存 `HashMap<seed, Image>`），保证与 Vue 版同一套确定性封面。
5. **TrackTable 歌手列简化**：歌手串整体一个链接（→ 主歌手页）；多人合唱逐人分链留到 M4 歌手页落地时评估（Slint struct 无嵌套数组，逐人分链需拆模型）。
6. **横向滚动**（direction=all）暂缺，内容页封面横排用到时补。
7. 旧 UI 的字符串页面标识（`current-page: "library"`）废弃，新 Nav 用枚举 + param（专辑/歌单/榜单 mid）。
8. **无边框窗口自绘标题栏（title-bar.slint，桌面壳新增无 Vue 对应物）**：`no-frame` 后窗口操作全靠自绘——顶栏空白区按住位移超 4px 阈值 → `WinChrome.drag()`（Rust 经 `slint::winit_030::WinitWindowAccessor` 拿 winit 句柄 `drag_window()`，系统级移动循环=贴靠/半屏分列原生；testing 后端无 winit 句柄自动空操作）；双击顶栏=最大化切换；右侧 46×32 三钮（关闭 48 宽、hover 红 `#C42B1C` 白 glyph）写 Window 根元素的 `minimized`/`maximized` 双向属性 + `close()`（Slint 1.17 无 WindowDragArea，窗口属性直接挂在 Window 根上，没有 `root.window.*` 语法）。`resize-border-width: 8px`（=layout-gap）补边缘拖拽改尺寸。**坑**：组件根元素不能访问 `parent`；组件实例不设 x 时首个实例会与后继实例重叠（必须在使用站点钉 x/y/w/h——本项目所有组件几何一律在使用站点声明的惯例由此来）；`viewbox-width/height` 是无单位数值；Path 1px 描边居中于路径，0.5 偏移坐标在 1x 下最锐。SC_MOVE 模态循环吞 release，drag 返回后补发合成 PointerReleased 复位 TouchArea。

## M8 接线记录（2026-09-07，首轮）

**架构**：沿 docs/PROJECT.md §8.6 解耦设计 —— 桌面 UI 是 daemon 的又一个适配器。
`src/backend.rs`：平台 IPC 客户端（Windows 命名管道 / Unix socket，长度前缀 JSON 帧，`hmp_core::ipc`），连接失败仿 CLI `connect_or_spawn` 拉起 `hmp serve --background`（`spawn_detached_exe` 定位 current_exe 同目录/PATH 上的 hmp 二进制；flock/first_pipe_instance 单实例仍归 daemon）；彻底失败 → 离线诚实降级（全空态，命令 no-op，daemon 退出不自动复活——`hmp quit` 语义优先）。订阅长连接收 `Event::StateChanged`，快照经 `invoke_from_event_loop` 回 UI 线程（推送 ~10Hz）；**事件帧解码失败 → 记日志断线重连**（不静默跳过，协议版本错配下防 UI 冻结）。队列重建只认 `QueueSummary.revision`。媒体库读 = 直读 `library.sqlite3`（`src/library_view.rs`，CLI 同契约）。

**真数据**：我喜欢 / 最近播放（真实时间戳文案）/ 音乐库（扫描根分组统计）/ 歌单（relation 分流，副标题 "N 首"）/ 侧栏歌单区（Data.sidebar-*，色对按 id 哈希确定性装饰）/ 播放条与队列（DaemonState → Player 单向映射，命令 → Request）。

**M4.5 详情页接线（2026-09-07 第二轮）**：歌单/专辑/歌手三页详情 = 导航时同步直读 sqlite（`library_view::playlist_detail / album_detail / artist_detail`，`with_db` 单连接单投影，失败 → found=false 诚实空态）。**详情页参数约定**：歌单 = DB id；专辑/歌手 = 展示名（远端 mid 要等内容接口，AUDIT §8.2）——`SongRow::to_track_row` 把 `artist_mid/album_mid` 填成展示名，TrackTable 空参禁用链接；歌手页对含分隔符的展示串（如 "张韶涵/HOYO-MiX"）按 `library_artists` 名字做最长包含归一。装卸点 = `bridge::apply_route`（navigate/back 共用）。

**诚实空态**：下载/已购两页（后端无此域，AUDIT §8.5；2026-09-29 删除 mock.rs）。

**已知偏差（M8 新增）**：
- 音质文案由 `actual_quality` 映射，无采样率（"FLAC" 而非 "FLAC · 44.1kHz"，诚实）；
- 最近播放时间戳为 UTC（桌面无 chrono/本地时区源，与 CLI `history` 口径一致，见 library_view::format_stamp）；
- 本地曲目键归一化（`canonical_local_key`，hmp-storage v5 迁移合并幽灵行）修复了收藏/歌单写路径与扫描器键格式分裂导致的"我喜欢显示裸键、0:00"问题；
- QQ 曲目封面 http → 程序化占位（UI 禁 HTTP，封面代理缺口 AUDIT §8.4）；本地曲目 `file://` 封面走真图；
- 队列点歌 = `Play(该曲 id)`，整队被该单曲替换（无 PlayAt IPC，AUDIT §8.8）；
- 队列行 QQ 曲目时长 0:00（`track_meta_batch` 无时长投影，AUDIT §8.11）；
- 客户端音质偏好暂不生效（与 daemon config.toml 两处存储，AUDIT §8.7）；音量以 daemon 推送为准（含 RG 补偿语义，AUDIT §8.12）；
- ~~歌手/专辑 mid 真数据行为空串~~（M4.5 收敛：详情页参数改用展示名，本地投影可跳转；QQ-only 歌手/专辑落"本地库无此内容"诚实空态，待内容接口换真 mid）；
- 库页是启动静态快照（无 LibraryChanged 事件，AUDIT §8.9）。

**后端缺口全集**：docs/AUDIT.md §8（12 条 + 汇总表）。

## 后端接口补齐轮（2026-09-07，AUDIT §8 大闭合）

桌面端需要的读/写接口全部落地（docs/AUDIT.md §8 状态表为权威）：

- **IPC 新面**（hmp-core/src/ipc.rs）：`QueuePlayAt(usize)`（事务式跳播，
  队列不被单曲替换；越界报错、点当前曲 = ensure-play）、`Search{keyword}`
  （免登录 smartbox）、`LyricGet{mid}`（LRC + 翻译；daemon 自查 song_type）、
  `AccountStatus`（登录态/uin/昵称/VIP 摘要）、`QualityGet/Set`（daemon 落
  config.toml，唯一事实源）、`CoverGet{url}`（QQ 封面下载进
  `<data_dir>/covers/<hash>.jpg` 回 file://，仅收 y.gtimg.cn 来源）、
  `Event::LibraryChanged`（watcher/sync/reconcile/收藏/歌单写触发）。
- **桌面接线**：队列抽屉点行 → QueuePlayAt；音量滑杆读写 `user_volume`
  原值（PlaybackState 新字段，serde default 兼容旧帧）；当前曲 QQ 封面
  异步 CoverGet 换真图（`Player.current-mid` 复核防串台，每 mid 每进程
  一次）；LibraryChanged → `bridge::refresh`（重查 sqlite + 重放当前详情
  路由，库页不再是启动静态快照）；音质四档选择即写 IPC、启动从 daemon
  同步（auto/未知别名保持现选）；设置账号页读 AccountStatus 真数据
  （昵称拉取受 QQ 服务端 10000 影响，回退 "QQ {uin}"，CLI 同失败）。
- **搜索页**（新 ui/search-page.slint + 侧栏入口）：关键词回车/按钮触发
  （不做即时搜索——Vue 版 300ms 防抖属于内容页里程碑再对齐）；结果仅
  歌曲（smartbox 窄投影无专辑/时长 → TrackTable 收起两列，时长显示 "—"
  而非 0:00）；点行播放该曲。Vue 版 SearchView 的专辑/歌手 tab 待内容页。
- **存储**：`track_meta_batch` 扩列 duration_ms/cover_uri（AUDIT §8.11），
  队列投影 QQ 曲目不再 0:00，本地行封面直接读盘。
- **真机冒烟**：release daemon + 帧协议脚本 13/13（搜索"薛凯琪 立春"
  真结果、歌词 LRC、账号、音质写读还原、QQMusicDownloads 三曲队列
  QueuePlayAt(2) 跳播不换队、LibraryChanged 推送、封面下载 file://）。

## M6 播放页收口（2026-09-09）

**曲目层取色（OKLab 管线移植）**：`src/track_theme.rs`（994 行，11 测试，数值与
TS 版零偏差）——oklab/binary-split/score 逐函数忠实移植（kmeans 不移：Vue 默认
refine:"none"）。UI 契约：新 `TrackPalette` global（stores.slint），Rust 随
封面+明暗权威写入整族；**作用域纪律对齐 Vue 的"只注入 overlay 子树"**：
`Theme.track-*` 保持静态品牌胡桃木永不被覆写（主界面消费），播放页/取色层消费
`TrackPalette.*`，双上下文复用组件（QualityBadge/VolumeControl/QueueDrawer）
经 `TrackPalette.ctx-*` 按 `Player.overlay-visible` 切换。接线点：
`apply_playback` 每帧推送按 `mid|url`/`mid|prog` 键去重（10Hz 零重算）；
QQ 真图回包同键重算；`clear_now_playing` → 品牌回退；主题循环/设档 +
`Theme.changed dark`（系统偏好漂移）→ `reapply_for_theme` 用缓存封面整族重算。

**环境层（背景）**：Vue `blur(80px) saturate(1.2)` 无 Slint 滤镜对应物 →
Rust 预烘焙：封面等比降采样（≤480）+ 3 趟盒滤波（半径随源宽 ~5%）+ 增饱和 ×1.2
进 `Player.ambient-cover`，UI 侧 `image-fit: cover` 等比铺满（**修掉旧 stretch
拉伸**）、opacity 0.25、124% 外扩；`ambient-tint` 渐变遮罩（透明 40%→端色）
对齐 Vue。控制台 backdrop-blur 无对应物，半透明端色底 + 发丝线近似。

**布局对齐 Vue**：舞台阅读列 68rem（1088px）居中；封面
`min(36%舞台高, 40%舞台宽, 336px)` + radius-lg + clip 圆角 + 深投影；标题
`clamp(28.8px, 4vw, 41.6px)`、喜欢图标随标题 ×0.8；控制台 72rem（1152px）
居中，控制键 40px/模式键 36px/主键 48px，hover 一律 accent-soft 圆底；
评论区顶端固定 112px（7rem）透明→端色过渡带（比例渐变会随内容高伸缩，
读作位置不定的"带"）。歌手/专辑行 → 详情页 TextLink（参数=展示名，M4.5
本地投影约定；点击同时收起 overlay——Vue 不收起但主视口被全屏层遮住，
桌面侧有意收口）。

**slide-bottom 入/退场**：内容收进 `slide := Rectangle`（y 自窗口底 0），
`entered` 由 16ms Timer 翻转起跳（挂载初值直达不触发 animate）；app.slint
挂载站 `overlay-ov || overlay-hold` 软卸载（同 QueueDrawer fade-hold 模式），
`leaving` 属性驱动滑出。

**队列抽屉 themed 态**：`Player.overlay-visible` → 贴右缘通高、底边停
`np-console-clearance`（120px）、右侧直角（四角独立 border-radius）；
底=专辑色渐变（亮 accent-soft→surface-2 60% / 暗 deep→surface-1 70%，
叠在面板中性底上、**圆角需同步重申**——方角兄弟会盖掉面板圆角）；hover 底
与当前行高亮走 accent/ctx；暗色下标题/正文换 deep-fg。

**视觉验收（headless 软件 renderer 的三不渲染，勿误诊为 bug）**：
`i-slint-renderer-software` 的 `draw_box_shadow` 是 TODO（阴影不画）、
`combine_clip` 忽略 radius（clip 圆角不生效）、纹理采样是定点步进=**最近邻**
（放大图呈块状）。圆角 clip/阴影/drop-shadow 在 femtovg（真机）都正常；
环境层烘焙源图给到 480 也是为把最近邻块压小（128 源 12px 块 → 1.67% 跳变）。
截图：`.shots/m6/`（light/dark/queue/comments 四视角，`--overlay --seed
[--theme dark] [--queue] [--wheel ...]`）。

## 已知偏差（相对 Vue 版）

- 数字等宽（tabular-nums）暂无 Slint 内建开关，接受默认字形；
- 播放页环境层/控制台无 backdrop-filter：模糊预烘焙进 ambient-cover（降采样+盒滤波+增饱和），控制台半透明底近似；
- 播放页歌手/专辑链接点击后收起 overlay（Vue 不收起；主视口被全屏层遮住，不收起看不到落点）；
- 播放页喜欢键仍为 mock 态（换曲回落未点亮，与原型一致）；
- 歌手列整体一个链接（→ 主歌手页），多人合唱逐人分链待 M4 评估；
- 文件大小公式修正了 TS 版的量纲 bug（`(kbps*1000*ms)/8` → `kbps*ms/8`，旧式单曲显示 ~26 GB）；
- 窗口标题栏 CJK 方框是 niri 装饰条字体问题，与应用无关；
- ~~**无窗口关闭按钮**~~：已由自绘标题栏补齐（`title-bar.slint` minimized/maximized/close 三钮 + winit drag 桥 + 8px resize 边，2026-09-09）。

## Slint 机制坑位（源码确证，2026-09-06）

- **has-hover 由命中的最顶层 TouchArea 独占**（i-slint-core input.rs：TouchArea 对 Moved 返回 EventAccepted，中止向更低兄弟的遍历；只沿祖先链传播）。行级 hover 传感必须把 TouchArea 做成内容的**祖先**（包住 @children），垫底兄弟永远收不到。行内更深的按钮仍各自拿 grab，不抢行 hover。
- **animate 只在绑定值变化时触发**（properties_animations.rs 的 ShouldStart 状态由原绑定脏标记驱动），恒定绑定永不动画；**负 delay 被当 `<=0` 即无延迟**，无 CSS 式错相。持续循环动画要用 Timer 驱动公式求值（Equalizer 的 EqClock 方案：33ms 共享时钟 + 余弦高度 + `paused ? frozen : t-offset` 三元暂停冻结——惰性分支不注册依赖，暂停零重绘）。
- **global 不能有子元素**（Timer 挂不进 global）；EqClock global + app.slint 挂 EqClockHost 组件驱动。
- **Flickable 滚动 = 隐藏 viewport 元素的 x/y（与 viewport-x/y 双向绑定）**，故 `absolute-position`（map_to_window 沿祖先几何累加）含滚动偏移且依赖是活的——集成测试的"上报矩形包含指针"不变量在滚动后 24/24 通过。
- **debug vs release 渲染差距巨大**：debug 全场景重绘 ~30fps、release ~200fps（FemtoVG+OpenGL，`SLINT_DEBUG_PERFORMANCE="refresh_full_speed,console"` 实测）。日常视觉验收用 `cargo run --release -p hmp-desktop`；持续动画（EQ）把重绘压到 30fps 采样。
- 集成测试基建：dev-dep `i-slint-backend-testing`（`init_no_event_loop` 进程级单例 → 一个测试二进制一个 #[test]）+ 公开 `Window::dispatch_event` 合成指针（`slint::platform::{WindowEvent, PointerEventButton}`）+ `mock_elapsed_time` 驱动 Timer；global 要在 app.slint `export {}` 后 Rust 侧才可读。
- **布局显式 `alignment` 会让 stretch 失效**（视觉验收发现的坑，2026-09-06）：HorizontalLayout/VerticalLayout 一旦写了 `alignment: start/end/...`，剩余空间按对齐分配、所有子元素取首选宽，`horizontal-stretch` 不再参与 —— "标题 stretch:1 + 右侧动作区"的 space-between 写法必须**不写 alignment**。曾导致 PageHeader 动作区/SectionHeader"更多"/hero 播放全部全部挤在标题旁。
- **绝对定位子元素默认在父级居中，不是 (0,0)**（2026-09-07 视觉验收确证）：非布局父级下，设了 width/height 但没设 x/y 的子元素落在 `((parent.w - self.w)/2, …)` —— 封面/标签列"莫名居中"都是它。要么显式 `x: 0`，要么干脆走布局（HorizontalLayout + cross-axis-alignment）。
- **布局子元素里引用 parent.width 算首选宽会成环**（2026-09-07）：`width: parent.width - self.x` 这类绑定让布局解出负宽（实测 -312px），子元素四散。布局内子元素的几何交给布局分配（fixed 宽 + stretch），不要自引用父宽。
- **HorizontalLayout/VerticalLayout 交叉轴默认 stretch**（读 i-slint-core layout.rs 确证）：定高子元素被顶对齐；可用 `cross-axis-alignment: center`（lower_layout.rs 的 "cross-axis-alignment" 绑定）或仓库旧的显式 `y: (parent.height - self.height)/2` 写法。**组件内固定高的部件（如 TextLink 的 `height: font-size*1.5`）放进高行布局同样被顶对齐**——曲目表专辑列文字上浮 11px（2026-09-08 用户报告，只有"有专辑链接"的行走 TextLink，故呈"部分歌曲"症状）；修法是在使用位点覆盖 `height: root.row-height`（不能写 `parent.height`——布局子元素引用父高会成环 layoutinfo-v→layout-cache→height）。
- **Slint 组件必须先声明后使用**（同文件内）：SettingsNav 引用 PillOption 需把 PillOption 放前面（或把使用方挪到文件末尾）。
- **圆角两轴独立钳制 → 999px 非正方形变椭圆**（2026-09-07 读 femtovg path.rs 确证）：Slint 圆角按横纵各自 `min(r, 边/2)` 钳制（`rounded_rect_varying`），CSS 是四角等比缩小到恰好相接——CSS `--radius-full: 9999px` 出胶囊端盖，Slint 同值非正方形画成**椭圆**（音质徽章 22px 高胶囊、开关 36×20 轨道都中招）。统一走 `Theme.pill(size)`（`= min(999px, size/2)`，theme.slint 纯函数），全部 radius-full 已改；另 PageHeader 返回键曾因布局纵向拉伸（36→84px）变竖椭圆——布局里定宽圆键要同时给显式 `height`。
- **animate 缺省缓动是 Linear**（EasingCurve derive Default）：Vue 全部过渡挂 `--ease-standard: cubic-bezier(0.2, 0, 0, 1)`（S 曲线），Slint 不写 `easing:` 就是线性起步、读作硬切。所有 hover 换色/淡入/几何 animate 显式补 `easing: cubic-bezier(0.2, 0, 0, 1)`（2026-09-07 全量补齐）；例外是 HoverGroup 滑块的前载 `cubic-bezier(0.22, 1, 0.36, 1)`（对齐 Vue 内联过渡）。
- **基座 TextInput 无 placeholder-text**（是 std-widgets LineEdit 的属性，直接编译错）；占位文案用覆盖 `if input.text == "": Text` 实现（search-page）。
- **HoverGroup 滑块必须每组私有（2026-09-08 重写）**：总线（HoverBus）只是运输层——hover() 上报携带组 id（HoverGroup init 时从 `next-group-id` 领号），组按 `active-group == group-id` 认领。**不要用几何包含判定**（行与组的 absolute-position 可能来自不同布局代际，实测偏差 ~4px，判定随机失败）；**不要把块几何直连全局总线**——跨组移动时淡出中的块会被拖去别组的行（混用滑块）。几何在组认领时由 `changed bus-seq` 回调拷入组本地属性（geo-x/y/w/h），块绑定本地几何 + HEAD 同款 animate（140/180ms、placed 门控）：跨组时本组 has-report 变 false → 块原地淡出、几何冻结。组 id 归属与布局无关，彻底消除混用。
- **testing backend 合成指针的命中测试落在"当时"的布局代际**：`changed width` 等处理器延迟到 `mock_elapsed_time` 才 flush，set_size(1280,800) 后立即 move_to(120,80) 会命中"侧栏仍收起(64px)"的旧代际——导航行 x≥90 全部 miss。测试开头必须**步进**推进 mock 时间（16ms×40）让布局与动画收敛；单次大步推进不会让属性动画走到目标值。
- **`--bus-hover` 签名变为 GROUP,X,Y,W,H[,suppress]**（GROUP = HoverGroup init 序号：1=侧栏 2/3=队列或监视文件夹 4=曲目表，以实际为准）；离屏截图宿主 `examples/shot-headless.rs`（testing backend + software renderer，无需合成器/焦点——niri 会话被 DMS 锁屏层抓走 focus 时用它做视觉验收）
- **Button 禁用 = 保留变体底色 + 整体 opacity 0.5**（Vue `.button:disabled`）；`!enabled → transparent` 的旧写法让深色下 disabled default 变体（搜索键/扫描键）只剩近黑文字、整键隐形，已改。
- **CLI 双轨坑（QA 数据准备）**：`hmp scan` 直读/直写 sqlite（跟随 XDG_DATA_HOME），`hmp playlist create/add` 走 daemon IPC（写 daemon 自己的库，与 shell 里的 XDG_DATA_HOME 无关）——沙箱库扫描 + CLI 建歌单会脑裂。沙箱 QA 要么全直读（歌单行手工 SQL），要么对真库验收（本轮 dshot3.sh 取消 XDG 隔离）。**dshot3.sh 只截图不编译**：cargo build 失败时它拍的是旧二进制，"改了没生效"先确认 build 真的成功。
- **逐行 stretch 列宽会漂移**：Slint 没有跨行共享的 CSS grid fr，每行自己的布局按"该行内容首选宽"分配 stretch 余量 → 短文本行的列位置肉眼可见地左右跳动（曲目表专辑列）。修法：按根宽算定宽（TrackTable 的 `grid-available × 1.4/2.2 / 0.8/2.2`；封面卡网格 `Theme.grid-card-width` 对应 Vue `repeat(auto-fill, minmax(9rem,1fr))`）。
- **舍入对齐**：TS `Math.round` 在 slint 侧用 `Math.round`、Rust 侧用 `f64::round`；别顺手写成 floor/div_ceil（时长"2 小时 5/6 分钟"、"m:ss" 都栽过）。
- **`absolute-position` 在 repeater 行 + 嵌套 layout 下双重累计 self.y**（2026-09-09 播放页实测，i-slint 1.17.1 headless）：行 abs-y − 容器 abs-y = 2×self.y（布局 y 被沿祖先链重复累计）——HoverGroup 式「差值法」在歌词页这种嵌套布局里会踩雷；歌词弹簧改用行组件直接上报 `self.y + self.height/2`（布局分配的 y 即内容系偏移，滚动不改它）绕开。做窗口系几何换算前先在 headless 下实测。
- 视觉 QA 基建：`examples/shot.rs`（`cargo run --release -p hmp-desktop --example shot -- <route> [--theme dark] [--playing] [--queue] [--hover X,Y] [--bus-hover X,Y,W,H]`，playing 走 `invoke_play_tracks` 让 PlayerHost 接管快照）+ `/tmp/hmp-qa2/dshot.sh`（niri 按 PID 找窗口 → focus-workspace/focus-window → `screenshot-window`，焦点校验+尺寸校验+重试）；Vue 对照用 `/tmp/hmp-qa/vshot2.mjs`（1420，视口 1018×1228 dpr=1.25，走真实主题循环切暗色）。winit 下 `dispatch_event(PointerMoved)` 不驱动 hover（testing backend 才行），hover 视觉用 `--bus-hover` 或真实指针。
- **非布局父级下的容器：width 与 height 都必须显式钉住（2026-10-03 窗口化 drag 回归实测）**：普通 Rectangle 容器的显式 height **不进 preferred 尺寸链**（repeater 无固有尺寸、子元素 width=parent.width 反向引用也不贡献 preferred）→ 外层布局把宿主压成 0 高 + `clip: true` → 整表不可见不可点；行 `width: parent.width` 引用一个靠子 preferred 推导宽度的容器 → 循环解出 0 宽 → 同样全灭。旧写法（VerticalLayout 包行）无此坑：布局会主动上报 preferred。修法：容器与宿主（track-table 的 hg 与行窗口容器）双双显式 `width/height`——布局对显式几何子元素是尊重的（表头 Rectangle 同布局内 `height: row-height` 即先例）。排查利器：testing backend + software renderer `Window::take_snapshot` 落 PNG 亲眼看渲染（`i_slint_backend_testing::TestingBackend::new(TestingBackendOptions { renderer_name: Some("software".into()), ..})`，普通 `init_no_event_loop` 无渲染器会报 take_snapshot not implemented）。
- **Slint 全局属性桥接唯一合法位 = property 声明处的双向别名**（2026-10-03 Viewport 实证）：元素作用域内语句级 `Global.a: expr;` 与 `Global.a <=> b.c;` 都是 parse error；全局 `in-out` 属性要在组件里声明 `property <T> p <=> Global.a;` 再由 changed 回调命令式回写，且 **changed 不保证初值触发**——初值必须在 `init` 里显式赋一次（Viewport.view-h 漏 init 会让窗口化静默退化全量渲染）。
- **for 整数即模型，无区间语法**（2026-10-03）：`for i in expr:` 迭代 0..expr-1（"use an integer as a model"）；`for i in 0..expr:` 直接报 "Range expressions are not supported"。另外 for 体内**属性不可命名为 `row`**（"Cannot override property 'row'"，基类无此属性也报）——行数据属性改名（rowd）绕开。
- **应用层缓存 `slint::Image` = 钉死 Slint 内部 5MB 纹理 LRU**（2026-10-03 内存审计）：`Image::load_from_path` 走 Slint 全局 5MB 权重 LRU，但应用层 HashMap 持有 Image 强引用后底层解码位图永不被逐出——任何手写无界 Image 缓存都是 RSS 单调增长（曾达 2.4GB）。统一走 `crate::cover_cache::get_or_load(path, max_side)`（字节加权 LRU 64MB + 装载即降采样，256=行/卡/侧栏、512=播放页大图），勿再手写。

## 构建与启动（M8 起）

- 构建：`cargo build --release`（桌面启动会自动拉起 daemon，需要 `hmp` 二进制在 PATH 或与桌面二进制同目录）。
- 测试：`cargo test -p hmp-desktop`（mock/nav/format 纯逻辑单测 + backend/library_view 投影单测）。
- 启动：`cargo run --release -p hmp-desktop --bin hmp-desktop`（niri 下逻辑目标 1024×1152 半宽 / 2048×1152 全宽）。
  - 启动即连接 daemon IPC 端点（Windows 命名管道 / Unix socket，同 `server::socket_path()`），连不上自动 `hmp serve --background`（CLI 同款）；失败降级离线（全空态、命令无效）。
  - 库页数据 = 启动时直读 `$XDG_DATA_HOME/hmp/library.sqlite3` 的静态快照：先 `hmp scan ~/Music` 入库本地曲目，QQ 侧 `hmp login` + `hmp library sync`。
  - 播放/收藏/歌单写全走 daemon（与 CLI/MPRIS 同一状态源）；`hmp quit` 后 UI 呈离线空态，不自动复活后端。
- 视觉 QA：`cargo run --release -p hmp-desktop --example shot -- <route> [--param <值>] [--theme dark] [--queue] [--playing]`（真实应用宿主，route 如 library/recent/local/downloads/purchased/playlist/album/artist/settings*）+ `/tmp/hmp-qa2/dshot3.sh <name> <args…>`（niri 截图；**不隔离 XDG_DATA_HOME**，直读真实用户库；daemon 需已在跑，播放态用 `hmp play` 预置）。

## Windows 适配（2026-09-09 落地）

- **IPC 传输**：`hmp-daemon/src/transport.rs` 平台抽象——Unix = domain socket，Windows = 命名管道 `\\.\pipe\hmp`（tokio `named_pipe`）。帧协议不变；端点统一 `PathBuf` 表示。服务端 `first_pipe_instance` 兼作单实例守卫（已绑定 → `ERROR_ACCESS_DENIED` → "already running" 退出）；accept 后立即补建下一实例，客户端 connect 对 `ERROR_PIPE_BUSY` 短重试兜底。并发读共用 `tokio::io::split`（NamedPipe 无内建 into_split）。
- **路径键规范化**：Windows `fs::canonicalize` 产出 `\\?\` verbatim 前缀，与用户拼写/事件路径/UI 展示全部失配（前缀匹配、查询、去重失灵）。统一走 `hmp_storage::{canonical_display_path, strip_verbatim}`——`begin_scan`/`scan_root_for`/`canonical_local_key`/v5 迁移/local `canonical_id`/`resolve_local` 全部收口。**新代码凡 canonicalize 结果要进库键或展示，必须过 strip**。测试构造期望键同样用该助手（TEMP 环境变量大小写与盘上真实大小写可能不一致）。
- **daemon**：`spawn_detached` Windows 走 `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP`（无 setsid）；信号 = `tokio::signal::ctrl_c()`；socket 文件清理/权限位（0600/0700）仅 Unix。MPRIS feature 不参与 Windows 构建（zbus 编译不过，勿开 `--all-features`）；SMTC 由 `#[cfg(windows)]` 无条件接入。
- **桌面端**：后端二进制解析带 `EXE_SUFFIX`（`hmp.exe`）；`xdg` 路径 Windows 走 Known Folders（见下方"数据目录"条，忽略 msys HOME）。
- **构建/启动**：`cargo build --release -p hmp-desktop -p hmp-cli`（`hmp.exe` 与 `hmp-desktop.exe` 同目录，桌面启动自动拉起 daemon）；CLI 全命令（status/play/pause/…）与 Linux 同语义。本地播放需先 `hmp scan <目录>` 入库。
- **测试口径**：进程级集成测试（`tests/daemon_cli.rs` ×2）用 Unix socket + SIGTERM，`#![cfg(unix)]` 门控；Windows 靠 lib 级 transport/server 测试 + 真机冒烟覆盖。
- **坑：无音频设备曾让"桌面拉不起 daemon"（2026-09-09 排查闭合）**：RDP 断开/VM/无声卡机器上，daemon 启动时 `open_default_output` 失败曾是致命错——进程秒死；桌面端表现即 SpawnTimeout → UI 离线，CLI `serve --background` 同样无声失败（中间进程 exit 0、孙进程即死）。且 detached 子进程 stdio 全 null、tracing WARN 落黑洞，零痕迹。修复在 `hmp-player/src/core.rs`：`PlayerCore::new()` 失败回退无设备静默 sink + 泵线程（时钟/EOS/自动切歌/SMTC 照常，仅无声；真机验证续播/暂停状态机正常）。排查口诀：**别盯着 spawn 链路猜，先前台跑 `hmp serve` 看真实错误**；另一坑——直接执行 `target/debug/hmp.exe` 不触发 cargo 重建，改完源码必须显式 `cargo build`，否则跑的是陈旧二进制（本次 debug 二进制恰早于修复、release 晚于修复，行为分裂一度误导排查方向）。链路诊断工具：`cargo run -p hmp-desktop --example spawn_probe`（无 GUI 逐步打印二进制解析/spawn/就绪探测）。
- **音频输出设备策略已平台分叉（2026-09-09）**：`hmp-player/src/core.rs` `collect_output_candidates`——Unix 保留 server-routed PCM 白名单（default/pipewire/pulse，防 ALSA plughw 直通独占，DAWN PRO2 事故回归守护，相关测试 `#[cfg(unix)]`）；Windows 走 WASAPI 共享模式全接纳：默认 render endpoint 优先、其余端点兜底，**不做设备名字面匹配**（Windows 设备名是本地化的，如"耳机 (DAWN PRO2)""扬声器 (Realtek(R) Audio)"——此前"无音频设备"是误诊，实为 Linux 名单过滤掉全部 Windows 设备，真机永远无声；本机实有 5 个输出端点）。诊断：`cargo run -p hmp-player --example probe_output`（列设备 + 按生产策略开流）。已知限制：流绑定开机时端点，播放中拔插/切换默认设备不自动迁移（需引擎级重建流，未做）。
- **数据目录 Windows Known Folders（2026-09-09 双数据目录事故）**：`hmp-storage/src/xdg.rs` 平台分叉——Windows：config=`%APPDATA%\hmp`、data=`%LOCALAPPDATA%\hmp`、cache=`%LOCALAPPDATA%\hmp\cache`；**有意忽略 HOME**（msys/git-bash 终端给子进程注入 Linux 形态 HOME，曾致终端拉起的 daemon 用 `~/.local/share` 而 GUI 线用 `AppData\Local`——同一用户两套库/两套播放状态，连 loop 模式都各有一份，表现为"EOS 后诡异重播"）。`XDG_*_HOME` 显式覆盖两平台均保留（测试依赖）。桌面偏好 `prefs.rs` 同步收口复用 `hmp_storage::config_dir()`（旧版读 HOME，GUI 拉起无 HOME 时静默不持久化）。遗留分裂目录 `C:\Users\<u>\.local\share\hmp`（tone 测试数据）确认无用后可手动清理。
