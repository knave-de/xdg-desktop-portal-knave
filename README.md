# xdg-desktop-portal-knave

[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)

**Source:** [knave-de/xdg-desktop-portal-knave](https://github.com/knave-de/xdg-desktop-portal-knave)

`xdg-desktop-portal-knave` is Knave Desktop's portal backend, based on
[xdg-desktop-portal-generic](https://github.com/lamco-admin/xdg-desktop-portal-generic).
It provides the XDG Desktop Portal backend services needed by sandboxed and
other desktop applications in a Wayland session.

Knave integrates monitor screenshots and sharing with Villain's standard
`ext-image-copy-capture-v1` protocol and Knave Shell's own Rust/wgpu UI toolkit.
The backend pins Knave configuration and the private picker contract to a merged
Git revision. Install a matching Knave Shell binary for native consent. Consent starts with no selected source; previews,
explicit confirmation, cancellation, and a Stop sharing control belong to Shell.

The backend connects to the compositor as a standalone Wayland client and uses
standard Wayland protocols. This keeps portal implementation separate from
Villain, Knave's compositor, unless a documented protocol change requires
compositor support.

The root package starts the Knave release series at `0.0.1`. The separately
published `lamco-data-control` workspace crate retains its own identity and
version.

## Supported Portals

| Portal Interface | Version | Primary Protocol | Fallback |
|------------------|---------|------------------|----------|
| `ScreenCast` | v6 | ext-image-copy-capture-v1 (monitor only) | wlr-screencopy-v1 |
| `Settings` | v2 | Canonical Knave configuration | Environment / GTK_THEME |
| `Screenshot` | v2 | Interactive monitor capture to PNG | -- |

The shipped descriptor advertises these three interfaces. RemoteDesktop,
InputCapture, Clipboard, window sharing, cursor metadata and native color picking
are not part of the supported Knave integration. PickColor returns NotSupported.
Without capture protocols the backend serves Settings independently of PipeWire.

Protocols are auto-detected at startup. The best available protocol is selected
automatically with ext- protocols preferred over wlr- equivalents.

## Dependencies

Build dependencies:

- Rust >= 1.87
- `libpipewire-0.3-dev`
- `libspa-0.2-dev`
- `libwayland-dev`
- `libxkbcommon-dev`
- `libclang-dev`

Runtime:

- A Wayland compositor with ext- or wlr- protocol support
- PipeWire (for ScreenCast)
- `xdg-desktop-portal` (the frontend daemon)

### Distro-Specific Packages

**Debian/Ubuntu:**

```sh
sudo apt install libpipewire-0.3-dev libspa-0.2-dev libwayland-dev \
    libxkbcommon-dev libclang-dev
```

**Fedora:**

```sh
sudo dnf install pipewire-devel wayland-devel libxkbcommon-devel clang-devel
```

**Arch:**

```sh
sudo pacman -S pipewire wayland libxkbcommon clang
```

## Building

```sh
cargo build --release
```

Or use the Makefile:

```sh
make build
```

## Installation

Install matching Villain, Knave, and Knave Shell binaries first using their
`scripts/install.sh --user` commands. Then run:

```sh
./scripts/install.py --user
```

| File | User-local location |
|------|---------------------|
| Binary | `~/.local/libexec/xdg-desktop-portal-knave` |
| Descriptor | `~/.local/share/xdg-desktop-portal/portals/knave.portal` |
| Routing defaults | `~/.local/share/xdg-desktop-portal/knave-portals.conf` |
| D-Bus activation | `~/.local/share/dbus-1/services/org.freedesktop.impl.portal.desktop.knave.service` |
| User unit | `$XDG_CONFIG_HOME/systemd/user/xdg-desktop-portal-knave.service` (normally `~/.config`) |

The installer generates absolute executable paths, reloads user units and D-Bus
service discovery without restarting the bus or desktop services, and does
not enable a global startup service. Existing user routing overrides are preserved.
Use `--prefix PATH --destdir STAGE` for packaging; generated paths refer to PATH,
not STAGE. Uninstall with `./scripts/install.py --user --uninstall`.

## Running

A direct Knave session exports `XDG_CURRENT_DESKTOP=Knave:Villain`, its Wayland
socket, and the resolved Shell binary to D-Bus/systemd activation. Its local share
directory precedes system data directories. Nested Winit sessions leave host
activation untouched. Start a new direct Knave session after installing; do not
restart the host desktop's portals to test a nested compositor.

Knave defaults route Settings, ScreenCast and Screenshot here, with `gtk` for
remaining dialogs (requires `xdg-desktop-portal-gtk`). An optional user override is
`~/.config/xdg-desktop-portal/knave-portals.conf`:

```ini
[preferred]
default=gtk
org.freedesktop.impl.portal.ScreenCast=knave
org.freedesktop.impl.portal.Settings=knave
org.freedesktop.impl.portal.Screenshot=knave
```

### Automatic Activation

When correctly installed, `xdg-desktop-portal` will automatically activate
`xdg-desktop-portal-knave` via D-Bus when an application requests a portal
that is configured to use this backend.

The executable, D-Bus name, and portal backend ID use the `knave` identity.
Existing `XDP_GENERIC_*` environment variables remain accepted as deprecated
fallbacks; a corresponding `XDP_KNAVE_*` variable takes precedence when both
are set.

### Manual Startup

For development and testing, you can start it directly:

```sh
RUST_LOG=xdg_desktop_portal_knave=debug xdg-desktop-portal-knave
```

## Configuration

Canonical preferences live in `~/.config/knave/config.toml` (honoring
`XDG_CONFIG_HOME`). Existing files without `[portal]` receive defaults:

```toml
[portal]
enabled = true
color_scheme = 0
accent_color = "0.21,0.52,0.89"
high_contrast = false
reduced_motion = false
```

Appearance changes emit SettingChanged through a directory inotify subscription;
invalid edits retain the last valid settings. `enabled` applies on session startup.
Explicit environment overrides are still supported:

Settings use the `XDP_KNAVE_*` prefix. For migration, the former
`XDP_GENERIC_*` names are accepted when the matching Knave-prefixed setting is
unset; Knave-prefixed values take precedence.

### Appearance Settings

| Variable | Values | Default | Description |
|----------|--------|---------|-------------|
| `XDP_KNAVE_COLOR_SCHEME` | `0` / `1` / `2` | `0` | Color scheme: 0 = detect from GTK_THEME, 1 = dark, 2 = light |
| `XDP_KNAVE_ACCENT_COLOR` | `r,g,b` floats | `0.21,0.52,0.89` | Accent color as comma-separated RGB floats (0.0-1.0) |
| `XDP_KNAVE_CONTRAST` | `0` / `1` | `0` | High contrast mode: 0 = normal, 1 = high |
| `XDP_KNAVE_REDUCED_MOTION` | `0` / `1` | `0` | Reduced motion: 0 = normal, 1 = reduced |

### Inherited input options (not advertised by Knave)

| Variable | Values | Default | Description |
|----------|--------|---------|-------------|
| `XDP_KNAVE_INPUT_PROTOCOL` | `eis` / `wlr` | `eis` | Force a specific input injection protocol |
| `XDP_KNAVE_INPUT_NO_FALLBACK` | `1` | unset | Disable automatic fallback to the other input protocol |
| `XDP_KNAVE_EIS_SOCKET` | path | auto | Custom EIS socket path |

### External Tools

| Variable | Value | Description |
|----------|-------|-------------|
| `XDP_KNAVE_SOURCE_PICKER` | path to executable | External tool for ScreenCast source selection UI |
| `XDP_KNAVE_SHELL_BINARY` | absolute executable path | Native picker and sharing control binary |

#### Picker protocol

The native command is `knave-shell portal-picker`. It reads one versioned JSON
request on stdin and returns one versioned JSON reply on stdout. The
`knave-portal-api` crate defines and validates this private contract. Requests are
limited to eight sources and 2 MiB, replies to 4 KiB, with one consent dialog at a
time and a 120-second consent deadline. Sharing controls live until session close.

`XDP_KNAVE_SOURCE_PICKER` (or deprecated `XDP_GENERIC_SOURCE_PICKER`) retains the
legacy tab-separated monitor input and output-name reply for development. Empty
output cancels; failed tools and unknown names never approve a source. This override
does not replace the native active-sharing control.

## Verification

`cargo test --all-features`, strict Clippy, and docs build validate the backend.
`examples/knave_smoke.rs` is an isolated-session integration client: `capture`
checks raw ext capture; `frontend` checks an interactive screenshot and ten real
video buffers through the frontend's restricted PipeWire fd, then closes the session.
Never run an automatic-selection fixture on the normal user session. Build debug binaries/examples in the matching siblings, then run
`python3 scripts/smoke.py` for settings updates, request cancellation, repeated
frontend sessions and resource snapshots. `--render` captures the native dialog;
`--native` requires manual source selection. `--release` uses release builds.
Native clicks, direct-TTY login, OBS/browser and sandbox apps need separate checks.
See [the validation record](docs/knave-validation.md) for measured results and limits.

## Compositor Compatibility

The inherited implementation is designed for compositors that do not ship
their own portal backend and expose the required standard protocols. Its
protocols are detected at runtime; the capabilities available depend on the
compositor and installed PipeWire/portal services.

Nested Villain raw capture, frontend screenshot and PipeWire delivery are exercised
by the isolated smoke script. Direct-TTY, multiple physical monitors and sandbox
application behavior remain separate live checks. `UseIn` retains legacy frontend
compatibility; current frontends select the shipped Knave routing defaults.

## Architecture

Three execution roles (Tokio uses two async worker threads):

```
+--------------------+     mpsc channels     +----------------------+
| Tokio async runtime| <------------------> | Wayland event loop    |
| (main thread)      |                      | (dedicated thread)    |
|                    |                      |                      |
| - D-Bus service    |  Arc<Mutex<>> state  | - Protocol dispatch   |
| - Session mgmt     | <------------------> | - Frame capture       |
| - Portal logic     |                      | - Clipboard events    |
+--------------------+                      +----------------------+
        |                                            |
        |  PipeWire node IDs                        |  SHM buffers
        v                                            v
+--------------------+
| PipeWire thread    |
| - Stream mgmt     |
| - Buffer delivery  |
+--------------------+
```

1. **Tokio async runtime** (main) -- D-Bus service, session management, portal
   interface logic
2. **Wayland event loop** (dedicated thread) -- Wayland protocol dispatch,
   frame capture, clipboard data control
3. **PipeWire thread** -- Stream creation and management, SHM buffer delivery
   to consuming applications

Communication between threads uses `mpsc` channels for commands and
`Arc<Mutex<>>` for shared state.

## Debugging

Enable debug logging:

```sh
RUST_LOG=xdg_desktop_portal_knave=debug xdg-desktop-portal-knave
```

For trace-level output including protocol messages:

```sh
RUST_LOG=xdg_desktop_portal_knave=trace xdg-desktop-portal-knave
```

### Useful Tools

- **`dbus-monitor`** -- Watch portal D-Bus requests:
  ```sh
  dbus-monitor --session "interface='org.freedesktop.impl.portal.ScreenCast'"
  ```
- **`busctl`** -- Inspect the D-Bus service:
  ```sh
  busctl --user introspect org.freedesktop.impl.portal.desktop.knave /
  ```
- **[portal-test](https://github.com/matthiasclasen/portal-test)** -- Flatpak
  app for testing portal implementations
- **`pw-top`** -- Monitor active PipeWire streams during screen capture
- **`wayland-info`** -- List compositor globals and verify protocol support

### Checking Protocol Support

To verify which protocols your compositor supports:

```sh
wayland-info | grep -E '(ext_image_copy_capture|zwlr_screencopy|ext_data_control|zwlr_data_control|wlr_virtual_pointer|zwp_virtual_keyboard)'
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, testing, and
contribution guidelines.

## License

Licensed under either of:

- MIT License ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

at your option.
