//! Clipboard backend on top of the `lamco-data-control` crate.
//!
//! The crate runs its own Wayland connection on a dedicated thread, so this
//! backend is a thin adapter: it translates [`ClipboardData`] to the crate's
//! `Content`, and keeps the serial-keyed transfer table the portal's
//! `SelectionTransfer` signal needs.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
};

use lamco_data_control::{Content, DataControl, Options, Protocol, TransferRequest};

use super::{ClipboardBackend, ClipboardProtocol};
use crate::{
    error::{PortalError, Result},
    types::ClipboardData,
};

/// Transfers raised by a paste and not yet answered, by portal serial.
type PendingTransfers = Arc<Mutex<HashMap<u32, TransferRequest>>>;

/// Clipboard backend using `ext-data-control-v1` or `wlr-data-control`.
pub struct DataControlBackend {
    clipboard: DataControl,
    pending: PendingTransfers,
    /// Health event sender for clipboard metrics.
    health_tx: Option<crate::health::HealthSender>,
}

impl DataControlBackend {
    /// Connect to the compositor's clipboard.
    ///
    /// # Errors
    ///
    /// Returns [`PortalError::Wayland`] if the connection cannot be made or
    /// no data-control protocol is usable.
    pub fn connect(options: &Options) -> Result<Self> {
        let clipboard =
            DataControl::connect_with(options).map_err(|e| PortalError::Wayland(e.to_string()))?;
        Ok(Self {
            clipboard,
            pending: Arc::new(Mutex::new(HashMap::new())),
            health_tx: None,
        })
    }
}

fn wayland_error(error: &lamco_data_control::Error) -> PortalError {
    PortalError::Wayland(error.to_string())
}

impl ClipboardBackend for DataControlBackend {
    fn protocol_type(&self) -> ClipboardProtocol {
        match self.clipboard.protocol() {
            Protocol::Wlr => ClipboardProtocol::WlrDataControl,
            // `Protocol` is non-exhaustive; ext is the standard one.
            _ => ClipboardProtocol::ExtDataControl,
        }
    }

    fn get_clipboard(&self) -> Result<ClipboardData> {
        Ok(ClipboardData {
            mime_types: self.clipboard.selection_mime_types(),
            data: HashMap::new(), // Data is fetched on demand via read_selection
        })
    }

    fn set_clipboard(&mut self, data: ClipboardData) -> Result<()> {
        tracing::debug!(mime_types = ?data.mime_types, "Setting clipboard");

        let total_bytes: usize = data.data.values().map(Vec::len).sum();

        let mut content = Content::new();
        for mime_type in &data.mime_types {
            content = match data.data.get(mime_type) {
                Some(bytes) => content.data(mime_type.clone(), bytes.clone()),
                None => content.advertise(mime_type.clone()),
            };
        }
        // Data supplied for a type that was not listed is still offered.
        for (mime_type, bytes) in &data.data {
            if !data.mime_types.contains(mime_type) {
                content = content.data(mime_type.clone(), bytes.clone());
            }
        }

        self.clipboard
            .set_selection(content)
            .map_err(|e| wayland_error(&e))?;

        if let Some(ref health_tx) = self.health_tx {
            let _ = health_tx.try_send(crate::health::PortalHealthEvent::ClipboardTransferResult {
                success: true,
                bytes: total_bytes,
            });
        }
        Ok(())
    }

    fn on_selection_changed(&mut self, callback: Box<dyn Fn(Vec<String>) + Send + Sync>) {
        self.clipboard.on_change(callback);
    }

    fn read_selection(&self, mime_type: &str) -> Result<Option<Vec<u8>>> {
        self.clipboard.read(mime_type).map_err(|e| match e {
            lamco_data_control::Error::Io(io) => PortalError::Io(io),
            other => wayland_error(&other),
        })
    }

    fn update_source_data(&mut self, mime_type: &str, data: Vec<u8>) -> Result<()> {
        self.clipboard
            .update_data(mime_type, data)
            .map_err(|e| wayland_error(&e))
    }

    fn on_transfer_requested(&mut self, callback: Box<dyn Fn(u32, String) + Send + Sync>) {
        let pending = Arc::clone(&self.pending);
        let next_serial = AtomicU32::new(1);
        self.clipboard.on_transfer(move |request| {
            let serial = next_serial.fetch_add(1, Ordering::Relaxed);
            let mime_type = request.mime_type().to_owned();
            if let Ok(mut table) = pending.lock() {
                table.insert(serial, request);
            }
            callback(serial, mime_type);
        });
    }

    fn complete_transfer(&mut self, serial: u32, data: Option<Vec<u8>>) -> Result<()> {
        let request = self
            .pending
            .lock()
            .map_err(|_| PortalError::Wayland("transfer table poisoned".to_string()))?
            .remove(&serial);
        let Some(request) = request else {
            tracing::debug!(serial, "Transfer already answered or unknown");
            return Ok(());
        };
        match data {
            Some(bytes) => request.complete(bytes),
            None => request.fail(),
        }
        .map_err(|e| wayland_error(&e))
    }

    fn clear_selection(&mut self) -> Result<()> {
        self.clipboard
            .clear_selection()
            .map_err(|e| wayland_error(&e))
    }

    fn set_health_sender(&mut self, tx: crate::health::HealthSender) {
        self.health_tx = Some(tx);
    }

    fn write_done(&mut self, serial: u32, success: bool) -> Result<()> {
        tracing::debug!(serial, success, "Selection write done");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use super::*;

    const TEXT: &str = "text/plain;charset=utf-8";

    /// Needs a compositor with data-control; changes its clipboard, so run
    /// only inside an isolated one (`kwin_wayland --virtual`).
    #[test]
    #[ignore = "changes the clipboard of the running compositor"]
    fn backend_round_trip_and_delayed_rendering() {
        let mut owner = DataControlBackend::connect(&Options::new()).expect("connect owner");
        let mut reader = DataControlBackend::connect(&Options::new()).expect("connect reader");

        let (change_tx, change_rx) = mpsc::channel();
        reader.on_selection_changed(Box::new(move |types| {
            let _ = change_tx.send(types);
        }));

        // Data supplied up front.
        owner
            .set_clipboard(ClipboardData {
                mime_types: vec![TEXT.to_owned()],
                data: HashMap::from([(TEXT.to_owned(), b"up front".to_vec())]),
            })
            .expect("set");
        change_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("change");
        assert_eq!(
            reader.read_selection(TEXT).expect("read"),
            Some(b"up front".to_vec())
        );

        // Advertised without data, answered through the serial table.
        let (transfer_tx, transfer_rx) = mpsc::channel();
        owner.on_transfer_requested(Box::new(move |serial, mime| {
            let _ = transfer_tx.send((serial, mime));
        }));
        owner
            .set_clipboard(ClipboardData {
                mime_types: vec![TEXT.to_owned()],
                data: HashMap::new(),
            })
            .expect("set delayed");
        change_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("change");

        let handle = std::thread::spawn(move || reader.read_selection(TEXT));
        let (serial, mime) = transfer_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("transfer raised");
        assert_eq!(mime, TEXT);
        owner
            .complete_transfer(serial, Some(b"late".to_vec()))
            .expect("complete");
        assert_eq!(
            handle.join().expect("join").expect("read"),
            Some(b"late".to_vec())
        );
    }
}
