# 胡桃音乐 Web UI 设计语言 —— 通透胡桃木 · 共振

版本 v0.1 · 2026-09-05 · 覆盖 `apps/hmp-tauri`

本文档是 Web 前端的唯一设计基准。它不推翻原型既有的「通透胡桃木」token 体系，而是在其上补一层**动态色彩**与一套**页面语法**，让胡桃音乐拥有区别于 Apple Music / NetEase / Spotify 的可识别身份。

---

## 0. 立场：与参照系的关系

| 参照 | 取 | 舍 |
|---|---|---|
| Apple Music | 信息架构（侧栏 + 内容 + 底部播放条）、封面优先的排版 | 中性磨砂玻璃的中立感——我们的界面**跟随音乐有温度** |
| 网易云音乐 | 播放页向下滚动见评论的内容纵深 | 高饱和红色 chrome、强运营位 |
| 原型既有资产 | 胡桃木色阶、HoverGroup 滑动物理高亮、自绘滚动条、长进度条 | —— |

三条设计原则，也是三个可识别签名：

1. **声音有颜色（Color Follows Sound）** —— 当前曲目的专辑封面驱动一层环境色，整 app 随曲目换妆；品牌胡桃木色只负责身份与动作，不负责氛围。
2. **时间有刻度（Time Has Scale）** —— 进度条是播放页的第一公民：全宽、带刻度、可悬停增粗、可拖拽，像一把尺子横在内容与控制之间。
3. **内容即界面（Content Is the Interface）** —— 播放页向下滚动依次展开歌词与评论，装饰性 chrome 压到最低；歌词、评论是一等公民，不是弹窗里的标签页。

---

## 1. 色彩系统：两层结构

### 1.1 品牌层（静态，已有）

即 `styles/index.css` 的 walnut 色阶 + 语义色。用于：侧栏、导航、主按钮、链接、焦点环、品牌标识。**不随曲目变化**，保证 app 身份恒定。

### 1.2 曲目层（动态，新增 `--track-*` 变量族）

由封面取色模块驱动（见 §2），**v0.3 起只注入播放页 overlay 根元素**：其余界面一律消费 `:root` 的品牌回退值，保证全局观感统一——专辑色是播放页的专属氛围，不再外溢。变量仍随当前曲目切换并做 320ms 交叉过渡：

| 变量 | 用途 | 亮色模式 | 暗色模式 |
|---|---|---|---|
| `--track-accent` | 播放态强调：进度条已播段、正在播放行、播放按钮 | OKLab L≈0.60 | OKLab L≈0.68 |
| `--track-on-accent` | accent 上的文字/图标 | WCAG ≥ 4.5 自适应黑/白 | 同左 |
| `--track-accent-soft` | 播放页内 hover 底、选中底、徽章底（列表页一律中性底，见 §1.2 约束与 v0.5） | accent @ 12% alpha | accent @ 20% alpha |
| `--track-deep` | 播放页/播放列表的深色面板底 | OKLab L≈0.35 | OKLab L≈0.22 |
| `--track-deep-fg` | deep 面板上的正文 | 暖白 | 暖白 |
| `--track-grad-from/to` | 播放页环境渐变两端 | accent-soft → surface | deep → neutral-950 |
| `--track-equalizer` | 正在播放动画条颜色 | accent | accent |

无封面 / 取色失败时整族回退到 walnut（`--track-accent: var(--accent)` 等），**UI 不得出现无色状态**。

约束：
- 曲目层永远不做**大面积正文底色**，只做氛围、强调与面板；
- **列表与导航一律中性底**（v0.4 收紧）：行 hover = `--neutral-200` 滑动块、选中态/徽章/chip = `--muted`，主题色（品牌或曲目）只保留给播放态指示（equalizer、正在播放行标题）与主动作按钮——氛围色不进列表；
- `--track-accent-soft` 上的文字一律用 `--foreground`，不得依赖对比度不可保证的组合；
- 所有曲目层颜色写入 CSS 变量后**组件只消费变量**，取色算法可整体替换。

---

## 2. 主色提取模块（解耦规格）

位置：`src/lib/color/`，纯 TypeScript，**不依赖 Vue / DOM**；入口与实现分离，后期可整体替换（如换 CNN 或主题库）而不动 UI。

```
index.ts          对外 API：extractPalette(pixels, options): TrackPalette
oklab.ts          sRGB ↔ 线性 RGB ↔ OKLab/Oklch 转换（色彩科学核心）
binary-split.ts   二分切分量化（默认，快）：沿最宽轴递归对分
kmeans.ts         可选精修：2 轮加权 k-means（默认关闭）
score.ts          聚类评分与 accent 挑选
adapter.ts        DOM 适配：图片 URL / ImageBitmap → 像素数组（OffscreenCanvas，64×64 降采样）
```

算法约定：
1. **空间**：一切计算在 OKLab（感知均匀）中进行；只在进出口做 sRGB gamma 处理。不用 HSV——它在蓝紫区非线性失真严重。
2. **量化**：默认二分切分（用户点名的"更快的二分"）：每次沿 L/a/b 中跨度最大的轴按中位数对分，至 8–12 叶；每叶记均值与像素数。可选 `refine: "kmeans"` 做 2 轮加权 k-means 精修。
3. **评分**：`score = chroma × 1.0 + coverage × 0.5`，其中 chroma = OKLch C（饱和度即"音乐性"），coverage = 叶像素占比的平方根（避免大面积背景垄断）。L 超出 [0.20, 0.90] 的叶直接淘汰（近黑近白不当主色）。
4. **accent 整形**：胜出色把 L 拉到目标窗（亮色模式 0.60±0.03，暗色 0.68±0.03，C 不足 0.04 时视为无彩色，启用回退）。
5. **可读性**：`onAccent` 按 WCAG 2.1 相对亮度对比在黑/白之间二选一，目标 ≥ 4.5:1。
6. **派生**：deep = accent 降 L 至目标值并去 20% 彩度；grad 两端由 deep 与 surface/neutral 混合；全部在 OKLab 插值后转回 sRGB。

测试（vitest）：纯像素数组进出，构造纯色/双色/渐变/无彩色图断言主色与回退；不 mock canvas。

---

## 3. 页面语法

### 3.0 通用件

- **PageHeader**：返回箭头 + 大标题（clamp 1.6–2.2rem，字重 650）+ 元信息行；吸顶时收缩为单行。
- **TrackTable**（曲目表）：列 = 序号/标题+歌手/专辑/时长；hover 行浮起中性滑动块（`--neutral-200`，与侧栏同款 HoverGroup），播放图标经 HoverGroup indicator 槽随滑动块纵向滑入序号位替代序号（不逐行淡入）；正在播放行显示 3 根 `--track-equalizer` 跳动条替代序号且优先级最高（hover 图标滑到其上时让位），标题 `--track-accent` 强调。数字一律 `font-variant-numeric: tabular-nums`。
- **CoverCard**（封面卡）：圆角 `--radius-lg`、`--shadow-md`，hover 上浮 2px + 阴影加深 + 播放浮层按钮；标题两行截断。
- **SectionHeader**：区块标题（1.15rem/650）+ 右侧"更多 →"；间距 `--space-8` 起节。
- 列表页统一用自绘 `Scroll` 组件；长列表预留虚拟化接口（v0.1 直接渲染，mock 数据量 ≤ 200 行）。

### 3.1 播放页 NowPlaying（签名页）

入口唯一：点击 PlayerBar 任意思源 → 全屏 overlay（沿用 slide-bottom 过渡）；主体由 `nowplaying/NowPlayingBody` 独占承载，不设 `/now-playing` 路由。ESC / 左上收起按钮关闭 overlay。

自上而下：

1. **环境层**：`--track-grad-from → --track-grad-to` 纵向渐变铺满；封面放大模糊 80px 置底 @ 25% 透明度，形成"光从音乐里透出来"的底。
2. **Hero**：左封面（`min(38vh, 340px)` 方形，`--radius-lg`，`--shadow-lg`）；右侧曲目信息——歌名 display 级（clamp 1.8–2.6rem/700）、歌手/专辑行（可点击跳转）、音质徽章（如 `FLAC · 44.1kHz`，`--track-accent-soft` 底）、动作排（喜欢/收藏到歌单/下载/更多）。
3. **刻度进度条（签名组件 RulerProgress）**：
   - 全宽横贯 hero 之下；静息高度 3px，hover/拖拽 6px，200ms 过渡；
   - 刻度：每 10% 一根 1px 细线（高 8px，前景 @ 18%），四分之一处（25/50/75%）加高至 12px；已播段覆盖为 `--track-accent`；
   - 拖拽：圆形 thumb（12px，白底 accent 环）仅 hover/拖拽出现；拖拽中时间跟随 thumb 显示气泡；落点即 seek；
   - 两端时间（已播/剩余总长）tabular-nums，剩余总长以 `-3:45` 形式。
4. **控制排**：居中主控——随机 / 上一曲 / **播放（48px 圆，`--track-accent` 底 + onAccent 前景，图标 24px；v0.12 由 60px 收敛，主次层级靠 accent 填充而非尺寸差）** / 下一曲 / 循环模式；左侧次控——**音质徽章 + 音量**（与播放页控制台同构，v0.11 统一到左）；右侧——**播放列表**（开/关抽屉）。
5. **歌词区**（首屏舞台右列，内部自滚，v0.9 重写、v0.12 取景重写）：Apple Music 式大字排版（clamp 1.3–1.6rem / 字重 600，行高 1.5），当前行字重 700 + 逐字扫色 accent；各行亮度/模糊/缩放按「行中心 ↔ 视口中心」像素距离逐帧衰减——焦点跟随视口（自动跟随时=播放行，手动滚动时=视口中心行），**歌词列不渲染任何滚动条**（Scroll `hide-scrollbar`）；上下缘 mask 渐隐；自动跟随 = rAF 临界阻尼弹簧把当前行推向视口垂直居中（ω=12、ζ=1，可随时打断）；用户手动滚动后暂停跟随 3s；点击任意行/词 seek 至对应时间。
6. **评论区**（继续下滚，网易云式；v0.10 重排，可读性优先）：
   - 吸顶小节头**无面板底**（白条/毛玻璃浮在氛围上即是"框"）：流内完全透明；真正吸顶后才经 IntersectionObserver 淡入全宽同色渐隐幕（`--track-grad-to` 纵向渐透明，无 blur 无边缘线），防正文从标题下穿越。`评论 5.6万`（总数 tabular-nums 灰字）+ 下划线排序 tab（最热 / 最新），激活下划线为 2px `--track-accent`、240ms S 曲线滑入（呼应刻度进度条）；
   - 发丝线分隔的编辑部式列表（条目间 1px `foreground 7%` 分隔线；无卡片、无整行 hover 底——评论不是导航行），每条一行一事：身份行（头像 36px 圆 + 昵称 600 + 属地灰字 + 置顶 `--track-accent-soft` 胶囊徽标）→ 正文 0.95rem/1.75 → 证据行（时间居左，点赞 + 回复居右，数字右对齐成可扫列）；
   - 回复以半透明引用块缩进（`foreground 4.5%` 板，随明暗主题与专辑底自适应；最多 2 条 + "共 N 条回复 ⌄"accent 链接）；`np-comments` 顶端 transparent → `--track-grad-to` 7rem 过渡带——环境层固定在视口而评论区随内容滚动，实心底会撞出随滚动的移动硬缝；
   - accent 语义收敛，只标记状态与导航：已赞（实心心形 + accent 计数）、激活 tab、展开回复链接、置顶徽标；热评靠排序本身传达，**不再**给前 3 条点赞数染色（与已赞态撞色）；点赞/回复/发送为 mock（不发送）。
7. **播放列表抽屉**：自右滑入（宽 380px，`max-width: 90vw`），**配色跟随打开上下文**（v0.3，v0.4 收紧）：从 PlayerBar（主界面）打开用中性表面面板（surface 渐变 + 中性 hover）；从播放页 overlay 打开则整只抽屉注入专辑色（`themed`）——绿色环境上不得再叠品牌红面板。行 = 序号/标题/歌手/时长，当前行 accent 高亮 + equalizer；行尾操作（移除）；顶部"播放列表 · N 首" + 清空。位置与定高规则见 v0.7，遮罩与开关行为见 v0.8。

**v0.2 增补（2026-09-05）**：

- **全屏接管**：播放页只有全屏 overlay 一种形态——侧栏与 PlayerBar 保持不动，overlay 铺满窗口（z-index 最高层）；ESC / 左上收起按钮关闭 overlay。（2026-09-05 移除 `/now-playing` 路由直达：双入口导致状态重复、路由页又没有退出路径，只保留 overlay。）
- **常驻控制台**：刻度进度条 + 控制排固定在播放页底端（毛玻璃底 + 顶部发丝线），不随歌词/评论滚走。控制排 = 左（音质徽章 + 音量）/ 中（随机·上一曲·播放·下一曲·循环）/ 右（评论·列表跳转）。
- **音质徽章 QualityBadge**：显示实际生效档位 = min(用户偏好, 曲目最高档)；点击弹出四档选择（标准/高清/无损/Hi-Res），超出曲目提供的档位禁用；偏好 localStorage 持久化（`lib/qualityStore.ts`）。PlayerBar 与播放页控制台共用。
- **竖向音量 VolumeControl**：静息只有图标；悬浮/点击向上展开竖向滑条（QQ/网易云式）。展开动画与数值平滑由 rAF 驱动；拖拽为手写实现（pointerdown 后 move/up 挂 window，不依赖 setPointerCapture）；图标点击 = 静音切换（图标随状态切换 volume_up / volume_off 资产），滚轮/方向键微调。弹层为**细长窄条**（v0.11：宽 2.75rem，数值 + 4px 细轨，高 160px），不做成宽白板。PlayerBar 与播放页控制台共用；播放页控制台居左侧（工具簇 = 音质徽章 + 音量），PlayerBar 居右侧（v0.12：与播放列表同簇，左侧只留音质徽章）。
- **逐字歌词**：`LyricLine.words`（QRC 式 startMs/endMs）；当前行按字/词扫色（done 全色 / active 扇形推进 / todo 未唱），扫色进度直写 CSS 变量不进渲染管线；渐变软边端点与 done/todo 颜色像素全等、交棒前钉 `--p=1`、`.word` 无 color 过渡——起唱/收字均无跳变（v0.12）；桥 250ms 快照之间用 rAF 插值，外推上限 500ms 防止后台标签页 rAF 冻结导致位置飞走。

**v0.2.1 增补（2026-09-05，播放页首屏舞台定稿）**：

- 首屏改为左右舞台：左列 = 封面，**曲目信息（歌名+喜欢裸图标 / 歌手 / 专辑）随封面之下**（v0.12：动作排与音质徽标移除，音质只留控制台徽章）；右列 = **整屏歌词**（内部自滚、自动跟随当前行）。歌词常驻首屏，不再单设第二屏歌词区（原"锚点头"小节随之移除）。
- 上滑一整屏即达评论区；舞台**必须定高**（`height: 100cqh; min-height: 30rem`）——歌词列内容很长，`min-height` 下 flex 行高会取其内容高度把舞台撑到数千像素，滚动节奏即被破坏。

**v0.3 增补（2026-09-05）**：

- **配色收窄**：`--track-*` 动态变量只注入播放页 overlay（`trackPaletteVars` 经 PlayerOverlay 根元素 style 绑定下发）；侧栏、内容页、PlayerBar、抽屉等全部消费 `:root` 品牌回退值。取色仍在 App 级预热缓存，overlay 打开即有色。
- **niri 平铺适配**（目标环境：2560×1440 @1.25 → 逻辑 2048×1152；半宽 1024×1152 主目标，⅓ 宽 683×1152 次目标；以精确逻辑视口截图验证，缩放仅用于核对实际观感）：
  - **≤52rem**：侧栏自动收成图标栏（`MainLayout` matchMedia 驱动；手动切换优先，跨越断点后重置回自动）。图标栏隐藏账户文案/歌单区，导航留 title 提示。
  - **≤48rem**：歌单/专辑页头部竖排（grid 单列，v0.5 起为两段式布局），曲目表保持整行宽度；播放页 hero 竖排居中（已有）。
  - **≤42rem**：曲目表隐去专辑列（`grid-template-areas` 收为四列），保住标题可读。
  - **比例**：播放页封面（v0.12 定稿）`min(36vh, 40%, 21rem)` 随窗高走（半宽主目标下 336px；v0.9 起随左右舞台重排，旧 `clamp(15rem, 38vh, 26rem)` 作废）；舞台限宽 68rem、侧距 `--space-10`（左列离窗框更远）、歌词区 56rem、评论区 46rem，全宽下形成收放节奏。

**v0.4 增补（2026-09-05）**：

- **歌词屏封面锚点**：歌词区改用 `100cqh` 恰好一屏（原 `calc(100vh - 14rem)` 魔数），顶部加锚点头——迷你封面（56px，`--radius-md`）+ 歌名/歌手纯文本；封面由首屏 416px 随上滑"收缩停靠"进歌词屏，不再滚走即消失（QQ/网易云竖版范式）。锚点头宽窗下与居中歌词同轴，≤68rem（半宽主目标）靠左与 hero 大封面左缘对齐；歌词区顶部 padding 4rem 让开左上收起按钮。
- **歌词尾部留白** 30vh → 40vh：窗体变满屏后最后一行仍能滚到居中位。

**v0.5 增补（2026-09-06，列表页降噪 + 专辑页重排）**：

- **主题色从列表撤退**：所有列表页（TrackTable、队列抽屉主界面态、侧栏、搜索 tab、排行榜角标、首页继续播放卡、歌手卡 hover）的 accent-soft 氛围底全部改为中性（hover = `--neutral-200` 滑动块、选中/徽章/chip/面板 = surface 系）；主题色只保留播放态指示（equalizer、正在播放行标题）与主动作按钮。队列抽屉 themed 态（overlay 内）维持专辑色不变。
- **专辑/歌单页两段式重排**：原"封面+正文同列 flex"把曲目表关在右列、列表被挤掉约 ⅓ 宽度；改为 grid 两段式——头部 `auto minmax(0,1fr)`（封面 | 标题/元信息/简介/动作）+ 曲目表**通栏全宽**，与歌手页 hero 同构；`align-items: center` 平衡头部两列，≤48rem 头部竖排单列。

**v0.7 增补（2026-09-06，播放列表抽屉定高与补齐）**：

- **定高锚定（修纵向不稳定）**：抽屉原先只有 `top/right` 锚点没有高度约束，高度随内容增长——队列一长直接溢出视口，内部 Scroll 因拿不到确定高度而完全无法滚动。现在两端锚定：主界面态 = 右上浮板（`layout-gap` 内缩 + `radius-lg` 全圆角 + 全边框，与内容区/PlayerBar 的浮板语言一致），底边停在 PlayerBar 上方（`gap×2 + player-bar-height`）；themed 态 = 贴 overlay 右缘、底边停在底部控制台上方（新 token `--np-console-clearance`，7.5rem），左缘圆角、控制台按钮保持可点。
- **头部补齐**：标题 + 元信息行（N 首 · 总时长，跨小时按"X 小时 Y 分钟"）+ 清空 + 关闭；清空走 PlayerBridge 附加式 `clear()`（桥未实现时控制器逐首 `removeAt` 兜底），空队列自动隐藏。
- **空态**：队列清空后显示居中提示，PlayerBar 同步回到"未在播放"。
- **纵向稳定性验收**：150 首注入实测——顶/底滚动、末行完整可见（尾部留白 `space-6`）、深行 `playAt` 与 PlayerBar 同步、hover 滑动高亮在长列表定位正确、清空→空态、亮暗两态与主界面/themed 双上下文，全部通过，无页面错误。
- browser 桥默认队列由"步长 23 取样"改为均匀取样 8 首（原步长在 ~45 首的池子里只能取到 3 首）；App.vue 新增 dev-only `window.__hmpPlayer` 测试钩子（`import.meta.env.DEV` 守卫，不进生产产物）。
- **幻影滚动条修复（Scroll 组件）**：`handleResize` 的 viewport 取自 `contentRect`（分数值，如 944.6875），而 `scrollHeight` 是取整值（945）——内容恰好放得下时会算出"差 0.3px 可滚"，thumb 被置为可见且铺满全高；满高 thumb 压在容器右缘，鼠标扫过即被点亮（队列抽屉 8 首时滚动条常驻即此因）。修法：viewport 进比较前取整，与 scrollHeight/clientHeight 同一粒度。全 app 的 Scroll 区域一并受益。
- **行尾死间隔修复**：Scroll 容器带内联 `width: 100%`，QueueDrawer 用负 margin 撑边时被显式宽度压住——列表实际只到内容盒宽（330px），行尾常年悬着 ~50px 空白。修法：该处 Scroll 传 `width="auto"` 让负 margin 重新生效（项目内仅此一处用此技巧）。同时行尾交互改为网易云式：时长贴右缘（与行首同为 17px），悬停/聚焦行时时长淡出、移除按钮原位浮现（`position: absolute`，不再常驻占位列），移除按钮 hover 底用 `--queue-hover` 与上下文联动。

**v0.8 增补（2026-09-06，队列抽屉遮罩与导航收敛）**：

- **弹出遮罩**：抽屉弹出时全屏遮罩压暗其余页面（dark 下压暗加重 0.2→0.45），点击遮罩任意处关闭；遮罩层级夹在播放页 overlay（`--z-overlay`）与抽屉（`--z-modal`）之间，主界面/themed 两种打开上下文行为一致。
- **开关语义**：PlayerBar 与播放页控制台的"播放列表"按钮改为 toggle（`toggleQueue`），开着时再点即收起；ESC / 关闭按钮 / 遮罩点击不变。
- **侧栏收敛**：移除"试听列表"导航项，队列入口收敛到播放条与播放页控制台。
- **侧栏单一滑块**：主导航、歌单分组入口行、二级返回行（含新建按钮）、歌单小项并入同一个 HoverGroup，整条侧栏滚动区只有一个滑动高亮块连续滑动；"我的歌单"为纯标题不停留。
- **侧栏悬停定稿**：滑块 = `--neutral-300` @ 0.6（原 neutral-200 过弱），组内按钮自身的 Button hover 底全部抵消——含激活项上漏出的白色 `surface-3`（特异性按 scoped 编译后 (0,8,0) 压过 Button.vue 的 (0,7,0)）；显隐 240ms 淡入淡出。曲目表滑块维持 `--neutral-200` @ 0.6，且经 `.track-rows` 负外边距外扩 0.75rem：高亮带比内容宽一圈（行内边距即内容到带缘的 padding），内容两缘与节标题/更多的对齐不受影响。
- **深色配色中性化**：主界面深色套的中性色/表面/边框整体去棕（surface-1/2/3 → `#1D1D1B` / `#2A2927` / `#3A3835`，border/track 同步），避免大范围暖棕底；品牌 walnut 只保留给主动作按钮、播放态强调等小面积元素。播放页 overlay 的专辑色氛围（`--track-*` 家族与取色 anchor）按 §1.2 设计不变。

### 3.2 内容页（后端已有能力、本版新增界面）

| 页面 | 路由 | 数据源（后端） | 结构 |
|---|---|---|---|
| 发现 | `/discover` | RecommendApi 五类 | 每日推荐 hero 卡（日期排印）→ 猜你喜欢（TrackTable 前 5 + 更多）→ 新歌速递（CoverCard 横排）→ 排行榜精选（榜单卡）→ 推荐歌单（CoverCard 网格） |
| 排行榜 | `/top`、`/top/:id` | TopApi | 榜单卡网格（封面 + 名 + 更新时间 + 前 3 预览）；详情页 = PageHeader + TrackTable（带名次与升降箭头） |
| 歌手 | `/artist/:id` | SingerApi | 大字名 hero + 歌曲139/专辑26/MV3 统计带 + tab：热门歌曲 / 专辑(CoverCard 网格) / MV / 相似歌手(圆形卡) / 简介 |
| 专辑 | `/album/:id` | AlbumApi | 封面 + 元信息 + 简介折叠 + TrackTable + "收藏专辑" |
| 歌单 | `/playlist/:id` | SonglistApi | 封面 + 创建者 + 标签 + 播放全部 + TrackTable |
| 搜索 | `/search` | quick_search | 大搜索框（自动聚焦）+ 三 tab（歌曲/专辑/歌手）+ 空态插画文案 |
| 最近播放 | `/library/recent` | recent_plays | TrackTable（含相对时间列） |
| 音乐库 | `/library` | api.library（liked/created/favorited） | 我喜欢 hero 卡（播放全部）→ 喜欢列表 → 最近播放预览（更多→最近播放页）→ 创建/收藏歌单网格 → 本地音乐空态 |

**本版全部跑 mock 数据**（`src/lib/api/`），接口形状即未来接线的契约（§4）；封面一律用**程序化 SVG data-URL**（按种子生成确定性的封面图，既可测取色又不依赖网络）。

**v0.6 增补（2026-09-06，音乐库与设置页）**：

- **音乐库 `/library` 落地**（原占位）：PageHeader 元信息（喜欢的歌数 · 歌单数）→ 我喜欢 hero 卡（中性表面卡 + 心形渐变封面与侧栏"喜欢的收藏"同款，播放全部接 `playTracks`）→ 喜欢列表 TrackTable → 最近播放预览 5 首（`more-to` 指向最近播放页）→ 创建/收藏歌单 CoverCard 网格 → 本地音乐诚实空态（后端未接线）。数据走新增的 `api.library.liked/created/favorited`（mock 由总池与歌单种子确定性派生，接口形状即未来 daemon Favorite / 歌单收藏契约）。
- **设置四页落地**（原 §3.3 明确不做，本版起实现）：`/settings` 总览 = 三张分类卡（常规/播放/账号，hover 上浮同 CoverCard 物理）；三个子页共用 `SettingsNav` 胶囊导航（路由驱动 active，中性 muted 底）。常规页 = 主题模式三选（新增 `themeStore.setThemeMode`，真实生效并持久化）+ 启动页/语言（未接线档位禁用展示）；播放页 = 音质四档单选卡（复用 `qualityStore`，与 PlayerBar 音质徽章联动）+ 默认音量横条（读写 PlayerController，无 player 环境整组隐藏）+ 播放行为开关（依赖 daemon 能力，禁用 + 说明）；账号页 = 本地资料卡 + 只读信息行 + 退出登录禁用（账号体系未接入）。
- **诚实空态纪律**：所有 mock 期无法真实生效的设置项一律**禁用 + 说明文案**，不做假开关；设置组卡片 = surface-3 圆角卡 + 行间发丝线，控件全中性底，主题色只保留给单选圆点等主动作指示。

**v0.9 增补（2026-09-06，歌词滚动重写 · Apple Music 式）**：

- **弹簧跟随**：跟随滚动弃用原生 `scrollTo({ behavior: "smooth" })`（连续换行时顿挫、无法打断），改在歌词 rAF 循环里跑临界阻尼弹簧（ω=12 rad/s、ζ=1，半隐式欧拉积分，dt 截断 64ms）直写 scrollTop——连续换行平滑衔接、大跨度 seek 一段滑到位；用户手动滚动期间弹簧贴住实际位置（恢复跟随时不跳变），3s 后自动接管；换词表/换曲 snap 直达当前行不从头滚；`prefers-reduced-motion` 下免动画直达。行/词索引仍走低频响应式，扫色进度照旧直写 CSS 变量。
- **景深与渐隐**：行透明度按到当前行的距离衰减（1 / .62 / .48 / .40 / .34），距离 ≥2 行渐次 blur（0.5 / 1 / 1.6px）出景深；歌词列上下缘 `mask-image` 渐隐（7% / 92%），边缘行淡出而非硬裁切。
- **排版**：字号 `clamp(1.3rem, 0.95rem + 0.85vw, 1.6rem)`、字重 600，当前行 700；**去掉 scale 1.05**（Apple 不缩放当前行，居中排版下 transform 缩放引起亚像素抖动）；翻译 0.78em 灰度随行下挂。
- **逐字扫色**：done = accent、active 软边扇形推进（渐变两端 ±0.06em 羽化）、todo 改中性前景 58%（原 accent 混色在暗色下发闷）；悬停非当前行快速提亮至 .85 便于找行，移出慢速沉回景深。
- **首尾留白**：歌词 Scroll 开 `fill`（父级链定高），行列表 padding 45cqh / 52cqh——第一行/最后一行也能滚到视口垂直居中位（替代 v0.4 的 40vh 尾部魔数）。
- **≤48rem 兜底**：歌词列由定高 52cqh 改为弹性填满舞台剩余高度（`flex: 1 1 0; min-height: 12rem`）——定高会把居中基准沉到常驻控制台底下。

**v0.12 增补（2026-09-06，歌词取景重写 + 首屏信息排简化）**：

- **景深取景随视口**：行亮度/模糊不再按「索引距播放行」阶梯取值——旧法在用户手动滚动后景深仍锁死在播放行；改按「行中心 ↔ 视口中心」像素距离逐帧计算，焦点随视口走（跟随播放时=播放行，滚动时=用户所视行）。透明度指数衰减 ≈ 旧阶梯（1→.34），blur 半行距起 2.7 行距满（0→1.6px）；先批量读 rect 再批量写样式避免逐行读写交叉触发强制布局，写入按取整精度门限去抖（静置零样式抖动）；衰减长度按前两行中心距伸缩，不硬编码行高。
- **焦点行缩放回归**：v0.9 曾去掉 scale 1.05（离散类切换引起亚像素抖动），现缩放随取景逐帧连续变化无跳变；`transform: scale` 1→0.92（2.2 行距内收满），不动布局，视口中心行最大。
- **逐字扫色端点全等**：active 渐变软边端点改为 `--p*(100%+0.24em)-0.12em`，p=0/p=1 时与 todo/done 颜色像素全等（边带完全推出文字外，截图 diff max≤7/255）；交棒前先把上个词 `--p` 钉到 1，换行清除行内扫色残留；`.word` 去掉 color 过渡——旧过渡让收字瞬间以 transparent→accent 淡入（背景已消失），是"播完跳色"的根源。
- **歌词列无滚动条**：Scroll 新增 `hide-scrollbar` prop——thumb 元素不渲染、ScrollBar 整体不构建（零开销）；歌词列为首个使用者，其余滚动区不变。
- **首屏信息排简化**：移除信息区音质徽标（音质只留控制台 QualityBadge）与收藏/下载动作排；喜欢改纯图标（裸 icon、无框无底、hover 只变色），右对齐标题行尾并加负右距贴齐封面右缘；图标尺寸取标题字号的 0.8 倍（共享 `--title-size` clamp 变量，随窗缩放保持字形对齐）；点亮态 = 红色实心（`--like-red #e5484d`，hover 加深），mock 状态点击切换、换曲回落（数据层未就绪）。
- **PlayerBar 布局回调**：音量从左侧工具簇移回右侧（与播放列表同簇，队列按钮仍居最右角）——左侧工具簇紧挨封面+歌名，再塞音质+音量排版失衡；左侧现只留音质徽章随曲目信息。播放页控制台左簇（音质+音量）不变。
- **静音图标修复**：静音态弃用 CSS 叠加斜杠（`::after` 无定位、飘在图标右下角），新增 `volume-off-rounded.svg`（Material volume_off，与 volume_up 同族），按静音状态切换图标资产。
- **收起键裸图标化**：播放页左上收起键去底框/阴影/毛玻璃，chevron 转向下（收起方向语义，expand-more）；墨色新增 `--track-on-ambient`——调色板管线按环境渐变起点 `gradFrom` 与暖黑/暖白两极（`#342827`/`#FAF9F8`）的 WCAG 对比度自动取深浅，idle 75% 透明度 hover 提满，随专辑与明暗模式自动切换。
- **歌单区宽窄双形态**：宽侧栏 = 分组折叠旧样式（分组标题行 + chevron + `grid-rows 0fr/1fr` 高度过渡 + 组内新建按钮）；窄边栏 = 两级滑动导航——一级为自建（播放列表图标）/收藏（**书签图标 `bookmark-rounded`**，避免与播放列表图标、"我喜欢"心形撞形）两个入口行，点分组整块左滑进二级：返回行 + 纯封面列表，**二级行与主导航行同规格**（同 `sidebar-button`、等高 2.5rem、图标 1.35rem、窄栏居中）——滑块几何与对齐和上方导航完全一致，返回键滑回。两级滑动的工程要点：激活 pane 回到文档流瞬时撑起容器高度（高度切换发生在动画之外），非激活 pane 绝对定位停视口外，两侧仅 translateX 过渡 + 容器 `overflow: hidden` 裁切——动画全程零高度重排；非激活 pane 挂 `inert` 防焦点落入隐藏区；`prefers-reduced-motion` 免动画；侧栏 hover-item wrapper 上 `display: flex` 消除块级 div 包 inline-flex 按钮的基线缝隙（否则 wrapper 比行高零点几~1px，滑块高度参差）。宽窄切换时两套 UI 以 150ms 交叉淡入淡出（离开侧绝对定位脱流防叠排）；侧栏所有行内文字（导航标签/分组名/歌单名）一律 `white-space: nowrap + ellipsis`——宽度过渡期间文字单行截断而非折行，杜绝行高剧烈变化（隐藏文字用 `span:last-child:not(.app-icon)`——纯图标行唯一 span 同时是 last-child，不能误杀）。
- **文字链接交互统一**：所有纯文字链接/按钮（表格内歌手/专辑名、专辑页歌手、播放页歌手/专辑、"更多"、简介展开、Button link 变体）统一套 `.text-link` 全局工具类（index.css）——基色 muted，hover 颜色过渡提亮至 foreground，**不用下划线**；唯一例外「展开回复」保留自绘 1px 下划线（`background-size` 过渡：hover 从左向右展开、移出原路收回）。
- **控制台跳转键纯图标化**：评论/列表去掉文字标签改纯图标，命中区与控制键同规格（2.25rem 方形，语义由 title/aria-label 承载）；换用 outlined 轻量图标（新增 `comment-outline-rounded` 空心气泡、`queue-music-outline-rounded` 2px 线风格——细杆 + 空心符头），消解实心气泡的视觉重感；PlayerBar 的队列按钮保持实心（与其旁实心音量图标同簇协调）。

### 3.3 明确不做（本版）

收藏/资料库云端同步页（后端未通账号同步，音乐库页先以本地 mock 呈现）、MV 播放器（只有列表入口）、登录流 UI（沿用原型的后续规划）。

---

## 4. 数据接线契约（mock-first）

`src/lib/api/types.ts` 中的类型 = 未来真实接口的形状。映射：

| UI 需求 | 未来实现 |
|---|---|
| 播放控制/队列/收藏/评论列表 | daemon IPC（`Request::Command/QueueList/CommentList/Favorite`）包一层本地桥或 HTTP 壳 |
| 搜索/榜单/歌手/专辑/推荐/歌词 | web 侧进程内直调 `hmp-qqmusic-api`（`TopApi/SingerApi/AlbumApi/RecommendApi/LyricApi`），经轻量 HTTP 服务暴露 |
| 封面取色 | 纯前端，无后端依赖 |

`PlayerBridge`（`lib/player.ts`）做**附加式扩展**：新增可选方法 `getQueue/getCurrentTrack/playAt/removeAt` 与 `onQueueChanged`；tauri 实现未提供的方法返回空值，不破坏原型 Tauri 路径。浏览器运行时经 `isTauriRuntime()` 探测选择 `browserPlayerBridge`（内置 8 首模拟曲目、rAF 计时推进、完整队列操作）。

---

## 5. 动效与无障碍

- 动效时长/缓动一律用现有 `--duration-*`/`--ease-*`；页面级过渡 ≤ 320ms；`prefers-reduced-motion` 关闭位移类动效。
- HoverGroup 滑动高亮继续作为导航的签名微交互，其余区域不得滥用。
- 焦点可见（`:focus-visible` 用 `--ring`），交互目标 ≥ 24px，评论/列表行整行可点。
- 图片全部 `alt`/`aria-hidden` 语义化；播放列表抽屉 `role="dialog"` + ESC / 遮罩点击关闭；遮罩 `aria-hidden`。

## 6. 质量门

每个功能提交前：`pnpm test`（含 color 模块单测）+ `pnpm build`（vue-tsc 全量类型检查）零错误；页面完成后经浏览器实测（截图核对布局、明暗两态、交互路径）。
