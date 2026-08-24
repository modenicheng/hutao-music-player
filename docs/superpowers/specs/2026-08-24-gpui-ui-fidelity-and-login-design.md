# GPUI UI Fidelity and QQ Music Login Design

## Status

Approved on 2026-08-24. The user approved the reference-first migration approach and authorized the remaining recommended design decisions without further approval prompts.

## Goal

Rework `hmp-desktop-gpui` so the visible shell, geometry, component hierarchy, spacing, effects, responsive behavior, and Now Playing layout remain faithful to `cradiy/gpui-apple-music-demo`, while all text, data, and behavior come from HMP. Add visual QQ Music login and real HMP playlist data without introducing a second playback or authentication implementation.

## Governing Sources

The UI golden source is the checked reference tree at:

```text
.deps/gpui-apple-music-demo/src/
```

The GPUI dependencies remain pinned together at:

```text
4c8abab1401d7369da55d9aab928c9405f0af309
```

The original migration plan remains authoritative for business boundaries: GPUI is a view, `hmp_core::PlaybackState` is the only playback state, and `AppCommand`/`AppEvent` remain the only UI-to-core protocol.

## Approved Decisions

1. Use a reference-first structural reset, not an incremental reskin of the current prototype.
2. Treat the demo render tree and layout constants as the golden source.
3. Replace only visible copy, data, derived view values, and event handlers with HMP equivalents.
4. Add one explicitly approved persistent deviation: a QQ Music account row pinned to the bottom of the sidebar.
5. Clicking the account row opens a centered glass login overlay inside the existing app shell.
6. Support the complete account lifecycle: logged out, QR creation, waiting for scan, waiting for confirmation, expired/error, cancellation, logged in, and logout.
7. Display real local and synchronized playlists from HMP storage in the demo playlist section.
8. Update user-facing and migration documentation only after the code work is complete.

## Non-Goals

- Do not depend on the Apple Music demo crate at runtime.
- Do not import `gpui_media` or the demo playback state.
- Do not change the Slint UI design.
- Do not replace `hmp-desktop` or remove Slint.
- Do not expose credentials, cookies, music keys, or refresh tokens to GPUI.
- Do not redesign the demo shell to look more like another music application.
- Do not implement unrelated QQ Music account features.
- Do not add custom shaders when the pinned GPUI fork already provides the effect.

## UI Contract

### Regular window

The regular window keeps the demo hierarchy:

```text
root
├── sidebar container
└── body
    ├── main column
    │   ├── top bar
    │   ├── page content
    │   └── floating player bar
    └── inline lyrics panel (responsive)
```

The following values are fixed to the demo values:

| Property | Value |
| --- | ---: |
| Sidebar width | 230 px |
| Sidebar gutter | 7 px |
| Lyrics panel width | 360 px |
| Hide lyrics below | 1080 px |
| Compact top bar below | 980 px |
| Top bar height | 56 px |
| Regular search width | 412 px |
| Compact search width | 310 px |
| Regular player width | 600 px |
| Compact player width | 500 px |
| Player bottom offset | 14 px |
| Player radius | 24 px |
| Non-Windows root radius | 22 px |

The app root preserves the demo background, border, shadow, overflow, sidebar margin, main-column nesting, and lyrics-panel placement. Windows keeps native snap/maximize behavior and DWM integration; macOS traffic-light positioning remains conditional; Linux retains the configured font and Wayland/X11 features.

### Sidebar

Restore the demo sidebar component structure:

```text
traffic-light header
primary navigation rows
library section label
four library navigation rows
playlist section label
playlist rows
account footer row (approved addition)
```

Navigation uses HMP semantics while preserving row height, padding, typography, selected/hover backgrounds, icon size, and color treatment. The six stable pages map into the six demo navigation slots:

| Demo slot | HMP page |
| --- | --- |
| Search | Search |
| Home | Recommend |
| Recently Added | Library |
| Artists | Queue |
| Albums | Lyrics |
| Songs | Settings |

Use the matching Lucide semantic glyph for each HMP page, while keeping the demo icon size, alignment, opacity, hover behavior, and selected-state styling unchanged.

The playlist section uses the demo `playlist_item` geometry and gradient-art treatment. It displays real `LibraryDb::list_playlists()` results. At the reference window height, the first six rows occupy the same slots as the demo. Additional rows use wheel scrolling inside a clipped playlist region with no permanently visible scrollbar; the header, primary navigation, and account row never move. An empty library shows one disabled 35 px row rather than removing the section.

The approved account footer is 35 px high and uses the same row metrics as navigation entries. Logged out copy is `登录 QQ 音乐`. Logged in copy uses the available safe display name, currently the stored UIN when no profile nickname is available. A generic local icon is used; remote avatar fetching is out of scope.

### Top bar

Keep the demo's symmetrical drag regions and centered search container. Do not add an avatar, account button, navigation button, or window action to the top bar. Adapt the pinned `uic::InputAppearance` API through standard `Styled` refinements while retaining the demo dimensions and colors.

### Content

Keep the demo content region as the only page-content slot: `flex_1`, `min_h_0`, and the same relation to the top bar and floating player. HMP pages render the list, loading, error, or empty-state body required by their data while using the demo typography, icon scale, row rhythm, corner radii, colors, and content padding. Page data must come from `AppEvent`; components may not access QQ Music or SQLite directly.

### Player bar

Use the demo player-bar render tree as the implementation body. Preserve control groups, metadata placement, progress-hover behavior, artwork size, radius, spacing, shadows, right-side buttons, and compact widths. Replace demo player calls with `CoreCommandSender` methods only:

| UI action | Command |
| --- | --- |
| Play/Pause | `AppCommand::TogglePlay` |
| Previous | `AppCommand::Previous` |
| Next | `AppCommand::Next` |
| Seek | `AppCommand::Seek` |
| Volume | `AppCommand::SetVolume` |
| Shuffle | `AppCommand::SetShuffle` |
| Repeat | `AppCommand::SetLoopMode` |

All metadata, position, duration, capabilities, volume, loop mode, and shuffle state derive from `hmp_core::PlaybackState`.

### Lyrics

Restore the demo panel hierarchy, 360 px inline panel, header placement, gradients, row spacing, typography, opacity curve, and focus-change animation. HMP line lyrics map into this view without adding a second lyric domain. The mode label is `LINE SYNC`; `TimedText` word animation remains inactive until HMP provides valid word ranges.

### Now Playing

Restore the demo structure and formulas, including Album Glow, overlay tint, cover-size calculation, content padding, 92 px stage gap, metadata controls, fullscreen player controls, window controls, and bottom-right view switcher. Only Now Playing may request continuous animation frames. The lyrics and queue views reuse the same right-hand stage; toggling them must not move the cover/control column.

## Authentication Design

### Shared UI model

Add a credential-free authentication view model to `hmp-desktop-common`:

```rust
pub enum UiLoginPhase {
    LoggedOut,
    CreatingQr,
    WaitingScan,
    WaitingConfirm,
    Expired,
    Error,
    LoggedIn,
}

pub struct UiAuthData {
    pub phase: UiLoginPhase,
    pub display_name: String,
    pub message: String,
}
```

`AppEvent` publishes authentication state and QR PNG bytes. It never publishes `Credential` or any sensitive field. Both desktop frontends consume the same events; adapting the Slint bridge must not change its visible design.

### Commands

The UI sends only:

```rust
AppCommand::LoginStart
AppCommand::LoginCancel
AppCommand::Logout
```

`LoginStart` cancels an older generation, publishes `CreatingQr`, obtains a QQ QR code, publishes the PNG and `WaitingScan`, then forwards distinct scan/confirmation states from the QQ Music login loop. Closing the modal sends `LoginCancel`. Expired and failed states retain the modal and provide a retry action that sends `LoginStart` again.

`Logout` cancels login work, calls `CredentialStore::delete`, clears the in-memory credential only after deletion succeeds, and publishes `LoggedOut`. Logout does not forcibly stop an already playing track; later authenticated requests use the resulting logged-out state.

At startup, AppCore publishes the credential-derived initial authentication state so GPUI does not infer login from the presence of a QR or a previous UI session.

### Login overlay

The overlay is rendered above the unchanged regular shell. It uses a centered frosted-glass panel, QR image, stable status copy, retry/cancel actions, and the existing accent palette. It does not create a second native window. Closing it cancels polling. Successful login clears QR memory, closes the overlay, updates the account footer, and refreshes playlists.

## Playlist Design

Add safe desktop view data:

```rust
pub struct UiPlaylistData {
    pub id: i64,
    pub name: String,
    pub track_count: i64,
    pub provider: String,
    pub relation: String,
    pub sync_state: String,
}

pub struct UiPlaylistTrackData {
    pub source_key: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: String,
}
```

The UI protocol adds refresh/open/play commands and success/failure events. AppCore opens `data_dir()/library.sqlite3`, calls `LibraryDb::list_playlists()` and `local_playlist_stubs()`, and translates storage rows into safe UI data. Database errors become visible events and do not cause the component to open SQLite itself.

Playlists load at core startup and refresh after successful login or logout. Clicking a sidebar playlist navigates the existing content slot to a playlist detail view. Selecting a track routes through AppCore, which resolves local and QQ sources using the existing media and playback pipeline before updating the real queue.

## Error Handling

- QR acquisition failure: show `Error`, keep the modal open, expose retry and cancel.
- QR expiry: show `Expired`, discard the expired PNG, expose retry and cancel.
- Login refusal: show `Error` with a refusal message; do not auto-retry.
- Credential save failure: do not mark the session logged in.
- Credential delete failure: retain the current logged-in state and display the error.
- Playlist database failure: keep the sidebar section and display a disabled error row; other playback features remain available.
- Playlist track no longer present: publish a playlist error and do not mutate the playback queue.
- Stale login, lyric, search, and playlist generations are ignored.

## Testing and Visual Verification

### Automated tests

1. View helpers: fixed layout constants, compact thresholds, time/progress formatting, login copy, playlist row projection, and lyric focus ranges.
2. Bridge commands: every player, auth, playlist refresh/open, and playlist playback action emits the expected `AppCommand`.
3. Auth state: startup snapshot, QR creation, scan/confirm transitions, stale generation rejection, cancel, expired, save failure, successful login, delete failure, and successful logout.
4. Playlist state: real in-memory `LibraryDb` rows map to UI data; empty, synced, pending, error, missing track, and ordered-track cases do not panic.
5. Event state: authentication and playlist events update GPUI state without touching `PlaybackState`.
6. Existing workspace tests remain green.

### Runtime checks

Build and launch the release GPUI binary on Windows. Check at minimum:

- 1280×800 regular shell;
- width below 1080 px with hidden lyrics;
- width below 980 px with compact search/player widths;
- resize, maximize, restore, and Windows Snap behavior;
- sidebar overflow with zero, six, and more than six playlists;
- login overlay creation, cancel, retry/error, and QR rendering without exposing credentials;
- Now Playing open/close and lyrics/queue switch;
- idle regular pages do not continuously request frames.

Visual comparison accepts differences only for HMP copy/data, semantic Lucide glyphs, dynamic artwork/lyrics, and the approved account footer. Shell bounds and fixed component geometry must match the demo at the same viewport, with at most one physical pixel of platform rasterization variance.

### Completion commands

```text
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace --quiet
cargo build --release -p hmp-desktop -p hmp-desktop-gpui
git diff --check
```

Linux Wayland/X11 and macOS checks are attempted when the required host or cross-toolchain is available. Missing host tooling must be reported as a validation gap, not represented as a passing runtime test.

## Documentation Order

After code and runtime verification are complete:

1. Update `docs/PROJECT.md` with the GPUI UI/auth/playlist architecture and current status.
2. Update `docs/USAGE.md` with GPUI startup, visual QQ login, cancel/retry/logout, and playlist usage.
3. Update `docs/gpui-desktop-prototype-performance.md` with the final pinned GPUI revision, restored reference UI scope, and fresh measurements if the render workload changed materially.
4. Preserve the reference attribution and license notice in `crates/hmp-desktop-gpui/THIRD_PARTY_NOTICES.md`.

Documentation must describe verified behavior only and must not claim untested Wayland or macOS runtime support.
