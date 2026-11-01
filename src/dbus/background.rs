//! XDG Background portal backend interface.
use std::{
    collections::{HashMap, HashSet},
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
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
        let (snapshot_tx, mut snapshot_rx) = watch::channel(HashMap::new());
        let stop = Arc::new(AtomicBool::new(false));
        let active_cancel = Arc::new(StdMutex::new(None));
        let worker_stop = Arc::clone(&stop);
        let worker_cancel = Arc::clone(&active_cancel);
        let worker: JoinHandle<()> = tokio::task::spawn_blocking(move || {
            while !worker_stop.load(Ordering::Acquire) {
                let mut subscription = match DesktopSubscription::connect() {
                    Ok(subscription) => subscription,
                    Err(error) => {
                        tracing::debug!(%error, "background app state monitor could not connect");
                        thread::sleep(Duration::from_secs(1));
                        continue;
                    }
                };
                match subscription.cancel_handle() {
                    Ok(cancel) => {
                        let mut active_cancel = worker_cancel
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        *active_cancel = Some(cancel);
                        if worker_stop.load(Ordering::Acquire) {
                            *active_cancel = None;
                            break;
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "background app state monitor could not create a cancellation handle");
                        thread::sleep(Duration::from_secs(1));
                        continue;
                    }
                }
                loop {
                    match subscription.next_snapshot() {
                        Ok(snapshot) => {
                            snapshot_tx.send_replace(Self::window_states(&snapshot.windows));
                        }
                        Err(error) => {
                            tracing::debug!(%error, "background app state stream ended");
                            break;
                        }
                    }
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                }
                *worker_cancel
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                snapshot_tx.send_replace(HashMap::new());
                if !worker_stop.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_secs(1));
                }
            }
        });
        let emitter = match SignalEmitter::new(&connection, "/org/freedesktop/portal/desktop") {
            Ok(emitter) => emitter,
            Err(error) => {
                tracing::warn!(%error, "background app state monitor could not create a signal emitter");
                stop.store(true, Ordering::Release);
                if let Some(cancel) = active_cancel
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                {
                    let _ = cancel.cancel();
                }
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
        stop.store(true, Ordering::Release);
        if let Some(cancel) = active_cancel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            let _ = cancel.cancel();
        }
        let _ = worker.await;
    }

    /// Derive the portal's running and focused state values from window snapshots.
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

    /// Recompute the published app state and report whether it changed.
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

    /// Show a cancellable consent prompt and validate its reply.
    async fn ask_user(
        request: BackgroundRequest,
        mut cancelled: watch::Receiver<bool>,
    ) -> Result<BackgroundChoice, String> {
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
        tokio::select! {
            changed = cancelled.changed() => {
                let _ = changed;
                Err("background permission prompt was cancelled".to_owned())
            }
            result = timeout(Duration::from_secs(300), operation) => {
                result.map_err(|_| "background permission prompt timed out".to_owned())?
            }
        }
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
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        let (request, cancelled) = super::RequestInterface::cancellable();
        server.at(&handle, request).await?;
        let choice = Self::ask_user(
            BackgroundRequest {
                request_id: handle.to_string(),
                app_id: app_id.clone(),
                name,
                reason: String::new(),
            },
            cancelled,
        )
        .await;
        let _ = server.remove::<super::RequestInterface, _>(&handle).await;
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
                tracing::warn!(%error, "background permission prompt failed");
                return Err(zbus::fdo::Error::Failed(error));
            }
        };
        let mut results = HashMap::new();
        results.insert("result".into(), OwnedValue::from(result));
        Ok((0, results))
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
