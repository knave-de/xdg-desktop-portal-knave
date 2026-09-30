//! A Wayland clipboard client built on the data-control protocols.
//!
//! Data-control lets a client that has no window read and set the clipboard.
//! This crate speaks both `ext-data-control-v1` and
//! `wlr-data-control-unstable-v1` and picks whichever the compositor offers.
//!
//! It has no async runtime and no dependency on any RDP, `D-Bus` or `PipeWire`
//! stack. One thread owns the Wayland connection; [`DataControl`] is the
//! handle you call from anywhere.
//!
//! # Reading
//!
//! ```no_run
//! use lamco_data_control::DataControl;
//!
//! # fn main() -> lamco_data_control::Result<()> {
//! let clipboard = DataControl::connect()?;
//! if let Some(bytes) = clipboard.read("text/plain;charset=utf-8")? {
//!     println!("{}", String::from_utf8_lossy(&bytes));
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Owning the clipboard, with delayed rendering
//!
//! ```no_run
//! use lamco_data_control::{Content, DataControl};
//!
//! # fn main() -> lamco_data_control::Result<()> {
//! let clipboard = DataControl::connect()?;
//! clipboard.on_transfer(|request| {
//!     // Produce the data only now that something is pasting it.
//!     let _ = request.complete("rendered on demand");
//! });
//! clipboard.set_selection(Content::new().advertise("text/plain;charset=utf-8"))?;
//! # Ok(())
//! # }
//! ```
//!
//! # Platform
//!
//! Linux and other systems with a Wayland compositor that offers a
//! data-control protocol. GNOME's Mutter does not.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod client;
mod dispatch;
mod error;
mod mime;
mod options;
mod state;
mod worker;

pub use client::{Content, DataControl, TransferRequest};
pub use error::{Error, Result};
pub use mime::find_mime_match;
pub use options::{Options, Preference, Protocol};
