# HMP Tauri Design System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 HMP Tauri 前端从未完成的主题变量升级为纯色分层设计系统，并让现有布局壳统一使用这些令牌。

**Architecture:** 设计令牌集中在 `src/styles/index.css`，所有主题共用尺寸、间距、圆角、阴影、动效和层级变量，浅色与 `.dark` 分别提供语义颜色映射。布局组件只消费语义令牌，不新增组件库或业务状态层；`PlayerOverlay` 继续通过 `close` 事件与父组件通信，并以固定定位覆盖整个窗口。

**Tech Stack:** Vue 3、TypeScript、Vite、CSS Custom Properties、pnpm。

## Global Constraints

- 完全移除毛玻璃效果和相关令牌：不使用背景模糊、透明卡片、透明叠加或渐变。
- 所有页面背景、表面、卡片和弹出层使用不透明纯色。
- 保留 `--walnut-50` 至 `--walnut-950` 胡桃木品牌色阶和现有浅色/深色方向。
- 不引入 Vue UI 组件库，不修改路由、Tauri 配置、Rust crate 或播放业务逻辑。
- 保留现有组件外部接口：`MainLayout` 的播放器栏点击行为和 `PlayerOverlay` 的 `close` 事件必须保持。
- 不覆盖当前工作区已有的源码未提交改动；只在计划列出的文件中实施本设计系统变更。
- 使用 `pnpm --dir apps/hmp-tauri build` 做前端类型检查和生产构建验证。

---

### Task 1: 建立纯色主题令牌系统

**Files:**
- Modify: `apps/hmp-tauri/src/styles/index.css:80-end`
- Test: no new test file; validate with token searches and the frontend build

**Interfaces:**
- Consumes: 当前 `:root` 与 `.dark` 中的胡桃木色阶、状态色和 Vuetify RGB 变量。
- Produces: `MainLayout.vue`、`Sidebar.vue`、`PlayerBar.vue`、`PlayerOverlay.vue` 可消费的统一 CSS 变量。

- [ ] **Step 1: 记录实施前的前端基线**

Run from repository root:

```text
pnpm --dir apps/hmp-tauri build
```

Expected: `vue-tsc --noEmit` 和 `vite build` 均成功；若失败，记录为已有基线问题，不在此任务中扩大范围。

- [ ] **Step 2: 替换浅色主题的表面变量和补齐通用令牌**

在 `:root` 中保留现有胡桃木色阶、语义状态色和 Vuetify RGB 变量，将背景区域改成不透明纯色，并添加以下令牌：

```css
  --background: #FAF8F7;
  --foreground: var(--neutral-900);

  --surface-1: #F2EFEC;
  --surface-2: #FFFFFF;
  --surface-3: #FDFCFB;
  --card: var(--surface-2);
  --card-foreground: var(--foreground);
  --popover: var(--surface-3);
  --popover-foreground: var(--foreground);

  --primary-hover: var(--walnut-400);
  --primary-active: var(--walnut-600);
  --border: #CDC5BC;
  --border-strong: #B0A69B;
  --input: #E3DED8;
  --ring: var(--walnut-400);
  --muted: #F2EFEC;
  --muted-foreground: var(--neutral-500);
  --track: var(--neutral-200);

  --space-1: 0.25rem;
  --space-2: 0.5rem;
  --space-3: 0.75rem;
  --space-4: 1rem;
  --space-5: 1.25rem;
  --space-6: 1.5rem;
  --space-8: 2rem;
  --space-10: 2.5rem;
  --space-12: 3rem;

  --radius-none: 0;
  --radius-sm: 0.25rem;
  --radius-md: 0.5rem;
  --radius-lg: 0.75rem;
  --radius-full: 9999px;

  --shadow-sm: 0 1px 2px rgba(36, 31, 27, 0.08);
  --shadow-md: 0 4px 12px rgba(36, 31, 27, 0.12);
  --shadow-lg: 0 12px 32px rgba(36, 31, 27, 0.18);

  --control-height-xs: 0.25rem;
  --control-height-sm: 2rem;
  --control-height-md: 2.5rem;
  --control-height-lg: 3rem;
  --sidebar-width: clamp(14rem, 17vw, 18rem);
  --top-bar-height: 5rem;
  --player-bar-height: 5rem;
  --layout-gap: var(--space-2);

  --duration-fast: 120ms;
  --duration-normal: 240ms;
  --duration-slow: 320ms;
  --ease-standard: cubic-bezier(0.2, 0, 0, 1);
  --ease-enter: cubic-bezier(0, 0, 0.18, 0.99);
  --ease-exit: cubic-bezier(0.42, 0, 1, 1);
  --z-base: 0;
  --z-sticky: 10;
  --z-dropdown: 20;
  --z-overlay: 30;
  --z-modal: 40;
```

The existing `--secondary`, `--accent`, status colors and their foreground variables remain available. Replace the old translucent values rather than adding parallel aliases.

- [ ] **Step 3: Map the dark theme to the same semantic contract**

In `.dark`, keep the current dark walnut and neutral scale values, then define matching opaque semantic variables:

```css
  --background: #12100D;
  --foreground: var(--neutral-900);

  --surface-1: #1F1B17;
  --surface-2: #2A241F;
  --surface-3: #36302A;
  --card: var(--surface-2);
  --card-foreground: var(--foreground);
  --popover: var(--surface-3);
  --popover-foreground: var(--foreground);

  --primary-hover: var(--walnut-400);
  --primary-active: var(--walnut-600);
  --border: #4F483E;
  --border-strong: #756A5E;
  --input: #36302A;
  --ring: var(--walnut-400);
  --muted: #1F1B17;
  --muted-foreground: var(--neutral-500);
  --track: var(--neutral-300);
```

Keep the common spacing, radius, shadow, size, motion and z-index values available from `:root`; override only the shadow values in `.dark` if the final contrast check shows they are too strong. Do not use `rgba()` for surfaces, borders or muted backgrounds.

- [ ] **Step 4: Remove obsolete glass and solid fallback variables**

Delete these declarations from `index.css` wherever they occur:

```text
--background-solid
--card-solid
--glass-blur
--glass-opacity
--radius-1
```

Also remove comments that describe the deleted glass/fallback system. A repository search for `glass|background-solid|card-solid|radius-1` in `apps/hmp-tauri/src` must return no results after the component tasks are complete.

- [ ] **Step 5: Validate the token layer before touching components**

Run:

```text
rg --no-heading --line-number "glass|background-solid|card-solid|radius-1" apps/hmp-tauri/src/styles/index.css
rg --no-heading --line-number "--(background|surface-[1-3]|card|popover|border|input|muted): rgba" apps/hmp-tauri/src/styles/index.css
pnpm --dir apps/hmp-tauri build
```

Expected: both searches return no output and the build succeeds. Shadow tokens may use low-opacity colors because shadows are not surfaces; if the build fails because components still reference removed variables, continue to Task 2 rather than adding compatibility aliases.

- [ ] **Step 6: Commit only the stylesheet change**

```text
git add apps/hmp-tauri/src/styles/index.css
git commit -m "refactor(ui): define opaque theme tokens"
```

Expected: the commit contains only `apps/hmp-tauri/src/styles/index.css`.

---

### Task 2: 将布局壳迁移到设计令牌

**Files:**
- Modify: `apps/hmp-tauri/src/layouts/MainLayout.vue:20-end`
- Modify: `apps/hmp-tauri/src/layouts/Sidebar.vue:1-end`
- Modify: `apps/hmp-tauri/src/layouts/PlayerBar.vue:8-end`
- Modify: `apps/hmp-tauri/src/layouts/PlayerOverlay.vue:8-end`
- Modify: `apps/hmp-tauri/src/layouts/TopBar.vue:1-end`
- Test: no new test file; validate behavior-preserving build and static searches

**Interfaces:**
- Consumes: Task 1 semantic tokens, especially `--layout-gap`, `--sidebar-width`, `--player-bar-height`, `--surface-*`, `--border`, `--radius-*`, `--control-height-*`, `--shadow-lg`, `--duration-*`, `--ease-*`, and `--z-overlay`.
- Produces: visually consistent layout shell while preserving `MainLayout` click toggling and `PlayerOverlay` `close` event.

- [ ] **Step 1: Replace `MainLayout.vue` local layout values**

Keep the existing template, imports, `playerOverlay` state, and click handler. Update `.app-layout`, `.content`, `.player-bar`, and transition rules as follows:

```css
.app-layout {
  width: 100%;
  height: 100%;
  display: grid;
  gap: var(--layout-gap);
  padding: var(--layout-gap);
  grid-template-columns: var(--sidebar-width) minmax(0, 1fr);
  grid-template-rows: minmax(0, 1fr) var(--player-bar-height);
  background: var(--background);
}

.content {
  grid-column: 2;
  grid-row: 1;
  min-width: 0;
  min-height: 0;
  overflow: auto;
  background: var(--surface-1);
  color: var(--foreground);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
}

.player-bar {
  grid-column: 2 / -1;
  grid-row: 2;
}

.slide-bottom-enter-active {
  transition: transform var(--duration-normal) var(--ease-enter);
}

.slide-bottom-leave-active {
  transition: transform var(--duration-fast) var(--ease-exit);
}
```

Keep `grid-column` and `grid-row` placement because they describe structure, not theme styling. Do not re-enable `TopBar` in this step.

- [ ] **Step 2: Add the `Sidebar.vue` container foundation**

Keep the empty template and add a scoped style block:

```vue
<style scoped>
.sidebar {
  width: 100%;
  height: 100%;
  padding: var(--space-4);
  overflow: auto;
  color: var(--foreground);
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
}
</style>
```

Do not add navigation data or router behavior in this task.

- [ ] **Step 3: Migrate `PlayerBar.vue` placeholder styling**

Keep the two existing placeholder elements and use opaque surfaces and tokenized dimensions:

```css
.player-bar {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  min-height: var(--player-bar-height);
  color: var(--foreground);
}

.progress-bar {
  width: 100%;
  height: var(--control-height-xs);
  background: var(--track);
  border-radius: var(--radius-full);
}

.status-card {
  width: 100%;
  flex: 1;
  min-height: 0;
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
}
```

The parent click behavior remains unchanged. `--info` must no longer be used for the unfinished track.

- [ ] **Step 4: Make `PlayerOverlay.vue` a fixed opaque overlay**

Keep the existing `defineEmits` signature and click handler. Replace the style with:

```css
.player-container {
  position: fixed;
  inset: 0;
  z-index: var(--z-overlay);
  width: 100%;
  height: 100%;
  color: var(--foreground);
  background: var(--surface-3);
  box-shadow: var(--shadow-lg);
  will-change: transform;
}
```

Do not add a separate transparent backdrop or a new event name. The existing transition in `MainLayout.vue` continues to animate this fixed element.

- [ ] **Step 5: Add only the future `TopBar.vue` container contract**

Keep the empty template and add a scoped style block without enabling the component in `MainLayout.vue`:

```vue
<style scoped>
.top-bar {
  height: var(--top-bar-height);
  padding: 0 var(--space-4);
  color: var(--foreground);
  background: var(--surface-2);
  border-bottom: 1px solid var(--border);
}
</style>
```

- [ ] **Step 6: Run static checks for obsolete values and preserved interfaces**

Run:

```text
rg --no-heading --line-number "glass|background-solid|card-solid|radius-1|background-color: var\\(--info\\)|gap: 0\\.3rem|height: 5rem|17vw" apps/hmp-tauri/src
rg --no-heading --line-number "defineEmits|close|playerOverlay|player-container" apps/hmp-tauri/src/layouts
```

Expected: the first command returns no obsolete glass/fallback or replaced placeholder layout values; the second command still finds the `close` emitter, `playerOverlay` state/toggle, and `player-container` element.

- [ ] **Step 7: Run the frontend verification suite**

Run:

```text
pnpm --dir apps/hmp-tauri build
```

Expected: `vue-tsc --noEmit` and `vite build` both pass.

- [ ] **Step 8: Review the diff for scope and commit the component migration**

Run:

```text
git diff --check
git diff -- apps/hmp-tauri/src/styles/index.css apps/hmp-tauri/src/layouts
```

Confirm the diff contains only design-token consumption and styling changes, with no router, event, or business logic changes. Then commit:

```text
git add apps/hmp-tauri/src/layouts/MainLayout.vue apps/hmp-tauri/src/layouts/Sidebar.vue apps/hmp-tauri/src/layouts/PlayerBar.vue apps/hmp-tauri/src/layouts/PlayerOverlay.vue apps/hmp-tauri/src/layouts/TopBar.vue
git commit -m "refactor(ui): apply design tokens to layout shell"
```

Expected: the commit contains only the five listed layout files.

---

### Task 3: Final specification and regression verification

**Files:**
- Modify: none unless a verification issue requires a scoped correction in Task 1 or Task 2 files
- Test: `apps/hmp-tauri` production build and repository searches

**Interfaces:**
- Consumes: completed token and component migration.
- Produces: verified implementation matching `docs/superpowers/specs/2026-08-16-hmp-tauri-design-system-design.md`.

- [ ] **Step 1: Verify the design system contract globally**

Run:

```text
rg --no-heading --line-number "--(background|foreground|surface-1|surface-2|surface-3|primary-hover|primary-active|border-strong|track|space-|radius-|shadow-|control-height-|sidebar-width|top-bar-height|player-bar-height|layout-gap|duration-|ease-|z-)" apps/hmp-tauri/src/styles/index.css
rg --no-heading --line-number "glass|background-solid|card-solid|radius-1" apps/hmp-tauri/src
```

Expected: the first command shows the complete token groups; the second command returns no output.

- [ ] **Step 2: Verify the frontend build and whitespace**

Run:

```text
pnpm --dir apps/hmp-tauri build
git diff --check HEAD~2..HEAD
```

Expected: both commands succeed. `pnpm` output must show successful `vue-tsc` and Vite build completion.

- [ ] **Step 3: Inspect final repository state**

Run:

```text
git status --short
git log -3 --oneline
```

Expected: the design-system commits are present. Any unrelated pre-existing source changes remain visible and are not reverted by this work.
