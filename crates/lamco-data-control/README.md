# lamco-data-control

A Wayland clipboard client built on the data-control protocols. It reads and
sets the clipboard without a window, and speaks both
`ext-data-control-v1` and `wlr-data-control-unstable-v1`, using whichever the
compositor offers.

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

Two runnable examples are in `examples/`: `paste` prints the clipboard and
`copy` sets it.

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

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
