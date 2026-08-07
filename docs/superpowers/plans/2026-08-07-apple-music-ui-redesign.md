# 胡桃音乐 Apple Music 风格桌面 UI 重构实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 hmp-desktop 重构为资料库优先、支持深浅主题、真实搜索/播放/登录、真实队列/歌词状态和明确开发中标识的 Slint 桌面音乐播放器 UI。

**Architecture:** 保留 Slint 1.17 和现有 Rust 应用核心。将当前单文件 UI 拆成主题、原语、导航、页面、播放栏和登录弹层组件；`AppWindow` 只持有 UI 状态和转发回调。`AppCore` 继续作为真实播放状态唯一来源，通过新增的队列快照、歌词状态事件和现有播放状态订阅向 UI 提供数据，推荐内容仅由本地演示数据模块提供。

**Tech Stack:** Rust 2024, Slint 1.17, `slint-build`, Tokio 1, GStreamer 播放器核心, QQ Music Rust API, `i-slint-backend-testing`, image crate.

## Global Constraints

- “本次重构采用‘资料库优先’的默认体验，新增资料库、推荐、搜索、队列、歌词、设置/关于等页面和统一导航。”
- “推荐、收藏与资料库同步等尚未具备完整后端能力的区域必须明确标记为开发中，不得用演示数据伪装成真实账号数据或返回虚假的成功结果。”
- “推荐页使用稳定的本地中文演示数据，不发起推荐网络请求。”
- “搜索页保持现有真实搜索链路。”
- “队列展示从 AppCore 的真实队列快照映射而来；空队列显示搜索入口。”
- “没有真实歌词数据时不能填充演示歌词。”
- “默认模式跟随系统。设置页支持跟随系统、浅色、深色。”
- “目标窗口尺寸为 `1100x720`，设置合理的最小尺寸。”
- “播放状态继续由 `PlaybackState` 发布。UI 只展示状态，不自行计算播放进度。”
- “当前 crate 固定使用 `slint = \"1.17\"`。”
- 不新增 WebView、GTK4、Tauri 或新的运行时；组件必须使用现有 Slint 工具链。
- 所有编辑保持 ASCII，除非已有中文产品文案或源文件字符集明确需要中文。

## File Structure

将按以下边界创建和修改文件；不修改 QQ Music 协议、GStreamer 播放器或 MPRIS 实现。

- Create: `crates/hmp-desktop/ui/theme.slint` - 深色/浅色语义颜色、间距、尺寸和字号令牌。
- Create: `crates/hmp-desktop/ui/primitives.slint` - 图标按钮、封面、开发中状态条、空状态和可复用列表行原语。
- Create: `crates/hmp-desktop/ui/sidebar.slint` - 品牌、主导航、队列/歌词快捷入口和账号区。
- Create: `crates/hmp-desktop/ui/library-page.slint` - 资料库真实状态和开发中区域。
- Create: `crates/hmp-desktop/ui/recommend-page.slint` - 本地推荐演示数据和开发中状态条。
- Create: `crates/hmp-desktop/ui/search-page.slint` - 真实搜索输入、结果状态和结果列表。
- Create: `crates/hmp-desktop/ui/queue-page.slint` - 真实队列快照、当前项和空状态。
- Create: `crates/hmp-desktop/ui/lyrics-page.slint` - 歌词状态、逐行模型和空状态。
- Create: `crates/hmp-desktop/ui/settings-page.slint` - 主题选择、关于信息和功能矩阵。
- Create: `crates/hmp-desktop/ui/player-bar.slint` - 常驻播放栏、播放控制、进度和音量。
- Create: `crates/hmp-desktop/ui/login-dialog.slint` - 二维码模态层及登录状态。
- Modify: `crates/hmp-desktop/ui/app.slint` - 根窗口、Slint 数据结构、路由属性和组件组合。
- Create: `crates/hmp-desktop/src/lyrics.rs` - LRC 文本解析和歌词 UI 数据转换。
- Modify: `crates/hmp-desktop/src/app.rs` - 队列快照、歌词命令/事件、歌词请求和 UI 数据结构。
- Modify: `crates/hmp-desktop/src/bridge.rs` - 新模型映射、导航/主题/队列/歌词回调和事件更新。
- Modify: `crates/hmp-desktop/src/lib.rs` - 导出歌词模块和 UI 数据辅助类型。
- Modify: `crates/hmp-desktop/src/main.rs` - 初始化资料库默认路由、推荐数据、主题和队列/歌词/播放状态订阅。
- Modify: `crates/hmp-desktop/src/bridge_tests.rs` - Slint testing backend 下的模型、路由、主题、队列、歌词和登录回归测试。
- Modify: `docs/PROJECT.md` - 增加桌面 UI 功能状态记录，明确已接入、部分接入和开发中的功能。
- Modify: `crates/hmp-desktop/Cargo.toml` - 添加已有工作区依赖 `tokio-util.workspace = true`，用于可取消的二维码登录会话；不引入新的第三方包。

---

### Task 1: 固定 UI 状态与 AppCore 事件契约

**Files:**
- Modify: `crates/hmp-desktop/ui/app.slint`
- Modify: `crates/hmp-desktop/src/app.rs`
- Modify: `crates/hmp-desktop/src/lib.rs`
- Test: `crates/hmp-desktop/src/bridge_tests.rs`

**Interfaces:**
- Consumes: existing `PlaybackState`, `QueueItem`, `AppCommand`, `AppEvent`, and generated `UiSong`/`UiPlayback` patterns.
- Produces: `UiPage`, `ThemeMode`, `UiQueueData`, `UiLyricData`, `UiFeatureData`, `AppEvent::SearchFailed`, `AppEvent::QueueUpdated`, `AppEvent::LyricsLoading`, `AppEvent::LyricsLoaded`, `AppEvent::LyricsFailed`, and `AppCommand::ReloadLyrics`.

- [ ] **Step 1: Write the failing Rust contract tests**

Add tests to `bridge_tests.rs` for the pure Rust values that do not require a rendered page:

```rust
#[test]
fn page_and_theme_values_use_stable_wire_names() {
    assert_eq!(UiPage::Library.as_str(), "library");
    assert_eq!(UiPage::parse("queue"), Some(UiPage::Queue));
    assert_eq!(UiPage::parse("unknown"), None);
    assert_eq!(ThemeMode::FollowSystem.as_str(), "system");
    assert_eq!(ThemeMode::parse("light"), Some(ThemeMode::Light));
}

#[test]
fn queue_event_contains_current_playing_flags() {
    let event = AppEvent::QueueUpdated(vec![UiQueueData {
        track_id: "mid-1".into(),
        title: "晴天".into(),
        artist: "周杰伦".into(),
        duration: "04:29".into(),
        is_current: true,
        is_playing: true,
    }]);
    assert!(matches!(event, AppEvent::QueueUpdated(items) if items[0].is_current));
}
```

Also add a test asserting `AppCommand::ReloadLyrics` can be constructed and is distinct from playback commands.

- [ ] **Step 2: Run the focused test to verify it fails**

Run:

```bash
cargo test -p hmp-desktop page_and_theme_values_use_stable_wire_names
```

Expected: FAIL because `UiPage`, `ThemeMode`, and `UiQueueData` are not defined.

- [ ] **Step 3: Implement the typed contract**

In `app.rs`, add the following public types and exact methods:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiPage { Library, Recommend, Search, Queue, Lyrics, Settings }

impl UiPage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Recommend => "recommend",
            Self::Search => "search",
            Self::Queue => "queue",
            Self::Lyrics => "lyrics",
            Self::Settings => "settings",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "library" => Some(Self::Library),
            "recommend" => Some(Self::Recommend),
            "search" => Some(Self::Search),
            "queue" => Some(Self::Queue),
            "lyrics" => Some(Self::Lyrics),
            "settings" => Some(Self::Settings),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode { FollowSystem, Light, Dark }

impl ThemeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FollowSystem => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::FollowSystem),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiQueueData {
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub duration: String,
    pub is_current: bool,
    pub is_playing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiLyricData {
    pub timestamp_ms: u64,
    pub time: String,
    pub text: String,
    pub translation: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiFeatureData {
    pub name: String,
    pub status: String,
    pub detail: String,
}
```

Extend `AppCommand` with `ReloadLyrics`. Extend `AppEvent` with `SearchFailed(String)`, `QueueUpdated(Vec<UiQueueData>)`, `LyricsLoading(String)`, `LyricsLoaded { mid: String, lines: Vec<UiLyricData> }`, and `LyricsFailed { mid: String, message: String }`. In `AppCore::search`, send `SearchFailed(error.to_string())` on the existing error branch. Keep navigation and theme local to the UI bridge; do not route those pure presentation changes through `AppCore`.

Add `state_rx: tokio::sync::watch::Receiver<PlaybackState>` and `last_queue_state: Option<(String, bool)>` fields to `AppCore`; initialize `state_rx` from `player.subscribe_state()` in `new`. Add `AppCore::queue_snapshot(&self) -> Vec<UiQueueData>` that maps `self.queue` and `self.queue_index`, using `format_secs` for known duration and `"--:--"` for unknown duration. It reads `self.state_rx.borrow()`, compares stable track IDs, and sets `is_playing` only when the matching item is current and `PlaybackStatus::Playing`. Task 5 extends `AppCore::run` with `tokio::select!` over `cmd_rx`, `login_rx`, and `state_rx.changed()`; it publishes `QueueUpdated` only when current track ID or playing/paused status changes, not on every position tick.

- [ ] **Step 4: Run the focused tests to verify they pass**

Run:

```bash
cargo test -p hmp-desktop page_and_theme_values_use_stable_wire_names
cargo test -p hmp-desktop queue_event_contains_current_playing_flags
cargo check -p hmp-desktop
```

Expected: PASS and successful Slint code generation with the existing UI still compiling.

- [ ] **Step 5: Commit the contract**

```bash
git add crates/hmp-desktop/ui/app.slint crates/hmp-desktop/src/app.rs crates/hmp-desktop/src/lib.rs crates/hmp-desktop/src/bridge_tests.rs
git commit -m "feat(desktop): define UI state contracts"
```

### Task 2: Build the shared theme and Slint primitives

**Files:**
- Create: `crates/hmp-desktop/ui/theme.slint`
- Create: `crates/hmp-desktop/ui/primitives.slint`
- Modify: `crates/hmp-desktop/ui/app.slint`
- Test: `crates/hmp-desktop/src/bridge_tests.rs`

**Interfaces:**
- Consumes: `ThemeMode` wire values (`system`, `light`, `dark`) and the Slint 1.17 generated component model.
- Produces: global semantic theme properties, `IconButton`, `CoverArt`, `DevelopmentBanner`, `EmptyState`, and `FeatureRow` components imported by every page.

- [ ] **Step 1: Add failing generated-UI tests**

Add a test that creates `AppWindow`, checks the default route is `library`, and changes `theme-mode` from `system` to `light` and `dark` through the generated setter:

```rust
#[test]
#[serial]
fn app_starts_in_library_and_accepts_theme_modes() {
    let ui = init_ui();
    assert_eq!(ui.get_current_page(), "library");
    assert_eq!(ui.get_theme_mode(), "system");
    ui.set_theme_mode("light".into());
    assert_eq!(ui.get_theme_mode(), "light");
    ui.set_theme_mode("dark".into());
    assert_eq!(ui.get_theme_mode(), "dark");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cargo test -p hmp-desktop app_starts_in_library_and_accepts_theme_modes
```

Expected: FAIL because `current-page` and `theme-mode` are not generated properties.

- [ ] **Step 3: Implement semantic theme tokens**

Create `theme.slint` with an exported `Theme` global containing both theme palettes and a `mode` property. Use only semantic properties from pages, including `background`, `sidebar`, `surface`, `surface-hover`, `text-primary`, `text-secondary`, `divider`, `accent`, `accent-muted`, and `banner-background`. Keep size tokens for `sidebar-width`, `player-height`, `content-padding`, `control-size`, and `cover-radius` in the same global.

Import `Palette` from `std-widgets.slint`, which Slint 1.17 publicly exports. In `AppWindow`, bind `Palette.color-scheme` to `ColorScheme.unknown` for `system`, `ColorScheme.light` for `light`, and `ColorScheme.dark` for `dark`. Slint 1.17's widget palette resolves `ColorScheme.unknown` against the backend system scheme. The custom `Theme` global derives semantic colors from the resolved public `Palette.background`, `Palette.foreground`, `Palette.alternate-background`, and `Palette.border` brushes, plus the fixed HMP accent `#ff2d55`; it does not read `SlintInternal` or treat `ColorScheme.unknown` as light.

Create `primitives.slint` with these public interfaces:

```slint
export component IconButton inherits Rectangle {
    in property <string> glyph;
    in property <bool> emphasized;
    in property <bool> enabled;
    callback clicked;
}

export component CoverArt inherits Rectangle {
    in property <image> source;
    in property <length> size;
}

export component DevelopmentBanner inherits Rectangle {
    in property <string> text;
}

export component EmptyState inherits Rectangle {
    in property <string> title;
    in property <string> detail;
    in property <string> action-label;
    callback action-clicked;
}
```

Use fixed dimensions, hover feedback, accessible text labels/tooltips where the Slint 1.17 public API supports them, and a stable cover placeholder when `source` is empty.

- [ ] **Step 4: Run compile and focused test**

Run:

```bash
cargo test -p hmp-desktop app_starts_in_library_and_accepts_theme_modes
cargo check -p hmp-desktop
```

Expected: PASS; the shared files compile through `build.rs`, and the generated properties exist.

- [ ] **Step 5: Commit shared styling**

```bash
git add crates/hmp-desktop/ui/theme.slint crates/hmp-desktop/ui/primitives.slint crates/hmp-desktop/ui/app.slint crates/hmp-desktop/src/bridge_tests.rs
git commit -m "feat(desktop): add shared UI theme primitives"
```

### Task 3: Implement the shell, navigation, player bar, and login dialog

**Files:**
- Create: `crates/hmp-desktop/ui/sidebar.slint`
- Create: `crates/hmp-desktop/ui/player-bar.slint`
- Create: `crates/hmp-desktop/ui/login-dialog.slint`
- Modify: `crates/hmp-desktop/ui/app.slint`
- Modify: `crates/hmp-desktop/src/app.rs`
- Modify: `crates/hmp-desktop/src/bridge.rs`
- Modify: `crates/hmp-desktop/src/main.rs`
- Modify: `crates/hmp-desktop/Cargo.toml`
- Test: `crates/hmp-desktop/src/bridge_tests.rs`

**Interfaces:**
- Consumes: `UiPlayback`, `UiSong`, shared primitives, `current-page`, `logged-in`, `show-login`, QR image, login status, and `CancellationToken` from workspace `tokio-util`.
- Produces: callbacks `navigate-requested(string)`, `queue-requested`, `lyrics-requested`, `theme-requested(string)`, existing playback callbacks, a non-blocking cancellable login session, and login callbacks with the existing `LoginStart`/`LoginCancel` command semantics.

- [ ] **Step 1: Add failing navigation callback tests**

Add a private login-session state test in `app.rs`:

```rust
#[test]
fn starting_or_cancelling_login_invalidates_the_previous_session() {
    let mut state = LoginSessionState::default();
    let (first_generation, first_token) = state.begin();
    let (second_generation, _) = state.begin();
    assert!(second_generation > first_generation);
    assert!(first_token.is_cancelled());
    state.cancel();
    assert!(!state.accepts(second_generation));
}
```

Add this UI-local callback test after introducing `bind_ui_state_callbacks(&AppWindow)`:

```rust
let ui = init_ui();
bind_ui_state_callbacks(&ui);
ui.invoke_navigate_requested("queue".into());
assert_eq!(ui.get_current_page(), "queue");
ui.invoke_navigate_requested("bad-page".into());
assert_eq!(ui.get_current_page(), "queue");
ui.invoke_theme_requested("light".into());
assert_eq!(ui.get_theme_mode(), "light");
ui.invoke_theme_requested("bad-theme".into());
assert_eq!(ui.get_theme_mode(), "light");
```

The accepted implementation must not add `AppCommand::Navigate` or `AppCommand::SetTheme`. `bind_ui_state_callbacks` captures `ui.as_weak()`, parses values with `UiPage::parse` and `ThemeMode::parse`, and sets generated root properties only for valid values.

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cargo test -p hmp-desktop ui_bridge_integration
```

Expected: FAIL because the root callback and shell components do not exist.

- [ ] **Step 3: Implement the shell components**

Create `sidebar.slint` with fixed width from `Theme.sidebar-width`, six navigation rows, selected-row styling based on `current-page`, a queue shortcut, a lyrics shortcut, and the existing login/account area. Each row must invoke `navigate-requested` with one of `library`, `recommend`, `search`, `queue`, `lyrics`, or `settings`.

Create `player-bar.slint` preserving the existing `UiPlayback` properties and callback names `play-pause`, `next-requested`, `prev-requested`, `seek-requested(float)`, and `volume-requested(float)`. Add `queue-clicked` and `lyrics-clicked` callbacks. Keep progress values bound to `UiPlayback.position` and `UiPlayback.duration`; do not add a timer or local position accumulator.

Create `login-dialog.slint` as an overlay inside the root window rather than a sidebar block. It must show QR image and `login-status`, expose `login-start`, `login-cancel`, and a close action, and remain visible only when `show-login` is true.

Refactor login orchestration so `AppCommand::LoginStart` never awaits the full QR session inside the command branch. Add `tokio-util.workspace = true`, private `LoginResult { generation: u64, result: Result<Credential, String> }`, an internal unbounded login-result channel, `login_generation: u64`, and `login_cancel: Option<CancellationToken>` to `AppCore`. `LoginSessionState::begin() -> (u64, CancellationToken)` cancels the prior token, increments the generation, creates and stores a new token, and returns both values; `cancel()` invalidates and cancels it; `accepts(generation)` accepts only the current live generation. `start_login()` uses that state, constructs a session-local `QqMusicClient::with_config(self.client.config())`, clones `events_tx`, and spawns the QR fetch/poll task. `AppCore::run` becomes a `tokio::select!` loop over UI commands and login results; only a result whose generation matches is allowed to save credentials and emit `LoginDone`. `LoginCancel` increments the generation, cancels the token, and emits `LoginStatus("登录已取消")`. This makes closing the modal cancel the network poll instead of only hiding it.

Update `app.slint` to import the new files, export `current-page`, `theme-mode`, `show-login`, and existing playback/login properties, and define the root callbacks. Keep `preferred-width: 1100px`, `preferred-height: 720px`, and add `min-width`/`min-height` constraints that leave the sidebar, content, and player bar usable.

- [ ] **Step 4: Bind only real playback/login actions**

In `bridge.rs`, retain the existing `AppCommand` mappings for search, play, playback controls, seek, volume, and login. Add `bind_ui_state_callbacks(&AppWindow)` exactly as tested above. Add no fake command for queue modification. In `main.rs`, call `bind_ui_state_callbacks(&ui)` once; route `queue-clicked` and `lyrics-clicked` through `invoke_navigate_requested` so all route validation remains in one callback.

- [ ] **Step 5: Run integration tests and compile**

Run:

```bash
cargo test -p hmp-desktop ui_bridge_integration
cargo test -p hmp-desktop starting_or_cancelling_login_invalidates_the_previous_session
cargo check -p hmp-desktop
```

Expected: PASS with login, search, playback, navigation, and login cancellation connected; no generated callback names are missing.

- [ ] **Step 6: Commit the shell**

```bash
git add crates/hmp-desktop/ui/sidebar.slint crates/hmp-desktop/ui/player-bar.slint crates/hmp-desktop/ui/login-dialog.slint crates/hmp-desktop/ui/app.slint crates/hmp-desktop/src/app.rs crates/hmp-desktop/src/bridge.rs crates/hmp-desktop/src/main.rs crates/hmp-desktop/src/bridge_tests.rs crates/hmp-desktop/Cargo.toml
git commit -m "feat(desktop): add player shell navigation and login dialog"
```

### Task 4: Add library, recommendation, and real search pages

**Files:**
- Create: `crates/hmp-desktop/ui/library-page.slint`
- Create: `crates/hmp-desktop/ui/recommend-page.slint`
- Create: `crates/hmp-desktop/ui/search-page.slint`
- Create: `crates/hmp-desktop/src/demo.rs`
- Modify: `crates/hmp-desktop/ui/app.slint`
- Modify: `crates/hmp-desktop/src/lib.rs`
- Modify: `crates/hmp-desktop/src/bridge.rs`
- Modify: `crates/hmp-desktop/src/main.rs`
- Test: `crates/hmp-desktop/src/bridge_tests.rs`

**Interfaces:**
- Consumes: `UiSong`, Rust `UiLibraryData`, `UiFeatureData`, `AppEvent::SearchDone`, `AppEvent::SearchFailed`, the existing `image` crate, Slint `SharedPixelBuffer`, and the exact `Search(String)` / `PlayIndex(usize)` commands.
- Produces: `library-items`, `recommend-items`, `feature-statuses`, search loading/error/empty properties, deterministic local bitmap covers, and `demo_recommendations() -> Vec<UiLibraryData>`.

- [ ] **Step 1: Add failing model and state tests**

Add tests for the local demo data and search model conversion:

```rust
#[test]
fn demo_recommendations_are_local_and_marked_as_demo() {
    let items = demo_recommendations();
    assert!(!items.is_empty());
    assert!(items.iter().all(|item| item.status == "demo"));
    assert!(items.iter().all(|item| item.cover.size().width == 320));
}

#[test]
fn search_done_replaces_results_without_touching_login_state() {
    let ui = init_ui();
    ui.set_logged_in(true);
    handle_event(&ui.as_weak(), AppEvent::SearchDone(vec![UiSongData {
        title: "晴天".into(), artist: "周杰伦".into(), duration: "04:29".into(),
    }]));
    assert_eq!(ui.get_songs().row_count(), 1);
    assert!(ui.get_logged_in());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
cargo test -p hmp-desktop demo_recommendations_are_local_and_marked_as_demo
cargo test -p hmp-desktop search_done_replaces_results_without_touching_login_state
```

Expected: FAIL because `demo_recommendations` and the new page state do not exist.

- [ ] **Step 3: Implement local demo data**

Create `demo.rs` with `UiLibraryData { kind: String, title: String, subtitle: String, status: String, cover: slint::Image }` and a deterministic `demo_recommendations() -> Vec<UiLibraryData>` containing at least six Chinese items. Generate each 320x320 RGBA bitmap with `SharedPixelBuffer<Rgba8Pixel>` using a distinct fixed palette and simple geometric bands/blocks keyed by item index, then convert it with `slint::Image::from_rgba8`. This provides offline visual assets without SVGs, remote downloads, or copyrighted album art. Every item uses `status: "demo"`; the module must not call `QqMusicClient` or mutate `AppCore`.

Add `feature_matrix() -> Vec<UiFeatureData>` returning these exact statuses:

```text
登录                 已接入
搜索                 已接入
播放                 已接入
队列展示             已接入
歌词展示             部分接入
推荐内容             开发中 / 演示数据
收藏与资料库同步     开发中
```

- [ ] **Step 4: Implement the three page components**

`library-page.slint` must show current playback summary, queue/library model rows, a stable empty state, and a development banner for cloud favorites/sync. It may display real queue items only from the supplied model.

`recommend-page.slint` must show `DevelopmentBanner` at the top and render `recommend-items` as browseable local cards/rows. It must not expose a fake favorite or cloud-save success callback.

`search-page.slint` must expose:

```slint
in property <[UiSong]> songs;
in-out property <string> search-text;
in property <bool> loading;
in property <string> error-text;
callback search-requested(string);
callback play-requested(int);
callback retry-requested;
```

The page must keep the existing real search input behavior, disable submit for whitespace-only input, show loading/error/no-results states, and keep list dimensions stable.

- [ ] **Step 5: Wire models and root routing**

Add `UiLibrary { kind: string, title: string, subtitle: string, status: string, cover: image }` and `UiFeature` Slint structs to `app.slint`, add `library-items`, `recommend-items`, and `feature-statuses` model properties, and show the page selected by `current-page`. Update `bridge.rs::handle_event` so `SearchDone` replaces the search model and clears loading/error state, while `SearchFailed` clears loading and sets `error-text` without discarding the prior results. In `bind_callbacks`, trim the submitted query; ignore it when empty, otherwise set search loading true, clear the prior error, and send `AppCommand::Search(trimmed.to_owned())`. Keep the current `UiSongData` mapping unchanged for title, artist, and duration compatibility.

- [ ] **Step 6: Run focused tests and compile**

Run:

```bash
cargo test -p hmp-desktop demo_recommendations_are_local_and_marked_as_demo
cargo test -p hmp-desktop search_done_replaces_results_without_touching_login_state
cargo check -p hmp-desktop
```

Expected: PASS and all three pages compile through `slint_build`.

- [ ] **Step 7: Commit content pages**

```bash
git add crates/hmp-desktop/ui/library-page.slint crates/hmp-desktop/ui/recommend-page.slint crates/hmp-desktop/ui/search-page.slint crates/hmp-desktop/src/demo.rs crates/hmp-desktop/ui/app.slint crates/hmp-desktop/src/lib.rs crates/hmp-desktop/src/bridge.rs crates/hmp-desktop/src/main.rs crates/hmp-desktop/src/bridge_tests.rs
git commit -m "feat(desktop): add library recommendation and search pages"
```

### Task 5: Expose the real queue and integrate lyrics

**Files:**
- Create: `crates/hmp-desktop/ui/queue-page.slint`
- Create: `crates/hmp-desktop/ui/lyrics-page.slint`
- Create: `crates/hmp-desktop/src/lyrics.rs`
- Modify: `crates/hmp-desktop/src/app.rs`
- Modify: `crates/hmp-desktop/src/lib.rs`
- Modify: `crates/hmp-desktop/ui/app.slint`
- Modify: `crates/hmp-desktop/src/bridge.rs`
- Modify: `crates/hmp-desktop/src/main.rs`
- Test: `crates/hmp-desktop/src/bridge_tests.rs`

**Interfaces:**
- Consumes: `QqMusicClient::with_config(self.client.config())`, `LyricApi::new(&client).get_lyric(mid, song_type, false, true, false, false)`, `QueueItem`, `PlaybackState`, and `AppCommand::PlayIndex`.
- Produces: `parse_lrc(&str, &str) -> Vec<UiLyricData>`, `AppCore::queue_snapshot() -> Vec<UiQueueData>`, `AppCommand::ReloadLyrics`, `AppEvent::QueueUpdated`, `AppEvent::LyricsLoading`, `AppEvent::LyricsLoaded`, and `AppEvent::LyricsFailed`.

- [ ] **Step 1: Write failing parser and bridge tests**

Create parser tests in `lyrics.rs`:

```rust
#[test]
fn parses_lrc_lines_and_matches_translation_by_timestamp() {
    let lines = parse_lrc("[ti:Test]\n[00:01.20]First\n[00:03.00]Second\n", "[00:01.20]译文\n");
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].timestamp_ms, 1_200);
    assert_eq!(lines[0].text, "First");
    assert_eq!(lines[0].translation, "译文");
}

#[test]
fn ignores_lrc_metadata_and_malformed_lines() {
    let lines = parse_lrc("[ar:Artist]\nnot-a-line\n[00:02]Valid\n", "");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].time, "00:02");
}
```

Add bridge tests that map `AppEvent::QueueUpdated` to the generated queue model and matching-MID `AppEvent::LyricsLoaded { mid, lines }` to the generated lyrics model, including current/play flags and translation text. Set `lyrics-request-mid` before the lyric event, then assert an event for a different MID is ignored.

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p hmp-desktop parses_lrc_lines_and_matches_translation_by_timestamp
cargo test -p hmp-desktop queue_event_updates_slint_model
```

Expected: FAIL because the parser, event variants, and page models do not exist.

- [ ] **Step 3: Implement the LRC parser**

Create `lyrics.rs` with:

```rust
pub fn parse_lrc(lyric: &str, translation: &str) -> Vec<UiLyricData>
```

Parse `[mm:ss]`, `[mm:ss.xx]`, and `[mm:ss.xxx]` timestamps into `timestamp_ms`; discard metadata tags such as `[ti:]`, malformed lines, and empty lyric text. Parse translation timestamps into a map and attach matching translations by timestamp. Sort by `timestamp_ms`, preserve duplicate timestamp lines in source order, and format `time` as `MM:SS`.

- [ ] **Step 4: Add real queue snapshot and lyric command handling**

In `AppCore`, implement:

```rust
pub fn queue_snapshot(&self) -> Vec<UiQueueData>
```

Extend `QueueItem` with `song_type: i64`. Replace `client_music_detail`'s tuple with a private `ResolvedSongDetail { media_mid: String, duration: Option<u64>, song_type: i64 }` populated from `detail.track.file.media_mid`, `detail.track.interval`, and `detail.track.song_type`. Store `song_type` on the resolved queue item. Change `resolve_stream` to `resolve_stream(&self, mid: &str, media_mid: &str, song_type: i64)` and pass that value to `SongFileInfo.song_type` instead of the current hard-coded `0`. Map each `QueueItem` to `track_id`, title, artist, duration, and `is_current == index == self.queue_index`; derive `is_playing` from the current `PlaybackState`. In `play_index` and `play_relative`, send `AppEvent::QueueUpdated(self.queue_snapshot())` after changing the queue index or replacing the queue.

Add `current_lyrics: Option<(String, i64)>` to `AppCore`. After a successful load in `play_index` or `play_relative`, store `(mid, song_type)` and call a non-blocking `start_lyrics_load(mid, song_type)`. `start_lyrics_load` sends `LyricsLoading(mid.clone())`, creates a session-local `QqMusicClient` from the existing config, clones `events_tx`, and uses `tokio::spawn` to call `LyricApi::new(&client).get_lyric(&mid, song_type, false, true, false, false)`. Parse `response.lyric` and `response.trans` with `parse_lrc`; send `LyricsLoaded { mid, lines }` even when `lines` is empty, and send `LyricsFailed { mid, message }` only on API failure. `AppCommand::ReloadLyrics` repeats this operation from `current_lyrics`; it does nothing when no current context exists. Do not generate fallback lyric text.

- [ ] **Step 5: Implement queue and lyrics pages**

`queue-page.slint` takes `[UiQueue]`, shows a real row for every item, highlights `is-current`, shows a play indicator for `is-playing`, and invokes `play-requested(index)`. Its empty state invokes `navigate-to-search`.

`lyrics-page.slint` takes `[UiLyric]`, `lyrics-state`, `current-title`, and `current-artist`; shows loading, no-lyrics, error, and line-list states. Each line has fixed vertical padding and displays optional translation without overlap. Add `load-lyrics-requested` so the root can request the current MID again.

- [ ] **Step 6: Wire lyric loading and active-line state**

Add generated Slint structs with these fields:

```slint
export struct UiQueue { track-id: string, title: string, artist: string, duration: string, is-current: bool, is-playing: bool }
export struct UiLyric { time: string, timestamp-ms: float, text: string, translation: string, is-active: bool }
```

Map Rust `UiLyricData` to `UiLyric`. Add `lyrics_model(lines: Vec<UiLyricData>, position_ms: f32) -> ModelRc<UiLyric>` that marks only the last timestamp not greater than `position_ms` as active. Add `lyrics_model_at_position(model: &ModelRc<UiLyric>, position_ms: f32) -> ModelRc<UiLyric>` that copies existing generated rows and recomputes only `is_active`. The main playback subscription calls this helper with `ui.get_lyrics()` and authoritative `PlaybackState.position`; the UI never calculates time and no separate lyric text cache is required.

In `bridge.rs`, `LyricsLoading(mid)` sets `lyrics-request-mid = mid` and `lyrics-state = "loading"`. `LyricsLoaded { mid, lines }` and `LyricsFailed { mid, message }` update the UI only when `mid == lyrics-request-mid`, which discards stale responses after a track change. Bind `load-lyrics-requested` to `AppCommand::ReloadLyrics`. In `main.rs`, the playback subscription updates active lyric flags from the current authoritative position; it does not initiate network requests or maintain a second playback clock.

- [ ] **Step 7: Run focused tests**

Run:

```bash
cargo test -p hmp-desktop parses_lrc_lines_and_matches_translation_by_timestamp
cargo test -p hmp-desktop ignores_lrc_metadata_and_malformed_lines
cargo test -p hmp-desktop queue_event_updates_slint_model
cargo test -p hmp-desktop lyric_event_updates_slint_model
cargo check -p hmp-desktop
```

Expected: PASS. A missing lyric response must result in an empty model and visible no-lyrics state, not test fixture text.

- [ ] **Step 8: Commit queue and lyrics**

```bash
git add crates/hmp-desktop/ui/queue-page.slint crates/hmp-desktop/ui/lyrics-page.slint crates/hmp-desktop/src/lyrics.rs crates/hmp-desktop/src/app.rs crates/hmp-desktop/src/lib.rs crates/hmp-desktop/ui/app.slint crates/hmp-desktop/src/bridge.rs crates/hmp-desktop/src/main.rs crates/hmp-desktop/src/bridge_tests.rs
git commit -m "feat(desktop): expose queue and lyric states"
```

### Task 6: Add settings, theme selection, and documented feature status

**Files:**
- Create: `crates/hmp-desktop/ui/settings-page.slint`
- Modify: `crates/hmp-desktop/ui/theme.slint`
- Modify: `crates/hmp-desktop/ui/app.slint`
- Modify: `crates/hmp-desktop/src/main.rs`
- Modify: `crates/hmp-desktop/src/bridge.rs`
- Modify: `docs/PROJECT.md`
- Test: `crates/hmp-desktop/src/bridge_tests.rs`

**Interfaces:**
- Consumes: `ThemeMode::{FollowSystem, Light, Dark}`, `feature_matrix()`, and `theme-requested(string)`.
- Produces: settings page with immediate theme updates and a user-visible feature matrix that matches the approved design spec.

- [ ] **Step 1: Add failing settings tests**

Add:

```rust
#[test]
#[serial]
fn settings_page_exposes_feature_matrix_and_theme_callback() {
    let ui = init_ui();
    ui.set_current_page("settings".into());
    assert_eq!(ui.get_feature_statuses().row_count(), 7);
    assert_eq!(ui.get_feature_statuses().row_data(5).unwrap().status, "开发中 / 演示数据");
    bind_ui_state_callbacks(&ui);
    ui.invoke_theme_requested("light".into());
    assert_eq!(ui.get_theme_mode(), "light");
    ui.invoke_theme_requested("invalid".into());
    assert_eq!(ui.get_theme_mode(), "light");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cargo test -p hmp-desktop settings_page_exposes_feature_matrix_and_theme_callback
```

Expected: FAIL because the settings component and feature model are not present.

- [ ] **Step 3: Implement settings page and theme controls**

Create `settings-page.slint` with a segmented control or radio-style three-option selector for `system`, `light`, and `dark`. Each selection invokes `theme-requested(mode)` and updates the shared theme global immediately. Render every `UiFeature` row with name, status, and detail; keep status copy exactly aligned with `feature_matrix()`.

Update `theme.slint` so changing the root mode switches the semantic palette without changing layout geometry. Bind the imported Slint 1.17 `Palette.color-scheme` in `AppWindow` to `ColorScheme.unknown`, `.light`, or `.dark` according to the selected mode; `ColorScheme.unknown` is the system-following value in the locked widget implementation. Derive custom colors from the resolved public Palette brushes instead of reading internal system state.

- [ ] **Step 4: Document the status matrix**

Add a “桌面 UI 功能状态” subsection to `docs/PROJECT.md` after the desktop architecture section. Include this exact table:

```markdown
| 功能 | 状态 | 说明 |
| --- | --- | --- |
| 登录 | 已接入 | QQ 音乐扫码登录与凭据状态 |
| 搜索 | 已接入 | 使用 QQ Music Rust API |
| 播放控制 | 已接入 | 播放、暂停、上一首、下一首、Seek、音量 |
| 队列展示 | 已接入 | 展示 AppCore 当前真实队列 |
| 歌词展示 | 部分接入 | 已接入接口与空状态，按真实返回展示 |
| 推荐内容 | 开发中 / 演示数据 | 当前使用本地演示数据 |
| 收藏与资料库同步 | 开发中 | 尚未接入账号云端同步 |
```

- [ ] **Step 5: Run tests and docs checks**

Run:

```bash
cargo test -p hmp-desktop settings_page_exposes_feature_matrix_and_theme_callback
cargo test -p hmp-desktop
cargo check -p hmp-desktop
```

Expected: PASS, and the documented table uses the same statuses shown in the app.

- [ ] **Step 6: Commit settings and records**

```bash
git add crates/hmp-desktop/ui/settings-page.slint crates/hmp-desktop/ui/theme.slint crates/hmp-desktop/ui/app.slint crates/hmp-desktop/src/main.rs crates/hmp-desktop/src/bridge.rs crates/hmp-desktop/src/bridge_tests.rs docs/PROJECT.md
git commit -m "feat(desktop): add theme settings and feature status"
```

### Task 7: Complete root wiring and regression coverage

**Files:**
- Modify: `crates/hmp-desktop/ui/app.slint`
- Modify: `crates/hmp-desktop/src/main.rs`
- Modify: `crates/hmp-desktop/src/bridge.rs`
- Modify: `crates/hmp-desktop/src/bridge_tests.rs`
- Modify: `crates/hmp-desktop/src/app.rs`

**Interfaces:**
- Consumes: every component and model from Tasks 1-6, existing `playback_snapshot`, `handle_event`, `bind_callbacks`, and `AppCore::queue_snapshot`.
- Produces: one fully wired `AppWindow`, with no duplicate playback state and no page that bypasses the root route.

- [ ] **Step 1: Add failing end-to-end bridge assertions**

Extend the single testing-backend integration test with these checks:

```rust
ui.set_current_page("library".into());
assert_eq!(ui.get_current_page(), "library");
handle_event(&ui.as_weak(), AppEvent::QueueUpdated(vec![UiQueueData {
    track_id: "mid-1".into(), title: "晴天".into(), artist: "周杰伦".into(),
    duration: "04:29".into(), is_current: true, is_playing: false,
}]));
assert_eq!(ui.get_queue().row_count(), 1);
assert!(ui.get_queue().row_data(0).unwrap().is_current);
handle_event(&ui.as_weak(), AppEvent::LyricsLoading("mid-1".into()));
handle_event(&ui.as_weak(), AppEvent::LyricsFailed {
    mid: "mid-1".into(), message: "无歌词".into(),
});
assert_eq!(ui.get_lyrics_state(), "error");
```

Preserve all current assertions for login, search, playback, Seek, and volume callbacks.

- [ ] **Step 2: Run the integration test to verify missing wiring**

Run:

```bash
cargo test -p hmp-desktop ui_bridge_integration
```

Expected: FAIL until the root model properties and event branches are complete.

- [ ] **Step 3: Finish root composition**

In `app.slint`, import every page and show exactly one content page based on `current-page`. Keep the sidebar and player bar outside the page switch so they remain mounted during navigation. Place `LoginDialog` as the final child so it visually overlays content when active. Give every dynamic list a fixed row height and bounded viewport.

In `main.rs`, initialize:

```rust
ui.set_current_page("library".into());
ui.set_theme_mode("system".into());
ui.set_recommend_items(bridge::library_model(demo::demo_recommendations()));
ui.set_feature_statuses(bridge::feature_model(demo::feature_matrix()));
ui.set_queue(bridge::queue_model(Vec::new()));
ui.set_lyrics(bridge::lyrics_model(Vec::new(), 0.0));
ui.set_lyrics_state("idle".into());
```

After `AppCore::new`, send the initial queue snapshot to the UI before moving `core` into its runtime task. Extend `AppCore::run`'s `tokio::select!` with a player-state receiver; compare `(current track ID, status == Playing)` to the last published key and send `QueueUpdated(self.queue_snapshot())` only when that key changes. In the existing main playback subscription, update only playback properties, current track identity, authoritative position, and lyric active-line flags; do not mutate playback position from Slint.

- [ ] **Step 4: Add invalid-state guards**

Ensure bridge handlers ignore unknown page names, unknown theme names, negative/out-of-range play indexes, empty lyric MIDs, and stale weak UI handles. Ensure `SearchDone` does not clear login/account state and `LoginDone` does not clear search, queue, or lyrics models.

- [ ] **Step 5: Run complete crate tests**

Run:

```bash
cargo test -p hmp-desktop
cargo check -p hmp-desktop
```

Expected: PASS with all existing and newly added tests.

- [ ] **Step 6: Commit root integration**

```bash
git add crates/hmp-desktop/ui/app.slint crates/hmp-desktop/src/main.rs crates/hmp-desktop/src/bridge.rs crates/hmp-desktop/src/bridge_tests.rs crates/hmp-desktop/src/app.rs
git commit -m "feat(desktop): wire the redesigned application shell"
```

### Task 8: Verify the desktop experience and finish the status record

**Files:**
- Modify: `docs/PROJECT.md` only if verification discovers a documented status mismatch.
- Test: `crates/hmp-desktop/src/bridge_tests.rs` and the built desktop binary.

**Interfaces:**
- Consumes: the complete `AppWindow` and all automated tests from Tasks 1-7.
- Produces: verified automated checks, a manual UI verification record, and a clean status report that distinguishes code defects from unavailable external services.

- [ ] **Step 1: Run formatting and static checks**

Run:

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo clippy -p hmp-desktop --all-targets --all-features -- -D warnings
```

Expected: all commands exit with status 0. Any warning must be fixed in the owning desktop file before continuing.

- [ ] **Step 2: Run the complete test suite**

Run:

```bash
cargo test -p hmp-desktop
cargo test --workspace
```

Expected: all desktop and workspace tests pass. Network-dependent tests may use the repository's existing fixtures and mock servers; do not weaken assertions or skip existing tests.

- [ ] **Step 3: Build the desktop binary**

Run:

```bash
cargo build -p hmp-desktop
```

Expected: the binary builds with Slint 1.17 code generation and no new undeclared dependencies.

- [ ] **Step 4: Perform manual visual verification**

Run the desktop binary from the repository environment and inspect `1100x720` plus the smallest allowed window size. Verify this checklist:

```text
[ ] Startup opens on 资料库.
[ ] Sidebar selection remains visible and changes content without stopping audio.
[ ] Player bar stays mounted on every page.
[ ] Dark, light, and system theme modes update semantic colors immediately.
[ ] Search handles loading, no results, error, and real result playback.
[ ] Queue page displays the real queue and current item.
[ ] Lyrics page shows empty/error states when no lyrics are returned.
[ ] Recommend page shows deterministic demo items and the development banner.
[ ] Settings shows the seven-row feature matrix.
[ ] Login dialog opens, displays QR status, and closes without layout compression.
[ ] Long Chinese titles and artist names elide without overlap.
[ ] Empty or failed cover images retain their fixed container size.
```

- [ ] **Step 5: Record verification results**

Append a dated verification note to `docs/PROJECT.md` only after the commands and manual checks have been run. Record the exact commands, whether the desktop binary launched, and any environment-specific limitation such as unavailable GStreamer audio devices or missing Secret Service. Keep the feature status table unchanged unless implementation genuinely changed a capability.

- [ ] **Step 6: Commit verification documentation**

```bash
git add docs/PROJECT.md
git commit -m "docs: record desktop UI verification"
```

## Final Self-Review Checklist

- [ ] Every approved requirement in `docs/superpowers/specs/2026-08-07-apple-music-ui-redesign-design.md` maps to at least one task above.
- [ ] No task enables recommendation, favorite, cloud-sync, or queue mutation behavior without a corresponding real AppCore/API implementation.
- [ ] `UiPage`, `ThemeMode`, `UiQueueData`, `UiLyricData`, `AppEvent`, and `AppCommand` names remain identical across all task interfaces.
- [ ] No page computes or owns authoritative playback position.
- [ ] Every task has a focused test command and a commit command.
- [ ] The plan contains no placeholders, deferred implementation markers, or unspecified error-handling step.
