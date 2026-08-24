# GPUI UI Fidelity and QQ Music Login Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore the GPUI desktop UI to the `gpui-apple-music-demo` structure, add a visual QQ Music account lifecycle, and display/play real HMP playlists without moving business logic into GPUI.

**Architecture:** The demo source remains the golden view implementation. `hmp-desktop-common::AppCore` owns authentication, credential storage, playlist database reads, queue construction, and playback commands; GPUI receives credential-free `AppEvent` snapshots and renders them through the demo component hierarchy.

**Tech Stack:** Rust 2024, GPUI fork `4c8abab1401d7369da55d9aab928c9405f0af309`, `gpui_effects`, `uic`, Tokio channels/watch, HMP AppCore, `hmp-storage::LibraryDb`, QQ Music QR login.

**Execution:** Inline in the current branch. The user explicitly requested no further decision prompts; multi-agent execution and worktrees are not used.

## Global Constraints

- The demo render tree and layout constants are the golden UI source.
- The only persistent shell addition is the approved 35 px QQ Music account row at the bottom of the sidebar.
- GPUI must never receive `Credential`, cookies, music keys, refresh tokens, or direct database handles.
- `hmp_core::PlaybackState` remains the only playback state.
- All GPUI actions use `AppCommand`; all non-playback data arrives through `AppEvent`.
- Do not depend on `gpui_media`, the Apple Music demo crate, or `hmp-player-gst` from `hmp-desktop-gpui`.
- Do not change the Slint UI design.
- Keep `gpui`, `gpui_effects`, `gpui_platform`, and `uic` pinned to the same revision.
- Update user-facing migration and usage documentation only after implementation and runtime verification.
- Preserve the pre-existing untracked `apps/` directory unchanged.

---

### Task 1: Lock the reference UI contract

**Files:**
- Create: `crates/hmp-desktop-gpui/src/ui_contract.rs`
- Modify: `crates/hmp-desktop-gpui/src/main.rs`
- Modify: `crates/hmp-desktop-gpui/src/theme.rs`

**Interfaces:**
- Produces: `RegularLayout`, `regular_layout(width: f32) -> RegularLayout`, and stable geometry constants consumed by the shell and tests.
- Consumes: no business state.

- [ ] **Step 1: Write failing contract tests**

Add tests before the module is exported:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_geometry_matches_demo() {
        assert_eq!(SIDEBAR_WIDTH, 230.0);
        assert_eq!(SIDEBAR_GUTTER, 7.0);
        assert_eq!(LYRICS_WIDTH, 360.0);
        assert_eq!(TOP_BAR_HEIGHT, 56.0);
        assert_eq!(PLAYER_BOTTOM, 14.0);
        assert_eq!(PLAYER_WIDTH, 600.0);
        assert_eq!(PLAYER_COMPACT_WIDTH, 500.0);
        assert_eq!(SEARCH_WIDTH, 412.0);
        assert_eq!(SEARCH_COMPACT_WIDTH, 310.0);
    }

    #[test]
    fn responsive_thresholds_match_demo() {
        assert_eq!(regular_layout(1080.0), RegularLayout { hide_lyrics: false, compact: false });
        assert_eq!(regular_layout(1079.0), RegularLayout { hide_lyrics: true, compact: false });
        assert_eq!(regular_layout(979.0), RegularLayout { hide_lyrics: true, compact: true });
    }
}
```

- [ ] **Step 2: Run the tests and verify RED**

Run:

```text
cargo test -p hmp-desktop-gpui ui_contract --quiet
```

Expected: compilation fails because `ui_contract` and its constants do not exist.

- [ ] **Step 3: Implement the reference contract**

Create the module with the exact public surface:

```rust
pub const SIDEBAR_WIDTH: f32 = 230.0;
pub const SIDEBAR_GUTTER: f32 = 7.0;
pub const LYRICS_WIDTH: f32 = 360.0;
pub const HIDE_LYRICS_BELOW: f32 = 1080.0;
pub const COMPACT_TOP_BAR_BELOW: f32 = 980.0;
pub const TOP_BAR_HEIGHT: f32 = 56.0;
pub const PLAYER_BOTTOM: f32 = 14.0;
pub const PLAYER_WIDTH: f32 = 600.0;
pub const PLAYER_COMPACT_WIDTH: f32 = 500.0;
pub const SEARCH_WIDTH: f32 = 412.0;
pub const SEARCH_COMPACT_WIDTH: f32 = 310.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegularLayout {
    pub hide_lyrics: bool,
    pub compact: bool,
}

pub fn regular_layout(width: f32) -> RegularLayout {
    RegularLayout {
        hide_lyrics: width < HIDE_LYRICS_BELOW,
        compact: width < COMPACT_TOP_BAR_BELOW,
    }
}
```

Export the module from `main.rs`. Make `theme::layout` re-export these constants so existing component imports remain stable.

- [ ] **Step 4: Run the tests and verify GREEN**

Run:

```text
cargo test -p hmp-desktop-gpui ui_contract --quiet
cargo check -p hmp-desktop-gpui
```

Expected: contract tests pass and the GPUI crate compiles.

- [ ] **Step 5: Commit**

```text
git add crates/hmp-desktop-gpui/src/ui_contract.rs crates/hmp-desktop-gpui/src/main.rs crates/hmp-desktop-gpui/src/theme.rs
git commit -m "test(gpui): lock reference layout contract"
```

### Task 2: Publish a credential-free authentication state machine

**Files:**
- Modify: `crates/hmp-qqmusic-api/src/login.rs`
- Modify: `crates/hmp-desktop-common/src/app.rs`
- Modify: `crates/hmp-desktop/src/bridge.rs`
- Test: existing unit-test modules in those files

**Interfaces:**
- Produces: `UiLoginPhase`, `UiAuthData`, `AppEvent::AuthChanged`, and `AppCommand::Logout`.
- Produces: `LoginApi::wait_qrcode_login_with_updates` while preserving the existing `wait_qrcode_login` API.
- Consumes: `CredentialStore::save/delete`, `QRCodeLoginEvents`, and existing generation cancellation.

- [ ] **Step 1: Write failing authentication tests**

Add tests for public, credential-free UI state and the exact phase mapping:

```rust
#[test]
fn qr_events_map_to_stable_ui_phases() {
    assert_eq!(login_phase(QRCodeLoginEvents::Scan), UiLoginPhase::WaitingScan);
    assert_eq!(login_phase(QRCodeLoginEvents::Conf), UiLoginPhase::WaitingConfirm);
    assert_eq!(login_phase(QRCodeLoginEvents::Timeout), UiLoginPhase::Expired);
}

#[test]
fn auth_snapshot_never_contains_credentials() {
    let auth = UiAuthData::logged_in("10001");
    assert_eq!(auth.phase, UiLoginPhase::LoggedIn);
    assert_eq!(auth.display_name, "10001");
    assert!(auth.message.is_empty());
}

#[test]
fn logout_command_is_part_of_the_shared_protocol() {
    assert!(matches!(AppCommand::Logout, AppCommand::Logout));
}
```

Add a fake `CredentialStore` test that verifies delete failure retains the logged-in credential and successful delete publishes `LoggedOut`.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```text
cargo test -p hmp-desktop-common auth --quiet
cargo test -p hmp-qqmusic-api login --quiet
```

Expected: compilation fails because the new state, command, event, and callback API do not exist.

- [ ] **Step 3: Add the shared UI authentication types**

Add:

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiLoginPhase {
    #[default]
    LoggedOut,
    CreatingQr,
    WaitingScan,
    WaitingConfirm,
    Expired,
    Error,
    LoggedIn,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiAuthData {
    pub phase: UiLoginPhase,
    pub display_name: String,
    pub message: String,
}

impl UiAuthData {
    pub fn logged_out() -> Self { Self::default() }
    pub fn logged_in(name: impl Into<String>) -> Self {
        Self { phase: UiLoginPhase::LoggedIn, display_name: name.into(), message: String::new() }
    }
}
```

Replace string-only login events with `AppEvent::AuthChanged(UiAuthData)` plus the existing QR PNG event. Add `AppCommand::Logout`.

- [ ] **Step 4: Expose QR transition callbacks without breaking CLI callers**

Implement:

```rust
pub async fn wait_qrcode_login_with_updates<F>(
    &self,
    qrcode: &QR,
    interval: PollInterval,
    timeout: Duration,
    cancel: Option<&CancellationToken>,
    mut on_update: F,
) -> Result<Credential, QqMusicError>
where
    F: FnMut(QRCodeLoginEvents),
```

Call `on_update(item.event)` only when `last_event` changes. Make the existing `wait_qrcode_login` delegate with `|_| {}` so CLI behavior and tests remain unchanged.

- [ ] **Step 5: Implement AppCore lifecycle events and logout**

At core startup publish `UiAuthData::logged_in(core.user_name())` or `logged_out()`. In `start_login`, publish `CreatingQr`, then `WaitingScan` with QR bytes, then map callback events to `WaitingScan`/`WaitingConfirm`. Map timeout/refusal/network failures to `Expired` or `Error` with stable messages. On success, save first, update memory second, publish `LoggedIn` last.

Implement logout in this order:

```rust
fn logout(&mut self) {
    self.cancel_login_session();
    match self.store.delete() {
        Ok(()) => {
            self.credential = None;
            let _ = self.events_tx.send(AppEvent::AuthChanged(UiAuthData::logged_out()));
        }
        Err(error) => {
            let current = UiAuthData {
                phase: UiLoginPhase::LoggedIn,
                display_name: self.user_name(),
                message: format!("退出登录失败: {error}"),
            };
            let _ = self.events_tx.send(AppEvent::AuthChanged(current));
        }
    }
}
```

Update the Slint bridge to translate `AuthChanged` into its existing properties without modifying any Slint file.

- [ ] **Step 6: Run tests and verify GREEN**

Run:

```text
cargo test -p hmp-qqmusic-api login --quiet
cargo test -p hmp-desktop-common auth --quiet
cargo test -p hmp-desktop --quiet
```

Expected: all focused tests pass, including existing CLI-compatible login behavior.

- [ ] **Step 7: Commit**

```text
git add crates/hmp-qqmusic-api/src/login.rs crates/hmp-desktop-common/src/app.rs crates/hmp-desktop/src/bridge.rs
git commit -m "feat(desktop): expose visual QQ Music auth state"
```

### Task 3: Expose real HMP playlists through AppCore

**Files:**
- Modify: `crates/hmp-desktop-common/src/app.rs`
- Test: `crates/hmp-desktop-common/src/app.rs`

**Interfaces:**
- Produces: `UiPlaylistData`, `UiPlaylistTrackData`.
- Produces commands: `RefreshPlaylists`, `OpenPlaylist(i64)`, `PlayPlaylistTrack { playlist_id: i64, index: usize }`.
- Produces events: `PlaylistsUpdated`, `PlaylistOpened`, `PlaylistsFailed`.
- Consumes: `LibraryDb::list_playlists`, `LibraryDb::local_playlist_stubs`, and the existing HMP playback resolver.

- [ ] **Step 1: Write failing playlist projection tests**

Use an in-memory real database:

```rust
#[test]
fn real_library_rows_project_to_safe_playlist_data() {
    let mut db = hmp_storage::LibraryDb::open_in_memory().unwrap();
    let id = db.create_playlist("测试歌单").unwrap();
    db.add_playlist_track(id, "qq", "mid-1", "歌曲").unwrap();

    let rows = load_playlist_summaries(&mut db).unwrap();
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].name, "测试歌单");
    assert_eq!(rows[0].track_count, 1);
}

#[test]
fn playlist_tracks_preserve_storage_order() {
    let mut db = hmp_storage::LibraryDb::open_in_memory().unwrap();
    let id = db.create_playlist("p").unwrap();
    db.add_playlist_track(id, "qq", "mid-1", "一").unwrap();
    db.add_playlist_track(id, "local", "local:/two.flac", "二").unwrap();
    let rows = load_playlist_tracks(&mut db, id).unwrap();
    assert_eq!(rows.iter().map(|row| row.title.as_str()).collect::<Vec<_>>(), ["一", "二"]);
}
```

- [ ] **Step 2: Run the tests and verify RED**

Run:

```text
cargo test -p hmp-desktop-common playlist --quiet
```

Expected: compilation fails because the view models and loaders do not exist.

- [ ] **Step 3: Implement safe playlist projections**

Define:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiPlaylistData {
    pub id: i64,
    pub name: String,
    pub track_count: i64,
    pub provider: String,
    pub relation: String,
    pub sync_state: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiPlaylistTrackData {
    pub source_key: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: String,
}
```

Implement `load_playlist_summaries` and `load_playlist_tracks` as pure `LibraryDb` adapters. Open `data_dir().join("library.sqlite3")` in AppCore as an optional library: publish `PlaylistsFailed` if it cannot open, but do not block search or playback startup.

- [ ] **Step 4: Add AppCore commands and events**

On startup, successful login, and successful logout, call `publish_playlists`. `OpenPlaylist(id)` publishes ordered track rows. `PlayPlaylistTrack` validates the selected row, constructs the existing local or QQ play request, and enters the same queue/playback resolver used by all other UI entry points. A missing/stale row emits `PlaylistsFailed` and leaves the queue unchanged.

- [ ] **Step 5: Run tests and verify GREEN**

Run:

```text
cargo test -p hmp-desktop-common playlist --quiet
cargo check -p hmp-desktop-common
```

Expected: real in-memory database tests and command/event tests pass.

- [ ] **Step 6: Commit**

```text
git add crates/hmp-desktop-common/src/app.rs
git commit -m "feat(desktop): expose stored playlists to frontends"
```

### Task 4: Extend the GPUI bridge and view state

**Files:**
- Modify: `crates/hmp-desktop-gpui/src/bridge/core.rs`
- Modify: `crates/hmp-desktop-gpui/src/state.rs`

**Interfaces:**
- Consumes: authentication and playlist protocol from Tasks 2-3.
- Produces: command sender methods and `EventState` fields used by all view components.

- [ ] **Step 1: Write failing bridge and state tests**

Extend `CoreCommandSender` tests to assert exact auth and playlist commands. Add state tests:

```rust
#[test]
fn auth_events_drive_modal_and_account_state() {
    let mut state = EventState::default();
    state.login_modal_open = true;
    state.apply(AppEvent::AuthChanged(UiAuthData::logged_in("10001")));
    assert_eq!(state.auth.phase, UiLoginPhase::LoggedIn);
    assert!(!state.login_modal_open);
    assert!(state.login_qr.is_none());
}

#[test]
fn playlist_events_replace_only_playlist_state() {
    let mut state = EventState::default();
    state.apply(AppEvent::PlaylistsUpdated(vec![playlist(1, "歌单")]));
    assert_eq!(state.playlists.len(), 1);
    assert!(state.search_results.is_empty());
}
```

- [ ] **Step 2: Run tests and verify RED**

Run:

```text
cargo test -p hmp-desktop-gpui bridge --quiet
cargo test -p hmp-desktop-gpui state --quiet
```

Expected: missing methods/fields/variants fail compilation.

- [ ] **Step 3: Implement command and state mapping**

Add sender methods:

```rust
pub fn start_login(&self) { self.send(AppCommand::LoginStart); }
pub fn cancel_login(&self) { self.send(AppCommand::LoginCancel); }
pub fn logout(&self) { self.send(AppCommand::Logout); }
pub fn refresh_playlists(&self) { self.send(AppCommand::RefreshPlaylists); }
pub fn open_playlist(&self, id: i64) { self.send(AppCommand::OpenPlaylist(id)); }
pub fn play_playlist_track(&self, playlist_id: i64, index: usize) {
    self.send(AppCommand::PlayPlaylistTrack { playlist_id, index });
}
```

Extend `EventState` with `auth`, `login_modal_open`, `login_qr`, `playlists`, `selected_playlist`, `playlist_tracks`, and `playlist_error`. Clear the QR and close the modal only on `LoggedIn`; keep it open for `Expired`/`Error`.

- [ ] **Step 4: Run tests and verify GREEN**

Run the two focused commands from Step 2. Expected: all pass.

- [ ] **Step 5: Commit**

```text
git add crates/hmp-desktop-gpui/src/bridge/core.rs crates/hmp-desktop-gpui/src/state.rs
git commit -m "feat(gpui): map auth and playlist events"
```

### Task 5: Restore the reference shell, top bar, and sidebar

**Files:**
- Modify: `crates/hmp-desktop-gpui/src/app.rs`
- Modify: `crates/hmp-desktop-gpui/src/components/sidebar.rs`
- Modify: `crates/hmp-desktop-gpui/src/components/top_bar.rs`
- Create: `crates/hmp-desktop-gpui/src/components/login_overlay.rs`
- Modify: `crates/hmp-desktop-gpui/src/components/mod.rs`

**Interfaces:**
- Consumes: `regular_layout`, GPUI `EventState`, and command sender methods.
- Produces: reference-faithful regular shell plus the approved sidebar footer and centered login overlay.

- [ ] **Step 1: Add failing pure view-helper tests**

Add tests for the six fixed navigation slots, account copy, login actions, and playlist viewport projection:

```rust
#[test]
fn demo_navigation_slots_map_to_hmp_pages() {
    assert_eq!(sidebar_pages(), [
        Page::Search, Page::Recommend, Page::Library,
        Page::Queue, Page::Lyrics, Page::Settings,
    ]);
}

#[test]
fn account_copy_tracks_auth_phase() {
    assert_eq!(account_label(&UiAuthData::logged_out()), "登录 QQ 音乐");
    assert_eq!(account_label(&UiAuthData::logged_in("10001")), "10001");
}
```

- [ ] **Step 2: Run and verify RED**

Run:

```text
cargo test -p hmp-desktop-gpui sidebar --quiet
cargo test -p hmp-desktop-gpui login_overlay --quiet
```

Expected: helper functions and overlay module are missing.

- [ ] **Step 3: Restore `app.rs` from the demo hierarchy**

Use `.deps/gpui-apple-music-demo/src/app.rs` as the direct structural source. Preserve the root/div nesting, sidebar margin, main column, top bar, content slot, player absolute position, responsive lyrics panel, border, background, shadow, and platform conditionals. Use `regular_layout(width.as_f32())` rather than duplicating thresholds. Render `login_overlay::render(self, cx)` as the final child only when `events.login_modal_open` is true, so it layers above the unchanged shell.

- [ ] **Step 4: Restore sidebar and top bar**

Use the demo `nav_section`, `nav_item`, `playlist_art`, `playlist_item`, traffic lights, row metrics, and glass appearance. Adapt only:

- labels/pages/icons to HMP semantics;
- playlist children to `events.playlists`;
- an internal clipped/wheel-scroll region for rows after the first six;
- the fixed account row.

Use `FrostedGlass`/`FrostedGlassAppearance` from the pinned GPUI revision while retaining the demo blur, saturation, brightness, tint, edge, radius, width, and border.

Keep top-bar drag regions, widths, height, input appearance, and shadow identical to the demo.

- [ ] **Step 5: Implement the central login overlay**

Render a full-shell absolute scrim and centered frosted panel. Decode QR bytes with `gpui::Image::from_bytes(ImageFormat::Png, bytes.clone())`. Buttons send only the bridge commands:

```rust
match app.events.auth.phase {
    UiLoginPhase::LoggedOut => app.commands.start_login(),
    UiLoginPhase::Expired | UiLoginPhase::Error => app.commands.start_login(),
    UiLoginPhase::LoggedIn => app.commands.logout(),
    _ => app.commands.cancel_login(),
}
```

Closing the overlay sends `cancel_login` unless already logged in. Never print or retain credential fields.

- [ ] **Step 6: Run tests/check and verify GREEN**

```text
cargo test -p hmp-desktop-gpui sidebar --quiet
cargo test -p hmp-desktop-gpui login_overlay --quiet
cargo check -p hmp-desktop-gpui
```

- [ ] **Step 7: Commit**

```text
git add crates/hmp-desktop-gpui/src/app.rs crates/hmp-desktop-gpui/src/components
git commit -m "feat(gpui): restore reference desktop shell"
```

### Task 6: Render HMP content and real playlist details in the reference content slot

**Files:**
- Modify: `crates/hmp-desktop-gpui/src/components/content.rs`
- Modify: `crates/hmp-desktop-gpui/src/state.rs`

**Interfaces:**
- Consumes: navigation, search, queue, playlists, playlist tracks, and errors from `EventState`.
- Produces: page bodies that do not alter the demo content-slot geometry.

- [ ] **Step 1: Write failing content projection tests**

Test that each HMP page maps to the intended demo slot copy/icon family, opening a playlist retains the selected stable ID, and list rows keep 58 px search/playlist-track rhythm.

```rust
#[test]
fn playlist_selection_is_stable_by_database_id() {
    let mut nav = NavigationState::default();
    nav.open_playlist(42);
    assert_eq!(nav.selected_playlist_id, Some(42));
    assert_eq!(nav.page, Page::Library);
}
```

- [ ] **Step 2: Run and verify RED**

```text
cargo test -p hmp-desktop-gpui content --quiet
```

- [ ] **Step 3: Implement page bodies**

Keep the demo outer content container. Render:

- Search: existing HMP results using demo type/color/radius tokens.
- Recommend/Library/Settings: demo empty-state geometry until their real event data is available.
- Queue: real `UiQueueData` rows and `play_queue_item`.
- Lyrics page: the same fullscreen lyrics renderer used by Now Playing.
- Selected playlist: title/count/status plus real ordered track rows; click sends `play_playlist_track`.

Every loading/error/empty state remains inside the same `flex_1/min_h_0` slot and reserves bottom space for the floating player.

- [ ] **Step 4: Run tests/check and verify GREEN**

```text
cargo test -p hmp-desktop-gpui content --quiet
cargo check -p hmp-desktop-gpui
```

- [ ] **Step 5: Commit**

```text
git add crates/hmp-desktop-gpui/src/components/content.rs crates/hmp-desktop-gpui/src/state.rs
git commit -m "feat(gpui): render real playlists in reference content"
```

### Task 7: Restore player, lyrics, and Now Playing render trees

**Files:**
- Modify: `crates/hmp-desktop-gpui/src/components/player_bar.rs`
- Modify: `crates/hmp-desktop-gpui/src/components/lyrics_panel.rs`
- Modify: `crates/hmp-desktop-gpui/src/components/now_playing.rs`
- Modify: `crates/hmp-desktop-gpui/src/bridge/playback.rs`

**Interfaces:**
- Consumes: exact HMP `PlaybackState`, `EventState` lyrics/queue, and `CoreCommandSender`.
- Produces: reference-faithful playback surfaces with no demo player state.

- [ ] **Step 1: Add failing adapter tests**

Cover progress, elapsed/remaining text, status-to-icon, repeat mode, shuffle selection, duration unknown, empty track, lyric active index, focus direction, and queue/lyrics Now Playing switch.

```rust
#[test]
fn unknown_duration_keeps_reference_remaining_placeholder() {
    let state = PlaybackState::default();
    assert_eq!(remaining_text(&state), "--");
}

#[test]
fn loop_mode_maps_to_reference_repeat_state() {
    assert_eq!(repeat_icon_state(LoopMode::None), RepeatIconState::Off);
    assert_eq!(repeat_icon_state(LoopMode::Track), RepeatIconState::One);
    assert_eq!(repeat_icon_state(LoopMode::Playlist), RepeatIconState::All);
}
```

- [ ] **Step 2: Run and verify RED**

```text
cargo test -p hmp-desktop-gpui playback --quiet
cargo test -p hmp-desktop-gpui lyrics --quiet
```

- [ ] **Step 3: Restore player bar**

Use the demo file as the structural body. Preserve 27/31 px controls, grouping, cover/metadata placement, progress hover zone, right controls, glass/background/border/shadow, and 500/600 px widths. Replace only demo calls with bridge commands. Seek uses the clicked/dragged HMP duration and submits seconds through `AppCommand::Seek`.

- [ ] **Step 4: Restore lyrics panel**

Preserve the demo panel/header/gradient/range/row/animation structure. Map `UiLyricData` to line-level rows and label the mode `LINE SYNC`. Keep focus-change animation generation stable so idle pages do not animate continuously.

- [ ] **Step 5: Restore Now Playing**

Preserve Album Glow, cover formula, stage padding, 92 px gap, metadata, controls, window buttons, and view switcher. Lyrics and queue occupy the same right stage. Only this render branch calls `window.request_animation_frame()`.

- [ ] **Step 6: Run tests/check and verify GREEN**

```text
cargo test -p hmp-desktop-gpui --quiet
cargo check -p hmp-desktop-gpui
```

- [ ] **Step 7: Commit**

```text
git add crates/hmp-desktop-gpui/src/components/player_bar.rs crates/hmp-desktop-gpui/src/components/lyrics_panel.rs crates/hmp-desktop-gpui/src/components/now_playing.rs crates/hmp-desktop-gpui/src/bridge/playback.rs
git commit -m "refactor(gpui): restore reference playback surfaces"
```

### Task 8: Run visual/runtime verification and update documentation last

**Files:**
- Modify after verification: `docs/PROJECT.md`
- Modify after verification: `docs/USAGE.md`
- Modify after verification: `docs/gpui-desktop-prototype-performance.md`
- Modify if attribution changed: `crates/hmp-desktop-gpui/THIRD_PARTY_NOTICES.md`

**Interfaces:**
- Consumes: completed implementation from Tasks 1-7.
- Produces: verified release artifacts and source-backed documentation.

- [ ] **Step 1: Run automated verification**

Run fresh commands and require exit code 0:

```text
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace --quiet
cargo build --release -p hmp-desktop -p hmp-desktop-gpui
git diff --check
git ls-files -u
```

- [ ] **Step 2: Perform Windows UI verification**

Launch the release GPUI binary and inspect it through Windows Computer Use at 1280×800, below 1080 px, and below 980 px. Verify shell bounds, sidebar/account/playlist behavior, top-bar symmetry, lyrics visibility, player widths, resize/maximize/restore, Now Playing, and login overlay QR/error/cancel states. Close every process after capture.

Expected: only HMP copy/data/semantic glyphs and the approved account footer differ from the demo. Fixed bounds differ by no more than one physical pixel at the same viewport.

- [ ] **Step 3: Re-run the bounded release benchmark if the render workload changed**

Use:

```text
powershell -File scripts/benchmark-desktop.ps1 -AppPath target/release/hmp-desktop-gpui.exe -Label GPUI-reference-faithful -WarmupSeconds 2 -GpuSamples 3
```

Record startup/RSS/CPU/GPU output without claiming authenticated playback measurements unless a deterministic playback run was actually completed.

- [ ] **Step 4: Update documentation from verified evidence**

Update, in this order:

1. `docs/PROJECT.md`: reference-first GPUI architecture, auth state model, playlist bridge, current completion status.
2. `docs/USAGE.md`: `cargo run --release -p hmp-desktop-gpui`, sidebar login, QR retry/cancel, logout, and playlist navigation.
3. `docs/gpui-desktop-prototype-performance.md`: pinned revision, restored UI workload, new measurements, and remaining platform gaps.
4. `THIRD_PARTY_NOTICES.md`: confirm demo attribution and pinned fork revision remain correct.

- [ ] **Step 5: Verify documentation and repository state**

```text
rg -n "4c8abab1401d7369da55d9aab928c9405f0af309|QQ 音乐|hmp-desktop-gpui" docs crates/hmp-desktop-gpui/THIRD_PARTY_NOTICES.md
git diff --check
git status --short --branch
```

Expected: only intended tracked files plus the pre-existing `?? apps/` remain.

- [ ] **Step 6: Commit**

```text
git add docs/PROJECT.md docs/USAGE.md docs/gpui-desktop-prototype-performance.md crates/hmp-desktop-gpui/THIRD_PARTY_NOTICES.md
git commit -m "docs(gpui): document faithful UI and visual login"
```

### Task 9: Final verification and branch handoff

**Files:**
- No intended file changes.

**Interfaces:**
- Consumes: all completed tasks.
- Produces: a clean, tested branch ready for user-selected integration.

- [ ] **Step 1: Run the complete final gate again**

```text
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace --quiet
cargo build --release -p hmp-desktop -p hmp-desktop-gpui
git diff --check
git diff --cached --check
git ls-files -u
git status --short --branch
```

- [ ] **Step 2: Report evidence and unresolved platform gaps**

Report exact test totals, build result, release smoke result, visual inspection viewports, benchmark output, current branch/HEAD, and the untouched `apps/` directory. State Linux/macOS gaps explicitly if the required host/toolchain remains unavailable.
