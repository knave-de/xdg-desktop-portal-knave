//! Tests against a running compositor.
//!
//! Ignored by default because they change the clipboard. Run them only inside
//! an isolated compositor, for example `kwin_wayland --virtual`:
//!
//! ```text
//! WAYLAND_DISPLAY=<isolated socket> cargo test -p lamco-data-control -- --ignored
//! ```

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use lamco_data_control::{Content, DataControl};

const TEXT: &str = "text/plain;charset=utf-8";

fn wait_for_change(rx: &mpsc::Receiver<Vec<String>>) -> Vec<String> {
    rx.recv_timeout(Duration::from_secs(5))
        .expect("selection change")
}

/// Wait until the handle has seen a selection newer than `serial`, which is
/// how a handle learns about a selection it set itself.
fn wait_for_serial(clipboard: &DataControl, serial: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while clipboard.serial() == serial {
        assert!(Instant::now() < deadline, "no selection event");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "changes the clipboard of the running compositor"]
fn set_then_read_back() {
    let clipboard = DataControl::connect().expect("connect");
    let before = clipboard.serial();

    clipboard
        .set_selection(Content::new().data(TEXT, "round trip"))
        .expect("set");
    wait_for_serial(&clipboard, before);

    let bytes = clipboard
        .read("text/plain")
        .expect("read")
        .expect("offered");
    assert_eq!(bytes, b"round trip");
}

#[test]
#[ignore = "changes the clipboard of the running compositor"]
fn a_second_client_reads_our_selection() {
    let owner = DataControl::connect().expect("connect owner");
    let reader = DataControl::connect().expect("connect reader");
    let (tx, rx) = mpsc::channel();
    reader.on_change(move |types| {
        let _ = tx.send(types);
    });

    owner
        .set_selection(Content::new().data(TEXT, "seen by another client"))
        .expect("set");
    wait_for_change(&rx);

    let bytes = reader.read(TEXT).expect("read").expect("offered");
    assert_eq!(bytes, b"seen by another client");
}

#[test]
#[ignore = "changes the clipboard of the running compositor"]
fn delayed_rendering_produces_data_on_paste() {
    let owner = DataControl::connect().expect("connect owner");
    let reader = DataControl::connect().expect("connect reader");
    let (tx, rx) = mpsc::channel();
    reader.on_change(move |types| {
        let _ = tx.send(types);
    });
    owner.on_transfer(|request| {
        request.complete("rendered late").expect("complete");
    });

    owner
        .set_selection(Content::new().advertise(TEXT))
        .expect("set");
    wait_for_change(&rx);

    let bytes = reader.read(TEXT).expect("read").expect("offered");
    assert_eq!(bytes, b"rendered late");
}

#[test]
#[ignore = "changes the clipboard of the running compositor"]
fn a_replaced_selection_is_not_served_from_our_cache() {
    let first = DataControl::connect().expect("connect first");
    let second = DataControl::connect().expect("connect second");
    let (tx, rx) = mpsc::channel();
    first.on_change(move |types| {
        let _ = tx.send(types);
    });
    let before = first.serial();

    first
        .set_selection(Content::new().data(TEXT, "old"))
        .expect("set old");
    wait_for_serial(&first, before);
    second
        .set_selection(Content::new().data(TEXT, "new"))
        .expect("set new");
    // `first` is told about the other client's selection through on_change.
    wait_for_change(&rx);

    let bytes = first.read(TEXT).expect("read").expect("offered");
    assert_eq!(bytes, b"new");
}
