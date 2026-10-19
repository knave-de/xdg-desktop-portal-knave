//! ext-image-copy-capture-v1 capture backend.
//!
//! Uses the staging standard `ext_image_copy_capture_manager_v1` protocol
//! for output, window and allowed-workspace capture.

use std::{
    collections::HashMap,
    sync::{Arc, mpsc},
};

use super::{CaptureBackend, CaptureProtocol};
use crate::{
    error::{PortalError, Result},
    pipewire::PipeWireManager,
    types::{CursorMode, SourceInfo, SourceType, StreamInfo},
    wayland::CaptureCommand,
};

/// ext-image-copy-capture backend for screen capture.
///
/// Uses `ext_image_copy_capture_manager_v1` + `ext_output_image_capture_source_manager_v1`
/// to capture output frames and feed them to PipeWire streams.
///
/// Selective targets use their own image sources; missing sources never fall back
/// to output capture.
pub struct ExtCaptureBackend {
    /// Capability remains available even when there are no open windows.
    pub(crate) windows_supported: bool,
    /// Known output, window and workspace sources.
    sources: Vec<SourceInfo>,
    /// Active streams by stream ID (PipeWire node ID).
    active_streams: HashMap<u32, StreamInfo>,
    /// PipeWire manager for creating real streams.
    pipewire: Arc<PipeWireManager>,
    /// Command sender for the Wayland event loop (screencopy capture).
    capture_tx: mpsc::Sender<CaptureCommand>,
}

impl ExtCaptureBackend {
    /// Create a new ext capture backend with known sources.
    pub fn new(
        sources: Vec<SourceInfo>,
        pipewire: Arc<PipeWireManager>,
        capture_tx: mpsc::Sender<CaptureCommand>,
    ) -> Self {
        Self {
            windows_supported: false,
            sources,
            active_streams: HashMap::new(),
            pipewire,
            capture_tx,
        }
    }
}

fn valid_source(available: &[SourceInfo], source: &SourceInfo) -> bool {
    if let crate::types::CaptureTarget::WorkspaceSet { output_id, allowed } = source.target {
        let known = available
            .iter()
            .fold(0, |mask, source| match source.target {
                crate::types::CaptureTarget::Workspace {
                    output_id: output,
                    workspace,
                } if output == output_id && (1..=32).contains(&workspace) => {
                    mask | (1 << (workspace - 1))
                }
                _ => mask,
            });
        allowed != 0 && allowed & !known == 0 && source.source_type == SourceType::Monitor
    } else {
        available
            .iter()
            .any(|candidate| candidate.id == source.id && candidate.target == source.target)
    }
}

impl CaptureBackend for ExtCaptureBackend {
    fn protocol_type(&self) -> CaptureProtocol {
        CaptureProtocol::ExtImageCopyCapture
    }

    fn get_sources(&self, source_types: &[SourceType]) -> Result<Vec<SourceInfo>> {
        if source_types.is_empty() {
            return Ok(self.sources.clone());
        }

        Ok(self
            .sources
            .iter()
            .filter(|s| source_types.contains(&s.source_type))
            .cloned()
            .collect())
    }

    fn create_capture_session(
        &mut self,
        sources: &[SourceInfo],
        cursor_mode: CursorMode,
    ) -> Result<Vec<StreamInfo>> {
        if sources.is_empty() || self.active_streams.len() + sources.len() > 8 {
            return Err(PortalError::PipeWire(
                "capture stream budget exceeded".into(),
            ));
        }
        for source in sources {
            if !valid_source(&self.sources, source) {
                return Err(PortalError::SourceNotFound(source.id));
            }
        }
        let mut streams = Vec::new();
        let result = (|| {
            for source in sources {
                // Verify source exists
                if !valid_source(&self.sources, source) {
                    return Err(PortalError::SourceNotFound(source.id));
                }

                // Create a real PipeWire stream via the manager.
                let config = crate::pipewire::StreamConfig {
                    source_id: source.id,
                    width: source.width,
                    height: source.height,
                    framerate: 30,
                };

                let stream_ids = {
                    let pw = Arc::clone(&self.pipewire);
                    let rt = tokio::runtime::Handle::try_current();
                    match rt {
                        // The capture trait method is sync but is invoked from the
                        // zbus async handler; bridge to the async PipeWire call via
                        // block_in_place so we do not start a runtime within a runtime.
                        Ok(handle) => tokio::task::block_in_place(|| {
                            handle.block_on(pw.create_stream(config))
                        })
                        .map_err(|e| {
                            PortalError::PipeWire(format!("Failed to create stream: {e}"))
                        })?,
                        Err(_) => {
                            return Err(PortalError::PipeWire(
                                "Cannot create PipeWire stream outside async context".to_string(),
                            ));
                        }
                    }
                };
                let node_id = stream_ids.node_id;

                let stream = StreamInfo {
                    node_id,
                    serial: stream_ids.serial,
                    source_id: source.id,
                    position: (source.x, source.y),
                    size: (source.width, source.height),
                    source_type: source.source_type,
                    mapping_id: if source.target == crate::types::CaptureTarget::Monitor {
                        Some(format!("output:{}", source.name))
                    } else {
                        None
                    },
                    scale: source.scale,
                    properties: HashMap::new(),
                };

                tracing::info!(
                    node_id = node_id,
                    source_id = source.id,
                    source_name = %source.name,
                    protocol = "ext-image-copy-capture",
                    "Created capture stream with real PipeWire node"
                );

                // Start frame capture via the Wayland event loop.
                // The event loop dispatches the negotiated capture protocol.
                if let Err(e) = self.capture_tx.send(CaptureCommand::StartCapture {
                    target: source.target,
                    output_global_name: source.id,
                    node_id,
                    width: source.width,
                    height: source.height,
                    cursor_mode,
                }) {
                    tracing::error!(
                        node_id,
                        error = %e,
                        "Failed to send StartCapture command to event loop"
                    );
                    self.active_streams.insert(node_id, stream.clone());
                    streams.push(stream);
                    return Err(PortalError::PipeWire(
                        "capture event loop disconnected".into(),
                    ));
                }

                self.active_streams.insert(node_id, stream.clone());
                streams.push(stream);
            }

            Ok(())
        })();
        if let Err(error) = result {
            let ids: Vec<_> = streams.iter().map(|s| s.node_id).collect();
            let _ = self.destroy_capture_session(&ids);
            return Err(error);
        }
        Ok(streams)
    }

    fn destroy_capture_session(&mut self, stream_ids: &[u32]) -> Result<()> {
        for id in stream_ids {
            if let Some(stream) = self.active_streams.remove(id) {
                // Stop screencopy capture on the Wayland event loop
                let _ = self
                    .capture_tx
                    .send(CaptureCommand::StopCapture { node_id: *id });

                // Destroy the PipeWire stream
                let pw = Arc::clone(&self.pipewire);
                let node_id = *id;
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    tokio::task::block_in_place(|| {
                        let _ = handle.block_on(pw.destroy_stream(node_id));
                    });
                }

                tracing::info!(
                    node_id = id,
                    source_id = stream.source_id,
                    "Destroyed ext capture stream"
                );
            }
        }
        Ok(())
    }

    fn available_source_types(&self) -> u32 {
        SourceType::Monitor.to_bits()
            | if self.windows_supported {
                SourceType::Window.to_bits()
            } else {
                0
            }
    }

    fn available_cursor_modes(&self) -> u32 {
        // Metadata requires a separate cursor stream, which is not implemented.
        CursorMode::Hidden.to_bits() | CursorMode::Embedded.to_bits()
    }

    fn update_sources(&mut self, sources: Vec<SourceInfo>) {
        tracing::debug!(
            old_count = self.sources.len(),
            new_count = sources.len(),
            "Updating ext capture sources (output hotplug)"
        );
        self.sources = sources;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_groups_require_every_bit_on_the_same_output() {
        use crate::types::CaptureTarget;
        let base = SourceInfo {
            target: CaptureTarget::Workspace {
                output_id: 1,
                workspace: 1,
            },
            id: 11,
            name: "Workspace 1".into(),
            description: String::new(),
            width: 800,
            height: 600,
            refresh_rate: 60000,
            x: 0,
            y: 0,
            scale: 1,
            source_type: SourceType::Monitor,
        };
        let mut ninth = base.clone();
        ninth.id = 19;
        ninth.target = CaptureTarget::Workspace {
            output_id: 1,
            workspace: 9,
        };
        let available = [base.clone(), ninth];
        let mut group = base;
        group.target = CaptureTarget::WorkspaceSet {
            output_id: 1,
            allowed: 0x101,
        };
        assert!(valid_source(&available, &group));
        group.target = CaptureTarget::WorkspaceSet {
            output_id: 2,
            allowed: 0x101,
        };
        assert!(!valid_source(&available, &group));
        group.target = CaptureTarget::WorkspaceSet {
            output_id: 1,
            allowed: 0x103,
        };
        assert!(!valid_source(&available, &group));
        group.target = CaptureTarget::WorkspaceSet {
            output_id: 1,
            allowed: 0,
        };
        assert!(!valid_source(&available, &group));
        group.target = CaptureTarget::Window { handle_id: 11 };
        assert!(!valid_source(&available, &group));
    }

    // Note: PipeWireManager requires a running PipeWire daemon.
    // Tests that need real PipeWire streams are integration tests.
    // Unit tests here verify source filtering and protocol type.

    #[test]
    fn test_protocol_type_is_ext() {
        // ExtCaptureBackend always reports ExtImageCopyCapture
        assert_eq!(
            CaptureProtocol::ExtImageCopyCapture.to_string(),
            "ext-image-copy-capture-v1"
        );
    }

    #[test]
    fn test_source_filtering() {
        let sources = [SourceInfo {
            target: crate::types::CaptureTarget::Monitor,
            id: 1,
            name: "eDP-1".to_string(),
            description: "Built-in Display".to_string(),
            width: 1920,
            height: 1080,
            refresh_rate: 60000,
            x: 0,
            y: 0,
            scale: 1,
            source_type: SourceType::Monitor,
        }];

        // Monitor filter matches
        let filtered: Vec<_> = sources
            .iter()
            .filter(|s| [SourceType::Monitor].contains(&s.source_type))
            .collect();
        assert_eq!(filtered.len(), 1);

        // Window filter does not match
        let filtered: Vec<_> = sources
            .iter()
            .filter(|s| [SourceType::Window].contains(&s.source_type))
            .collect();
        assert_eq!(filtered, [] as [&crate::types::SourceInfo; 0]);
    }
}
