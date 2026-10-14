//! Reject an invalid workspace mask on an isolated compositor connection.
//! Run only on the private desktop harness, never on the normal user session.
use knave_capture_protocol::client::knave_workspace_capture_manager_v1::{
    self as workspace, KnaveWorkspaceCaptureManagerV1,
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_output::WlOutput, wl_registry::WlRegistry},
};
use wayland_protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::ExtImageCaptureSourceV1;

#[derive(Default)]
struct Probe {
    count: u32,
}
impl Dispatch<WlRegistry, GlobalListContents> for Probe {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<KnaveWorkspaceCaptureManagerV1, ()> for Probe {
    fn event(
        state: &mut Self,
        _: &KnaveWorkspaceCaptureManagerV1,
        event: workspace::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let workspace::Event::WorkspaceCount { count } = event {
            state.count = count;
        }
    }
}
delegate_noop!(Probe: ignore WlOutput);
delegate_noop!(Probe: ignore ExtImageCaptureSourceV1);
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mask: u32 = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "0".into())
        .parse()?;
    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<Probe>(&connection)?;
    let qh = queue.handle();
    let manager: KnaveWorkspaceCaptureManagerV1 = globals.bind(&qh, 1..=1, ())?;
    let output: WlOutput = globals.bind(&qh, 1..=4, ())?;
    let mut state = Probe::default();
    queue.roundtrip(&mut state)?;
    if state.count == 0 || state.count > 32 {
        return Err("invalid compositor workspace count".into());
    }
    let valid = mask != 0 && (state.count == 32 || mask & !((1 << state.count) - 1) == 0);
    if valid {
        return Err("probe requires an invalid mask".into());
    }
    let _source = manager.create_source(&output, mask, &qh, ());
    if queue.roundtrip(&mut state).is_ok() {
        return Err("compositor accepted an invalid workspace mask".into());
    }
    println!("Invalid mask {mask} rejected; reconnect separately to verify compositor survival");
    Ok(())
}
