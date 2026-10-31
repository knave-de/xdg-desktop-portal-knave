//! XDG GlobalShortcuts portal backed by Villain's desktop control contract.

use std::{
    collections::HashMap,
    process::Stdio,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use knave_desktop_api::{DesktopClient, DesktopRequest, DesktopResponse};
use knave_portal_api::{
    MAX_REQUEST, ShortcutAssignment, ShortcutAssignmentReply, ShortcutAssignmentRequest,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{Mutex, SemaphorePermit},
    time::timeout,
};
use zbus::{
    interface,
    object_server::SignalEmitter,
    zvariant::{ObjectPath, OwnedValue, Value},
};

use super::{RequestInterface, Response, empty_results};

#[derive(Clone, Default)]
struct ShortcutState {
    sessions: HashMap<String, ShortcutSession>,
}

#[derive(Clone)]
struct ShortcutSession {
    app_id: String,
    owner: String,
    bound: bool,
    shortcuts: Vec<Shortcut>,
}

#[derive(Clone)]
struct Shortcut {
    id: String,
    description: String,
    trigger: Option<String>,
}

/// XDG GlobalShortcuts portal interface.
#[derive(Clone)]
pub struct GlobalShortcutsInterface {
    state: Arc<Mutex<ShortcutState>>,
}

impl GlobalShortcutsInterface {
    /// Create the interface and its in-memory session registry.
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ShortcutState::default())),
        }
    }

    /// Release all registrations when the backend service shuts down.
    pub async fn clear(&self) {
        let sessions = {
            let mut state = self.state.lock().await;
            std::mem::take(&mut state.sessions)
        };
        for (session_handle, session) in sessions {
            for shortcut in session.shortcuts {
                if shortcut.trigger.is_some() {
                    Self::unregister(session.app_id.clone(), session_handle.clone(), shortcut.id)
                        .await;
                }
            }
        }
    }

    /// Emit activation for a registered shortcut.
    pub async fn emit_activated(
        connection: &zbus::Connection,
        session_handle: &str,
        shortcut_id: &str,
    ) -> zbus::Result<()> {
        let emitter = SignalEmitter::new(connection, "/org/freedesktop/portal/desktop")?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self::activated(
            &emitter,
            ObjectPath::try_from(session_handle)?,
            shortcut_id,
            timestamp,
            HashMap::new(),
        )
        .await
    }

    /// Emit deactivation when the registered key is released.
    pub async fn emit_deactivated(
        connection: &zbus::Connection,
        session_handle: &str,
        shortcut_id: &str,
    ) -> zbus::Result<()> {
        let emitter = SignalEmitter::new(connection, "/org/freedesktop/portal/desktop")?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self::deactivated(
            &emitter,
            ObjectPath::try_from(session_handle)?,
            shortcut_id,
            timestamp,
            HashMap::new(),
        )
        .await
    }

    fn read_string(options: &HashMap<String, OwnedValue>, name: &str) -> Option<String> {
        options
            .get(name)
            .and_then(|value| <String>::try_from(value.clone()).ok())
    }

    fn format_shortcuts(shortcuts: &[Shortcut]) -> Vec<(String, HashMap<String, OwnedValue>)> {
        shortcuts
            .iter()
            .filter_map(|shortcut| Some((shortcut, shortcut.trigger.as_ref()?)))
            .map(|(shortcut, trigger)| {
                let mut values = HashMap::new();
                values.insert(
                    "description".into(),
                    OwnedValue::try_from(Value::from(shortcut.description.clone()))
                        .expect("description is a valid D-Bus string"),
                );
                values.insert(
                    "trigger_description".into(),
                    OwnedValue::try_from(Value::from(trigger.clone()))
                        .expect("trigger is a valid D-Bus string"),
                );
                (shortcut.id.clone(), values)
            })
            .collect()
    }

    fn shortcuts_result(shortcuts: &[Shortcut]) -> HashMap<String, OwnedValue> {
        let mut results = HashMap::new();
        let value = Value::from(Self::format_shortcuts(shortcuts));
        if let Ok(owned) = OwnedValue::try_from(value) {
            results.insert("shortcuts".into(), owned);
        }
        results
    }

    async fn register(
        app_id: &str,
        session_handle: &str,
        shortcut_id: &str,
        trigger: &str,
    ) -> Result<String, String> {
        tokio::task::spawn_blocking({
            let app_id = app_id.to_owned();
            let session_handle = session_handle.to_owned();
            let shortcut_id = shortcut_id.to_owned();
            let trigger = trigger.to_owned();
            move || {
                let mut client = DesktopClient::connect().map_err(|error| error.to_string())?;
                match client
                    .request(&DesktopRequest::RegisterGlobalShortcut {
                        app_id,
                        session_handle,
                        shortcut_id,
                        trigger,
                    })
                    .map_err(|error| error.to_string())?
                {
                    DesktopResponse::GlobalShortcutRegistered { trigger } => Ok(trigger),
                    other => Err(format!(
                        "compositor returned unexpected response: {other:?}"
                    )),
                }
            }
        })
        .await
        .map_err(|error| error.to_string())?
    }

    async fn replace(
        app_id: &str,
        session_handle: &str,
        shortcut_id: &str,
        trigger: &str,
    ) -> Result<String, String> {
        tokio::task::spawn_blocking({
            let app_id = app_id.to_owned();
            let session_handle = session_handle.to_owned();
            let shortcut_id = shortcut_id.to_owned();
            let trigger = trigger.to_owned();
            move || {
                let mut client = DesktopClient::connect().map_err(|error| error.to_string())?;
                match client
                    .request(&DesktopRequest::ReplaceGlobalShortcut {
                        app_id,
                        session_handle,
                        shortcut_id,
                        trigger,
                    })
                    .map_err(|error| error.to_string())?
                {
                    DesktopResponse::GlobalShortcutRegistered { trigger } => Ok(trigger),
                    DesktopResponse::Error(error) => Err(error.message),
                    other => Err(format!(
                        "compositor returned unexpected response: {other:?}"
                    )),
                }
            }
        })
        .await
        .map_err(|error| error.to_string())?
    }

    async fn configure_dialog(
        request: &ShortcutAssignmentRequest,
        _permit: &SemaphorePermit<'_>,
    ) -> Result<Option<(String, String)>, String> {
        request.validate()?;
        let payload = serde_json::to_vec(request).map_err(|error| error.to_string())?;
        if payload.len() > MAX_REQUEST {
            return Err("shortcut assignment request exceeds budget".into());
        }
        let mut child = Command::new(crate::picker::shell_binary())
            .arg("shortcut-assignment")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("could not start shortcut assignment UI: {error}"))?;
        let operation = async {
            let mut stdin = child.stdin.take().ok_or("missing shortcut UI stdin")?;
            stdin
                .write_all(&payload)
                .await
                .map_err(|error| error.to_string())?;
            drop(stdin);
            let stdout = child.stdout.take().ok_or("missing shortcut UI stdout")?;
            let mut output = Vec::new();
            stdout
                .take(4097)
                .read_to_end(&mut output)
                .await
                .map_err(|error| error.to_string())?;
            if output.len() > 4096 {
                return Err("shortcut assignment reply exceeds budget".to_string());
            }
            let status = child.wait().await.map_err(|error| error.to_string())?;
            if !status.success() {
                return Err(format!("shortcut assignment UI exited with {status}"));
            }
            let reply: ShortcutAssignmentReply = serde_json::from_slice(&output)
                .map_err(|error| format!("invalid shortcut assignment reply: {error}"))?;
            reply.validate(request)?;
            Ok(reply.shortcut_id.zip(reply.trigger))
        };
        timeout(Duration::from_secs(120), operation)
            .await
            .map_err(|_| "shortcut assignment timed out".to_string())?
    }

    async fn unregister(app_id: String, session_handle: String, shortcut_id: String) {
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok(mut client) = DesktopClient::connect() {
                let _ = client.request(&DesktopRequest::UnregisterGlobalShortcut {
                    app_id,
                    session_handle,
                    shortcut_id,
                });
            }
        })
        .await;
    }
}

#[interface(
    name = "org.freedesktop.impl.portal.GlobalShortcuts",
    introspection_docs = false
)]
impl GlobalShortcutsInterface {
    #[zbus(name = "CreateSession")]
    async fn create_session(
        &self,
        handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        app_id: &str,
        _options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        if app_id.is_empty() || app_id.len() > 255 {
            return Ok((Response::Other.to_u32(), empty_results()));
        }
        let owner = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing sender".into()))?
            .to_string();
        let key = session_handle.to_string();
        let mut state = self.state.lock().await;
        if state.sessions.len() >= 128 || state.sessions.contains_key(&key) {
            return Ok((Response::Other.to_u32(), empty_results()));
        }
        state.sessions.insert(
            key.clone(),
            ShortcutSession {
                app_id: app_id.to_owned(),
                owner,
                bound: false,
                shortcuts: Vec::new(),
            },
        );
        drop(state);
        let _ = server.at(&handle, RequestInterface::standalone()).await;
        if let Err(error) = server
            .at(
                &session_handle,
                GlobalShortcutSessionInterface {
                    state: Arc::clone(&self.state),
                    session_handle: key.clone(),
                },
            )
            .await
        {
            self.state.lock().await.sessions.remove(&key);
            return Err(error.into());
        }
        let _ = server.remove::<RequestInterface, _>(&handle).await;
        Ok((Response::Success.to_u32(), empty_results()))
    }

    #[zbus(name = "BindShortcuts")]
    async fn bind_shortcuts(
        &self,
        _handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        shortcuts: Vec<(String, HashMap<String, OwnedValue>)>,
        _parent_window: &str,
        _options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let session_key = session_handle.to_string();
        if shortcuts.len() > 64 {
            return Ok((Response::Other.to_u32(), empty_results()));
        }
        let owner = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing sender".into()))?
            .to_string();
        let (app_id, already_bound) = {
            let mut state = self.state.lock().await;
            let Some(session) = state.sessions.get_mut(&session_key) else {
                return Ok((Response::Other.to_u32(), empty_results()));
            };
            if session.owner != owner {
                return Ok((Response::Other.to_u32(), empty_results()));
            }
            let already = session.bound;
            session.bound = true;
            (session.app_id.clone(), already)
        };
        if already_bound {
            return Ok((Response::Other.to_u32(), empty_results()));
        }

        let mut requested = Vec::new();
        for (id, options) in shortcuts {
            if id.is_empty() || id.len() > 128 {
                continue;
            }
            let description = Self::read_string(&options, "description").unwrap_or_default();
            if description.len() > 512 {
                continue;
            }
            let preferred = Self::read_string(&options, "preferred_trigger");
            let trigger = if let Some(preferred) = preferred {
                match Self::register(&app_id, &session_key, &id, &preferred).await {
                    Ok(trigger) => Some(trigger),
                    Err(error) => {
                        tracing::debug!(%error, shortcut_id = %id, "global shortcut was not bound");
                        None
                    }
                }
            } else {
                None
            };
            requested.push(Shortcut {
                id,
                description,
                trigger,
            });
        }
        self.state
            .lock()
            .await
            .sessions
            .get_mut(&session_key)
            .map(|session| session.shortcuts = requested.clone());
        Ok((
            Response::Success.to_u32(),
            Self::shortcuts_result(&requested),
        ))
    }

    #[zbus(name = "ListShortcuts")]
    async fn list_shortcuts(
        &self,
        _handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let owner = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing sender".into()))?
            .to_string();
        let state = self.state.lock().await;
        let Some(session) = state
            .sessions
            .get(session_handle.as_str())
            .filter(|session| session.owner == owner)
        else {
            return Ok((Response::Other.to_u32(), empty_results()));
        };
        Ok((
            Response::Success.to_u32(),
            Self::shortcuts_result(&session.shortcuts),
        ))
    }

    #[zbus(name = "ConfigureShortcuts")]
    async fn configure_shortcuts(
        &self,
        session_handle: ObjectPath<'_>,
        _parent_window: &str,
        _options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        let session_key = session_handle.to_string();
        let owner = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing sender".into()))?
            .to_string();
        let (app_id, shortcuts) = {
            let state = self.state.lock().await;
            let Some(session) = state
                .sessions
                .get(&session_key)
                .filter(|s| s.owner == owner)
            else {
                return Err(zbus::fdo::Error::Failed("unknown shortcut session".into()));
            };
            (session.app_id.clone(), session.shortcuts.clone())
        };
        if shortcuts.is_empty() {
            return Ok(());
        }
        let permit = crate::picker::DIALOG.acquire().await.map_err(|_| {
            zbus::fdo::Error::Failed("shortcut configuration UI is unavailable".into())
        })?;
        let mut request = ShortcutAssignmentRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            app_id: app_id.clone(),
            shortcuts: shortcuts
                .iter()
                .map(|shortcut| ShortcutAssignment {
                    id: shortcut.id.clone(),
                    description: shortcut.description.clone(),
                    trigger: shortcut.trigger.clone(),
                })
                .collect(),
            error: None,
        };
        for _ in 0..3 {
            let Some((shortcut_id, trigger)) = Self::configure_dialog(&request, &permit)
                .await
                .map_err(zbus::fdo::Error::Failed)?
            else {
                return Ok(());
            };
            let was_bound = shortcuts
                .iter()
                .any(|shortcut| shortcut.id == shortcut_id && shortcut.trigger.is_some());
            let updated = if was_bound {
                Self::replace(&app_id, &session_key, &shortcut_id, &trigger).await
            } else {
                Self::register(&app_id, &session_key, &shortcut_id, &trigger).await
            };
            let updated = updated.map_err(|error| {
                request.error = Some(error.chars().take(512).collect());
                zbus::fdo::Error::Failed(
                    "shortcut conflicts with an existing binding; choose another".into(),
                )
            });
            match updated {
                Ok(trigger) => {
                    let mut state = self.state.lock().await;
                    let Some(session) = state
                        .sessions
                        .get_mut(&session_key)
                        .filter(|s| s.owner == owner)
                    else {
                        drop(state);
                        Self::unregister(app_id.clone(), session_key.clone(), shortcut_id).await;
                        return Err(zbus::fdo::Error::Failed(
                            "shortcut session closed during configuration".into(),
                        ));
                    };
                    if let Some(shortcut) =
                        session.shortcuts.iter_mut().find(|s| s.id == shortcut_id)
                    {
                        shortcut.trigger = Some(trigger);
                    } else {
                        return Err(zbus::fdo::Error::Failed(
                            "configured shortcut no longer exists".into(),
                        ));
                    }
                    let changed = Self::format_shortcuts(&session.shortcuts);
                    drop(state);
                    let emitter = SignalEmitter::new(connection, "/org/freedesktop/portal/desktop")
                        .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
                    Self::shortcuts_changed(&emitter, session_handle, changed)
                        .await
                        .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
                    return Ok(());
                }
                Err(_) => continue,
            }
        }
        Err(zbus::fdo::Error::Failed(
            "shortcut configuration failed after repeated conflicts".into(),
        ))
    }

    #[zbus(signal, name = "Activated")]
    async fn activated(
        emitter: &SignalEmitter<'_>,
        session_handle: ObjectPath<'_>,
        shortcut_id: &str,
        timestamp: u64,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::Result<()>;

    #[zbus(signal, name = "Deactivated")]
    async fn deactivated(
        emitter: &SignalEmitter<'_>,
        session_handle: ObjectPath<'_>,
        shortcut_id: &str,
        timestamp: u64,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::Result<()>;

    #[zbus(signal, name = "ShortcutsChanged")]
    async fn shortcuts_changed(
        emitter: &SignalEmitter<'_>,
        session_handle: ObjectPath<'_>,
        shortcuts: Vec<(String, HashMap<String, OwnedValue>)>,
    ) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    async fn version(&self) -> u32 {
        2
    }
}

struct GlobalShortcutSessionInterface {
    state: Arc<Mutex<ShortcutState>>,
    session_handle: String,
}

#[interface(
    name = "org.freedesktop.impl.portal.Session",
    introspection_docs = false
)]
impl GlobalShortcutSessionInterface {
    async fn close(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let owner = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing sender".into()))?
            .to_string();
        let session = {
            let mut state = self.state.lock().await;
            if state
                .sessions
                .get(&self.session_handle)
                .is_some_and(|session| session.owner == owner)
            {
                state.sessions.remove(&self.session_handle)
            } else {
                None
            }
        };
        if let Some(session) = session {
            for shortcut in session.shortcuts {
                GlobalShortcutsInterface::unregister(
                    session.app_id.clone(),
                    self.session_handle.clone(),
                    shortcut.id,
                )
                .await;
            }
        }
        let _ = Self::closed(&emitter).await;
        if let Ok(session_path) = ObjectPath::try_from(self.session_handle.as_str()) {
            let _ = server.remove::<Self, _>(&session_path).await;
        }
        Ok(())
    }

    #[zbus(signal)]
    async fn closed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    async fn version(&self) -> u32 {
        1
    }
}
