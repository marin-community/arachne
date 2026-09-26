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
use crate::loom::{LaunchOptionsView, SessionSummaryView, SessionView};

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
    // Health is public; prove the supplied credential can read the fleet
    // before showing a green connection indicator.
    client.list_sessions().await?;
    client.session_layout().await?;

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

#[tauri::command]
pub async fn launch_options(state: State<'_, LoomState>) -> Result<LaunchOptionsView, UiError> {
    let client = state_client(&state).await?;
    client.launch_options().await.map_err(Into::into)
}

/// Change the runtime selection of an idle ACP session. The Loom server
/// rejects active turns; Arachne never interrupts one to force a handoff.
#[tauri::command]
pub async fn handoff_session(
    state: State<'_, LoomState>,
    id: String,
    profile: String,
    agent: Option<String>,
    model: Option<String>,
    effort: Option<String>,
) -> Result<SessionView, UiError> {
    let client = state_client(&state).await?;
    client
        .handoff_session(&id, &profile, agent.as_deref(), model.as_deref(), effort.as_deref())
        .await
        .map_err(Into::into)
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
            let topics = fleet_topics(&sessions);
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
                                    let changed_topics = fleet_topics(&list) != topics;
                                    emit_fleet(&app, &client, list).await;
                                    // A launch/archive can change the set of session
                                    // event topics. Reconnect immediately so the new
                                    // session's attention changes are live as well.
                                    if changed_topics { break; }
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

fn fleet_topics(sessions: &[SessionSummaryView]) -> Vec<String> {
    let mut live: Vec<&SessionSummaryView> = sessions
        .iter()
        .filter(|session| session.status != "archived")
        .collect();
    live.sort_by(|a, b| b.last_activity_at.cmp(&a.last_activity_at));
    let mut topics = vec!["layout".to_owned()];
    topics.extend(live.into_iter().take(63).map(|session| format!("session:{}", session.id)));
    topics
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

fn thread_note_body(source_title: &str, source_id: &str, note: &str) -> String {
    format!(
        "From Arachne thread: {} ({})\n\n{}",
        source_title.trim(),
        source_id,
        note.trim()
    )
}

/// Send a human-written note to another session's durable Loom channel.
/// The source identity is resolved by Loom (never accepted as display text
/// from the webview), and also included in the channel's structured payload.
#[tauri::command]
pub async fn send_to_thread(
    state: State<'_, LoomState>,
    source_id: String,
    destination_id: String,
    note: String,
    idempotency_key: String,
) -> Result<(), UiError> {
    if source_id == destination_id {
        return Err(UiError {
            message: "choose a different destination thread".into(),
            unreachable: false,
        });
    }
    if note.trim().is_empty() || idempotency_key.trim().is_empty() {
        return Err(UiError {
            message: "note and delivery key are required".into(),
            unreachable: false,
        });
    }
    let client = state_client(&state).await?;
    let source = client.get_session(&source_id).await?;
    let destination = client.get_session(&destination_id).await?;
    if destination.status == "archived" {
        return Err(UiError {
            message: "the destination thread is archived".into(),
            unreachable: false,
        });
    }
    let title = if source.branch.title.trim().is_empty() {
        source.branch.name.as_str()
    } else {
        source.branch.title.as_str()
    };
    let body = thread_note_body(title, &source.id, &note);
    client
        .send_to_thread(
            &destination.id,
            &body,
            serde_json::json!({
                "arachne": {
                    "kind": "thread_note",
                    "source_session_id": source.id,
                    "source_branch_id": source.branch.id,
                    "source_title": title,
                }
            }),
            &idempotency_key,
        )
        .await?;
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
    title: Option<String>,
    description: Option<String>,
    profile: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    effort: Option<String>,
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
    // The sidebar's composer sends only `task` (title falls back to it, like
    // the CLI); the topic card sends an explicit short `title` plus a longer
    // `description`.
    let label = title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .unwrap_or_else(|| task.chars().take(80).collect());
    let description = description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(String::from);
    let view = client
        .launch(&crate::loom::SessionsLaunchInput {
            repo: Some(repo),
            title: Some(label),
            goal: Some(task),
            parent_branch: parent_branch.clone(),
            profile,
            agent,
            model,
            effort,
            ..Default::default()
        })
        .await?;
    // `sessions.launch` has no description field (only GitHub issues seed
    // one server-side), so the topic card's description is stamped right
    // after launch via `sessions.update`. Best-effort: an empty description
    // means "none was typed" — nothing to stamp.
    if let Some(desc) = description {
        let _ = client
            .update_session(&crate::loom::SessionsUpdateInput {
                description: Some(desc),
                session: Some(view.id.clone()),
                ..Default::default()
            })
            .await;
    }
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

/// Edit a topic's metadata: title (compare-and-swap fenced), goal, and
/// description. Loom's `sessions.update` publishes no SSE event for these
/// fields, so the fresh view is re-emitted on the fleet snapshot path to
/// keep the sidebar's topic card in sync immediately.
#[tauri::command]
pub async fn update_session(
    app: AppHandle,
    state: State<'_, LoomState>,
    session: String,
    title: Option<String>,
    expected_title: Option<String>,
    expected_title_provenance: Option<String>,
    goal: Option<String>,
    description: Option<String>,
) -> Result<SessionView, UiError> {
    let client = state_client(&state).await?;
    let view = client
        .update_session(&crate::loom::SessionsUpdateInput {
            title,
            expected_title,
            expected_title_provenance,
            goal,
            description,
            session: Some(session),
        })
        .await?;
    // Fresh summary + layout, pushed as a fleet snapshot so every sidebar
    // surface (inbox lanes and the topic card) sees the edit at once.
    if let Ok(list) = client.list_sessions().await {
        emit_fleet(&app, &client, list).await;
    }
    Ok(view)
}

/// Build a Zed target from the authoritative Loom host and the session's
/// server-side checkout path. The bootstrap control plane runs its sessions
/// on the same host; a future multi-runner Loom view should supply the
/// runner's SSH identity explicitly instead of using this host fallback.
fn zed_target(host: &str, work_dir: &str) -> Result<String, UiError> {
    if !work_dir.starts_with('/') || work_dir.contains('\0') {
        return Err(UiError {
            message: "session has no absolute checkout path to open".into(),
            unreachable: false,
        });
    }
    if host == "localhost" || host == "127.0.0.1" || host == "::1" {
        return Ok(work_dir.to_string());
    }
    // URL path segments must be encoded (notably spaces and `#`), while
    // slashes remain separators. Zed accepts the resulting ssh:// URL.
    let mut url = reqwest::Url::parse(&format!("ssh://{host}")).map_err(|e| UiError {
        message: format!("invalid Loom host for Zed: {e}"),
        unreachable: false,
    })?;
    url.set_path(work_dir);
    Ok(url.to_string())
}

/// Open the exact session checkout in Zed, using SSH for remote Loom.
#[tauri::command]
pub async fn open_in_zed(state: State<'_, LoomState>, id: String) -> Result<(), UiError> {
    let client = state_client(&state).await?;
    let view = client.get_session(&id).await?;
    if view.status == "archived" {
        return Err(UiError {
            message: "this session is archived and its checkout is no longer active".into(),
            unreachable: false,
        });
    }
    let host = client.server_host().ok_or_else(|| UiError {
        message: "Loom URL has no host".into(),
        unreachable: false,
    })?;
    let target = zed_target(host, &view.work_dir)?;
    // GUI apps often inherit a minimal PATH without /usr/local/bin, where
    // Zed installs its CLI symlink. Prefer the app-bundled CLI on macOS.
    let cli = if std::path::Path::new("/Applications/Zed.app/Contents/MacOS/cli").exists() {
        "/Applications/Zed.app/Contents/MacOS/cli"
    } else {
        "zed"
    };
    let status = tokio::process::Command::new(cli)
        .arg(&target)
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

#[cfg(test)]
mod zed_tests {
    use super::{thread_note_body, zed_target};

    #[test]
    fn local_checkout_is_a_path() {
        assert_eq!(zed_target("127.0.0.1", "/tmp/a b").unwrap(), "/tmp/a b");
    }

    #[test]
    fn remote_checkout_uses_ssh_and_encodes_path() {
        assert_eq!(
            zed_target("100.121.8.110", "/home/dlwh/worktrees/a b#1").unwrap(),
            "ssh://100.121.8.110/home/dlwh/worktrees/a%20b%231"
        );
    }

    #[test]
    fn missing_checkout_is_rejected() {
        assert!(zed_target("100.121.8.110", "relative/path").is_err());
    }

    #[test]
    fn delivered_note_names_its_source_for_the_agent() {
        assert_eq!(
            thread_note_body(" Source topic ", "session-1", " Check the PR "),
            "From Arachne thread: Source topic (session-1)\n\nCheck the PR"
        );
    }
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
