# xdg-desktop-portal-knave

[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)

**Source:** [knave-de/xdg-desktop-portal-knave](https://github.com/knave-de/xdg-desktop-portal-knave)

`xdg-desktop-portal-knave` is Knave Desktop's portal backend, based on
[xdg-desktop-portal-generic](https://github.com/lamco-admin/xdg-desktop-portal-generic).
It provides the XDG Desktop Portal backend services needed by sandboxed and
other desktop applications in a Wayland session.

The fork retains the upstream backend's compositor-independent implementation
and is the home for Knave-specific portal integration. The Knave naming and
activation identity are in place; live integration with a Knave session still
needs end-to-end validation.

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
| `RemoteDesktop` | v2 | EIS (libei) bridge mode | wlr-virtual-pointer + zwp-virtual-keyboard |
| `ScreenCast` | v6 | ext-image-copy-capture-v1 | wlr-screencopy-v1 |
| `Clipboard` | v1 | ext-data-control-v1 | wlr-data-control-v1 |
| `Settings` | v2 | Environment variable config | GTK_THEME detection |
| `Screenshot` | v2 | Single-frame capture to PNG | -- |
| `InputCapture` | v2 | Barrier surfaces (wlr-layer-shell-v1) + pointer lock (zwp-pointer-constraints-v1) + EIS bridge (receiver context) | -- |

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

```sh
sudo make install
```

This installs:

| File | Location |
|------|----------|
| Binary | `/usr/libexec/xdg-desktop-portal-knave` |
| Portal config | `/usr/share/xdg-desktop-portal/portals/knave.portal` |
| D-Bus service | `/usr/share/dbus-1/services/org.freedesktop.impl.portal.desktop.knave.service` |
| Systemd unit | `/usr/lib/systemd/user/xdg-desktop-portal-knave.service` |

To uninstall:

```sh
sudo make uninstall
```

## Running

### Environment Setup

Ensure your compositor exports the required environment variables into D-Bus:

```sh
dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_CURRENT_DESKTOP
```

Most compositors do this automatically.

### Portal Configuration

Create a portal configuration file to tell `xdg-desktop-portal` which backend
to use. Create `~/.config/xdg-desktop-portal/portals.conf` (or the appropriate
file for your `XDG_CURRENT_DESKTOP`):

```ini
[preferred]
default=gtk
org.freedesktop.impl.portal.RemoteDesktop=knave
org.freedesktop.impl.portal.ScreenCast=knave
org.freedesktop.impl.portal.Clipboard=knave
org.freedesktop.impl.portal.Settings=knave
org.freedesktop.impl.portal.Screenshot=knave
org.freedesktop.impl.portal.InputCapture=knave
```

See the [portal configuration docs](https://flatpak.github.io/xdg-desktop-portal/docs/portals.conf.html)
for more information on the `portals.conf` format.

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

All configuration is via environment variables, set before the service starts
(e.g., in your compositor config or systemd override).

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

### Input Protocol

| Variable | Values | Default | Description |
|----------|--------|---------|-------------|
| `XDP_KNAVE_INPUT_PROTOCOL` | `eis` / `wlr` | `eis` | Force a specific input injection protocol |
| `XDP_KNAVE_INPUT_NO_FALLBACK` | `1` | unset | Disable automatic fallback to the other input protocol |
| `XDP_KNAVE_EIS_SOCKET` | path | auto | Custom EIS socket path |

### External Tools

| Variable | Value | Description |
|----------|-------|-------------|
| `XDP_KNAVE_SOURCE_PICKER` | path to executable | External tool for ScreenCast source selection UI |
| `XDP_KNAVE_COLOR_PICKER` | path to executable | External tool for Screenshot color picking |

#### Source Picker Protocol

The source picker tool receives available sources on stdin as tab-separated
lines (`name\ttype\tid`), one per line. It should output selected source names
on stdout, one per line. Exit without output to cancel.

Example using `fzf`:

```sh
#!/bin/sh
fzf --multi --with-nth=1 --delimiter='\t' | cut -f1
```

#### Color Picker Protocol

The color picker tool receives a PNG screenshot path on stdin. It should output
`x y` coordinates (space-separated integers) on stdout. The color at those
coordinates will be returned to the requesting application.

## Compositor Compatibility

The inherited implementation is designed for compositors that do not ship
their own portal backend and expose the required standard protocols. Its
protocols are detected at runtime; the capabilities available depend on the
compositor and installed PipeWire/portal services.

Knave's Villain compositor is a target for this fork. Actual feature support
must be checked against Villain's advertised protocols and validated in a live
Knave session; compilation or protocol declarations alone do not establish
end-to-end portal behavior. The `UseIn` list in `knave.portal` is retained
for legacy portal frontend compatibility.

## Architecture

Three-thread model:

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
