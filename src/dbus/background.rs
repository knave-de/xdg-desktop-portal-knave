//! XDG Background portal backend interface.
use std::{
    collections::{HashMap, HashSet},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use knave_desktop_api::{DesktopSubscription, WindowSummary};
use knave_portal_api::{BackgroundChoice, BackgroundReply, BackgroundRequest, MAX_REQUEST};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{Mutex, watch},
    task::JoinHandle,
    time::timeout,
};
use zbus::{Connection, interface, object_server::SignalEmitter, zvariant::OwnedValue};

use crate::picker::{DIALOG, shell_binary};

/// XDG Background portal implementation interface.
#[derive(Clone)]
pub struct BackgroundInterface {
    approved_apps: Arc<Mutex<HashSet<String>>>,
    windows: Arc<Mutex<HashMap<String, u32>>>,
    app_states: Arc<Mutex<HashMap<String, u32>>>,
}

impl Default for BackgroundInterface {
    fn default() -> Self {
        Self::new()
    }
}

impl BackgroundInterface {
    /// Create the interface with an empty set of known background applications.
    pub fn new() -> Self {
        Self {
            approved_apps: Arc::new(Mutex::new(HashSet::new())),
            windows: Arc::new(Mutex::new(HashMap::new())),
            app_states: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Watch compositor window snapshots and emit state changes until D-Bus closes.
    pub async fn monitor(&self, connection: Connection) {
        let connected = tokio::task::spawn_blocking(DesktopSubscription::connect).await;
        let Ok(Ok(mut subscription)) = connected else {
            tracing::warn!("background app state monitor could not connect to the desktop API");
            return;
        };
        let initial = match subscription.next_snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::warn!(%error, "background app state monitor did not receive an initial snapshot");
                return;
            }
        };
        let cancel = match subscription.cancel_handle() {
            Ok(cancel) => cancel,
            Err(error) => {
                tracing::warn!(%error, "background app state monitor could not create a cancellation handle");
                return;
            }
        };
        *self.windows.lock().await = Self::window_states(&initial.windows);
        self.refresh_state().await;

        let (snapshot_tx, mut snapshot_rx) = watch::channel(HashMap::new());
        let worker: JoinHandle<()> = tokio::task::spawn_blocking(move || {
            while let Ok(snapshot) = subscription.next_snapshot() {
                snapshot_tx.send_replace(Self::window_states(&snapshot.windows));
            }
        });
        let emitter = match SignalEmitter::new(&connection, "/org/freedesktop/portal/desktop") {
            Ok(emitter) => emitter,
            Err(error) => {
                tracing::warn!(%error, "background app state monitor could not create a signal emitter");
                let _ = cancel.cancel();
                let _ = worker.await;
                return;
            }
        };
        loop {
            tokio::select! {
                () = connection.closed() => break,
                changed = snapshot_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let windows = snapshot_rx.borrow_and_update().clone();
                    *self.windows.lock().await = windows;
                    if self.refresh_state().await
                        && let Err(error) = Self::running_applications_changed(&emitter).await
                    {
                        tracing::debug!(%error, "could not emit background application state change");
                    }
                }
            }
        }
        let _ = cancel.cancel();
        let _ = worker.await;
    }

    fn window_states(windows: &[WindowSummary]) -> HashMap<String, u32> {
        let mut states = HashMap::new();
        for window in windows {
            let state = if window.focused { 2 } else { 1 };
            states
                .entry(window.app_id.clone())
                .and_modify(|current: &mut u32| *current = (*current).max(state))
                .or_insert(state);
        }
        states
    }

    async fn refresh_state(&self) -> bool {
        let mut state = self
            .approved_apps
            .lock()
            .await
            .iter()
            .map(|app_id| (app_id.clone(), 0))
            .collect::<HashMap<_, u32>>();
        state.extend(
            self.windows
                .lock()
                .await
                .iter()
                .map(|(app_id, value)| (app_id.clone(), *value)),
        );
        let mut current = self.app_states.lock().await;
        if *current == state {
            false
        } else {
            *current = state;
            true
        }
    }

    async fn ask_user(request: BackgroundRequest) -> Result<BackgroundChoice, String> {
        request.validate()?;
        let _permit = DIALOG
            .try_acquire()
            .map_err(|_| "another portal dialog is already active".to_owned())?;
        let payload = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
        if payload.len() > MAX_REQUEST {
            return Err("background request exceeds budget".into());
        }
        let mut child = Command::new(shell_binary())
            .arg("background-permission")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("could not start background permission UI: {error}"))?;
        let operation = async {
            let mut stdin = child.stdin.take().ok_or("missing prompt stdin")?;
            stdin
                .write_all(&payload)
                .await
                .map_err(|error| error.to_string())?;
            drop(stdin);
            let stdout = child.stdout.take().ok_or("missing prompt stdout")?;
            let mut output = Vec::new();
            stdout
                .take(4097)
                .read_to_end(&mut output)
                .await
                .map_err(|error| error.to_string())?;
            if output.len() > 4096 {
                return Err("background prompt reply exceeds budget".into());
            }
            let status = child.wait().await.map_err(|error| error.to_string())?;
            if !status.success() {
                return Err(format!("background prompt exited with {status}"));
            }
            let reply: BackgroundReply =
                serde_json::from_slice(&output).map_err(|error| error.to_string())?;
            reply.validate(&request)?;
            Ok(reply.choice)
        };
        timeout(Duration::from_secs(300), operation)
            .await
            .map_err(|_| "background permission prompt timed out".to_owned())?
    }
}

#[interface(name = "org.freedesktop.impl.portal.Background")]
impl BackgroundInterface {
    /// Return the current background state for known applications.
    async fn get_app_state(&self) -> HashMap<String, OwnedValue> {
        self.app_states
            .lock()
            .await
            .iter()
            .map(|(app_id, state)| (app_id.clone(), OwnedValue::from(*state)))
            .collect()
    }

    /// Ask the user whether the application may run in the background.
    async fn notify_background(
        &self,
        handle: zbus::zvariant::ObjectPath<'_>,
        app_id: String,
        name: String,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let choice = Self::ask_user(BackgroundRequest {
            request_id: handle.to_string(),
            app_id: app_id.clone(),
            name,
            reason: String::new(),
        })
        .await;
        let result = match choice {
            Ok(BackgroundChoice::Deny) => {
                self.approved_apps.lock().await.remove(&app_id);
                if self.refresh_state().await
                    && let Err(error) = Self::running_applications_changed(&emitter).await
                {
                    tracing::debug!(%error, "could not emit background application state change");
                }
                0
            }
            Ok(BackgroundChoice::Allow) => {
                self.approved_apps.lock().await.insert(app_id);
                if self.refresh_state().await
                    && let Err(error) = Self::running_applications_changed(&emitter).await
                {
                    tracing::debug!(%error, "could not emit background application state change");
                }
                1
            }
            Ok(BackgroundChoice::AllowOnce) => {
                self.approved_apps.lock().await.insert(app_id);
                if self.refresh_state().await
                    && let Err(error) = Self::running_applications_changed(&emitter).await
                {
                    tracing::debug!(%error, "could not emit background application state change");
                }
                2
            }
            Err(error) => {
                tracing::warn!(%error, "background activity request was denied");
                0
            }
        };
        let mut results = HashMap::new();
        results.insert("result".into(), OwnedValue::from(result));
        (0, results)
    }

    /// Autostart is deprecated by the portal frontend and is not enabled here.
    #[expect(
        clippy::unused_self,
        reason = "zbus interface methods require a receiver; autostart is intentionally unsupported"
    )]
    fn enable_autostart(
        &self,
        app_id: String,
        enable: bool,
        commandline: Vec<String>,
        flags: u32,
    ) -> bool {
        let _ = (app_id, enable, commandline, flags);
        false
    }

    #[zbus(signal)]
    async fn running_applications_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}
