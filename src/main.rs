//! XDG Desktop Portal backend for Wayland compositors.
//!
//! Standalone D-Bus service that connects to the compositor as a Wayland client.

use std::sync::Arc;

use anyhow::Result;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use xdg_desktop_portal_knave::{
    PortalBackend,
    pipewire::PipeWireManager,
    services::{
        capture::{CapturePreference, create_capture_backend},
        clipboard::{ClipboardPreference, create_clipboard_backend},
        input::{InputBackendConfig, create_input_backend},
    },
    wayland::WaylandConnection,
};

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(
            EnvFilter::from_default_env().add_directive("xdg_desktop_portal_knave=debug".parse()?),
        )
        .init();

    tracing::info!("Starting xdg-desktop-portal-knave");
    if std::env::var("XDP_KNAVE_ENABLED").as_deref() == Ok("0") {
        anyhow::bail!("Knave portal backend is disabled for this session");
    }

    // Connect to compositor as a Wayland client
    let mut wayland = WaylandConnection::connect()?;
    let disconnected = wayland.disconnected();
    let protocols = wayland.available_protocols().clone();
    let sources = wayland.state().get_sources();
    tracing::info!("Discovered {} output sources", sources.len());

    if !protocols.ext_image_copy_capture && !protocols.wlr_screencopy {
        tracing::info!("Capture unavailable; serving Settings independently");
        return xdg_desktop_portal_knave::run_settings_service().await;
    }

    // Start PipeWire manager on a dedicated thread.
    // PipeWire starts BEFORE the Wayland event loop because the event loop
    // needs a PipeWire reference to deliver captured frames directly.
    let pipewire_manager = Arc::new(PipeWireManager::start()?);

    // Standalone mode: use env-based preferences (no server to pass hints)
    let capture_prefs = CapturePreference::from_env();

    // Configure ext-capture handshake timeout before spawning event loop
    if capture_prefs.handshake_timeout_ms > 0 {
        wayland.set_ext_capture_handshake_timeout(std::time::Duration::from_millis(
            capture_prefs.handshake_timeout_ms,
        ));
    }

    // InputCapture barrier-surface command channel, created before the event
    // loop spawns -- the sender is only valid once wired in, and the
    // WaylandConnection::spawn_event_loop* tuple shape can't grow another
    // slot without breaking positionally-destructuring downstream consumers.
    let input_capture_tx = wayland.create_input_capture_channel();

    // Same pre-spawn-only constraint: InputCapture activation-lifecycle
    // events (barrier lock/unlock, relative motion) flow out of the
    // Wayland thread over this channel.
    let (input_capture_activation_tx, input_capture_activation_rx) =
        tokio::sync::mpsc::unbounded_channel();
    wayland.set_input_capture_activation_sender(input_capture_activation_tx);

    // Spawn the Wayland event loop on a dedicated thread.
    // This continuously dispatches Wayland events (screencopy frames,
    // output hotplug, data control) and updates the shared state.
    // The PipeWire manager is given to the event loop for frame delivery.
    let (wayland_stop, shared_wayland_state, capture_tx, wayland_thread) =
        wayland.spawn_event_loop(Arc::clone(&pipewire_manager));

    // Create backends based on detected protocols
    let input_config = InputBackendConfig::from_env();
    let mut input_backend =
        create_input_backend(&input_config, &protocols).unwrap_or_else(|error| {
            tracing::info!(%error, "Remote control unavailable; serving capture only");
            Box::new(xdg_desktop_portal_knave::services::input::UnavailableInput)
        });
    input_backend.set_shared_wayland_state(shared_wayland_state.clone());

    // Clone capture_tx before passing to backend — Screenshot needs its own sender
    let screenshot_capture_tx = capture_tx.clone();

    let capture_backend = create_capture_backend(
        &protocols,
        &capture_prefs,
        sources,
        Arc::clone(&pipewire_manager),
        capture_tx,
    )?;

    let clipboard_prefs = ClipboardPreference::from_env();
    let clipboard_backend = create_clipboard_backend(&protocols, &clipboard_prefs);

    // Create and run the portal backend
    let mut backend = PortalBackend::new(
        input_backend,
        capture_backend,
        clipboard_backend,
        Arc::clone(&pipewire_manager),
        protocols,
        screenshot_capture_tx,
    );
    backend.set_shared_wayland_state(shared_wayland_state);
    backend.set_input_capture_channel(input_capture_tx);
    backend.set_input_capture_activation_receiver(input_capture_activation_rx);

    tracing::info!("Registering D-Bus interfaces...");
    let pipewire_stopped = pipewire_manager.stopped();
    let result = tokio::select! {
        result = backend.run() => result,
        () = disconnected.notified() => { tracing::info!("compositor disconnected; stopping portal"); Ok(()) },
        () = pipewire_stopped.notified() => Err(anyhow::anyhow!("PipeWire worker stopped")),
        _ = tokio::signal::ctrl_c() => Ok(()),
        () = async {
            if let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                signal.recv().await;
            } else { std::future::pending::<()>().await; }
        } => Ok(()),
    };

    let manager = backend.session_manager();
    let mut manager = manager.lock().await;
    let handles: Vec<_> = manager.sessions().map(|s| s.id.clone()).collect();
    for handle in handles {
        manager.close_session(&handle);
    }
    drop(manager);
    // Clean shutdown
    wayland_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    pipewire_manager.shutdown();
    let _ = wayland_thread.join();

    result
}
