//! Arachne's Tauri commands: the bridge between the Vue UI and the Loom client.
//!
//! All state lives in `tauri::State<LoomState>` — the client, the SSE
//! subscriptions, and the open-session id. The UI drives everything through
//! these commands plus the `loom://*` emits for pushed frames.
//!
//! Subscription lifecycle is the part worth being careful with: each open
//! session owns a chat forwarder, and a connect owns the fleet poller. Both
//! run in spawned tasks that watch a cancellation token; `open_session`
//! cancels the previous forwarder before starting the new one, and
//! `connect` cancels the old poller. Without that, every session switch or
//! reconnect leaks another SSE connection (and another duplicate event
//! stream into the webview) — the browser's connection cap is exactly why
//! loom multiplexes, and Arachne must not undo it from the other end.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::client::{LoomClient, LoomError};
use crate::loom::{SessionSummaryView, SessionView};

#[derive(Default)]
pub struct LoomState {
    pub client: tokio::sync::RwLock<Option<Arc<LoomClient>>>,
    /// Cancels the current fleet poller so a reconnect can replace it.
    fleet_cancel: tokio::sync::RwLock<Option<CancellationToken>>,
    /// Cancels the open session's chat forwarder so a switch can replace it.
    chat_cancel: tokio::sync::RwLock<Option<CancellationToken>>,
    /// The open session's next-older cursor from the last fetch_chat —
    /// `None` once history is exhausted.
    older_cursor: tokio::sync::RwLock<Option<crate::loom::ChatCursorView>>,
}

/// The open session's older cursor, for the UI's "load older" button.
/// `None` means history is exhausted (or no session open).
#[tauri::command]
pub async fn chat_older_cursor(
    state: State<'_, LoomState>,
) -> Result<Option<crate::loom::ChatCursorView>, UiError> {
    Ok(state.older_cursor.read().await.clone())
}

/// The dashboard snapshot the fleet poller pushes: the session summaries plus
/// the workstream layout, so the sidebar can group by workstream and nest
/// children under parents in one coherent render.
#[derive(Debug, Clone, Serialize)]
pub struct FleetSnapshot {
    pub sessions: Vec<SessionSummaryView>,
    pub layout: crate::loom::SessionLayoutView,
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

    // Replace any previous fleet poller (reconnect to a different loom).
    if let Some(old) = state.fleet_cancel.write().await.take() {
        old.cancel();
    }
    // A reconnect invalidates the open session's chat forwarder too — its
    // subscriptions belong to the old client.
    if let Some(old) = state.chat_cancel.write().await.take() {
        old.cancel();
    }
    *state.client.write().await = Some(client.clone());

    let cancel = CancellationToken::new();
    *state.fleet_cancel.write().await = Some(cancel.clone());
    spawn_fleet_poller(app, client, cancel);
    Ok(())
}

/// Fetch summaries + layout together and push them as a fleet snapshot.
async fn emit_fleet(app: &AppHandle, client: &Arc<LoomClient>, mut sessions: Vec<SessionSummaryView>) {
    let layout = match client.session_layout().await {
        Ok(l) => l,
        Err(e) => {
            let _ = app.emit("loom://error", UiError::from(e));
            return;
        }
    };
    sessions.sort_by(|a, b| a.last_activity_at.cmp(&b.last_activity_at));
    let _ = app.emit(
        "loom://fleet",
        &FleetSnapshot { sessions, layout },
    );
}

fn spawn_fleet_poller(app: AppHandle, client: Arc<LoomClient>, cancel: CancellationToken) {
    tauri::async_runtime::spawn(async move {
        loop {
            if cancel.is_cancelled() {
                return;
            }
            let mut sessions = match client.list_sessions().await {
                Ok(s) => s,
                Err(e) => {
                    let _ = app.emit("loom://error", UiError::from(e));
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
                    }
                    continue;
                }
            };
            let layout = match client.session_layout().await {
                Ok(l) => l,
                Err(e) => {
                    let _ = app.emit("loom://error", UiError::from(e));
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
                    }
                    continue;
                }
            };
            // "layout" covers layout mutations, but attention/tag changes
            // only publish on each session's own topic — so subscribe to the
            // live sessions too (loom caps a multiplexed stream at 64 topics:
            // layout + 63 newest sessions).
            let mut topics: Vec<String> = vec!["layout".into()];
            topics.extend(
                sessions
                    .iter()
                    .filter(|s| s.status != "archived")
                    .map(|s| format!("session:{}", s.id))
                    .take(63),
            );
            sessions.sort_by(|a, b| a.last_activity_at.cmp(&b.last_activity_at));
            emit_fleet(&app, &client, sessions).await;
            match client.subscribe(&topics).await {
                Ok(mut rx) => loop {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        frame = rx.recv() => {
                            let Some(_frame) = frame else { break };
                            // Any tag/status/layout change re-snapshots the
                            // fleet (tags publish on each session's own topic,
                            // not on `layout`).
                            match client.list_sessions().await {
                                Ok(list) => {
                                    emit_fleet(&app, &client, list).await;
                                }
                                Err(e) => {
                                    let _ = app.emit("loom://error", UiError::from(e));
                                }
                            }
                        }
                    }
                },
                Err(e) => {
                    let _ = app.emit("loom://error", UiError::from(e));
                }
            }
            // Subscription dropped (loom restarted): pause, retry, unless cancelled.
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
            }
        }
    });
}

/// Open a session: fetch its view, emit it, and start (replacing any previous)
/// chat SSE forwarder for this session.
#[tauri::command]
pub async fn open_session(
    app: AppHandle,
    state: State<'_, LoomState>,
    id: String,
) -> Result<SessionView, UiError> {
    let client = state_client(&state).await?;
    let view = client.get_session(&id).await?;

    // Cancel the previous session's forwarder: its frames would interleave
    // with the new session's otherwise.
    if let Some(old) = state.chat_cancel.write().await.take() {
        old.cancel();
    }
    let cancel = CancellationToken::new();
    *state.chat_cancel.write().await = Some(cancel.clone());
    spawn_chat_forwarder(app, client.clone(), id.clone(), cancel);
    state.older_cursor.write().await.take();
    Ok(view)
}

fn spawn_chat_forwarder(
    app: AppHandle,
    client: Arc<LoomClient>,
    id: String,
    cancel: CancellationToken,
) {
    tauri::async_runtime::spawn(async move {
        let topics = vec![format!("chat:{id}"), format!("session:{id}")];
        // Reconnect loop: the stream ends (loom restart) → brief pause →
        // resubscribe. Cancelled when another session opens.
        loop {
            if cancel.is_cancelled() {
                return;
            }
            if let Ok(mut rx) = client.subscribe(&topics).await {
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        frame = rx.recv() => {
                            let Some(frame) = frame else { break };
                            let _ = app.emit("loom://chat-event", &frame);
                        }
                    }
                }
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
            }
        }
    });
}

/// Fetch the chat journal, narrowed for display. Pass `before_turn`/
/// `before_seq` (the previous page's older_cursor) to page backward;
/// omit for the newest tail.
#[tauri::command]
pub async fn fetch_chat(
    state: State<'_, LoomState>,
    id: String,
    before_turn: Option<i64>,
    before_seq: Option<i64>,
) -> Result<Vec<crate::blocks::DisplayBlock>, UiError> {
    let client = state_client(&state).await?;
    let before = match (before_turn, before_seq) {
        (Some(turn), Some(seq)) => Some(crate::loom::ChatCursorView { turn, seq }),
        _ => None,
    };
    let chat = client.session_chat(&id, before.as_ref()).await?;
    *state.older_cursor.write().await = chat.older_cursor;
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

/// Launch a session. When `parent_id` is set this is a delegation: the
/// child records the parent's branch as `parent_branch`, getting origin=agent
/// and nesting under the parent in the sidebar.
#[tauri::command]
pub async fn launch_session(
    app: AppHandle,
    state: State<'_, LoomState>,
    repo: String,
    task: String,
    parent_id: Option<String>,
) -> Result<SessionView, UiError> {
    let client = state_client(&state).await?;
    let parent_branch = match parent_id {
        Some(id) if !id.is_empty() => {
            // Resolve the parent session to its branch id for the link.
            let parent = client.get_session(&id).await?;
            Some(parent.branch.id)
        }
        _ => None,
    };
    let view = client
        .launch(&crate::loom::SessionsLaunchInput {
            repo: Some(repo),
            title: Some(task.chars().take(80).collect()),
            goal: Some(task),
            parent_branch,
            ..Default::default()
        })
        .await?;
    // Only dashboard-originated launches yank selection to the new session;
    // delegations keep the user on the parent thread (the child appears
    // nested in the sidebar instead).
    if parent_branch.is_none() {
        let _ = app.emit("loom://launched", &view);
    }
    Ok(view)
}

/// Create a child session delegated to the given parent: same repo as the
/// parent, linked via `parent_branch` so it nests + inherits placement.
#[tauri::command]
pub async fn delegate_task(
    app: AppHandle,
    state: State<'_, LoomState>,
    parent_id: String,
    task: String,
) -> Result<SessionView, UiError> {
    let client = state_client(&state).await?;
    let parent = client.get_session(&parent_id).await?;
    // repo_root is ~/.weaver/repos/<owner>/<name> — the launch input wants
    // the `owner/name` slug.
    let slug_parts: Vec<&str> = parent
        .branch
        .repo_root
        .trim_end_matches('/')
        .rsplit('/')
        .take(2)
        .collect();
    let repo = if slug_parts.len() == 2 {
        format!("{}/{}", slug_parts[1], slug_parts[0])
    } else {
        "marin-community/arachne".to_string()
    };
    launch_session(app, state, repo, task, Some(parent_id)).await
}

/// Archive a session: tear down its terminal + worktree, keep the branch.
#[tauri::command]
pub async fn archive_session(
    app: AppHandle,
    state: State<'_, LoomState>,
    id: String,
) -> Result<(), UiError> {
    let client = state_client(&state).await?;
    client.archive(&id).await?;
    // The fleet poller's layout event will refresh the list; give the UI an
    // immediate signal that this specific session is gone.
    let _ = app.emit("loom://archived", &id);
    Ok(())
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
pub async fn refresh_fleet(state: State<'_, LoomState>) -> Result<FleetSnapshot, UiError> {
    let client = state_client(&state).await?;
    let mut sessions = client.list_sessions().await?;
    sessions.sort_by(|a, b| a.last_activity_at.cmp(&b.last_activity_at));
    let layout = client.session_layout().await?;
    Ok(FleetSnapshot { sessions, layout })
}

/// Create a workstream (layout group) in a space.
#[tauri::command]
pub async fn create_workstream(
    state: State<'_, LoomState>,
    name: String,
    space_id: Option<String>,
) -> Result<crate::loom::SessionLayoutView, UiError> {
    let client = state_client(&state).await?;
    // Default to the user space's Inbox space if not specified — Arachne's
    // workstreams live in the human's primary space.
    let space = match space_id {
        Some(id) => id,
        None => {
            let layout = client.session_layout().await?;
            layout
                .spaces
                .iter()
                .find(|s| s.system_key.as_deref() == Some("inbox") || s.name.eq_ignore_ascii_case("user"))
                .or_else(|| layout.spaces.first())
                .map(|s| s.id.clone())
                .ok_or_else(|| UiError {
                    message: "no space found for workstream".into(),
                    unreachable: false,
                })?
        }
    };
    client.create_group(&space, &name).await.map_err(Into::into)
}

/// Move sessions into a workstream.
#[tauri::command]
pub async fn move_to_workstream(
    state: State<'_, LoomState>,
    session_ids: Vec<String>,
    group_id: String,
) -> Result<crate::loom::SessionLayoutView, UiError> {
    let client = state_client(&state).await?;
    let refs: Vec<&str> = session_ids.iter().map(|s| s.as_str()).collect();
    client.move_sessions(&refs, &group_id).await.map_err(Into::into)
}

/// Delete a workstream; its sessions move to the destination group first.
#[tauri::command]
pub async fn delete_workstream(
    state: State<'_, LoomState>,
    group_id: String,
    destination_group_id: String,
) -> Result<crate::loom::SessionLayoutView, UiError> {
    let client = state_client(&state).await?;
    client
        .delete_group(&group_id, &destination_group_id)
        .await
        .map_err(Into::into)
}
