//! `ScreenCast` D-Bus interface implementation.
//!
//! Implements `org.freedesktop.impl.portal.ScreenCast` version 6.

use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;
use zbus::{
    interface,
    zvariant::{ObjectPath, OwnedValue, Value},
};

use super::{Response, empty_results, get_option_bool, get_option_u32};
use crate::{
    error::PortalError,
    pipewire::PipeWireManager,
    services::{capture::CaptureBackend, input::InputBackend},
    session::{PersistMode, RestoreData, SessionManager},
    types::{CursorMode, SourceType, StreamInfo},
};

fn is_supported_restore_vendor(vendor: &str) -> bool {
    vendor == crate::RESTORE_DATA_VENDOR || vendor == crate::LEGACY_RESTORE_DATA_VENDOR
}

/// `ScreenCast` portal interface implementation.
pub struct ScreenCastInterface {
    /// Session manager.
    session_manager: Arc<Mutex<SessionManager>>,
    /// Capture backend for screen capture operations.
    capture_backend: Arc<Mutex<Box<dyn CaptureBackend>>>,
    /// `PipeWire` manager for stream lifecycle and active-sharing controls.
    pipewire_manager: Arc<PipeWireManager>,
    /// Input backend for session cleanup.
    input_backend: Arc<Mutex<Box<dyn InputBackend>>>,
    capture_tx: Option<std::sync::mpsc::Sender<crate::wayland::CaptureCommand>>,
}

impl ScreenCastInterface {
    /// Create a new `ScreenCast` interface with a capture backend.
    pub fn new(
        session_manager: Arc<Mutex<SessionManager>>,
        capture_backend: Arc<Mutex<Box<dyn CaptureBackend>>>,
        pipewire_manager: Arc<PipeWireManager>,
        input_backend: Arc<Mutex<Box<dyn InputBackend>>>,
    ) -> Self {
        Self {
            session_manager,
            capture_backend,
            pipewire_manager,
            input_backend,
            capture_tx: None,
        }
    }

    /// Wire native selection to the shared Wayland capture worker.
    #[must_use]
    pub fn with_capture_sender(
        mut self,
        sender: std::sync::mpsc::Sender<crate::wayland::CaptureCommand>,
    ) -> Self {
        self.capture_tx = Some(sender);
        self
    }

    /// Extract `persist_mode` from options.
    fn get_persist_mode(options: &HashMap<String, OwnedValue>) -> PersistMode {
        get_option_u32(options, "persist_mode").map_or(PersistMode::None, PersistMode::from_dbus)
    }

    /// Extract source types from options.
    fn get_source_types(options: &HashMap<String, OwnedValue>) -> Vec<SourceType> {
        get_option_u32(options, "types")
            .map_or_else(|| vec![SourceType::Monitor], SourceType::from_bits)
    }

    /// Extract cursor mode from options.
    fn get_cursor_mode(options: &HashMap<String, OwnedValue>) -> CursorMode {
        get_option_u32(options, "cursor_mode")
            .map_or_else(CursorMode::default, CursorMode::from_bits)
    }

    /// Extract multiple sources flag.
    fn get_multiple(options: &HashMap<String, OwnedValue>) -> bool {
        get_option_bool(options, "multiple").unwrap_or(false)
    }

    /// Try to parse `restore_data` from D-Bus options.
    ///
    /// The `restore_data` option is a `(suv)` tuple: (vendor, version, data).
    /// We accept the current `"knave"` vendor and the former `"generic"`
    /// vendor for compatibility, at version 1. The data variant contains a
    /// string array of output names.
    fn parse_restore_data(options: &HashMap<String, OwnedValue>) -> Option<RestoreData> {
        let rd = options.get("restore_data")?;
        // Try to decode the (suv) structure
        let value: &Value<'_> = rd.downcast_ref().ok()?;
        if let Value::Structure(s) = value {
            let fields = s.fields();
            if fields.len() >= 3 {
                let vendor: &str = fields[0].downcast_ref().ok()?;
                if !is_supported_restore_vendor(vendor) {
                    tracing::debug!(vendor, "Unknown restore_data vendor, ignoring");
                    return None;
                }
                let version = u32::try_from(&fields[1]).ok()?;
                if version != 1 {
                    tracing::debug!(version, "Unknown restore_data version, ignoring");
                    return None;
                }
                // Data variant: try to extract as array of strings
                if let Value::Value(inner) = &fields[2] {
                    if let Value::Array(arr) = inner.as_ref() {
                        let names: Vec<String> = arr
                            .iter()
                            .filter_map(|v| {
                                if let Value::Str(s) = v {
                                    Some(s.to_string())
                                } else {
                                    None
                                }
                            })
                            .collect();
                        if !names.is_empty() {
                            return Some(RestoreData {
                                vendor: crate::RESTORE_DATA_VENDOR.to_string(),
                                version,
                                output_names: names,
                            });
                        }
                    }
                }
            }
        }
        None
    }

    /// Build stream results for D-Bus response.
    ///
    /// Includes `position`/`size`, `source_type`, the v5 `mapping_id`
    /// (persistent output identifier), and — when the compositor's PipeWire
    /// assigns one — the v6 `pipewire-serial` (`object.serial`) so clients can
    /// re-follow the stream across output reconfiguration.
    #[expect(
        clippy::expect_used,
        reason = "infallible zvariant Value-to-OwnedValue conversions"
    )]
    fn build_stream_results(streams: &[StreamInfo]) -> HashMap<String, OwnedValue> {
        let stream_data: Vec<(u32, HashMap<String, OwnedValue>)> = streams
            .iter()
            .map(|s| {
                let mut props: HashMap<String, OwnedValue> = HashMap::new();
                props.insert(
                    "position".to_string(),
                    OwnedValue::try_from(Value::from((s.position.0, s.position.1)))
                        .expect("tuple Value converts to OwnedValue"),
                );
                props.insert(
                    "size".to_string(),
                    OwnedValue::try_from(Value::from((s.size.0, s.size.1)))
                        .expect("tuple Value converts to OwnedValue"),
                );
                props.insert(
                    "source_type".to_string(),
                    OwnedValue::from(s.source_type.to_bits()),
                );
                if let Some(ref mapping_id) = s.mapping_id {
                    if let Ok(val) = OwnedValue::try_from(Value::from(mapping_id.as_str())) {
                        props.insert("mapping_id".to_string(), val);
                    }
                }
                // ScreenCast v6: the PipeWire object.serial, emitted only when
                // present (it is filtered to a non-zero value upstream).
                if let Some(serial) = s.serial {
                    props.insert("pipewire-serial".to_string(), OwnedValue::from(serial));
                }
                (s.node_id, props)
            })
            .collect();

        let mut results = HashMap::new();
        results.insert(
            "streams".to_string(),
            OwnedValue::try_from(Value::from(stream_data))
                .expect("stream data Value converts to OwnedValue"),
        );
        results
    }
}

#[interface(
    name = "org.freedesktop.impl.portal.ScreenCast",
    introspection_docs = false
)]
impl ScreenCastInterface {
    /// Create a new `ScreenCast` session.
    #[zbus(name = "CreateSession")]
    async fn create_session(
        &self,
        handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        app_id: &str,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing sender".to_string()))?
            .to_string();

        tracing::debug!(
            handle = %handle,
            session_handle = %session_handle,
            app_id = %app_id,
            sender = %sender,
            "ScreenCast.CreateSession called"
        );

        // Register Request object at handle path for cancellation support
        let request_iface = super::RequestInterface::new(Arc::clone(&self.session_manager));
        let _ = server.at(&handle, request_iface).await;

        let persist_mode = Self::get_persist_mode(&options);

        let mut manager = self.session_manager.lock().await;

        let result = match manager.create_session(
            session_handle.to_owned(),
            sender,
            app_id.to_string(),
            persist_mode,
        ) {
            Ok(_session) => {
                // Register a Session D-Bus object at the session handle path
                let session_iface = super::SessionInterface::new(
                    Arc::clone(&self.session_manager),
                    session_handle.to_owned(),
                    Arc::clone(&self.input_backend),
                    Arc::clone(&self.capture_backend),
                    Arc::clone(&self.pipewire_manager),
                    None,
                );
                if let Err(e) = server.at(&session_handle, session_iface).await {
                    tracing::warn!(
                        session_handle = %session_handle,
                        error = %e,
                        "Failed to register Session D-Bus object"
                    );
                }

                let mut results = HashMap::new();
                results.insert(
                    "session_handle".to_string(),
                    OwnedValue::from(session_handle.to_owned()),
                );
                Ok((Response::Success.to_u32(), results))
            }
            Err(e) => {
                tracing::error!(error = %e, "ScreenCast.CreateSession failed");
                Ok((Response::Other.to_u32(), empty_results()))
            }
        };

        // Remove Request object after method completes
        let _ = server.remove::<super::RequestInterface, _>(&handle).await;
        result
    }

    /// Select sources for capture.
    #[zbus(name = "SelectSources")]
    async fn select_sources(
        &self,
        handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        app_id: &str,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing sender".to_string()))?
            .to_string();

        tracing::debug!(
            handle = %handle,
            session_handle = %session_handle,
            app_id = %app_id,
            "SelectSources called"
        );

        let (request, cancelled) = super::RequestInterface::cancellable();
        let cancellation = request.cancellation_sender();
        server.at(&handle, request).await?;
        let result = async {
            let source_types = Self::get_source_types(&options);
            let multiple = Self::get_multiple(&options);
            let restore_data = Self::parse_restore_data(&options);
            let persist_mode = Self::get_persist_mode(&options);
            let cursor_mode = Self::get_cursor_mode(&options);
            let requested_mode =
                get_option_u32(&options, "cursor_mode").unwrap_or(cursor_mode.to_bits());
            let supported = self.capture_backend.lock().await.available_cursor_modes();
            if !matches!(requested_mode, 1 | 2) || requested_mode & supported == 0 {
                return Err(zbus::fdo::Error::InvalidArgs(
                    "Unsupported cursor mode".into(),
                ));
            }
            {
                let mut manager = self.session_manager.lock().await;
                manager.validate_session(&session_handle, app_id, &sender)?;
                let session = manager
                    .get_session_mut(&session_handle)
                    .ok_or_else(|| PortalError::SessionNotFound(session_handle.to_string()))?;
                session.sharing_stop = cancellation;
                session.cursor_mode = cursor_mode;
            }
            let sources = self
                .capture_backend
                .lock()
                .await
                .get_sources(&source_types)
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
            if sources.is_empty() {
                return Ok((Response::Other.to_u32(), empty_results()));
            }
            let tx = self.capture_tx.as_ref().ok_or_else(|| {
                zbus::fdo::Error::Failed("native capture selection is not wired".into())
            })?;
            let selected = crate::picker::select(
                app_id,
                handle.as_str(),
                knave_portal_api::Operation::Share,
                multiple,
                &sources,
                tx,
                cancelled,
            )
            .await;
            let selected = match selected {
                Ok(selected) if !selected.is_empty() => selected,
                Ok(_) => return Ok((Response::Cancelled.to_u32(), empty_results())),
                Err(error) => {
                    tracing::warn!(%error, "source selection failed");
                    return Ok((Response::Other.to_u32(), empty_results()));
                }
            };
            let mut manager = self.session_manager.lock().await;
            manager.validate_session(&session_handle, app_id, &sender)?;
            let session = manager
                .get_session_mut(&session_handle)
                .ok_or_else(|| PortalError::SessionNotFound(session_handle.to_string()))?;
            session.persist_mode = persist_mode;
            session.restore_data = restore_data;
            session.select_sources(selected)?;
            Ok((Response::Success.to_u32(), empty_results()))
        }
        .await;
        let _ = server.remove::<super::RequestInterface, _>(&handle).await;
        result
    }

    /// Start the `ScreenCast` session.
    #[zbus(name = "Start")]
    #[expect(
        clippy::too_many_arguments,
        reason = "D-Bus method signature requires all parameters"
    )]
    async fn start(
        &self,
        handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        app_id: &str,
        parent_window: &str,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let _ = (parent_window, options);
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing sender".to_string()))?
            .to_string();

        tracing::debug!(
            handle = %handle,
            session_handle = %session_handle,
            app_id = %app_id,
            "ScreenCast.Start called"
        );

        // Register Request object at handle path for cancellation support
        let request_iface = super::RequestInterface::for_session(
            Arc::clone(&self.session_manager),
            session_handle.to_string(),
        );
        let _ = server.at(&handle, request_iface).await;

        let result = async {
            let mut manager = self.session_manager.lock().await;
            manager.validate_session(&session_handle, app_id, &sender)?;

            let session = manager
                .get_session_mut(&session_handle)
                .ok_or_else(|| PortalError::SessionNotFound(session_handle.to_string()))?;

            if !session.sources_selected {
                return Err(PortalError::InvalidState {
                    expected: "Sources selected".to_string(),
                    actual: "No sources selected".to_string(),
                }
                .into());
            }

            let cursor_mode = session.cursor_mode;
            let sources = session.sources.clone();
            let persist_mode = session.persist_mode;

            // Create capture streams via capture backend
            let mut backend = self.capture_backend.lock().await;
            let streams = backend
                .create_capture_session(&sources, cursor_mode)
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;

            drop(backend);

            // Start the session with streams
            session.start(streams.clone())?;
            let (stop, cancelled) = tokio::sync::watch::channel(false);
            session.sharing_stop = Some(stop);
            crate::picker::sharing(
                crate::picker::request(
                    app_id,
                    session_handle.as_str(),
                    knave_portal_api::Operation::Sharing,
                    false,
                    &sources,
                ),
                Arc::clone(&self.session_manager),
                Arc::clone(&self.capture_backend),
                Arc::clone(&self.pipewire_manager),
                connection.clone(),
                cancelled,
            );

            let mut results = Self::build_stream_results(&streams);

            // If persist_mode is set, generate and return restore_data
            if persist_mode != PersistMode::None {
                let output_names: Vec<String> = sources.iter().map(|s| s.name.clone()).collect();
                // Build restore_data as (suv): ("knave", 1, variant(as))
                let names_value = Value::from(output_names);
                let rd_tuple = Value::from((crate::RESTORE_DATA_VENDOR, 1u32, names_value));
                if let Ok(rd_owned) = OwnedValue::try_from(rd_tuple) {
                    results.insert("restore_data".to_string(), rd_owned);
                }
                results.insert(
                    "persist_mode".to_string(),
                    OwnedValue::from(persist_mode.to_dbus()),
                );
            }

            tracing::info!(
                session_id = %session_handle,
                stream_count = streams.len(),
                persist = ?persist_mode,
                "ScreenCast session started"
            );

            Ok((Response::Success.to_u32(), results))
        }
        .await;
        let _ = server.remove::<super::RequestInterface, _>(&handle).await;
        result
    }

    // OpenPipeWireRemote belongs to the frontend, which grants restricted node
    // permissions. It is deliberately not a backend interface method.

    // === Properties ===

    /// Available source types.
    #[zbus(property, name = "AvailableSourceTypes")]
    async fn available_source_types(&self) -> u32 {
        let backend = self.capture_backend.lock().await;
        backend.available_source_types()
    }

    /// Available cursor modes.
    #[zbus(property, name = "AvailableCursorModes")]
    async fn available_cursor_modes(&self) -> u32 {
        let backend = self.capture_backend.lock().await;
        backend.available_cursor_modes()
    }

    /// Interface version.
    #[zbus(property, name = "version")]
    #[expect(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "zbus interface requires async"
    )]
    async fn version(&self) -> u32 {
        6
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restore_vendors_remain_backward_readable() {
        assert!(is_supported_restore_vendor("knave"));
        assert!(is_supported_restore_vendor("generic"));
        assert!(!is_supported_restore_vendor("other"));
    }
}
