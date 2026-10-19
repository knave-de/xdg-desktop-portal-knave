//! Serve only the Settings portal on the current session bus.
//!
//! Isolated preference checks and demos use this to exercise
//! `org.freedesktop.impl.portal.Settings` without a compositor, PipeWire or
//! capture negotiation. Point `KNAVE_CONFIG` at the configuration to serve.

#[tokio::main(worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    xdg_desktop_portal_knave::run_settings_service().await
}
