# Knave portal validation, 2026-10-01

Matching sibling branches: `feat/knave-portals` in Knave, Villain, Knave Shell and
this backend. User-local release binaries and activation metadata were installed;
the prior binaries/metadata were saved under
`~/.local/state/knave/portal-backups/20261001T031527Z/restore.py`.
The canonical config and user routing overrides were preserved.

## Checked behavior

- Workspace tests, strict Clippy, release builds and diff checks in all four owners;
  backend all-feature tests and rustdoc with warnings denied.
- Real nested Villain ext SHM capture at 1904x982; captured native dialog inspected
  for orientation, monitor labels, disabled confirmation and cancellation controls.
- Real `/usr/lib/xdg-desktop-portal` frontend routing, interactive screenshot URI,
  monitor selection, ScreenCast start, ten nonempty video buffers from the frontend's
  restricted PipeWire fd, and session close. Three repeated sessions plus control
  failure were exercised per run.
- Live Settings.Read updates and SettingChanged; malformed config retains the
  previous valid appearance. Directory writes and deletes were exercised.
- Request.Close cancels a blocking picker and reaps it. Killing the native sharing
  control delivers frontend Session.Closed and destroys capture/PipeWire resources.
- Installed executable activation via its generated D-Bus service on a private bus;
  generated systemd user unit verification. The unit is static, not enabled.
- Atomic install, custom prefixes containing spaces and percent signs, and uninstall.
- Native pointer selection, deselection, confirmation and Stop sharing tests include
  a redraw between pointer press/release, preserving toolkit pointer capture.

The harness creates private runtime/config/data/cache directories, its own D-Bus,
PipeWire and policy-only WirePlumber services, and a nested Wayland window. Its
automatic source fixture is confined to that private bus. It neither imports nor
restarts host activation services. Use `scripts/smoke.py --release --installed`
for installed activation, `--release` for cancellation and resource checks,
`--render` for a native render capture, and `--native` for manual selection.

## Resource sample

Release sample: one 1904x982 output, three sequential monitor sessions, three seconds
of capture before a ten-frame consumer, and a final sharing-control failure. CPU is
percentage of one core over each complete screenshot/sharing workload, rather than
an isolated renderer microbenchmark. Idle samples lasted three seconds; zero means
below that sampling resolution.

| Process | Idle RSS / CPU | Active peak RSS / CPU | Threads / fds |
|---------|----------------|-----------------------|---------------|
| Backend | 11.4 MiB / 0% sampled | 76.8 MiB / 7.05-7.29% | 8 / 39 peak, 28 after close |
| Nested Villain | 94.9 MiB / 0% sampled | 110.0 MiB / 8.69-9.17% | 9 / 40 peak |
| Native picker, separate settled sample | 193.4 MiB / 0% sampled | Startup not profiled | 36 / 51 |

Backend voluntary thread switches were approximately 109-124/s during the active
workload. Villain peak RSS and fd counts remained identical across the three
sessions after fixing SHM pool destruction. Backend post-close RSS stayed around
26-34 MiB, with stable threads/fds; the final failure path briefly retained about
33.4 MiB. These are short local samples, not a sustained maximum-concurrency or 4K
benchmark. GPU initialization has a substantial per-dialog cost with this driver.

## Verification limits

Manual native clicks in a desktop session, direct-TTY/login/reboot activation,
multiple physical outputs, transformed/scaled physical outputs, window contents
changing over time, OBS/browser and sandbox application workflows remain unverified.
Window sharing, RemoteDesktop/InputCapture, cursor metadata, DMA-BUF capture and
native color picking remain deferred. Resize requires a new screencast session;
live PipeWire format renegotiation is not implemented. The standard capture globals
use Villain's existing trusted Wayland client boundary; security-context filtering
is not introduced here.
