//! Clipboard backend abstraction.
//!
//! Provides a [`ClipboardBackend`] trait and [`DataControlBackend`], its
//! implementation on top of the `lamco-data-control` crate. That crate owns
//! its own Wayland connection and speaks both `ext-data-control-v1`
//! (preferred) and `wlr-data-control-unstable-v1`.

mod backend;

pub use backend::DataControlBackend;
use lamco_data_control::{Options, Preference};

use crate::{error::Result, types::ClipboardData, wayland::AvailableProtocols};

/// Clipboard protocol in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClipboardProtocol {
    /// ext-data-control-v1 (staging standard).
    ExtDataControl,
    /// zwlr-data-control-manager-v1 (wlroots).
    WlrDataControl,
}

impl std::fmt::Display for ClipboardProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipboardProtocol::ExtDataControl => write!(f, "ext-data-control-v1"),
            ClipboardProtocol::WlrDataControl => write!(f, "wlr-data-control-v1"),
        }
    }
}

/// Clipboard protocol selection preferences.
///
/// Mirrors [`super::capture::CapturePreference`] pattern.
#[derive(Debug, Clone)]
pub struct ClipboardPreference {
    /// Preferred protocol. `None` = auto-detect (ext preferred over wlr).
    pub preferred: Option<ClipboardProtocol>,

    /// Allow fallback to alternative protocol.
    pub allow_fallback: bool,
}

impl Default for ClipboardPreference {
    fn default() -> Self {
        Self {
            preferred: None,
            allow_fallback: true,
        }
    }
}

impl ClipboardPreference {
    /// Create preferences from environment variables.
    ///
    /// Reads:
    /// - `XDP_GENERIC_CLIPBOARD_PROTOCOL`: "ext" or "wlr"
    /// - `XDP_GENERIC_CLIPBOARD_NO_FALLBACK`: "1" to disable fallback
    pub fn from_env() -> Self {
        let mut prefs = Self::default();

        if let Ok(protocol) = std::env::var("XDP_GENERIC_CLIPBOARD_PROTOCOL") {
            match protocol.to_lowercase().as_str() {
                "ext" | "ext-data-control" => {
                    prefs.preferred = Some(ClipboardProtocol::ExtDataControl);
                }
                "wlr" | "wlr-data-control" => {
                    prefs.preferred = Some(ClipboardProtocol::WlrDataControl);
                }
                _ => tracing::warn!("Unknown clipboard protocol: {}", protocol),
            }
        }

        if std::env::var("XDP_GENERIC_CLIPBOARD_NO_FALLBACK").is_ok() {
            prefs.allow_fallback = false;
        }

        prefs
    }
}

/// Abstraction over clipboard Wayland protocols.
///
/// This trait provides a unified interface for clipboard access,
/// regardless of which Wayland protocol is used underneath.
pub trait ClipboardBackend: Send + Sync {
    /// Get the clipboard protocol this backend implements.
    fn protocol_type(&self) -> ClipboardProtocol;

    /// Get current clipboard content.
    ///
    /// Returns available MIME types and any cached data.
    fn get_clipboard(&self) -> Result<ClipboardData>;

    /// Set clipboard content.
    ///
    /// Takes ownership of the selection with the given MIME types and data.
    fn set_clipboard(&mut self, data: ClipboardData) -> Result<()>;

    /// Register callback for clipboard selection changes.
    ///
    /// Called when the compositor's clipboard content changes.
    fn on_selection_changed(&mut self, callback: Box<dyn Fn(Vec<String>) + Send + Sync>);

    /// Read clipboard data for a specific MIME type.
    ///
    /// Returns the data bytes, or None if the MIME type is not available.
    fn read_selection(&self, mime_type: &str) -> Result<Option<Vec<u8>>>;

    /// Update source data for a MIME type after `set_clipboard`.
    ///
    /// Inserts data into the source cache without re-creating the Wayland
    /// data source. Used when data wasn't available at announcement time
    /// (e.g., eager fetch from a remote clipboard over RDP).
    fn update_source_data(&mut self, mime_type: &str, data: Vec<u8>) -> Result<()>;

    /// Register a callback for pastes of advertised MIME types that have no
    /// data yet (delayed rendering).
    ///
    /// The paste is held open and the callback gets a serial and the MIME
    /// type; answer it with [`complete_transfer`](Self::complete_transfer).
    /// Without a callback such pastes get no data.
    fn on_transfer_requested(&mut self, _callback: Box<dyn Fn(u32, String) + Send + Sync>) {}

    /// Answer a paste raised through `on_transfer_requested`. `None` closes
    /// the waiting pastes with no data.
    fn complete_transfer(&mut self, _serial: u32, _data: Option<Vec<u8>>) -> Result<()> {
        Ok(())
    }

    /// Give up our selection if we still own it (the remote that supplied it
    /// has gone).
    fn clear_selection(&mut self) -> Result<()> {
        Ok(())
    }

    /// Set the health event sender for clipboard metrics reporting.
    ///
    /// When set, the backend emits [`crate::health::PortalHealthEvent`] variants
    /// for clipboard operations (set, read, selection changes).
    fn set_health_sender(&mut self, _tx: crate::health::HealthSender) {
        // Default no-op for backends that don't implement health monitoring
    }

    /// Notify the backend that a clipboard write operation has completed.
    ///
    /// Called after the client finishes writing data through a
    /// `SelectionWrite` pipe. The `serial` matches the value from the
    /// corresponding `SelectionTransfer` signal, and `success` indicates
    /// whether the write completed successfully.
    fn write_done(&mut self, serial: u32, success: bool) -> Result<()>;
}

/// Map the portal's clipboard preferences onto the crate's connection options.
fn options_for(prefs: &ClipboardPreference) -> (Preference, bool) {
    let preference = match prefs.preferred {
        None => Preference::Auto,
        Some(ClipboardProtocol::ExtDataControl) => Preference::Ext,
        Some(ClipboardProtocol::WlrDataControl) => Preference::Wlr,
    };
    (preference, prefs.allow_fallback)
}

/// Create a clipboard backend based on preferences and available protocols.
///
/// Returns `None` if the compositor offers no data-control protocol, or the
/// preferred one is missing and fallback is disabled. Connection failures are
/// logged and also give `None`, so the portal runs without a clipboard.
pub fn create_clipboard_backend(
    protocols: &AvailableProtocols,
    prefs: &ClipboardPreference,
) -> Option<Box<dyn ClipboardBackend>> {
    if !protocols.has_clipboard() {
        tracing::warn!("No clipboard protocols available");
        return None;
    }

    match DataControlBackend::connect(&{
        let (preference, allow_fallback) = options_for(prefs);
        Options::new()
            .preference(preference)
            .allow_fallback(allow_fallback)
    }) {
        Ok(backend) => {
            tracing::info!(protocol = %backend.protocol_type(), "Clipboard backend ready");
            Some(Box::new(backend))
        }
        Err(error) => {
            tracing::warn!(%error, "Clipboard backend unavailable");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clipboard_protocol_display() {
        assert_eq!(
            ClipboardProtocol::ExtDataControl.to_string(),
            "ext-data-control-v1"
        );
        assert_eq!(
            ClipboardProtocol::WlrDataControl.to_string(),
            "wlr-data-control-v1"
        );
    }

    #[test]
    fn test_no_protocols_gives_no_backend() {
        let prefs = ClipboardPreference::default();
        assert!(create_clipboard_backend(&AvailableProtocols::default(), &prefs).is_none());
    }

    #[test]
    fn test_default_preferences_map_to_auto_with_fallback() {
        assert_eq!(
            options_for(&ClipboardPreference::default()),
            (Preference::Auto, true)
        );
    }

    #[test]
    fn test_explicit_wlr_preference() {
        let prefs = ClipboardPreference {
            preferred: Some(ClipboardProtocol::WlrDataControl),
            allow_fallback: true,
        };
        assert_eq!(options_for(&prefs), (Preference::Wlr, true));
    }

    #[test]
    fn test_preferred_without_fallback_is_passed_through() {
        let prefs = ClipboardPreference {
            preferred: Some(ClipboardProtocol::ExtDataControl),
            allow_fallback: false,
        };
        assert_eq!(options_for(&prefs), (Preference::Ext, false));
    }
}
