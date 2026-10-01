//! Run only inside an isolated test compositor and D-Bus session.
use futures::StreamExt;
use std::{collections::HashMap, sync::Arc, time::Duration};
use xdg_desktop_portal_knave::{
    pipewire::PipeWireManager,
    wayland::{CaptureCommand, WaylandConnection},
};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};
#[tokio::main(worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("capture") => capture().await,
        Some("frontend") => frontend(false).await,
        Some("stop") => frontend(true).await,
        Some("native") => frontend_screenshot(false).await,
        Some("cancel") => frontend_screenshot(true).await,
        _ => anyhow::bail!("usage: knave_smoke capture|frontend|native"),
    }
}
async fn capture() -> anyhow::Result<()> {
    let wayland = WaylandConnection::connect()?;
    anyhow::ensure!(
        wayland.available_protocols().ext_image_copy_capture,
        "ext capture missing"
    );
    let sources = wayland.state().get_sources();
    let source = sources
        .first()
        .ok_or_else(|| anyhow::anyhow!("no output"))?;
    let pipewire = Arc::new(PipeWireManager::start()?);
    let (stop, _, tx, thread) = wayland.spawn_event_loop(Arc::clone(&pipewire));
    let (reply, result) = tokio::sync::oneshot::channel();
    tx.send(CaptureCommand::CaptureScreenshot {
        output_global_name: source.id,
        reply,
    })?;
    let frame = tokio::time::timeout(Duration::from_secs(5), result)
        .await??
        .map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        frame.data.len() >= (frame.width * frame.height * 4) as usize,
        "short capture"
    );
    anyhow::ensure!(frame.data.iter().any(|v| *v != 0), "empty capture");
    let path = std::env::var("KNAVE_SMOKE_PNG").unwrap_or_else(|_| "/tmp/knave-capture.png".into());
    let mut encoder = png::Encoder::new(std::fs::File::create(&path)?, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&frame.data)?;
    println!(
        "ext capture: {}x{}, {} bytes, {}",
        frame.width,
        frame.height,
        frame.data.len(),
        path
    );
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    pipewire.shutdown();
    thread
        .join()
        .map_err(|_| anyhow::anyhow!("capture thread panicked"))?;
    Ok(())
}
async fn response(
    stream: &mut zbus::MessageStream,
    handle: &ObjectPath<'_>,
    expected: u32,
) -> anyhow::Result<HashMap<String, OwnedValue>> {
    tokio::time::timeout(Duration::from_secs(150), async {
        while let Some(message) = stream.next().await {
            let message = message?;
            if message.header().path().is_some_and(|p| p == handle) {
                let (code, results): (u32, HashMap<String, OwnedValue>) =
                    message.body().deserialize()?;
                anyhow::ensure!(code == expected, "portal request failed with code {code}");
                return Ok(results);
            }
        }
        anyhow::bail!("response stream ended")
    })
    .await?
}
async fn stream(connection: &zbus::Connection) -> anyhow::Result<zbus::MessageStream> {
    let rule = zbus::MatchRule::builder()
        .interface("org.freedesktop.portal.Request")?
        .member("Response")?
        .build();
    Ok(zbus::MessageStream::for_match_rule(rule, connection, Some(8)).await?)
}
async fn frontend_screenshot(cancel: bool) -> anyhow::Result<()> {
    let conn = zbus::Connection::session().await?;
    let proxy = zbus::Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Screenshot",
    )
    .await?;
    let mut replies = stream(&conn).await?;
    let handle: OwnedObjectPath = proxy
        .call(
            "Screenshot",
            &(
                "",
                HashMap::from([("interactive".to_string(), OwnedValue::from(true))]),
            ),
        )
        .await?;
    if cancel {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let request = zbus::Proxy::new(
            &conn,
            "org.freedesktop.portal.Desktop",
            &handle,
            "org.freedesktop.portal.Request",
        )
        .await?;
        request.call::<_, _, ()>("Close", &()).await?;
        // Closing a frontend request suppresses its Response signal.
        tokio::time::sleep(Duration::from_millis(500)).await;
        println!("frontend request cancelled");
        return Ok(());
    }
    let results = response(&mut replies, &handle, 0).await?;
    let uri = String::try_from(
        results
            .get("uri")
            .ok_or_else(|| anyhow::anyhow!("missing URI"))?
            .try_clone()?,
    )?;
    anyhow::ensure!(
        std::path::Path::new(uri.trim_start_matches("file://")).is_file(),
        "missing screenshot file"
    );
    println!("frontend screenshot: {uri}");
    Ok(())
}
async fn frontend(stop: bool) -> anyhow::Result<()> {
    frontend_screenshot(false).await?;
    let conn = zbus::Connection::session().await?;
    let proxy = zbus::Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.ScreenCast",
    )
    .await?;
    let closed_rule = zbus::MatchRule::builder()
        .interface("org.freedesktop.portal.Session")?
        .member("Closed")?
        .build();
    let mut closed = zbus::MessageStream::for_match_rule(closed_rule, &conn, Some(4)).await?;
    let mut replies = stream(&conn).await?;
    let handle: OwnedObjectPath = proxy
        .call(
            "CreateSession",
            &(HashMap::from([(
                "session_handle_token".to_string(),
                OwnedValue::try_from(zbus::zvariant::Value::from("knave_smoke"))?,
            )]),),
        )
        .await?;
    let results = response(&mut replies, &handle, 0).await?;
    let session = OwnedObjectPath::try_from(String::try_from(
        results
            .get("session_handle")
            .ok_or_else(|| anyhow::anyhow!("missing session"))?
            .try_clone()?,
    )?)?;
    let mut options = HashMap::new();
    options.insert("types".to_string(), OwnedValue::from(1u32));
    let handle: OwnedObjectPath = proxy.call("SelectSources", &(&session, options)).await?;
    response(&mut replies, &handle, 0).await?;
    let handle: OwnedObjectPath = proxy
        .call(
            "Start",
            &(&session, "", HashMap::<String, OwnedValue>::new()),
        )
        .await?;
    let results = response(&mut replies, &handle, 0).await?;
    let streams = Vec::<(u32, HashMap<String, OwnedValue>)>::try_from(
        results
            .get("streams")
            .ok_or_else(|| anyhow::anyhow!("missing streams"))?
            .try_clone()?,
    )?;
    anyhow::ensure!(!streams.is_empty(), "no PipeWire streams");
    let remote: zbus::zvariant::OwnedFd = proxy
        .call(
            "OpenPipeWireRemote",
            &(&session, HashMap::<String, OwnedValue>::new()),
        )
        .await?;
    use std::os::fd::AsRawFd;
    println!(
        "frontend screencast: node {}, remote fd {}",
        streams[0].0,
        remote.as_raw_fd()
    );
    if let Ok(hold) = std::env::var("KNAVE_SMOKE_HOLD") {
        tokio::time::sleep(Duration::from_secs(hold.parse()?)).await;
    }
    let fd: std::os::fd::OwnedFd = remote.into();
    let node = streams[0].0;
    tokio::task::spawn_blocking(move || consume(node, fd)).await??;
    if stop {
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(message) = closed.next().await {
                if message?
                    .header()
                    .path()
                    .is_some_and(|p| p.as_str() == session.as_str())
                {
                    return Ok::<_, anyhow::Error>(());
                }
            }
            anyhow::bail!("session signal stream ended")
        })
        .await??;
        println!("frontend session closed after sharing control failure");
        return Ok(());
    }
    let close = zbus::Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        &session,
        "org.freedesktop.portal.Session",
    )
    .await?;
    close.call::<_, _, ()>("Close", &()).await?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    println!("frontend session closed");
    Ok(())
}

fn consume(node: u32, fd: std::os::fd::OwnedFd) -> anyhow::Result<()> {
    use pipewire::{self as pw, properties::properties, spa};
    use std::{cell::Cell, rc::Rc};
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_fd_rc(fd, None)?;
    let count = Rc::new(Cell::new(0));
    let received = Rc::clone(&count);
    let done = mainloop.clone();
    let stream = pw::stream::StreamBox::new(
        &core,
        "knave-capture-test",
        properties! {
            "media.type" => "Video", "media.category" => "Capture", "media.role" => "Screen",
            "target.object" => node.to_string(),
        },
    )?;
    let _listener = stream
        .add_local_listener_with_user_data(())
        .state_changed(|_, (), old, new| eprintln!("consumer: {old:?} -> {new:?}"))
        .process(move |stream, ()| {
            if let Some(mut buffer) = stream.dequeue_buffer() {
                if let Some(data) = buffer.datas_mut().first_mut() {
                    let size = data.chunk().size() as usize;
                    if let Some(bytes) = data.data() {
                        if size > 0 && size <= bytes.len() && bytes[..size].iter().any(|v| *v != 0)
                        {
                            received.set(received.get() + 1);
                            if received.get() >= 10 {
                                done.quit();
                            }
                        }
                    }
                }
            }
        })
        .register()?;
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: [
            (
                spa::sys::SPA_FORMAT_mediaType,
                spa::sys::SPA_MEDIA_TYPE_video,
            ),
            (
                spa::sys::SPA_FORMAT_mediaSubtype,
                spa::sys::SPA_MEDIA_SUBTYPE_raw,
            ),
            (
                spa::sys::SPA_FORMAT_VIDEO_format,
                spa::sys::SPA_VIDEO_FORMAT_BGRx,
            ),
        ]
        .into_iter()
        .map(|(key, value)| spa::pod::Property {
            key,
            flags: spa::pod::PropertyFlags::empty(),
            value: spa::pod::Value::Id(spa::utils::Id(value)),
        })
        .collect(),
    };
    let values = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )?
    .0
    .into_inner();
    stream.connect(
        spa::utils::Direction::Input,
        Some(node),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut [spa::pod::Pod::from_bytes(&values)
            .ok_or_else(|| anyhow::anyhow!("invalid video format pod"))?],
    )?;
    let timeout = mainloop.clone();
    let timer = mainloop.loop_().add_timer(move |_| timeout.quit());
    timer
        .update_timer(Some(Duration::from_secs(10)), None)
        .into_result()?;
    mainloop.run();
    anyhow::ensure!(
        count.get() >= 10,
        "PipeWire consumer received only {} frames",
        count.get()
    );
    println!("PipeWire consumer: {} nonempty video frames", count.get());
    Ok(())
}
