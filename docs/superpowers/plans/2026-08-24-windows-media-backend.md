# Windows Media Backend Compatibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the legacy native multimedia stack, install a Rust-native audio backend, and expose the daemon through Windows SMTC without changing playback policy or frontend ownership.

**Architecture:** Backend-neutral load/event protocol types live in `hmp-core`; `hmp-player` implements `PlaybackDriver` behavior with Rodio and seekable file/HTTP inputs. Linux MPRIS and Windows SMTC are sibling projections of the same command, state, and capability channels.

**Tech Stack:** Rust 2024, Tokio, Rodio/CPAL, stream-download, Windows WinRT (`windows` crate), MPRIS/zbus, Vue 3, Tauri 2.

## Global Constraints

- Preserve `PlaybackEngine -> PlaybackDriver` and `PlaybackEngine -> SourceResolver` seams.
- `PlaybackState` remains the only playback state source.
- Remove all active legacy multimedia dependencies, code paths, setup, and runtime requirements.
- Windows system controls use SMTC and communicate only through `PlayerCommand` and watch snapshots.
- Do not modify the user's current frontend work in `HomeView.vue`, `HoverGroup.vue`, or `HoverItem.vue`.

---

### Task 1: Backend-neutral playback protocol

**Files:**
- Modify: `crates/hmp-core/src/player.rs`
- Modify: `crates/hmp-core/src/lib.rs`
- Modify: `crates/hmp-daemon/src/player.rs`
- Modify: `crates/hmp-daemon/src/engine.rs`

**Interfaces:**
- Produces: `hmp_core::LoadRequest` and `hmp_core::PlayerEvent`.
- Consumes: existing `Track`, `AudioQuality`, `HmpError`, and generation semantics.

- [ ] **Step 1: Write a serialization-independent domain test**

```rust
#[test]
fn player_event_preserves_load_generation() {
    let event = PlayerEvent::PlaybackEnded { load_gen: 42 };
    assert_eq!(event.load_gen(), Some(42));
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run: `cargo test -p hmp-core player_event_preserves_load_generation`

Expected: compilation fails because `LoadRequest`, `PlayerEvent`, or `load_gen()` is not defined.

- [ ] **Step 3: Add the domain types and replace concrete-crate imports**

```rust
pub struct LoadRequest {
    pub track: Track,
    pub uri: String,
    pub quality: AudioQuality,
    pub load_gen: u64,
}

pub enum PlayerEvent {
    TrackChanged,
    PlaybackEnded { load_gen: u64 },
    Error { load_gen: u64, error: HmpError },
    BufferingChanged(Option<f64>),
}
```

- [ ] **Step 4: Run the focused core and daemon tests**

Run: `cargo test -p hmp-core player_event_preserves_load_generation && cargo test -p hmp-daemon --lib engine::tests`

Expected: PASS once concrete player dependencies are no longer needed for protocol types.

### Task 2: Rust-native player crate

**Files:**
- Delete: the legacy player crate
- Create: `crates/hmp-player/Cargo.toml`
- Create: `crates/hmp-player/src/lib.rs`
- Create: `crates/hmp-player/src/source.rs`
- Create: `crates/hmp-player/src/core.rs`
- Modify: `Cargo.toml`
- Modify: `crates/hmp-daemon/Cargo.toml`
- Modify: `crates/hmp-daemon/src/player.rs`
- Modify: `crates/hmp-desktop/Cargo.toml`
- Modify: `crates/hmp-desktop/src/app.rs`

**Interfaces:**
- Consumes: `hmp_core::{LoadRequest, PlayerCommand}`.
- Produces: `PlayerCore`, state watch, and `PlayerEvent` broadcast.

- [ ] **Step 1: Write failing source and state-machine tests**

```rust
#[test]
fn file_uri_roundtrips_windows_paths() {
    let path = std::env::temp_dir().join("hmp player test.wav");
    let uri = url::Url::from_file_path(&path).unwrap();
    assert_eq!(file_path(&uri).unwrap(), path);
}

#[tokio::test]
async fn generated_wav_loads_seeks_and_ends_without_device() {
    let core = PlayerCore::new_silent_for_test();
    core.load(wav_request(7));
    wait_for_status(&core, PlaybackStatus::Playing).await;
    core.seek(Duration::from_millis(20));
    assert_eq!(core.subscribe_state().borrow().load_gen, 7);
}
```

- [ ] **Step 2: Run the player tests and verify RED**

Run: `cargo test -p hmp-player`

Expected: package or implementation is missing.

- [ ] **Step 3: Implement file/HTTP preparation and Rodio control loop**

Use `Decoder::try_from(File)` for local files and `StreamDownload::new_http(...,
TempStorageProvider::new(), Settings::default())` for HTTP(S). Append decoded sources
to a Rodio `Player`; publish load errors with the request generation.

- [ ] **Step 4: Run focused player and engine tests**

Run: `cargo test -p hmp-player && cargo test -p hmp-daemon --lib`

Expected: PASS with no multimedia SDK installed.

### Task 3: Windows SMTC adapter

**Files:**
- Create: `crates/hmp-smtc/Cargo.toml`
- Create: `crates/hmp-smtc/src/lib.rs`
- Create: `crates/hmp-smtc/src/model.rs`
- Create: `crates/hmp-smtc/src/windows.rs`
- Modify: `Cargo.toml`
- Modify: `crates/hmp-daemon/Cargo.toml`
- Create: `crates/hmp-daemon/src/media_session.rs`
- Modify: `crates/hmp-daemon/src/lib.rs`
- Modify: `crates/hmp-daemon/src/serve.rs`

**Interfaces:**
- Consumes: `PlayerCommand`, `PlaybackState`, and `PlaybackCapabilities`.
- Produces: `SmtcService` and `PlatformMediaService` lifetime guards.

- [ ] **Step 1: Write failing pure projection tests**

```rust
#[test]
fn playing_state_projects_metadata_timeline_and_capabilities() {
    let projection = Projection::from_state(&sample_playing_state(), caps(true, false));
    assert_eq!(projection.status, ProjectedStatus::Playing);
    assert_eq!(projection.title.as_deref(), Some("Song"));
    assert!(projection.can_seek);
    assert!(projection.can_next);
}

#[test]
fn smtc_buttons_map_to_domain_commands() {
    assert_eq!(map_button(ProjectedButton::Play), Some(PlayerCommand::Play));
    assert_eq!(map_button(ProjectedButton::Next), Some(PlayerCommand::Next));
}
```

- [ ] **Step 2: Run and verify RED**

Run: `cargo test -p hmp-smtc`

Expected: package/projection types are missing.

- [ ] **Step 3: Implement projection and the Windows owner thread**

The WinRT thread initializes MTA, creates a `MediaPlayer`, disables its command
manager, configures its SMTC, registers callbacks, and applies projection snapshots.
No callback modifies playback state directly.

- [ ] **Step 4: Compose the platform service in daemon startup**

Start SMTC after `Daemon::start` on Windows, MPRIS on Linux with the existing feature,
and retain the returned guard until engine termination.

- [ ] **Step 5: Run SMTC and daemon tests on Windows**

Run: `cargo test -p hmp-smtc && cargo test -p hmp-daemon --all-targets`

Expected: PASS and Windows WinRT APIs compile natively.

### Task 4: Remove implementation-specific configuration and packaging

**Files:**
- Delete: the legacy Windows multimedia SDK setup script
- Modify: `crates/hmp-storage/src/config.rs`
- Modify: `crates/hmp-daemon/src/main.rs`
- Modify: `crates/hmp-daemon/src/serve.rs`
- Modify: `crates/hmp-cli/tests/daemon_cli.rs`
- Modify: `README.md`
- Modify: `docs/PROJECT.md`
- Modify: `docs/USAGE.md`
- Modify: active specs/plans that prescribe the removed runtime

**Interfaces:**
- Preserves: `[audio].replaygain`.
- Removes: `[audio].sink`, `--sink`, fake sink smoke paths, SDK setup.

- [ ] **Step 1: Change configuration tests first**

```rust
#[test]
fn legacy_sink_is_ignored_but_replaygain_is_loaded() {
    write_config("[audio]\nsink = 'old'\nreplaygain = false\n");
    assert!(!Config::load().audio.replaygain);
}
```

- [ ] **Step 2: Run and verify RED against the old `sink` field expectation**

Run: `cargo test -p hmp-storage config::tests::legacy_sink_is_ignored_but_replaygain_is_loaded`

Expected: FAIL until the test and schema reflect the backend-neutral config.

- [ ] **Step 3: Remove the option, script, test assumptions, and active documentation**

Unknown legacy TOML fields remain tolerated by Serde, so existing users are not blocked.

- [ ] **Step 4: Prove the removed runtime is absent**

Run: inspect manifests, lockfile, scripts, and active docs for the removed SDK and crate names.

Expected: no matches.

### Task 5: Full verification and audit closure

**Files:**
- Modify: `Cargo.lock`
- Review: all changed files and repository status

**Interfaces:**
- Produces: reproducible Windows build/test evidence and an audit summary.

- [ ] **Step 1: Regenerate dependency resolution**

Run: `cargo check --workspace --all-targets`

Expected: exits 0 on Windows without `PKG_CONFIG_PATH` or an external media SDK.

- [ ] **Step 2: Run formatting, tests, lint, and both desktop builds**

```powershell
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo check --manifest-path apps/hmp-tauri/src-tauri/Cargo.toml --all-targets
pnpm --dir apps/hmp-tauri test -- --run
pnpm --dir apps/hmp-tauri build
```

- [ ] **Step 3: Inspect the final diff and preserve user work**

Run: `git diff --check; git status --short --branch; git diff --stat`

Expected: no whitespace errors; the three pre-existing frontend paths remain unstaged and
otherwise untouched.
