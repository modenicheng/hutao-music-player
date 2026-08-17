# Cross-platform Daemon, Tauri Lifecycle, and Tray Design

## Goal

Connect the Tauri Rust backend to the existing playback runtime while keeping the
playback kernel independent of every interactive frontend. The result must support
Tauri today, a native Slint frontend later, CLI-only/headless use, one playback
daemon per user session, and correct Windows and Linux lifecycle behavior.

## Architectural boundaries

- `hmp-core` contains playback, queue, media, and other domain types. It does not
  know about IPC transports, windows, trays, Tauri, Slint, or controller leases.
- `hmp-player-gst` implements the audio driver used by the playback runtime.
- `hmp-daemon` owns the single playback runtime, persistence, cross-platform IPC
  server, and process lifecycle. It does not create a window or tray.
- A shared `hmp-control` crate owns the versioned control protocol, client,
  subscriptions, connection/reconnection behavior, and platform transports.
- CLI, Tauri, tray, and future Slint code are controllers. Every controller sends
  the same commands and consumes the same state snapshots.
- The tray is logically independent from the GUI implementation but is hosted by
  the active desktop frontend process. It never directly owns the player.

The dependency direction is:

```text
GUI / CLI / Tray -> ControlClient -> IPC -> Daemon runtime -> Playback kernel
```

## Processes and distribution

The daemon is exposed as an `hmpd` executable. Tauri bundles it as a sidecar, so a
GUI user is never required to install the CLI. CLI and future desktop frontends use
the same executable and protocol.

On startup a desktop frontend first connects to an existing daemon. It starts its
bundled `hmpd --frontend-owned` only when no daemon is reachable. CLI automatic
startup uses `hmpd --autonomous` so headless playback may continue after the CLI
command exits. A GUI that finds a CLI-owned daemon attaches without replacing or
restarting it.

Only one daemon may run in a user login session. Each desktop frontend is also
single-instance, so the active frontend creates exactly one tray.

## Protocol and transport

`hmp-control` separates engine requests from host requests:

- Engine requests cover playback, queue, library, query, and exit intents.
- Host requests cover protocol negotiation, state subscription, and a frontend
  lifecycle lease.

Frontend lease messages are handled by the daemon host and never enter the
playback engine.

Linux retains a Unix domain socket. Its directory is owner-only, the socket is
mode `0600`, and a lock file protects the single daemon instance.

Windows uses Tokio named pipes. The name contains the current logon-session SID,
the pipe rejects remote clients, and its DACL grants access only to that logon
session. The first server instance uses `FILE_FLAG_FIRST_PIPE_INSTANCE`. The
accept loop creates the next pipe instance before handing off a connection so
there is no interval in which clients observe a missing endpoint.

The framing and command semantics are shared above the transport layer. CLI and
desktop code do not branch on Unix sockets versus named pipes.

## Daemon lifecycle

The daemon state machine is:

```text
Starting -> Running -> Draining -> Exited
```

- `Starting` acquires the instance guard, binds the endpoint, restores the saved
  session, and initializes the player.
- `Running` accepts controller connections and serializes commands through the
  existing engine command channel.
- `Draining` stops accepting new commands, closes the current play session,
  persists queue/position/volume, shuts down GStreamer, closes IPC, and releases
  the instance guard.
- `Exited` terminates the process.

Every explicit exit converges on the existing `Quit` engine request. GUI tray,
GUI application menu, `Ctrl+Q`, CLI `hmp quit`, and operating-system exit handling
all use the same idempotent shutdown path.

### Frontend-owned orphan protection

When a GUI starts a frontend-owned daemon, its long-lived subscription connection
also holds a frontend lease. Closing the window to the tray keeps the GUI process
and lease alive. If the GUI crashes, the lease is disconnected and the daemon
allows 30 seconds for a restarted GUI to take over. If no frontend reconnects,
the daemon enters `Draining` and exits cleanly.

An autonomous daemon has no frontend-owner lease and is not stopped when a GUI
disconnects. This mode is only created by CLI/headless operation, where the CLI
provides the explicit `quit` path.

## Desktop frontend lifecycle

The desktop host state machine is:

```text
Booting -> Ready(Visible | Hidden) -> Quitting -> Exited
```

The single-instance handler runs before normal setup. A second launch restores,
unminimizes, and focuses the first window.

During setup the frontend connects or starts `hmpd`, establishes the subscription
and lease when applicable, and creates the tray. Playback controls remain disabled
until the daemon is ready.

When the main window receives a close request:

- If the tray is available and the app is not quitting, prevent destruction and
  hide the window while playback continues.
- If the tray is unavailable, never hide the only recovery surface; close through
  the complete shutdown path instead.

A tray click or second application launch shows, unminimizes, and focuses the main
window.

The tray menu contains:

- Show/hide main window
- Play/pause
- Previous
- Next
- Stop
- Exit

Tray actions only send control commands. State subscriptions update labels and
enabled state.

## Complete GUI exit

Complete exit is an idempotent operation:

1. Atomically transition to `Quitting`.
2. Disable tray playback controls and reject new GUI commands.
3. Send `Quit` to the daemon.
4. Wait for daemon termination notification or IPC EOF.
5. Stop waiting after three seconds. Do not force-kill an autonomous daemon that
   this GUI did not create.
6. Remove the tray and exit Tauri.

The operation is available from the tray, an in-window application/settings menu,
and `Ctrl+Q`, so tray health is never the only way to exit.

## Tauri bridge and frontend player state

Tauri manages one `ControlClient` and exposes asynchronous commands to Vue. A
background subscription forwards daemon snapshots as typed Tauri events. Neither
Tauri commands nor tray callbacks perform blocking playback work on the main event
loop.

The Vue `PlayerController` no longer creates or controls an `HTMLAudioElement`.
Playback status, position, duration, volume, queue capabilities, and errors come
from daemon snapshots. Seek, volume, play/pause, previous, next, and stop are sent
through the Rust control bridge. Backend snapshots update local UI state without
echoing commands back to the daemon.

## Failure handling

- A daemon startup error remains visible in the GUI and prevents entry into a
  hidden-to-tray state.
- Temporary IPC loss triggers bounded reconnect and resubscription. Non-idempotent
  commands are never automatically replayed.
- An unexpected daemon exit disables tray playback actions and displays a retryable
  error while keeping the window accessible.
- Tray creation failure makes window close perform complete exit.
- A tray failure after startup restores the main window. If registration can be
  retried after the desktop shell recovers, the frontend recreates the icon.
- Session writes remain atomic and throttled. A later daemon startup closes stale
  playback records after abnormal process termination.
- Operating-system shutdown gets a bounded graceful attempt; correctness does not
  depend on receiving unlimited shutdown time.

## Windows GStreamer requirements

Development and verification require the official MSVC x64 GStreamer Runtime and
Development components. The shipped Windows application must include the runtime
DLLs and required plugins, or install them as an application prerequisite. A clean
runtime verification must not depend on a developer-only `PATH` configuration.

## Testing and acceptance

Automated tests cover:

- Protocol negotiation, framing, subscriptions, disconnects, and bounded reconnect.
- Unix socket permissions, single-instance behavior, and multiple clients.
- Windows named-pipe multi-client operation, first-instance exclusion, subscription
  delivery, disconnect handling, and session-scoped access.
- Frontend-owned lease timeout, takeover during the grace period, and autonomous
  daemon independence.
- Desktop close-to-tray behavior, tray-unavailable fallback, idempotent exit, and
  daemon-shutdown timeout.
- Tray menu command mapping and play/pause label synchronization.
- Vue player state synchronization and command routing with a fake bridge.

Windows acceptance requires a native Tauri build and manual smoke test proving:

1. Audio is produced by the Rust/GStreamer kernel rather than the WebView.
2. Closing the window hides it while playback continues.
3. Tray actions control the same daemon state seen by the GUI and CLI.
4. A GUI attaches to a daemon previously started by CLI without starting another.
5. Complete exit removes the tray, daemon process, and named pipe after persistence.
6. Killing a frontend-owned GUI leaves no orphan daemon after the 30-second grace.

## Out of scope

- Running the daemon as a Windows Service or Linux system service.
- Shipping two trays concurrently for multiple desktop frontends.
- Replacing GStreamer or changing the playback engine state machine.
- Implementing the future Slint frontend.
