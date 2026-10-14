//! Capture source discovery on the Wayland owner thread.
use super::dispatch::WaylandState;
use crate::types::{CaptureTarget, SourceInfo, SourceType};
use knave_capture_protocol::client::knave_workspace_capture_manager_v1::{
    self as workspace, KnaveWorkspaceCaptureManagerV1,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, globals::GlobalList};
use wayland_protocols::ext::{
    foreign_toplevel_list::v1::client::{
        ext_foreign_toplevel_handle_v1::{self as handle, ExtForeignToplevelHandleV1},
        ext_foreign_toplevel_list_v1::{self as list, ExtForeignToplevelListV1},
    },
    image_capture_source::v1::client::{
        ext_foreign_toplevel_image_capture_source_manager_v1::{
            self as windows, ExtForeignToplevelImageCaptureSourceManagerV1,
        },
        ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    },
};
const WINDOW_BASE: u32 = 0x2000_0000;
const WORKSPACE_BASE: u32 = 0x4000_0000;

pub(crate) struct WindowSource {
    pub(crate) id: u32,
    pub(crate) handle: ExtForeignToplevelHandleV1,
    title: String,
    app_id: String,
    done: bool,
}
#[derive(Default)]
pub(crate) struct SourceCatalogue {
    pub(crate) windows_manager: Option<ExtForeignToplevelImageCaptureSourceManagerV1>,
    pub(crate) list: Option<ExtForeignToplevelListV1>,
    pub(crate) workspaces_manager: Option<KnaveWorkspaceCaptureManagerV1>,
    pub(crate) workspace_count: u32,
    pub(crate) windows: Vec<WindowSource>,
    next_window: u32,
}
impl SourceCatalogue {
    pub(crate) fn bind(&mut self, globals: &GlobalList, qh: &QueueHandle<WaylandState>) {
        self.windows_manager = globals.bind(qh, 1..=1, ()).ok();
        if self.windows_manager.is_some() {
            self.list = globals.bind(qh, 1..=1, ()).ok();
        }
        self.workspaces_manager = globals.bind(qh, 1..=1, ()).ok();
    }
    pub(crate) fn sources(&self, monitor: &SourceInfo) -> Vec<SourceInfo> {
        let mut result = Vec::new();
        if self.windows_manager.is_some() && self.list.is_some() {
            for window in self.windows.iter().filter(|window| window.done).take(48) {
                result.push(SourceInfo {
                    id: window.id,
                    target: CaptureTarget::Window {
                        handle_id: window.id,
                    },
                    name: if window.title.is_empty() && window.app_id.is_empty() {
                        "Untitled window".into()
                    } else if window.title.is_empty() {
                        window.app_id.chars().take(60).collect::<String>()
                    } else {
                        window.title.chars().take(60).collect()
                    },
                    description: window.app_id.chars().take(240).collect(),
                    source_type: SourceType::Window,
                    ..monitor.clone()
                });
            }
        }
        if self.workspaces_manager.is_some() {
            for workspace in 1..=self.workspace_count.min(10) {
                result.push(SourceInfo {
                    id: WORKSPACE_BASE + workspace,
                    target: CaptureTarget::Workspace {
                        output_id: monitor.id,
                        workspace,
                    },
                    name: format!("Workspace {workspace}"),
                    description: "Sharing freezes when you visit an unselected workspace".into(),
                    ..monitor.clone()
                });
            }
        }
        result
    }
    pub(crate) fn create_source(
        &self,
        qh: &QueueHandle<WaylandState>,
        target: CaptureTarget,
        output: &wayland_client::protocol::wl_output::WlOutput,
        data: u32,
    ) -> Result<ExtImageCaptureSourceV1, String> {
        match target {
            CaptureTarget::Window { handle_id } => {
                let window = self
                    .windows
                    .iter()
                    .find(|window| window.id == handle_id)
                    .ok_or("selected window closed")?;
                Ok(self
                    .windows_manager
                    .as_ref()
                    .ok_or("window capture unavailable")?
                    .create_source(&window.handle, qh, data))
            }
            CaptureTarget::Workspace { workspace, .. } => {
                if !(1..=self.workspace_count).contains(&workspace) {
                    return Err("selected workspace unavailable".into());
                }
                Ok(self
                    .workspaces_manager
                    .as_ref()
                    .ok_or("workspace capture unavailable")?
                    .create_source(output, 1 << (workspace - 1), qh, data))
            }
            CaptureTarget::WorkspaceSet { allowed, .. } => {
                let count = self.workspace_count.min(32);
                if count == 0 || allowed == 0 || (count < 32 && allowed & !((1 << count) - 1) != 0)
                {
                    return Err("selected workspace set unavailable".into());
                }
                Ok(self
                    .workspaces_manager
                    .as_ref()
                    .ok_or("workspace capture unavailable")?
                    .create_source(output, allowed, qh, data))
            }
            CaptureTarget::Monitor => Err("monitor sources use the standard output manager".into()),
        }
    }
}
impl Dispatch<ExtForeignToplevelListV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &ExtForeignToplevelListV1,
        event: list::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let list::Event::Toplevel { toplevel } = event {
            if state.catalogue.windows.len() >= 64 {
                toplevel.destroy();
                return;
            }
            let Some(next) = state
                .catalogue
                .next_window
                .checked_add(1)
                .filter(|id| *id < 0x1000_0000)
            else {
                toplevel.destroy();
                return;
            };
            state.catalogue.next_window = next;
            state.catalogue.windows.push(WindowSource {
                id: WINDOW_BASE + next,
                handle: toplevel,
                title: String::new(),
                app_id: String::new(),
                done: false,
            });
        }
    }
    wayland_client::event_created_child!(WaylandState, ExtForeignToplevelListV1, [0 => (ExtForeignToplevelHandleV1, ())]);
}
impl Dispatch<ExtForeignToplevelHandleV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        proxy: &ExtForeignToplevelHandleV1,
        event: handle::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let handle::Event::Closed = event {
            state
                .catalogue
                .windows
                .retain(|window| window.handle.id() != proxy.id());
            proxy.destroy();
            return;
        }
        let Some(window) = state
            .catalogue
            .windows
            .iter_mut()
            .find(|window| window.handle.id() == proxy.id())
        else {
            return;
        };
        match event {
            handle::Event::Title { title } => window.title = title.chars().take(256).collect(),
            handle::Event::AppId { app_id } => window.app_id = app_id.chars().take(256).collect(),
            handle::Event::Done => window.done = true,
            _ => {}
        }
    }
}
impl Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &ExtForeignToplevelImageCaptureSourceManagerV1,
        _: windows::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<KnaveWorkspaceCaptureManagerV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &KnaveWorkspaceCaptureManagerV1,
        event: workspace::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let workspace::Event::WorkspaceCount { count } = event {
            state.catalogue.workspace_count = count.min(32);
        }
    }
}
