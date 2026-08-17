# Cross-platform Daemon and Tauri Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route Tauri, CLI, and tray playback control through one cross-platform daemon, with Unix sockets on Linux, named pipes on Windows, frontend-owned orphan cleanup, and correct close-to-tray/full-exit behavior.

**Architecture:** `hmp-core` remains transport-agnostic; a new `hmp-control` crate owns the wire protocol, client, framing, and platform endpoints. `hmpd` is the only playback-runtime process, while Tauri owns the desktop lifecycle and tray and talks to `hmpd` exactly like CLI or a future Slint frontend.

**Tech Stack:** Rust 2024, Tokio, Tauri 2, Windows named pipes, Unix domain sockets, GStreamer, Vue 3, TypeScript, Vitest.

## Global Constraints

- Preserve the user's existing uncommitted changes in `apps/hmp-tauri/src/lib/player.ts` and `apps/hmp-tauri/src/lib/lrcParser.ts`.
- Exactly one daemon may run per login session, and exactly one active desktop frontend instance may create a tray.
- Closing a tray-capable window hides it; complete exit remains available in tray and GUI and converges on `Request::Quit`.
- Frontend-owned daemon leases use a 30-second orphan grace period; autonomous CLI daemons do not depend on a GUI lease.
- Linux uses owner-only Unix sockets; Windows uses session-scoped named pipes and rejects remote clients.
- Tauri and Vue never create an `HTMLAudioElement`; Rust/GStreamer is the only audio engine.
- New behavior follows red-green-refactor. Configuration-only changes are verified by the nearest build or integration test.

---

## File Structure

- `crates/hmp-control/src/protocol.rs`: requests, responses, events, version handshake, frame codec.
- `crates/hmp-control/src/transport.rs`: platform-neutral boxed async stream and endpoint helpers.
- `crates/hmp-control/src/transport/unix.rs`: Unix socket connector/listener and owner-only endpoint.
- `crates/hmp-control/src/transport/windows.rs`: named-pipe connector/listener, first-instance guard, and logon-session security.
- `crates/hmp-control/src/client.rs`: request client, subscription stream, connect retry.
- `crates/hmp-daemon/src/server.rs`: transport-neutral request server and frontend lease notification.
- `crates/hmp-daemon/src/lifecycle.rs`: daemon ownership mode and orphan timer state machine.
- `crates/hmp-daemon/src/serve.rs`: cross-platform daemon orchestration.
- `crates/hmp-daemon/src/main.rs`: `hmpd` executable entrypoint.
- `apps/hmp-tauri/src-tauri/src/control.rs`: Tauri-to-daemon commands, state DTO, subscription forwarding.
- `apps/hmp-tauri/src-tauri/src/lifecycle.rs`: testable desktop lifecycle reducer and complete-exit coordinator.
- `apps/hmp-tauri/src-tauri/src/tray.rs`: native tray construction and command routing.
- `apps/hmp-tauri/src/lib/player.ts`: daemon-backed Vue player controller.

---

### Task 1: Extract the versioned control protocol from `hmp-core`

**Files:**
- Create: `crates/hmp-control/Cargo.toml`
- Create: `crates/hmp-control/src/lib.rs`
- Create: `crates/hmp-control/src/protocol.rs`
- Modify: `Cargo.toml`
- Modify: `crates/hmp-core/src/lib.rs`
- Modify: `crates/hmp-core/src/ipc.rs`
- Modify: `crates/hmp-daemon/Cargo.toml`
- Modify: `crates/hmp-cli/Cargo.toml`
- Modify: Rust imports that currently use `hmp_core::{Request, Response, Event}` or `hmp_core::ipc` framing helpers

**Interfaces:**
- Consumes: domain types from `hmp-core`.
- Produces: `hmp_control::{Request, Response, Event, HostMode, PROTOCOL_VERSION, encode_frame, decode_frame}`.

- [ ] **Step 1: Write failing protocol tests**

Add tests that require a version handshake and a lease-bearing subscription:

```rust
#[test]
fn host_messages_roundtrip() {
    let messages = [
        Request::Hello { protocol: PROTOCOL_VERSION },
        Request::Subscribe { frontend_lease: true },
        Request::Subscribe { frontend_lease: false },
    ];
    for message in messages {
        let frame = encode_frame(&message).unwrap();
        assert_eq!(decode_frame::<Request>(&frame).unwrap(), message);
    }
    assert_eq!(
        decode_frame::<Response>(&encode_frame(&Response::Hello {
            protocol: PROTOCOL_VERSION,
        }).unwrap()).unwrap(),
        Response::Hello { protocol: PROTOCOL_VERSION },
    );
}
```

- [ ] **Step 2: Run the test and observe the missing crate/API failure**

Run: `cargo test -p hmp-control protocol::tests::host_messages_roundtrip`

Expected: FAIL because package `hmp-control` does not exist.

- [ ] **Step 3: Create `hmp-control` and move wire-only types**

Keep `PlayRequest`, `TrackProvider`, `TrackRef`, `DaemonState`, `EnginePhase`, `ErrorInfo`, `PlaylistWriteOp`, `CommentPage`, and `QueuePage` in `hmp-core`. Move `Request`, `Response`, `Event`, `FrameError`, `MAX_FRAME`, `encode_frame`, and `decode_frame` into `hmp-control`, adding:

```rust
pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    Hello { protocol: u16 },
    Subscribe { frontend_lease: bool },
    // Existing engine and query variants retain their serialized representation.
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Response {
    Hello { protocol: u16 },
    // Existing response variants.
}
```

Update all workspace imports to take wire types from `hmp-control` and domain types from `hmp-core`. Remove `pub mod ipc` and wire re-exports from `hmp-core` after all consumers compile.

- [ ] **Step 4: Run focused and workspace protocol tests**

Run: `cargo test -p hmp-control && cargo test -p hmp-core`

Expected: PASS with all frame round-trips and domain tests green.

- [ ] **Step 5: Commit**

```powershell
git add Cargo.toml Cargo.lock crates/hmp-control crates/hmp-core crates/hmp-daemon crates/hmp-cli
git commit -m "refactor(ipc): extract shared control protocol"
```

### Task 2: Add platform-neutral clients and Windows named-pipe transport

**Files:**
- Create: `crates/hmp-control/src/client.rs`
- Create: `crates/hmp-control/src/transport.rs`
- Create: `crates/hmp-control/src/transport/unix.rs`
- Create: `crates/hmp-control/src/transport/windows.rs`
- Modify: `crates/hmp-control/src/lib.rs`
- Modify: `crates/hmp-control/Cargo.toml`
- Modify: `crates/hmp-cli/src/client.rs`

**Interfaces:**
- Consumes: versioned `Request`/`Response`/`Event` frames.
- Produces: `ControlClient::connect`, `ControlClient::request`, `Subscription::next`, `Listener::bind`, and `Listener::accept`.

- [ ] **Step 1: Write failing in-memory client tests**

Use `tokio::io::duplex` to prove handshake and response correlation without an OS transport:

```rust
#[tokio::test]
async fn client_handshakes_before_first_request() {
    let (client_io, mut server_io) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        assert_eq!(read_frame::<Request, _>(&mut server_io).await.unwrap(),
                   Request::Hello { protocol: PROTOCOL_VERSION });
        write_frame(&mut server_io, &Response::Hello { protocol: PROTOCOL_VERSION }).await.unwrap();
        assert_eq!(read_frame::<Request, _>(&mut server_io).await.unwrap(), Request::Status);
        write_frame(&mut server_io, &Response::Status(Default::default())).await.unwrap();
    });
    let mut client = ControlClient::from_stream(Box::new(client_io)).await.unwrap();
    assert!(matches!(client.request(Request::Status).await.unwrap(), Response::Status(_)));
    server.await.unwrap();
}
```

- [ ] **Step 2: Run and observe the missing client failure**

Run: `cargo test -p hmp-control client::tests::client_handshakes_before_first_request`

Expected: FAIL because `ControlClient` and framed async I/O helpers are absent.

- [ ] **Step 3: Implement boxed async streams and platform transports**

Expose the shared boundary:

```rust
pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T> AsyncStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}
pub type BoxStream = Box<dyn AsyncStream>;

pub struct Listener { /* cfg-specific inner listener */ }
impl Listener {
    pub async fn bind() -> io::Result<Self>;
    pub async fn accept(&mut self) -> io::Result<BoxStream>;
}
pub async fn connect() -> io::Result<BoxStream>;
```

Unix uses `tokio::net::{UnixListener, UnixStream}` and the existing XDG path/permission rules. Windows uses `tokio::net::windows::named_pipe`; the first instance sets `first_pipe_instance(true)` and `reject_remote_clients(true)`, constructs the next instance before returning the connected one, and passes a logon-SID-only security descriptor through `create_with_security_attributes_raw`.

- [ ] **Step 4: Add platform integration tests**

On Windows, bind one listener, connect two clients sequentially, exchange frames, and assert a second first-instance listener fails. On Unix, assert the socket mode is `0600`. Run:

`cargo test -p hmp-control --all-targets`

Expected: PASS on the host platform.

- [ ] **Step 5: Replace the CLI-local Unix client**

Make `crates/hmp-cli/src/client.rs` a thin wrapper/type alias around `hmp_control::ControlClient`; preserve CLI error formatting and `connect_or_spawn` behavior while removing direct `UnixStream` imports.

- [ ] **Step 6: Commit**

```powershell
git add Cargo.lock crates/hmp-control crates/hmp-cli
git commit -m "feat(ipc): support unix sockets and windows named pipes"
```

### Task 3: Make the daemon server transport-neutral and add ownership lifecycle

**Files:**
- Create: `crates/hmp-daemon/src/lifecycle.rs`
- Modify: `crates/hmp-daemon/src/lib.rs`
- Modify: `crates/hmp-daemon/src/server.rs`
- Modify: `crates/hmp-daemon/src/serve.rs`
- Modify: `crates/hmp-daemon/Cargo.toml`

**Interfaces:**
- Consumes: `hmp_control::Listener`, `BoxStream`, and subscription lease flags.
- Produces: `DaemonLifecycle`, `LeaseGuard`, and a server that compiles on Unix and Windows.

- [ ] **Step 1: Write failing paused-time lifecycle tests**

```rust
#[tokio::test(start_paused = true)]
async fn frontend_owned_daemon_quits_after_orphan_grace() {
    let (quit_tx, mut quit_rx) = mpsc::unbounded_channel();
    let lifecycle = DaemonLifecycle::frontend_owned(Duration::from_secs(30), quit_tx);
    let lease = lifecycle.acquire_frontend();
    drop(lease);
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(quit_rx.try_recv().is_err());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(quit_rx.recv().await, Some(Request::Quit));
}

#[tokio::test(start_paused = true)]
async fn reconnect_cancels_orphan_shutdown() {
    let (quit_tx, mut quit_rx) = mpsc::unbounded_channel();
    let lifecycle = DaemonLifecycle::frontend_owned(Duration::from_secs(30), quit_tx);
    let first = lifecycle.acquire_frontend();
    drop(first);
    tokio::time::advance(Duration::from_secs(20)).await;
    let _replacement = lifecycle.acquire_frontend();
    tokio::time::advance(Duration::from_secs(20)).await;
    assert!(quit_rx.try_recv().is_err());
}
```

- [ ] **Step 2: Run and observe the missing lifecycle failure**

Run: `cargo test -p hmp-daemon lifecycle::tests --no-default-features`

Expected: FAIL because `DaemonLifecycle` is absent.

- [ ] **Step 3: Implement lease generation cancellation**

`DaemonLifecycle` stores mode, active frontend count, and a monotonically increasing generation. Dropping the last `LeaseGuard` increments generation and spawns the grace timer; the timer sends `Request::Quit` only if the generation and zero-count condition still match. Autonomous mode ignores frontend loss.

- [ ] **Step 4: Refactor server I/O over `BoxStream`**

Change connection handling to split any boxed async stream rather than naming `UnixStream`. `Request::Hello` and `Request::Subscribe { frontend_lease }` are handled by the server. A lease guard lives for exactly the subscription connection lifetime. Engine/query handling retains its current behavior and response mapping.

- [ ] **Step 5: Run daemon unit tests**

Run: `cargo test -p hmp-daemon --lib --no-default-features`

Expected: PASS when the platform GStreamer development libraries are available.

- [ ] **Step 6: Commit**

```powershell
git add Cargo.lock crates/hmp-daemon
git commit -m "feat(daemon): add cross-platform host lifecycle"
```

### Task 4: Add `hmpd` and cross-platform daemon startup

**Files:**
- Create: `crates/hmp-daemon/src/main.rs`
- Modify: `crates/hmp-daemon/src/serve.rs`
- Modify: `crates/hmp-daemon/src/daemon.rs`
- Modify: `crates/hmp-cli/src/client.rs`
- Modify: `crates/hmp-cli/src/main.rs`
- Modify: daemon and CLI integration tests with platform cfg modules

**Interfaces:**
- Consumes: `DaemonLifecycleMode::{FrontendOwned, Autonomous}` and platform `Listener`.
- Produces: `hmpd --frontend-owned`, `hmpd --autonomous`, and platform-specific detached spawning.

- [ ] **Step 1: Write failing argument and shutdown tests**

```rust
#[test]
fn lifecycle_mode_cli_is_explicit() {
    assert_eq!(Args::try_parse_from(["hmpd", "--frontend-owned"]).unwrap().mode(),
               LifecycleMode::FrontendOwned);
    assert_eq!(Args::try_parse_from(["hmpd", "--autonomous"]).unwrap().mode(),
               LifecycleMode::Autonomous);
}
```

Add an integration test that starts the test server, sends `Quit`, waits for the terminated watch channel, and verifies that accepting new connections stops.

- [ ] **Step 2: Run and observe the missing binary failure**

Run: `cargo test -p hmp-daemon --bin hmpd --no-default-features`

Expected: FAIL because the `hmpd` target and arguments do not exist.

- [ ] **Step 3: Implement `hmpd` orchestration**

Use `clap` with two mutually exclusive ownership flags. `serve::run` binds the platform listener before constructing the playback daemon, starts signal handling where available, waits on the engine terminated watch, aborts the accept loop, then drops endpoint and instance guards.

Unix detached startup keeps `setsid`. Windows detached startup uses `std::os::windows::process::CommandExt` with `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP`. CLI `connect_or_spawn` launches `hmpd --autonomous`; it no longer recursively launches `hmp serve --background`.

- [ ] **Step 4: Gate Linux-only integrations**

Make daemon default features platform-neutral. Compile `ksni`, MPRIS, Unix signals, `flock`, Unix socket tests, and permission APIs only under `cfg(unix)`. The daemon runtime and `hmpd` compile under Windows with `--no-default-features`.

- [ ] **Step 5: Run host checks**

Run: `cargo check -p hmp-daemon --no-default-features --all-targets` and `cargo check -p hmp-cli --all-targets`.

Expected: both checks exit 0 when GStreamer is installed.

- [ ] **Step 6: Commit**

```powershell
git add Cargo.lock crates/hmp-daemon crates/hmp-cli
git commit -m "feat(daemon): add portable hmpd process host"
```

### Task 5: Connect Tauri Rust to the daemon and build lifecycle/tray controllers

**Files:**
- Create: `apps/hmp-tauri/src-tauri/src/control.rs`
- Create: `apps/hmp-tauri/src-tauri/src/lifecycle.rs`
- Create: `apps/hmp-tauri/src-tauri/src/tray.rs`
- Modify: `apps/hmp-tauri/src-tauri/src/lib.rs`
- Modify: `apps/hmp-tauri/src-tauri/Cargo.toml`
- Modify: `apps/hmp-tauri/src-tauri/tauri.conf.json`
- Modify: `apps/hmp-tauri/src-tauri/capabilities/default.json`
- Modify: `apps/hmp-tauri/package.json`

**Interfaces:**
- Consumes: `hmp_control::ControlClient` and daemon state subscriptions.
- Produces: Tauri commands `get_player_state`, `toggle_play`, `seek`, `set_volume`, `previous`, `next`, `stop`, and `complete_exit`; event `hmp://player-state`.

- [ ] **Step 1: Write failing pure lifecycle tests**

```rust
#[test]
fn close_hides_only_when_tray_is_ready() {
    assert_eq!(Lifecycle::ready(true).on_close_requested(), CloseAction::Hide);
    assert_eq!(Lifecycle::ready(false).on_close_requested(), CloseAction::Quit);
}

#[test]
fn complete_exit_is_idempotent() {
    let lifecycle = Lifecycle::ready(true);
    assert!(lifecycle.begin_quit());
    assert!(!lifecycle.begin_quit());
}
```

- [ ] **Step 2: Run and observe the missing modules failure**

Run from `apps/hmp-tauri/src-tauri`: `cargo test --lib lifecycle::tests`

Expected: FAIL because the lifecycle module is absent.

- [ ] **Step 3: Implement the Tauri control bridge**

Create serializable millisecond-based DTOs instead of exposing Rust `Duration` directly:

```rust
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStateDto {
    pub status: String,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub volume: f64,
    pub can_seek: bool,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub error: Option<String>,
}
```

Tauri commands acquire a request-client mutex only for frame request/response. A separate subscription connection emits `hmp://player-state`. If connection fails, use the bundled `hmpd` sidecar with `--frontend-owned`, wait for the endpoint, and subscribe with `frontend_lease: true`.

- [ ] **Step 4: Implement tray and window lifecycle**

Register the single-instance plugin first. Build menu IDs `toggle-window`, `toggle-play`, `previous`, `next`, `stop`, and `quit`. Menu callbacks spawn async commands; left-click shows/unminimizes/focuses `main`. `CloseRequested` prevents close and hides only when `CloseAction::Hide` is returned.

`complete_exit` atomically begins quitting, sends `Request::Quit`, waits at most three seconds for daemon termination/EOF, removes the tray, and calls `AppHandle::exit(0)`.

- [ ] **Step 5: Configure sidecar and permissions**

Add `hmpd` to `bundle.externalBin`, enable Tauri's `tray-icon` feature, add the shell and single-instance plugins, and grant only the sidecar spawn permission needed for `hmpd`.

- [ ] **Step 6: Run Rust Tauri checks**

Run from `apps/hmp-tauri/src-tauri`: `cargo test --lib && cargo check --all-targets`.

Expected: PASS with GStreamer development libraries installed and the sidecar placeholder/build artifact present.

- [ ] **Step 7: Commit**

```powershell
git add apps/hmp-tauri/src-tauri apps/hmp-tauri/package.json apps/hmp-tauri/pnpm-lock.yaml
git commit -m "feat(tauri): connect daemon lifecycle and tray"
```

### Task 6: Replace browser audio with the Rust player bridge

**Files:**
- Modify: `apps/hmp-tauri/src/lib/player.test.ts`
- Modify: `apps/hmp-tauri/src/lib/player.ts`
- Modify: `apps/hmp-tauri/src/App.vue`
- Modify: `apps/hmp-tauri/src/layouts/PlayerBar.vue` only if binding names change
- Modify: `apps/hmp-tauri/src/layouts/PlayerOverlay.vue` only if binding names change

**Interfaces:**
- Consumes: Tauri commands and `hmp://player-state` events.
- Produces: the existing `PlayerController` UI-facing state plus backend-driven duration, progress, and command methods.

- [ ] **Step 1: Replace audio mocks with a fake bridge in tests**

```typescript
interface PlayerBridge {
  getState(): Promise<PlayerSnapshot>;
  subscribe(listener: (state: PlayerSnapshot) => void): Promise<() => void>;
  command(command: PlayerCommand): Promise<void>;
}

it("routes play and seek through the Rust bridge", async () => {
  const bridge = createBridge();
  const player = new PlayerController({ bridge, storage: createStorage() });
  await player.mount();
  await player.togglePlay();
  player.startDragging();
  player.setDragPercent(0.5);
  await player.setProgress();
  expect(bridge.command).toHaveBeenNthCalledWith(1, { type: "togglePlay" });
  expect(bridge.command).toHaveBeenNthCalledWith(2, { type: "seek", positionMs: 50_000 });
});
```

- [ ] **Step 2: Run and observe the old audio API failure**

Run from `apps/hmp-tauri`: `pnpm test -- src/lib/player.test.ts`

Expected: FAIL because `PlayerController` still expects a URI/`PlayerAudio`.

- [ ] **Step 3: Implement the daemon-backed controller**

Preserve the user's volume persistence and unmount cleanup changes. Remove `Audio`, `requestAnimationFrame`, and direct media mutation. `mount` loads one snapshot and subscribes; `unmount` removes DOM listeners and awaits/unregisters the state listener. Snapshot application updates local state without sending commands back. Seek and volume commands use clamped values.

- [ ] **Step 4: Wire the production Tauri bridge**

Use `invoke` from `@tauri-apps/api/core` and `listen` from `@tauri-apps/api/event`. `App.vue` constructs the controller with this bridge and no media URI.

- [ ] **Step 5: Run frontend verification**

Run from `apps/hmp-tauri`: `pnpm test && pnpm build`.

Expected: all Vitest tests pass and Vue/TypeScript production build exits 0.

- [ ] **Step 6: Commit without absorbing unrelated user work**

Review `git diff` first. Stage only intended hunks in the pre-existing modified files and retain unrelated `lrcParser.ts` work.

```powershell
git add apps/hmp-tauri/src/lib/player.ts apps/hmp-tauri/src/lib/player.test.ts apps/hmp-tauri/src/App.vue
git commit -m "feat(player): route webview controls through rust core"
```

### Task 7: Windows packaging, native verification, and documentation

**Files:**
- Create: `scripts/setup-gstreamer-windows.ps1`
- Create: `apps/hmp-tauri/scripts/stage-sidecar.ps1`
- Modify: `apps/hmp-tauri/package.json`
- Modify: `apps/hmp-tauri/README.md`
- Modify: `docs/USAGE.md`
- Modify: CI workflow files under `.github/workflows` that build desktop targets

**Interfaces:**
- Consumes: official MSVC x64 GStreamer Runtime/Development installation and built `hmpd`.
- Produces: reproducible Windows developer setup, staged sidecar, Tauri build, and acceptance checklist.

- [ ] **Step 1: Add a failing staging preflight**

The staging script resolves the current Rust host triple, checks for `target/release/hmpd.exe`, copies it to `apps/hmp-tauri/src-tauri/binaries/hmpd-<target>.exe`, and exits non-zero with a concrete build command when absent. Run it before building `hmpd` and confirm the expected non-zero result.

- [ ] **Step 2: Add GStreamer discovery setup**

The setup script locates the official MSVC x64 installation, sets `GSTREAMER_1_0_ROOT_MSVC_X86_64`, prepends its `bin` directory to the current process `PATH`, and sets `PKG_CONFIG_PATH` to its pkg-config directory. It never downloads or installs software silently.

- [ ] **Step 3: Build and stage the daemon**

Run:

```powershell
./scripts/setup-gstreamer-windows.ps1
cargo build -p hmp-daemon --bin hmpd --release --no-default-features
./apps/hmp-tauri/scripts/stage-sidecar.ps1
```

Expected: all commands exit 0 and the target-suffixed sidecar exists.

- [ ] **Step 4: Run full automated verification**

Run:

```powershell
cargo fmt --all -- --check
cargo test -p hmp-control --all-targets
cargo test -p hmp-daemon --no-default-features --all-targets
cargo test -p hmp-cli --all-targets
Push-Location apps/hmp-tauri
pnpm test
pnpm build
pnpm tauri build
Pop-Location
```

Expected: every command exits 0 with zero test failures.

- [ ] **Step 5: Perform Windows lifecycle smoke test**

Launch the packaged application, play the bundled/local FLAC through GStreamer, close the window and confirm playback continues, restore by tray click, exercise tray playback commands, launch the CLI and confirm the same state/daemon PID, choose complete exit, then verify no `hmpd` process or named pipe remains. Kill a frontend-owned GUI once and verify `hmpd` exits after the 30-second grace.

- [ ] **Step 6: Document evidence and limitations**

Update usage docs with Windows prerequisites, endpoint selection, daemon ownership modes, tray behavior, complete-exit paths, and the clean-runtime packaging requirement. Record any manual step that could not be automated as an explicit remaining acceptance item rather than claiming success.

- [ ] **Step 7: Commit**

```powershell
git add scripts apps/hmp-tauri/scripts apps/hmp-tauri/README.md apps/hmp-tauri/package.json docs/USAGE.md .github/workflows
git commit -m "build(windows): package and verify daemon sidecar"
```

---

## Final Verification

- [ ] `git diff --check` reports no whitespace errors.
- [ ] `git status --short` contains only known user-owned changes or intentional task changes.
- [ ] `cargo test -p hmp-control --all-targets` passes on Windows.
- [ ] `cargo test -p hmp-daemon --no-default-features --all-targets` passes with GStreamer installed.
- [ ] `cargo test -p hmp-cli --all-targets` passes.
- [ ] `pnpm test` and `pnpm build` pass in `apps/hmp-tauri`.
- [ ] `pnpm tauri build` succeeds with the target-suffixed `hmpd` sidecar.
- [ ] Manual Windows lifecycle/tray/one-daemon smoke evidence is recorded.
