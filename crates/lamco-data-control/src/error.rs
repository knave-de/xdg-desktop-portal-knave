//! Errors returned by the clipboard client.

/// Errors returned by [`DataControl`](crate::DataControl).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The connection to the Wayland compositor could not be made.
    #[error("cannot connect to the Wayland compositor: {0}")]
    Connect(String),

    /// The compositor offers neither data-control protocol.
    #[error("the compositor offers neither ext-data-control-v1 nor wlr-data-control-unstable-v1")]
    Unsupported,

    /// The requested protocol is not offered and fallback is disabled.
    #[error("the requested data-control protocol is not offered by the compositor")]
    ProtocolUnavailable,

    /// The compositor advertises no seat to attach the clipboard to.
    #[error("the compositor advertises no seat")]
    NoSeat,

    /// The compositor reported a protocol error or the connection failed.
    #[error("Wayland error: {0}")]
    Wayland(String),

    /// The worker thread has stopped, so the request could not be delivered.
    #[error("the clipboard worker has stopped")]
    Stopped,

    /// Clipboard data exceeded the size limit.
    #[error("clipboard data of {size} bytes exceeds the {limit} byte limit")]
    TooLarge {
        /// Bytes received when the limit was hit.
        size: usize,
        /// The limit.
        limit: usize,
    },

    /// The source did not deliver its data in time.
    #[error("the clipboard source did not deliver its data in time")]
    Timeout,

    /// An I/O error while creating or reading a pipe.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;
