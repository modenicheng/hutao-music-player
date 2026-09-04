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

由封面取色模块驱动（见 §2），挂在 `html` 根上，随当前曲目切换并做 320ms 交叉过渡：

| 变量 | 用途 | 亮色模式 | 暗色模式 |
|---|---|---|---|
| `--track-accent` | 播放态强调：进度条已播段、正在播放行、播放按钮 | OKLab L≈0.60 | OKLab L≈0.68 |
| `--track-on-accent` | accent 上的文字/图标 | WCAG ≥ 4.5 自适应黑/白 | 同左 |
| `--track-accent-soft` | hover 底、选中底、徽章底 | accent @ 12% alpha | accent @ 20% alpha |
| `--track-deep` | 播放页/播放列表的深色面板底 | OKLab L≈0.35 | OKLab L≈0.22 |
| `--track-deep-fg` | deep 面板上的正文 | 暖白 | 暖白 |
| `--track-grad-from/to` | 播放页环境渐变两端 | accent-soft → surface | deep → neutral-950 |
| `--track-equalizer` | 正在播放动画条颜色 | accent | accent |

无封面 / 取色失败时整族回退到 walnut（`--track-accent: var(--accent)` 等），**UI 不得出现无色状态**。

约束：
- 曲目层永远不做**大面积正文底色**，只做氛围、强调与面板；
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
- **TrackTable**（曲目表）：列 = 序号/标题+歌手/专辑/操作/时长；hover 行浮起 `--accent-soft`；正在播放行显示 3 根 `--track-equalizer` 跳动条替代序号。数字一律 `font-variant-numeric: tabular-nums`。
- **CoverCard**（封面卡）：圆角 `--radius-lg`、`--shadow-md`，hover 上浮 2px + 阴影加深 + 播放浮层按钮；标题两行截断。
- **SectionHeader**：区块标题（1.15rem/650）+ 右侧"更多 →"；间距 `--space-8` 起节。
- 列表页统一用自绘 `Scroll` 组件；长列表预留虚拟化接口（v0.1 直接渲染，mock 数据量 ≤ 200 行）。

### 3.1 播放页 NowPlaying（签名页）

入口两条：点击 PlayerBar 任意思源 → 全屏 overlay（沿用 slide-bottom 过渡）；路由 `/now-playing` 直达同一内容组件。ESC / 左上收起按钮关闭 overlay。

自上而下：

1. **环境层**：`--track-grad-from → --track-grad-to` 纵向渐变铺满；封面放大模糊 80px 置底 @ 25% 透明度，形成"光从音乐里透出来"的底。
2. **Hero**：左封面（`min(38vh, 340px)` 方形，`--radius-lg`，`--shadow-lg`）；右侧曲目信息——歌名 display 级（clamp 1.8–2.6rem/700）、歌手/专辑行（可点击跳转）、音质徽章（如 `FLAC · 44.1kHz`，`--track-accent-soft` 底）、动作排（喜欢/收藏到歌单/下载/更多）。
3. **刻度进度条（签名组件 RulerProgress）**：
   - 全宽横贯 hero 之下；静息高度 3px，hover/拖拽 6px，200ms 过渡；
   - 刻度：每 10% 一根 1px 细线（高 8px，前景 @ 18%），四分之一处（25/50/75%）加高至 12px；已播段覆盖为 `--track-accent`；
   - 拖拽：圆形 thumb（12px，白底 accent 环）仅 hover/拖拽出现；拖拽中时间跟随 thumb 显示气泡；落点即 seek；
   - 两端时间（已播/剩余总长）tabular-nums，剩余总长以 `-3:45` 形式。
4. **控制排**：居中主控——随机 / 上一曲 / **播放（64px 圆，`--track-accent` 底 + onAccent 前景，图标 28px）** / 下一曲 / 循环模式；右侧次控——音量（滑条）、歌词开关、**播放列表**（开抽屉）。
5. **歌词区**（向下滚动进入）：居中排版，行高 2.2，当前行 `--track-accent` + scale 1.05 + 字重 700，前后行逐级降透明度（1 / 0.55 / 0.32）；翻译以 0.8em 灰度随行下挂；自动跟随滚动，用户手动滚动后暂停跟随 3s；点击任意行 seek 至该行。
6. **评论区**（继续下滚，网易云式）：
   - 吸顶小节头：`评论 · 12.4万` + 排序 tab（最热 / 最新 / 神评）；
   - 评论卡：头像 32px 圆 + 昵称（灰度 600）+ 时间右对齐 + 正文 0.92rem + 底部动作排（点赞数/回复/举报）；回复以引用块缩进展示（最多 2 条 + "共 N 条回复 >"）；
   - 置顶评论带 `--track-accent-soft` 左边条；热评按点赞数降序，前 3 条点赞数用 `--track-accent` 强调；
   - 顶部输入框（mock，不发送）。
7. **播放列表抽屉**：自右滑入（宽 380px，高于 PlayerBar），面板底 `light: --track-accent-soft 混 surface / dark: --track-deep`；行 = 序号/标题/歌手/时长，当前行 accent 高亮 + equalizer；行尾操作（移除）；顶部"播放列表 · N 首" + 清空。**抽屉是"播放列表适配专辑配色"的载体**。

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

**本版全部跑 mock 数据**（`src/lib/api/`），接口形状即未来接线的契约（§4）；封面一律用**程序化 SVG data-URL**（按种子生成确定性的封面图，既可测取色又不依赖网络）。

### 3.3 明确不做（本版）

收藏/资料库云端同步页（后端未通账号同步）、设置四页（保持占位）、MV 播放器（只有列表入口）、登录流 UI（沿用原型的后续规划）。

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
- 图片全部 `alt`/`aria-hidden` 语义化；播放列表抽屉 `role="dialog"` + ESC 关闭。

## 6. 质量门

每个功能提交前：`pnpm test`（含 color 模块单测）+ `pnpm build`（vue-tsc 全量类型检查）零错误；页面完成后经浏览器实测（截图核对布局、明暗两态、交互路径）。
