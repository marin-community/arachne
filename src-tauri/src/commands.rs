//! Arachne's Tauri commands: the bridge between the Vue UI and the Loom client.
//!
//! All state lives in `tauri::State<LoomState>` — the client, the SSE
//! subscriptions, and the open-session id. The UI drives everything through
//! these commands plus the `loom://*` emits for pushed frames.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::client::{LoomClient, LoomError};
use crate::loom::{EventFrame, SessionSummaryView, SessionView};

#[derive(Default)]
pub struct LoomState {
    pub client: tokio::sync::RwLock<Option<Arc<LoomClient>>>,
}

#[derive(Debug, Serialize, Clone)]
pub struct UiError {
    pub message: String,
    pub unreachable: bool,
}

impl From<LoomError> for UiError {
    fn from(e: LoomError) -> Self {
        let unreachable = e.is_unreachable();
        UiError {
            message: e.to_string(),
            unreachable,
        }
    }
}

async fn state_client(state: &LoomState) -> Result<Arc<LoomClient>, UiError> {
    state
        .client
        .read()
        .await
        .clone()
        .ok_or_else(|| UiError {
            message: "not connected to loom".into(),
            unreachable: true,
        })
}

/// Connect to a loom server. Loopback needs no token; remote accepts a bearer
/// token for later (DGX over Tailscale). Verifies health, then starts the
/// fleet poller (snapshot + layout SSE → `loom://fleet` emits).
#[tauri::command]
pub async fn connect(
    app: AppHandle,
    state: State<'_, LoomState>,
    base_url: String,
    token: Option<String>,
) -> Result<(), UiError> {
    let client = Arc::new(LoomClient::new(&base_url, token)?);
    client.health().await?;
    let poller_client = client.clone();
    *state.client.write().await = Some(client);
    spawn_fleet_poller(app.clone(), poller_client);
    Ok(())
}

fn spawn_fleet_poller(app: AppHandle, client: Arc<LoomClient>) {
    tauri::async_runtime::spawn(async move {
        loop {
            match client.list_sessions().await {
                Ok(sessions) => {
                    let _ = app.emit("loom://fleet", &sessions);
                }
                Err(e) => {
                    let _ = app.emit("loom://error", UiError::from(e));
                }
            }
            // Subscribe to layout SSE; each event means the fleet changed.
            if let Ok(mut rx) = client.subscribe(&["layout".to_string()]).await {
                while let Some(_frame) = rx.recv().await {
                    match client.list_sessions().await {
                        Ok(sessions) => {
                            let _ = app.emit("loom://fleet", &sessions);
                        }
                        Err(e) => {
                            let _ = app.emit("loom://error", UiError::from(e));
                        }
                    }
                }
            }
            // Subscription dropped (loom restarted): brief pause, reconnect.
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
}

/// Open a session: fetch its view, emit it, and (in the background) start the
/// chat SSE forwarder for this session.
#[tauri::command]
pub async fn open_session(
    app: AppHandle,
    state: State<'_, LoomState>,
    id: String,
) -> Result<SessionView, UiError> {
    let client = state_client(&state).await?;
    let view = client.get_session(&id).await?;
    spawn_chat_forwarder(app, client, id);
    Ok(view)
}

fn spawn_chat_forwarder(app: AppHandle, client: Arc<LoomClient>, id: String) {
    tauri::async_runtime::spawn(async move {
        let topics = vec![format!("chat:{id}"), format!("session:{id}")];
        if let Ok(mut rx) = client.subscribe(&topics).await {
            while let Some(frame) = rx.recv().await {
                let _ = app.emit("loom://chat-event", &frame);
            }
        }
    });
}

/// Fetch the chat journal, narrowed for display.
#[tauri::command]
pub async fn fetch_chat(
    state: State<'_, LoomState>,
    id: String,
) -> Result<Vec<crate::blocks::DisplayBlock>, UiError> {
    let client = state_client(&state).await?;
    let chat = client.session_chat(&id).await?;
    Ok(chat.blocks.iter().map(crate::blocks::DisplayBlock::from_view).collect())
}

/// Send input to a session: ACP prompt (agents) or terminal text (raw).
#[tauri::command]
pub async fn send_input(
    state: State<'_, LoomState>,
    id: String,
    text: String,
    protocol: String,
) -> Result<(), UiError> {
    let client = state_client(&state).await?;
    if protocol == "terminal" {
        client.send_text(&id, &text, true).await?;
    } else {
        client.send_prompt(&id, &text).await?;
    }
    Ok(())
}

/// Interrupt the current turn.
#[tauri::command]
pub async fn interrupt(state: State<'_, LoomState>, id: String) -> Result<(), UiError> {
    let client = state_client(&state).await?;
    client.interrupt(&id).await.map_err(Into::into)
}

/// Launch a new session (worktree + agent, seeded with a task).
#[tauri::command]
pub async fn launch_session(
    app: AppHandle,
    state: State<'_, LoomState>,
    repo: String,
    task: String,
) -> Result<SessionView, UiError> {
    let client = state_client(&state).await?;
    let view = client
        .launch(&crate::loom::SessionsLaunchInput {
            repo: Some(repo),
            title: Some(task.chars().take(80).collect()),
            goal: Some(task),
            ..Default::default()
        })
        .await?;
    let _ = app.emit("loom://launched", &view);
    Ok(view)
}

/// Open the session's worktree in Zed.
#[tauri::command]
pub async fn open_in_zed(work_dir: String) -> Result<(), UiError> {
    let status = tokio::process::Command::new("zed")
        .arg(&work_dir)
        .status()
        .await
        .map_err(|e| UiError {
            message: format!("launching zed: {e} (is zed installed and on PATH?)"),
            unreachable: false,
        })?;
    if !status.success() {
        return Err(UiError {
            message: format!("zed exited with {status}"),
            unreachable: false,
        });
    }
    Ok(())
}

/// Manual fleet refresh.
#[tauri::command]
pub async fn refresh_fleet(state: State<'_, LoomState>) -> Result<Vec<SessionSummaryView>, UiError> {
    let client = state_client(&state).await?;
    client.list_sessions().await.map_err(Into::into)
}

// Keep EventFrame referenced so its import is used.
#[allow(dead_code)]
fn _anchor(_: Option<EventFrame>) {}
