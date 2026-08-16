# HMP Tauri 设计系统补全规格

## 1. 背景与目标

`apps/hmp-tauri/src/styles/index.css` 当前已有胡桃木色板、浅色/深色主题和基础 reset，但尺寸、间距、圆角、阴影、层级、动效和组件状态尚未形成完整规则。同时，现有布局组件仍混用局部硬编码值和已经不再符合实际的“毛玻璃”变量命名。

本规格的目标是建立一套轻量、纯色、可扩展的桌面音乐播放器设计系统，并让现有布局壳使用统一令牌。设计系统不引入第三方组件库，不改变播放业务、路由或 Tauri 配置。

## 2. 已确认的设计原则

- 采用“通透胡桃木”作为品牌方向，但不实现毛玻璃效果。
- 删除毛玻璃相关概念：不使用背景模糊、透明卡片、透明叠加或渐变。
- 所有页面背景、表面、卡片和弹出层使用不透明纯色。
- 使用颜色层级、边框和少量阴影表达结构关系。
- 浅色和深色主题使用相同的语义令牌名称。
- 组件优先复用全局令牌，避免在单文件中重复定义视觉数值。
- 保留现有外部组件接口和布局职责，采用渐进式落地。

## 3. 令牌系统

### 3.1 色彩令牌

#### 品牌色板

保留现有 `--walnut-50` 至 `--walnut-950` 色阶。浅色主题使用当前暖胡桃色值；深色主题使用当前提亮后的色值，以保证文字和控件可读性。

#### 语义颜色

两套主题都必须定义以下语义变量：

- `--background`：应用根背景。
- `--foreground`：根文本颜色。
- `--surface-1`：主要内容区域。
- `--surface-2`：侧栏、播放器栏和普通面板。
- `--surface-3`：浮层、弹出层和强调面板。
- `--primary`：主要操作和播放进度。
- `--primary-hover`：主要操作悬停状态。
- `--primary-active`：主要操作按下状态。
- `--primary-foreground`：主色背景上的文本或图标。
- `--secondary`、`--secondary-foreground`：次要操作。
- `--accent`、`--accent-foreground`：强调信息。
- `--muted`、`--muted-foreground`：弱化背景和文本。
- `--border`：常规分隔线。
- `--border-strong`：高可见度分隔线。
- `--input`：输入控件边框或背景。
- `--ring`：键盘焦点指示。
- `--track`：播放器进度轨道未完成部分。
- `--success`、`--warning`、`--error`、`--info` 及各自的 `-foreground`：状态反馈。

`--background-solid`、`--card-solid`、`--glass-blur`、`--glass-opacity` 等旧变量删除，不保留兼容别名。因为新语义变量本身全部是不透明值，不再需要“solid”后备层。

### 3.2 间距令牌

采用 `0.25rem` 为基础单位：

| 令牌 | 值 | 用途 |
| --- | --- | --- |
| `--space-1` | `0.25rem` | 图标与文字的微间距 |
| `--space-2` | `0.5rem` | 紧凑控件内部间距 |
| `--space-3` | `0.75rem` | 控件之间的常规间距 |
| `--space-4` | `1rem` | 面板内边距 |
| `--space-5` | `1.25rem` | 宽松面板内边距 |
| `--space-6` | `1.5rem` | 区块间距 |
| `--space-8` | `2rem` | 页面区块间距 |
| `--space-10` | `2.5rem` | 大区块间距 |
| `--space-12` | `3rem` | 页面级留白 |

### 3.3 圆角令牌

删除当前唯一的 `--radius-1`，改用以下语义明确的圆角：

- `--radius-none: 0`
- `--radius-sm: 0.25rem`
- `--radius-md: 0.5rem`
- `--radius-lg: 0.75rem`
- `--radius-full: 9999px`

容器默认使用 `--radius-lg`，紧凑控件使用 `--radius-sm` 或 `--radius-md`，圆形按钮使用 `--radius-full`。

### 3.4 阴影令牌

阴影只用于悬浮和覆盖层，不用于制造毛玻璃效果：

- `--shadow-sm`：轻微悬浮层次。
- `--shadow-md`：卡片或局部面板悬浮。
- `--shadow-lg`：全屏覆盖层和高层级弹出内容。

阴影颜色使用中性深色并保持低透明度。深色主题可以适度降低阴影可见度，但不改变令牌名称。

### 3.5 尺寸令牌

- `--control-height-xs: 0.25rem`：播放器进度条。
- `--control-height-sm: 2rem`：紧凑图标按钮和辅助控件。
- `--control-height-md: 2.5rem`：标准按钮、导航项和输入框。
- `--control-height-lg: 3rem`：主要操作控件。
- `--sidebar-width: clamp(14rem, 17vw, 18rem)`。
- `--top-bar-height: 5rem`：预留顶栏高度。
- `--player-bar-height: 5rem`。
- `--layout-gap: var(--space-2)`。

### 3.6 动效和层级令牌

- `--duration-fast: 120ms`
- `--duration-normal: 240ms`
- `--duration-slow: 320ms`
- `--ease-standard: cubic-bezier(0.2, 0, 0, 1)`
- `--ease-enter: cubic-bezier(0, 0, 0.18, 0.99)`
- `--ease-exit: cubic-bezier(0.42, 0, 1, 1)`
- `--z-base: 0`
- `--z-sticky: 10`
- `--z-dropdown: 20`
- `--z-overlay: 30`
- `--z-modal: 40`

动效只用于状态变化、面板进入/退出和可感知的交互反馈。不得用动效替代状态表达。

## 4. 主题映射

### 4.1 浅色主题

- `--background` 使用暖白色。
- `--surface-1` 使用比根背景略深的米白色。
- `--surface-2` 使用白色或浅胡桃色。
- `--surface-3` 使用更高对比度的白色。
- 主色继续使用当前 `--walnut-500`。
- 边框使用低饱和暖灰色，不使用透明度。

### 4.2 深色主题

- `--background` 使用当前深炭棕色。
- `--surface-1` 使用略亮的深棕色。
- `--surface-2` 使用面板深棕色。
- `--surface-3` 使用更亮的浮层深棕色。
- 主色继续使用当前深色主题的提亮胡桃色。
- 文本使用当前中性色反转体系。
- 边框使用可见但克制的深暖灰色，不使用透明度。

## 5. 组件落地

### 5.1 `MainLayout.vue`

继续使用 CSS Grid，两列结构保持不变：

- 根容器使用 `--layout-gap` 作为 `gap` 和 `padding`。
- 第一列使用 `--sidebar-width`。
- 第二列使用 `minmax(0, 1fr)`。
- 播放器栏高度使用 `--player-bar-height`。
- 根背景使用 `--background`。
- 内容区使用 `--surface-1`。
- 侧栏和播放器栏使用 `--surface-2`。
- 内容区继续保持独立滚动。
- `TopBar` 保持暂不启用，不改变当前 Grid 行结构。
- 播放器覆盖层不加入 Grid 第三列，使用固定定位覆盖窗口。

### 5.2 `Sidebar.vue`

当前为空壳，本轮建立容器基础样式：

- 背景使用 `--surface-2`。
- 使用 `--border` 和 `--radius-lg`。
- 内边距使用 `--space-4`。
- 导航项高度使用 `--control-height-md`。
- 普通状态使用 `--muted-foreground`。
- 悬停状态使用 `--surface-3`。
- 选中状态使用 `--primary-light` 和 `--primary-light-foreground`。

### 5.3 `PlayerBar.vue`

保留现有占位 DOM 和点击行为，完成基础视觉骨架：

- 外层容器背景使用 `--surface-2`。
- 进度条高度使用 `--control-height-xs`。
- 进度轨道使用 `--track`，已播放部分使用 `--primary`。
- 状态区域使用 `--surface-3` 或 `--surface-2`。
- 内部间距使用 `--space-2`。
- 不再引用 `--background-solid`。

### 5.4 `PlayerOverlay.vue`

保留 `close` 事件接口，调整为全窗口覆盖层：

- 使用 `position: fixed` 和 `inset: 0`。
- 使用 `z-index: var(--z-overlay)`。
- 背景使用 `--surface-3`。
- 使用 `--shadow-lg` 表达层级。
- 继续支持点击关闭。
- 入场和退场动画使用动效令牌。
- 未来增加封面、歌词、控制器和队列时，不改变父组件接口。

### 5.5 `TopBar.vue`

本轮不启用，只保留未来规范：

- 高度使用 `--top-bar-height`。
- 背景使用 `--surface-2`。
- 底部边框使用 `--border`。

## 6. 交互状态规范

交互控件统一遵循：

- 默认：当前表面色。
- 悬停：提升到 `--surface-3` 或 `--primary-hover`。
- 按下：使用 `--primary-active`。
- 选中：使用 `--primary-light` 和对应前景色。
- 聚焦：`outline: 2px solid var(--ring)`，并保留可见偏移。
- 禁用：使用弱化文本和边框颜色，不通过透明背景隐藏控件。
- 错误、警告、成功、信息：使用对应语义色和前景色。

## 7. 不在本轮范围内

- 不引入 Vue UI 组件库。
- 不启用顶栏。
- 不实现音频播放状态管理。
- 不实现真实进度计算、音量、歌词或队列逻辑。
- 不修改路由、Tauri 配置和 Rust crate。
- 不加入渐变、毛玻璃、透明卡片、背景模糊。
- 不重构与设计系统无关的现有未提交改动。

## 8. 验收标准

1. `index.css` 中不存在 `glass`、`background-solid`、`card-solid` 等旧毛玻璃或后备层变量。
2. 浅色和深色主题都定义完整的表面、状态、间距、圆角、阴影、尺寸、动效和层级令牌。
3. `MainLayout.vue`、`PlayerBar.vue`、`PlayerOverlay.vue` 不再使用本地硬编码的布局设计数值，必要的 Grid 结构值除外。
4. 现有播放器栏点击打开和覆盖层点击关闭行为保持不变。
5. `TopBar.vue`、`Sidebar.vue` 即使仍为空壳，也具备与令牌体系一致的基础容器样式。
6. 前端类型检查和 Vite 构建通过。
7. 设计文档与实现保持一一对应，新增令牌不得没有用途说明。
