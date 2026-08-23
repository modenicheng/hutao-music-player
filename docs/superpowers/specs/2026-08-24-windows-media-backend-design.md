# Windows Media Backend Compatibility Design

## Context

The backend already has the correct high-level separation: application policy lives in
`PlaybackEngine`, audio I/O is behind `PlaybackDriver`, sources are behind
`SourceResolver`, and desktop media integrations consume the same command channel and
`PlaybackState` watch. This migration preserves those boundaries.

The current implementation nevertheless has three Windows blockers:

1. The legacy player makes every workspace build depend on an external GLib-based
   multimedia SDK and runtime. A clean Windows build currently fails in `pkg-config`
   before HMP code is compiled.
2. `LoadRequest` and `PlayerEvent` are owned by the concrete legacy player crate, so the
   daemon's otherwise generic `PlaybackDriver` leaks an implementation choice.
3. Linux MPRIS is the only system media protocol. Windows does not publish a System
   Media Transport Controls (SMTC) session and cannot receive media-key or timeline
   requests through the backend.

## Goals

- Remove the legacy native multimedia stack, dependencies, setup scripts, runtime instructions, and active
  configuration from the repository.
- Replace the audio implementation with a Rust-native Rodio/CPAL backend that supports
  local `file://` sources and seekable HTTP(S) streams.
- Preserve one authoritative `PlaybackState` and the existing engine/driver/resolver
  separation.
- Add a Windows SMTC adapter with playback state, metadata, capabilities, timeline,
  play/pause/stop/next/previous, seek, repeat, and shuffle integration.
- Keep Linux MPRIS behavior unchanged and make platform protocol selection explicit.
- Make the complete Rust workspace compile and test on Windows without an external
  multimedia SDK.

## Non-goals

- Rewriting queue policy, QQ Music resolution, QMC2 decryption, IPC, or frontend state.
- Moving audio playback into Tauri/WebView.
- Selecting arbitrary output devices in this migration. CPAL's default device is used;
  the old `sink` option is removed because it names implementation-specific elements.
- Using Windows Media Foundation as the decoder. That would bind the audio engine to
  Windows and weaken the existing cross-platform driver boundary.

## Architecture

```text
UI / CLI / system media protocol
              |
              v
     PlayerCommand / Request
              |
              v
       PlaybackEngine              queue, rollback, sessions, policy
          |          |
          |          +---- SourceResolver ---- QQ/local/QMC2 URI
          v
    PlaybackDriver                 backend-neutral interface
          |
          v
      hmp-player                   Rodio + CPAL + seekable stream cache
          |
          +---- PlaybackState watch
          +---- PlayerEvent broadcast

PlaybackState watch + command sender + capabilities watch
          |
          +---- Linux: hmp-mpris
          +---- Windows: hmp-smtc
```

### Stable playback protocol

`LoadRequest` and `PlayerEvent` move into `hmp-core::player`. The daemon and tests use
only these domain protocol types. The concrete `hmp-player` crate consumes and emits
them but does not own them.

`PlaybackDriver` remains the only engine/audio seam. `RodioDriver` wraps
`hmp_player::PlayerCore`, while fake drivers continue to test engine policy without an
audio device or network.

### Audio backend

`hmp-player` owns a single Rodio output device and player. The async control loop:

- parses `file://` with `url::Url::to_file_path`, opens the file, and constructs a
  seekable Rodio decoder;
- opens `http://` and `https://` through `stream-download` backed by a temporary file,
  providing the blocking `Read + Seek` interface required by the decoder while the
  download continues in the background;
- publishes `Loading` before preparation and `Playing` only after a source has been
  decoded and appended;
- polls Rodio position and empty state at a bounded interval to publish position and a
  generation-tagged end event;
- maps seek failures and load/decode failures to a generation-tagged `PlayerEvent::Error`;
- keeps loop and shuffle values as state only; queue semantics remain in the engine.

Tests use Rodio's in-memory mixer and a generated WAV fixture, so no physical audio
device is required.

### Windows SMTC

`hmp-smtc` is a platform adapter. Its public constructor consumes the same three
interfaces already used by MPRIS:

```rust
pub fn start(
    command_tx: tokio::sync::mpsc::UnboundedSender<hmp_core::PlayerCommand>,
    state_rx: tokio::sync::watch::Receiver<hmp_core::PlaybackState>,
    capabilities_rx: tokio::sync::watch::Receiver<hmp_core::PlaybackCapabilities>,
) -> Result<SmtcService, SmtcError>;
```

On Windows, a dedicated MTA thread owns a WinRT `MediaPlayer` solely as the lifetime
owner of its `SystemMediaTransportControls`; Rodio remains the only audio engine. The
automatic command manager is disabled. SMTC callbacks send `PlayerCommand` values and
never mutate playback state directly. Tokio forwarding tasks send state/capability
snapshots to the owner thread, avoiding cross-apartment ownership of WinRT objects.

State projection rules:

- `Playing` -> `MediaPlaybackStatus::Playing`
- `Paused` -> `MediaPlaybackStatus::Paused`
- all other statuses -> `MediaPlaybackStatus::Stopped`
- title, artist, album, and cover come from `PlaybackState.current`
- timeline uses zero start/min, current position, and known duration as max/end
- play/pause/stop are enabled when a track exists; next/previous come from engine
  capabilities; seek is enabled only when `can_seek` and duration are known
- repeat and shuffle changes are converted to `SetLoopMode` and `SetShuffle`

Dropping the service aborts forwarding tasks, disables SMTC, unregisters handlers, and
joins the owner thread.

### Platform composition

The daemon owns a `PlatformMediaService` guard:

- Linux starts MPRIS when the `mpris` feature is enabled.
- Windows starts SMTC by default through a target-specific dependency.
- Other targets use a no-op guard.

Protocol startup failures are logged and do not prevent audio or IPC startup. The
daemon is still usable without a desktop session.

## Configuration and packaging

The legacy backend-specific `audio.sink`, `--sink`, and setup script are removed. ReplayGain
remains under `[audio]`. Windows packages need only the Rust/Tauri artifacts; no media
SDK runtime or plugin tree is installed separately.

## Error handling

- Unsupported URI schemes fail as `HmpError::Playback` without changing the applied
  generation.
- File, HTTP, and decoder errors publish `PlaybackStatus::Error` and a tagged event.
- Audio-device initialization failure is returned from `PlayerCore::new` and daemon
  startup remains explicitly failed rather than silently running without sound.
- SMTC initialization failure is non-fatal and logged because it is an integration,
  not the state owner.

## Verification

Automated gates:

```powershell
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo check --manifest-path apps/hmp-tauri/src-tauri/Cargo.toml --all-targets
pnpm --dir apps/hmp-tauri test -- --run
pnpm --dir apps/hmp-tauri build
git grep -n -i -E "legacy multimedia package names" -- ':!docs/superpowers/specs/2026-08-24-windows-media-backend-design.md'
```

The final search must return no active code, dependency, script, README, or usage-guide
references. Historical superseded design records may be rewritten to generic "audio
backend" language so repository-wide guidance cannot reintroduce the removed runtime.
