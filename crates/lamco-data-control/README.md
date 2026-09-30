# lamco-data-control

[![Crates.io](https://img.shields.io/crates/v/lamco-data-control.svg)](https://crates.io/crates/lamco-data-control)
[![Documentation](https://docs.rs/lamco-data-control/badge.svg)](https://docs.rs/lamco-data-control)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A Wayland clipboard client built on the data-control protocols.

## Overview

Data-control lets a client with no window read and set the clipboard. This
crate speaks both `ext-data-control-v1` and `wlr-data-control-unstable-v1` and
uses whichever the compositor offers. It exists because clipboard bridges such
as remote desktop servers need to see every selection change and to serve
pastes on demand, which the toolkit clipboard libraries do not expose.

One thread owns the Wayland connection, and `DataControl` is the handle you
call from any thread. A selection this handle set itself is not reported back
as a change, so a bridge does not echo its own clipboard.

## Features

- No async runtime, and no dependency on D-Bus, PipeWire or any RDP stack.
- One thread owns the Wayland connection; `DataControl` is the handle you call
  from anywhere.
- Delayed rendering: advertise a type without data and produce the bytes only
  when something pastes it.
- Charset-tolerant type matching: `text/plain` and `text/plain;charset=utf-8`
  find each other.

## Usage

```rust,no_run
use lamco_data_control::{Content, DataControl};

fn main() -> lamco_data_control::Result<()> {
    let clipboard = DataControl::connect()?;

    // Read what is on the clipboard.
    if let Some(bytes) = clipboard.read("text/plain;charset=utf-8")? {
        println!("{}", String::from_utf8_lossy(&bytes));
    }

    // Take ownership and offer new content.
    clipboard.set_selection(Content::new().data("text/plain;charset=utf-8", "hello"))?;
    Ok(())
}
```

Delayed rendering:

```rust,no_run
use lamco_data_control::{Content, DataControl};

fn main() -> lamco_data_control::Result<()> {
    let clipboard = DataControl::connect()?;
    clipboard.on_transfer(|request| {
        let _ = request.complete("rendered on demand");
    });
    clipboard.set_selection(Content::new().advertise("text/plain;charset=utf-8"))?;
    std::thread::park();
    Ok(())
}
```

## Examples

See `examples/`: `paste` prints the clipboard and `copy` sets it.

## Compositor support

| Compositor | Protocol |
|---|---|
| KDE Plasma (KWin) | `ext-data-control-v1`, `wlr-data-control-unstable-v1` |
| Sway, Hyprland, labwc and other wlroots compositors | `wlr-data-control-unstable-v1` |
| GNOME (Mutter) | none, so `connect` returns `Error::Unsupported` |

## Limits

`Options::max_read_bytes` caps one read at 100 MiB by default. A source that
delivers nothing for five seconds makes `read` return `Error::Timeout`.

The primary selection is not handled.

## Minimum Rust version

1.87.

## About Lamco

This crate is part of the Lamco RDP Server project. Lamco develops RDP server
solutions for Wayland and Linux.

**Open source foundation:** portal integration and protocol components.
**Commercial products:** Lamco RDP Server, Lamco VDI.

Learn more: https://www.lamco.ai

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you shall be dual licensed as above, without any
additional terms or conditions.
