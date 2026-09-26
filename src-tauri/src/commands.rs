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

/// `fetch_chat`'s reply: the journal page plus the live-turn signals the
/// thread view needs to show its working indicator. `live_turn` is set while
/// an ACP turn is in flight (loom's durable `acp_inflight` state — it survives
/// a reload of the app, unlike an in-memory flag); `pending_prompt` is a
/// message queued behind the running turn, waiting to start its own. The two
/// timestamps restore the elapsed clock after a reload: the live turn's
/// opening user message is the turn start, the newest block of that turn is
/// the last server-observed progress (mirrors loom SPA's `restoreLiveTiming`).
#[derive(Debug, Clone, Serialize)]
pub struct ChatSnapshot {
    pub blocks: Vec<crate::blocks::DisplayBlock>,
    pub live_turn: Option<i64>,
    pub pending_prompt: Option<String>,
    pub live_started_at: Option<String>,
    pub live_progress_at: Option<String>,
}

impl ChatSnapshot {
    /// Derive the live turn's timing from the journal page: the opening
    /// block's `created_at` (the user message the turn was spawned from) and
    /// the newest block's `created_at`.
    pub fn live_timing(chat: &crate::loom::SessionChatView) -> (Option<String>, Option<String>) {
        let Some(turn) = chat.live_turn else {
            return (None, None);
        };
        // The turn's opening block is its start (the user message the turn
        // was spawned from); the newest block is the last observed progress.
        let mut started = None;
        let mut progress = None;
        for block in chat.blocks.iter().filter(|b| b.turn == turn) {
            if started.is_none() {
                started = Some(block.created_at.clone());
            }
            progress = Some(block.created_at.clone());
        }
        (started, progress)
    }
}

/// The dashboard snapshot the fleet poller pushes: the session summaries plus
/// the topic layout, so the sidebar can nest children under their topic's
/// leader chat in one coherent render.
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

/// Persist the loom bearer token to the macOS Keychain. An empty token
/// removes the stored credential.
#[tauri::command]
pub async fn save_token(token: String) -> Result<(), UiError> {
    tokio::task::spawn_blocking(move || crate::secret::save(&token))
        .await
        .map_err(|e| UiError {
            message: format!("saving token: {e}"),
            unreachable: false,
        })?
        .map_err(|e| UiError {
            message: e,
            unreachable: false,
        })
}

/// Read the stored loom bearer token; `None` when nothing is stored.
#[tauri::command]
pub async fn load_token() -> Result<Option<String>, UiError> {
    tokio::task::spawn_blocking(crate::secret::load)
        .await
        .map_err(|e| UiError {
            message: format!("loading token: {e}"),
            unreachable: false,
        })?
        .map_err(|e| UiError {
            message: e,
            unreachable: false,
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

/// Fetch the chat journal, narrowed for display, plus the live-turn state.
/// Pass `before_turn`/`before_seq` (the previous page's older_cursor) to page
/// backward; omit for the newest tail.
#[tauri::command]
pub async fn fetch_chat(
    state: State<'_, LoomState>,
    id: String,
    before_turn: Option<i64>,
    before_seq: Option<i64>,
) -> Result<ChatSnapshot, UiError> {
    let client = state_client(&state).await?;
    let before = match (before_turn, before_seq) {
        (Some(turn), Some(seq)) => Some(crate::loom::ChatCursorView { turn, seq }),
        _ => None,
    };
    let chat = client.session_chat(&id, before.as_ref()).await?;
    let (live_started_at, live_progress_at) = ChatSnapshot::live_timing(&chat);
    *state.older_cursor.write().await = chat.older_cursor;
    Ok(ChatSnapshot {
        blocks: chat
            .blocks
            .iter()
            .map(crate::blocks::DisplayBlock::from_view)
            .collect(),
        live_turn: chat.live_turn,
        pending_prompt: chat.pending_prompt,
        live_started_at,
        live_progress_at,
    })
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
            parent_branch: parent_branch.clone(),
            ..Default::default()
        })
        .await?;
    // A top-level launch IS a topic: stamp the durable marker so the sidebar
    // keeps its shape even if every child finishes and archives. (Delegations
    // are stamped by `delegate_task` on the parent, not the child.)
    if parent_branch.is_none() {
        let _ = client.set_tag(&view.id, "topic", "true").await;
    }
    // Selection/activation is the frontend's job: it routes the new
    // session through open_session (chat forwarder + live streaming) via
    // the returned view. Delegations stay on the parent thread — the child
    // just appears nested in the sidebar.
    Ok(view)
}

/// Create a child session delegated to the given parent.
///
/// The child targets the parent's repository using only server-side truth:
/// - `repo`: the parent's managed `owner/name` slug when it launched against
///   a managed repo;
/// - `cwd`: the parent's worktree path (a path on the *server's* filesystem,
///   valid whether loom runs locally or on the DGX) — the server ignores
///   `cwd` whenever `repo` is present, so both can be passed unconditionally.
///
/// No hardcoded slug, no Mac-shaped path parsing, no client-side git.
#[tauri::command]
pub async fn delegate_task(
    state: State<'_, LoomState>,
    parent_id: String,
    task: String,
) -> Result<SessionView, UiError> {
    let client = state_client(&state).await?;
    let parent = client.get_session(&parent_id).await?;
    let view = client
        .launch(&crate::loom::SessionsLaunchInput {
            repo: parent.github_repo.clone(),
            cwd: parent.work_dir.clone(),
            title: Some(task.chars().take(80).collect()),
            goal: Some(task),
            parent_branch: Some(parent.branch.id),
            ..Default::default()
        })
        .await?;
    // Self-heal the parent's topic marker: leaders launched from the CLI (or
    // before this marker existed) should still hold their shape.
    let _ = client.set_tag(&parent_id, "topic", "true").await;
    Ok(view)
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

/// Delete a lane (placement group); its sessions move to the destination
/// group first. Lanes are just filing — they are not topics.
#[tauri::command]
pub async fn delete_group(
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

/// Move sessions into a lane (placement group).
#[tauri::command]
pub async fn move_to_group(
    state: State<'_, LoomState>,
    session_ids: Vec<String>,
    group_id: String,
) -> Result<crate::loom::SessionLayoutView, UiError> {
    let client = state_client(&state).await?;
    let refs: Vec<&str> = session_ids.iter().map(|s| s.as_str()).collect();
    client.move_sessions(&refs, &group_id).await.map_err(Into::into)
}

/// Re-parent a session under another (its topic's top-level chat), or
/// detach it to top level. The session follows the parent's placement group.
#[tauri::command]
pub async fn reparent_session(
    state: State<'_, LoomState>,
    session_id: String,
    parent_id: Option<String>,
) -> Result<(), UiError> {
    let client = state_client(&state).await?;
    client
        .reparent_session(&session_id, parent_id.as_deref().filter(|p| !p.is_empty()))
        .await?;
    // Joining a parent makes that parent a topic — stamp the durable marker.
    if let Some(parent) = parent_id.as_deref().filter(|p| !p.is_empty()) {
        let _ = client.set_tag(parent, "topic", "true").await;
    }
    Ok(())
}
