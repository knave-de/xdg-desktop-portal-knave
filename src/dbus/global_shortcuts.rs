//! XDG GlobalShortcuts portal backed by Villain's desktop control contract.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use knave_desktop_api::{DesktopClient, DesktopRequest, DesktopResponse};
use tokio::sync::Mutex;
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
    trigger: String,
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
                Self::unregister(session.app_id.clone(), session_handle.clone(), shortcut.id).await;
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
            .and_then(|value| <String>::try_from(value).ok())
    }

    fn format_shortcuts(shortcuts: &[Shortcut]) -> Vec<(String, HashMap<String, OwnedValue>)> {
        shortcuts
            .iter()
            .map(|shortcut| {
                let mut values = HashMap::new();
                values.insert(
                    "description".into(),
                    OwnedValue::from(shortcut.description.clone()),
                );
                values.insert(
                    "trigger_description".into(),
                    OwnedValue::from(shortcut.trigger.clone()),
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
            return Err(error);
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

        let mut accepted = Vec::new();
        for (id, options) in shortcuts {
            if id.is_empty() || id.len() > 128 {
                continue;
            }
            let Some(preferred) = Self::read_string(&options, "preferred_trigger") else {
                continue;
            };
            let description = Self::read_string(&options, "description").unwrap_or_default();
            if description.len() > 512 {
                continue;
            }
            match Self::register(&app_id, &session_key, &id, &preferred).await {
                Ok(trigger) => accepted.push(Shortcut {
                    id,
                    description,
                    trigger,
                }),
                Err(error) => {
                    tracing::debug!(%error, shortcut_id = %id, "global shortcut was not bound")
                }
            }
        }
        self.state
            .lock()
            .await
            .sessions
            .get_mut(&session_key)
            .map(|session| session.shortcuts = accepted.clone());
        Ok((
            Response::Success.to_u32(),
            Self::shortcuts_result(&accepted),
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
    ) -> zbus::fdo::Result<()> {
        let owner = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing sender".into()))?
            .to_string();
        if self
            .state
            .lock()
            .await
            .sessions
            .get(session_handle.as_str())
            .is_some_and(|session| session.owner == owner)
        {
            Err(zbus::fdo::Error::NotSupported(
                "Knave does not yet provide a shortcut assignment window".into(),
            ))
        } else {
            Err(zbus::fdo::Error::Failed("unknown shortcut session".into()))
        }
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
