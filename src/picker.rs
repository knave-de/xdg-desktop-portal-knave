//! Bounded native consent processes. The backend owns validation and cancellation.
use crate::{
    SourceInfo,
    wayland::{CaptureCommand, ScreenshotData},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use knave_portal_api::{MAX_REQUEST, MAX_SOURCES, Operation, Reply, Request, Source};
use std::{
    process::Stdio,
    sync::{Arc, mpsc},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{Semaphore, watch},
};
static DIALOG: Semaphore = Semaphore::const_new(1);
fn shell_binary() -> String {
    crate::env::var("SHELL_BINARY").unwrap_or_else(|_| {
        std::env::var("HOME").map_or_else(
            |_| "knave-shell".into(),
            |home| format!("{home}/.local/bin/knave-shell"),
        )
    })
}
pub(crate) fn request(
    app_id: &str,
    id: &str,
    operation: Operation,
    multiple: bool,
    sources: &[SourceInfo],
) -> Request {
    Request {
        request_id: id.into(),
        app_id: app_id.into(),
        operation,
        multiple,
        sources: sources
            .iter()
            .flat_map(|s| {
                if operation == Operation::Sharing
                    && let crate::types::CaptureTarget::WorkspaceSet { allowed, .. } = s.target
                {
                    return (1..=32)
                        .filter(|workspace| allowed & (1 << (workspace - 1)) != 0)
                        .map(|workspace| Source {
                            id: 0x4000_0000 + workspace,
                            name: format!("Workspace {workspace}"),
                            description: s.description.clone(),
                            width: s.width,
                            height: s.height,
                            preview_png: None,
                            kind: knave_portal_api::SourceKind::Workspace { workspace },
                        })
                        .collect::<Vec<_>>();
                }
                vec![Source {
                    id: s.id,
                    name: s.name.clone(),
                    description: s.description.clone(),
                    width: s.width,
                    height: s.height,
                    preview_png: None,
                    kind: match s.target {
                        crate::types::CaptureTarget::Window { .. } => {
                            knave_portal_api::SourceKind::Window
                        }
                        crate::types::CaptureTarget::Workspace { workspace, .. } => {
                            knave_portal_api::SourceKind::Workspace { workspace }
                        }
                        _ => knave_portal_api::SourceKind::Monitor,
                    },
                }]
            })
            .collect(),
    }
}
async fn invoke(
    request: &Request,
    cancel: &mut watch::Receiver<bool>,
    timeout: Option<Duration>,
) -> Result<Vec<u32>, String> {
    request.validate()?;
    let legacy = if request.operation == Operation::Sharing {
        None
    } else {
        crate::env::var("SOURCE_PICKER").ok()
    };
    let payload = if legacy.is_some() {
        request
            .sources
            .iter()
            .map(|s| format!("{}\t{}\t{}x{}", s.name, s.description, s.width, s.height))
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes()
    } else {
        serde_json::to_vec(request).map_err(|e| e.to_string())?
    };
    if payload.len() > MAX_REQUEST {
        return Err("picker payload exceeds budget".into());
    }
    let mut command = if let Some(tool) = &legacy {
        let mut parts = tool.split_whitespace();
        let mut command = Command::new(parts.next().ok_or("empty source picker command")?);
        command.args(parts);
        command
    } else {
        let mut command = Command::new(shell_binary());
        command.arg("portal-picker");
        command
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not start native portal UI: {e}"))?;
    let operation = async {
        let mut stdin = child.stdin.take().ok_or("missing picker stdin")?;
        stdin.write_all(&payload).await.map_err(|e| e.to_string())?;
        drop(stdin);
        let stdout = child.stdout.take().ok_or("missing picker stdout")?;
        let mut output = Vec::new();
        stdout
            .take(4097)
            .read_to_end(&mut output)
            .await
            .map_err(|e| e.to_string())?;
        if output.len() > 4096 {
            return Err("picker reply exceeds budget".into());
        }
        let status = child.wait().await.map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("portal UI exited with {status}"));
        }
        if output.is_empty() {
            return Ok(Vec::new());
        }
        let reply: Reply = if legacy.is_some() {
            let names = std::str::from_utf8(&output).map_err(|e| e.to_string())?;
            let mut selected = Vec::new();
            for name in names
                .lines()
                .map(|s| s.split('\t').next().unwrap_or("").trim())
                .filter(|s| !s.is_empty())
            {
                selected.push(
                    request
                        .sources
                        .iter()
                        .find(|s| s.name == name)
                        .ok_or("picker returned unknown source")?
                        .id,
                );
            }
            Reply {
                request_id: request.request_id.clone(),
                selected,
            }
        } else {
            serde_json::from_slice(&output).map_err(|e| e.to_string())?
        };
        reply.validate(request)?;
        Ok(reply.selected)
    };
    let result = tokio::select! {
        result = operation => result,
        _ = cancel.changed() => Ok(Vec::new()),
        () = async { if let Some(duration) = timeout { tokio::time::sleep(duration).await; } else { std::future::pending::<()>().await; } } => Err("portal UI timed out".into()),
    };
    // Also reap failed/cancelled processes; kill_on_drop covers task abort.
    if child.id().is_some() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}
pub(crate) async fn select(
    app_id: &str,
    id: &str,
    operation: Operation,
    multiple: bool,
    sources: &[SourceInfo],
    tx: &mpsc::Sender<CaptureCommand>,
    mut cancel: watch::Receiver<bool>,
) -> Result<Vec<SourceInfo>, String> {
    if *cancel.borrow() {
        return Ok(Vec::new());
    }
    let _permit = DIALOG
        .try_acquire()
        .map_err(|_| "another portal dialog is active")?;
    let monitor_only =
        operation == Operation::Screenshot || crate::env::var("SOURCE_PICKER").is_ok();
    let mut sources: Vec<_> = sources
        .iter()
        .filter(|source| !monitor_only || source.target == crate::types::CaptureTarget::Monitor)
        .cloned()
        .collect();
    if sources.len() > MAX_SOURCES {
        return Err("too many capture sources".into());
    }
    let mut request = request(app_id, id, operation, multiple, &sources);
    request.validate()?;
    let mut preview_bytes = 0;
    let mut unavailable = Vec::new();
    for (index, source) in request.sources.iter_mut().enumerate() {
        // Workspace choices are checkboxes; capture no workspace preview before consent.
        if matches!(source.kind, knave_portal_api::SourceKind::Workspace { .. }) {
            continue;
        }
        let (reply, rx) = tokio::sync::oneshot::channel();
        tx.send(CaptureCommand::CaptureScreenshot {
            target: sources[index].target,
            output_global_name: source.id,
            reply,
        })
        .map_err(|e| e.to_string())?;
        let captured = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(2), rx) => result,
            _ = cancel.changed() => return Ok(Vec::new()),
        };
        if let Ok(Ok(Ok(frame))) = captured {
            source.width = frame.width;
            source.height = frame.height;
            sources[index].width = frame.width;
            sources[index].height = frame.height;
            if let Ok(preview) = thumbnail(&frame) {
                if preview.len() <= 192 * 1024 && preview_bytes + preview.len() <= MAX_REQUEST / 2 {
                    preview_bytes += preview.len();
                    source.preview_png = Some(preview);
                }
            }
        } else if source.kind == knave_portal_api::SourceKind::Window {
            unavailable.push(source.id);
        }
    }
    request
        .sources
        .retain(|source| !unavailable.contains(&source.id));
    sources.retain(|source| !unavailable.contains(&source.id));
    let selected = invoke(&request, &mut cancel, Some(Duration::from_secs(120))).await?;
    let selected: Vec<_> = selected
        .iter()
        .filter_map(|id| sources.iter().find(|s| s.id == *id).cloned())
        .collect();
    if selected.iter().any(|source: &SourceInfo| {
        matches!(source.target, crate::types::CaptureTarget::Workspace { .. })
    }) {
        let mut allowed = 0;
        let mut output = None;
        for source in &selected {
            let crate::types::CaptureTarget::Workspace {
                output_id,
                workspace,
            } = source.target
            else {
                return Err("cannot mix workspace and other capture modes".into());
            };
            if output.is_some_and(|existing| existing != output_id) {
                return Err("workspace sets require one output".into());
            }
            output = Some(output_id);
            allowed |= 1 << (workspace - 1);
        }
        let mut source = selected[0].clone();
        source.id = 0x7000_0000 | allowed;
        source.target = crate::types::CaptureTarget::WorkspaceSet {
            output_id: output.ok_or("no workspace output")?,
            allowed,
        };
        source.name = format!(
            "Workspaces {}",
            (1..=32)
                .filter(|workspace| allowed & (1 << (workspace - 1)) != 0)
                .map(|workspace| workspace.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Ok(vec![source]);
    }
    Ok(selected)
}
pub(crate) fn thumbnail(frame: &ScreenshotData) -> Result<String, String> {
    if frame.width == 0
        || frame.height == 0
        || u64::from(frame.width) * u64::from(frame.height) > 8 * 1024 * 1024
    {
        return Err("preview frame exceeds capture budget".into());
    }
    let rgba = crate::dbus::screenshot::convert_bgrx_to_rgba(
        &frame.data,
        frame.width,
        frame.height,
        frame.stride,
        frame.format_raw,
    );
    // Leave room for base64 and PNG overhead even for incompressible pixels.
    let width = frame
        .width
        .min(240)
        .min((u64::from(frame.width) * 135 / u64::from(frame.height)).max(1) as u32);
    let height = ((u64::from(frame.height) * u64::from(width) / u64::from(frame.width.max(1)))
        as u32)
        .clamp(1, 135);
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let offset = (((y * frame.height / height) * frame.width + x * frame.width / width) * 4)
                as usize;
            pixels.extend_from_slice(
                rgba.get(offset..offset + 4)
                    .ok_or("invalid preview pixels")?,
            );
        }
    }
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&pixels)
            .map_err(|e| e.to_string())?;
    }
    Ok(STANDARD.encode(png))
}
pub(crate) fn sharing(
    request: Request,
    manager: Arc<tokio::sync::Mutex<crate::SessionManager>>,
    capture: Arc<tokio::sync::Mutex<Box<dyn crate::CaptureBackend>>>,
    pipewire: Arc<crate::pipewire::PipeWireManager>,
    connection: zbus::Connection,
    mut cancel: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        let id = request.request_id.clone();
        if let Err(error) = invoke(&request, &mut cancel, None).await {
            tracing::warn!(%error, "sharing control failed; stopping capture");
        }
        let Ok(handle) = zbus::zvariant::ObjectPath::try_from(id.as_str()) else {
            return;
        };
        let closed = manager.lock().await.close_session(&handle);
        if let Some(session) = closed {
            let streams = session.stream_ids();
            let _ = capture.lock().await.destroy_capture_session(&streams);
            for node in streams {
                let _ = pipewire.destroy_stream(node).await;
            }
            if let Ok(emitter) =
                zbus::object_server::SignalEmitter::new(&connection, handle.clone())
            {
                let _ = connection
                    .emit_signal(
                        None::<&str>,
                        emitter.path(),
                        "org.freedesktop.impl.portal.Session",
                        "Closed",
                        &(),
                    )
                    .await;
            }
            let _ = connection
                .object_server()
                .remove::<crate::dbus::SessionInterface, _>(&handle)
                .await;
        }
    });
}
