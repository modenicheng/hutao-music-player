# GPUI desktop prototype performance report

Date: 2026-08-24

Branch: `feat/gpui-desktop-prototype`

Build: Cargo `release`, rustc 1.97.1

## Result

The prototype validates the architecture and visual direction, but it does not
yet demonstrate a performance advantage over the existing Slint desktop.

- GPUI's stable idle working set was 36.5 MiB higher than Slint in this run.
- Static-page CPU was effectively tied and unexpectedly high for both apps.
- The animated AlbumGlow view added 41.6 MiB working set and substantial GPU
  activity compared with the GPUI static page.
- GPUI produced the smaller executable, by 1.3 MiB.
- The GPUI window, static shell, real search results, and Now Playing surface
  were exercised successfully on Windows.

This is a prototype result, not a recommendation to remove Slint. A replacement
decision should wait for CPU profiling, an authenticated playback run, and a
native Linux Wayland measurement.

## Test system

| Item | Value |
| --- | --- |
| OS | Windows 11 Pro 10.0.26200 |
| CPU | Intel Core Ultra 9 285H, 16 logical processors |
| Memory | 31.4 GiB |
| Discrete GPU | NVIDIA GeForce RTX 5060 Laptop GPU, driver 32.0.15.9144 |
| Integrated GPU | Intel Arc 140T GPU, driver 32.0.101.8132 |
| Window size | 1280 × 800 |

## Method

Both applications were built together with:

```powershell
cargo build --release -p hmp-desktop -p hmp-desktop-gpui
```

[`scripts/benchmark-desktop.ps1`](../scripts/benchmark-desktop.ps1) then:

1. starts exactly one executable and records its PID;
2. polls for a main window for at most 15 seconds;
3. allows a bounded preparation and warm-up period;
4. samples process CPU time, working/private memory, and Windows per-process
   `GPU Engine(*)\Utilization Percentage` counters;
5. requests a normal window close and terminates only the recorded PID if the
   process does not close within three seconds.

The GPU value is the mean of the per-sample sum of counters whose instance name
contains the measured PID. It is useful for relative comparison on this machine,
but it is not the same as whole-device GPU utilization. CPU "one core" can
exceed 100%; "system" divides that result by the 16 logical processors.

Window-ready time was observed more than once after the release build. OS file
cache could not be flushed, so the range below is a cold-ish launch range rather
than a laboratory cold-boot measurement.

## Windows measurements

| Scenario | Window ready | Working set | Private | CPU, one core | CPU, system | GPU engine | Binary |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Slint static idle | 0.82–1.12 s | 115.5 MiB | 79.6 MiB | 129.04% | 8.065% | 0.00% | 30.5 MiB |
| GPUI static idle | 1.49–1.87 s | 152.0 MiB | 104.9 MiB | 126.39% | 7.899% | 0.00% | 29.2 MiB |
| GPUI Now Playing / AlbumGlow | included above | 193.6 MiB | 146.7 MiB | 140.98% | 8.811% | 36.23% | 29.2 MiB |

Stable idle rows used a 20-second preparation period, a three-second warm-up,
and five GPU samples. Now Playing used a 40-second preparation period, was
opened through the actual player-bar cover control, then used a three-second
warm-up and five samples. The fallback cover was used; no fake playback state
or alternate audio engine was injected.

### Interpretation

- GPUI static idle consumed 31.6% more working-set memory than Slint in this
  sample (`152.0 / 115.5 - 1`).
- GPUI static idle used 2.65 percentage points less of one CPU core, which is too
  small to treat as a meaningful win in a single run.
- Both static views consumed roughly 1.3 logical CPU cores. That is not an
  acceptable idle target. Ordinary GPUI pages do not call
  `request_animation_frame`, so a profiler should split GUI renderer cost from
  the shared `AppCore`/audio/system-integration threads before optimization.
- AlbumGlow added 41.6 MiB working set, 14.59 percentage points of one CPU core,
  and 36.23% aggregate per-process GPU-engine activity over the GPUI static
  sample. The effect is visually successful but should gain an animation
  quality/power policy before production use.

## Playback measurement

The live GPUI search path returned real QQ Music results through the existing
`AppCore`, confirming that the UI did not call the provider API directly. A
deterministic playing state was not confirmed inside the bounded benchmark
window, so playback RSS/CPU/GPU values are intentionally left unreported. No
mock track, alternate player, or direct media URL was introduced merely to
produce a number.

Before a replacement decision, repeat the script with a known playable account
and record both frontends while playing the same track, at the same position and
window size.

## Linux and macOS status

The installed `x86_64-unknown-linux-gnu` Rust target allowed a cross-check to
start, but the Windows host lacks `x86_64-linux-gnu-gcc`. The build stopped in
`ring`'s C build script before compiling the HMP GPUI crate. Consequently,
Wayland/X11 launch and Linux performance numbers were not collected here.

The GPUI manifest enables both `wayland` and `x11`, and the frontend keeps Linux
font configuration behind `cfg(target_os = "linux")`. A Linux runner still needs
to execute:

```bash
cargo check -p hmp-desktop-gpui
cargo run --release -p hmp-desktop-gpui
```

under both Wayland and X11, then run an equivalent native sampler. The macOS
target is not installed on this host; transparent titlebar and traffic-light
handling remain isolated behind macOS `cfg` blocks, but a native macOS compile
is still required.

## Recommendation

Keep the GPUI prototype alongside Slint. It has proven that HMP can reuse its
existing core, playback state, commands, search, queue, lyrics, Windows native
window behavior, and advanced GPU effects without adding a second player.
However, it has not yet met the low-idle-resource part of the experiment.

The next evidence-gathering pass should:

1. profile the shared high idle CPU with Windows Performance Recorder or a Rust
   profiler;
2. add an AlbumGlow quality/power toggle or frame-rate cap and remeasure;
3. capture authenticated playback for both frontends;
4. run the same release scenarios natively on Linux Wayland and X11;
5. repeat each scenario enough times to report median and variability.
