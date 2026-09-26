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
use crate::resources::{ResourceDraft, ResourceKind, ResourceMention, TopicResource, TopicResourceContent, TopicResourcesView, MANIFEST_NAME};

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
    topic_id: Option<String>,
    resource_ids: Option<Vec<String>>,
) -> Result<(), UiError> {
    let client = state_client(&state).await?;
    let mut prompt = text;
    let ids = resource_ids.unwrap_or_default();
    if !ids.is_empty() {
        let topic_id = topic_id.ok_or_else(|| resource_error("resource mentions require a topic"))?;
        let mentions = ids.into_iter().map(|resource_id| ResourceMention { topic_id: topic_id.clone(), resource_id }).collect();
        prompt.push_str(&resource_context(&client, mentions).await?);
    }
    if protocol == "terminal" {
        client.send_text(&id, &prompt, true).await?;
    } else {
        client.send_prompt(&id, &prompt).await?;
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
    app: AppHandle,
    state: State<'_, LoomState>,
    repo: String,
    task: String,
    parent_id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    one_off: Option<bool>,
    mentions: Option<Vec<ResourceMention>>,
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
    // Quick one-offs send only `task` (title falls back to it, like the CLI);
    // the topic card sends an explicit short title and longer body.
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
    let mut goal = task;
    if let Some(mentions) = mentions {
        goal.push_str(&resource_context(&client, mentions).await?);
    }
    let view = client
        .launch(&crate::loom::SessionsLaunchInput {
            repo: Some(repo),
            title: Some(label),
            goal: Some(goal),
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
    // A named topic retains its durable marker even after its children
    // archive. Quick one-offs stay in the inbox without becoming topics.
    if parent_branch.is_none() && !one_off.unwrap_or(false) {
        let _ = client.set_tag(&view.id, "topic", "true").await;
    }
    // Loom does not publish a fleet event for description updates. Publish
    // the final launch state so the new topic card has its body immediately.
    if let Ok(list) = client.list_sessions().await {
        emit_fleet(&app, &client, list).await;
    }
    let view = client.get_session(&view.id).await.unwrap_or(view);
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
    let fleet = client.list_sessions().await?;
    // Conversation ancestry is independent of git ancestry. A child of a
    // worker starts from the topic's accepted branch when one exists.
    let base = resolve_integration_target(&fleet, &parent_id)
        .map(|target| target.target_branch)
        .unwrap_or_else(|| parent.branch.branch.clone());
    let view = client
        .launch(&crate::loom::SessionsLaunchInput {
            repo: parent.github_repo.clone(),
            cwd: parent.work_dir.clone(),
            base: Some(base),
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

// ---------------------------------------------------------------------------
// Integration (spec: docs/integration-and-landing.md)
// ---------------------------------------------------------------------------

/// The resolved coordinator for an integration: where the structured request
/// lands, and the branch the work integrates toward.
#[derive(Debug, Clone, Serialize)]
pub struct IntegrationTarget {
    /// The coordinator session id receiving the request ("wake the appropriate
    /// coordinating Thread and invoke an integration skill").
    pub coordinator_id: String,
    /// The topic's canonical branch resource for the relevant repository —
    /// the nearest ancestor scope that has one, else the topic leader itself.
    pub target_branch: String,
    /// Human label of the coordinator, for the confirmation UI.
    pub coordinator_name: String,
}

/// Resolve where a worker's integration request goes.
///
/// Spec ("Integration target"): do not assume thread ancestry equals git
/// ancestry — default to the nearest ancestor scope with a writable canonical
/// Resource for the relevant repository; if none, the Topic's canonical
/// Resource for that repository.
///
/// Walk conversation ancestors, selecting the topic leader for the same
/// repository when available, otherwise the nearest live parent in that
/// repository. A thread cannot integrate into itself.
pub fn resolve_integration_target(
    fleet: &[SessionSummaryView],
    session_id: &str,
) -> Option<IntegrationTarget> {
    let by_id: std::collections::HashMap<&str, &SessionSummaryView> =
        fleet.iter().map(|s| (s.id.as_str(), s)).collect();
    let by_branch: std::collections::HashMap<&str, &SessionSummaryView> =
        fleet.iter().map(|s| (s.branch.id.as_str(), s)).collect();

    let source = by_id.get(session_id).copied()?;
    let repo = &source.branch.repo_root;
    let mut cur = source;
    let mut seen = std::collections::HashSet::new();
    let mut nearest = None;
    for _ in 0..32 {
        if !seen.insert(cur.id.as_str()) { break; }
        let parent = cur
            .parent_session_id
            .as_deref()
            .and_then(|id| by_id.get(id).copied())
            .or_else(|| {
                cur.parent_id
                    .as_deref()
                    .and_then(|bid| by_branch.get(bid).copied())
            });
        let Some(p) = parent else { break; };
        if p.branch.repo_root == *repo && p.status != "archived" && !p.branch.branch.is_empty() {
            let target = IntegrationTarget {
                coordinator_id: p.id.clone(),
                target_branch: p.branch.branch.clone(),
                coordinator_name: p.branch.name.clone(),
            };
            if nearest.is_none() { nearest = Some(target.clone()); }
            if p.branch.tags.iter().any(|t| t.key == "topic" && t.value != "false") {
                return Some(target);
            }
        }
        cur = p;
    }
    nearest
}

/// Send a structured integration request for a completed worker to its
/// coordinator, with the chosen strategy. The button and the sentence are
/// the same operation (spec: "Integration is an LLM/skill operation").
#[tauri::command]
pub async fn integrate_session(
    app: AppHandle,
    state: State<'_, LoomState>,
    session_id: String,
    strategy: String,
) -> Result<IntegrationTarget, UiError> {
    let client = state_client(&state).await?;
    let strategy = crate::loom::IntegrationStrategy::parse(&strategy).ok_or_else(|| UiError {
        message: format!("unknown integration strategy {strategy:?}"),
        unreachable: false,
    })?;
    if strategy == crate::loom::IntegrationStrategy::Push {
        return Err(UiError { message: "push is a landing strategy".into(), unreachable: false });
    }
    let fleet = client.list_sessions().await?;
    let target =
        resolve_integration_target(&fleet, &session_id).ok_or_else(|| UiError {
            message: "no coordinator found for this session — it has no topic to integrate into".into(),
            unreachable: false,
        })?;
    let view = client.get_session(&session_id).await?;
    let coordinator = client.get_session(&target.coordinator_id).await?;
    if view.branch.repo_root != coordinator.branch.repo_root {
        return Err(UiError {
            message: "source and target are in different repositories; choose a coordinator for this repository".into(),
            unreachable: false,
        });
    }
    if view.branch.branch == target.target_branch {
        return Err(UiError { message: "source and target already use the same branch".into(), unreachable: false });
    }
    if coordinator.status == "archived" {
        return Err(UiError { message: "the coordinator thread is archived; reopen it before integrating".into(), unreachable: false });
    }
    let request = crate::loom::IntegrationRequest {
        action: "integrate",
        source_session: view.id.clone(),
        source_branch: view.branch.branch.clone(),
        source_work_dir: view.work_dir.clone(),
        repo_root: view.branch.repo_root.clone(),
        target_session: target.coordinator_id.clone(),
        target_branch: target.target_branch.clone(),
        strategy,
        requested_by: "user",
    };
    let topic_name = coordinator.branch.name.clone();
    let prompt = request.to_prompt(&topic_name);
    // Queue the structured request behind any active ACP turn. Terminal
    // sessions get the text directly because Loom has no terminal prompt queue.
    if coordinator.protocol == "terminal" {
        client
            .send_text(&target.coordinator_id, &prompt, true)
            .await?;
    } else {
        client
            .queue_prompt(&target.coordinator_id, &prompt)
            .await?;
    }
    // A sent request is not a successful integration. The coordinator's
    // durable conversation records the result after git and validation.
    let _ = app.emit("loom://integration-sent", &target);
    Ok(target)
}

/// Land a topic: send a landing request to the topic leader (the topic's
/// coordinator thread), moving accepted state toward the upstream target.
#[tauri::command]
pub async fn land_topic(
    state: State<'_, LoomState>,
    session_id: String,
    strategy: String,
    upstream: Option<String>,
) -> Result<(), UiError> {
    let client = state_client(&state).await?;
    let strategy = crate::loom::IntegrationStrategy::parse(&strategy).ok_or_else(|| UiError {
        message: format!("unknown landing strategy {strategy:?}"),
        unreachable: false,
    })?;
    if strategy == crate::loom::IntegrationStrategy::CherryPick {
        return Err(UiError { message: "cherry-pick is an integration strategy".into(), unreachable: false });
    }
    let view = client.get_session(&session_id).await?;
    if view.status == "archived" {
        return Err(UiError { message: "the topic thread is archived; reopen it before landing".into(), unreachable: false });
    }
    let fleet = client.list_sessions().await?;
    let summary = fleet.iter().find(|session| session.id == session_id).ok_or_else(|| UiError {
        message: "topic is no longer in the fleet".into(), unreachable: false,
    })?;
    if summary.parent_session_id.is_some() || summary.parent_id.is_some() {
        return Err(UiError { message: "landing belongs to a topic; integrate this worker first".into(), unreachable: false });
    }
    let branches = client.list_branches().await?;
    // The upstream target defaults to the branch's recorded base — the
    // branch loom forked it from, which is the natural upstream.
    let upstream = upstream
        .or_else(|| branches_list_base(&branches, &view.branch.branch))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| UiError { message: "no upstream branch is recorded for this topic".into(), unreachable: false })?;
    let request = crate::loom::LandingRequest {
        action: "land",
        source_branch: view.branch.branch.clone(),
        target_upstream: upstream.clone(),
        strategy,
        requested_by: "user",
    };
    let prompt = request.to_prompt(&view.branch.name);
    if view.protocol == "terminal" {
        client.send_text(&view.id, &prompt, true).await?;
    } else {
        client.queue_prompt(&view.id, &prompt).await?;
    }
    // Do not mark the topic landed before the coordinator reports success.
    Ok(())
}

/// Diff totals against Loom's recorded base ref for this checkout. The
/// endpoint includes committed and uncommitted changes.
#[derive(Debug, Clone, Serialize)]
pub struct WorkSummary {
    pub additions: u32,
    pub deletions: u32,
    pub files: u32,
    pub has_commits: bool,
}

/// Find a branch's recorded base branch from `branches.list` rows.
/// The DTO carries `base_branch` per branch (BranchView).
fn branches_list_base(
    branches: &[serde_json::Value],
    branch: &str,
) -> Option<String> {
    branches
        .iter()
        .find(|b| b.get("branch").and_then(|v| v.as_str()) == Some(branch))
        .and_then(|b| b.get("base_branch"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

#[tauri::command]
pub async fn work_summary(
    state: State<'_, LoomState>,
    session_id: String,
) -> Result<WorkSummary, UiError> {
    let client = state_client(&state).await?;
    let changes = client.session_changes(&session_id).await?;
    Ok(WorkSummary {
        additions: changes.totals.additions,
        deletions: changes.totals.deletions,
        files: changes.totals.files,
        has_commits: changes.head_oid.as_deref() != changes.base.get("oid").and_then(|value| value.as_str()),
    })
}

/// The complete bounded Loom change set for the review pane.
#[tauri::command]
pub async fn work_changes(
    state: State<'_, LoomState>,
    session_id: String,
) -> Result<crate::loom::ChangeSetView, UiError> {
    let client = state_client(&state).await?;
    client.session_changes(&session_id).await.map_err(Into::into)
}

fn resource_error(message: impl Into<String>) -> UiError {
    UiError { message: message.into(), unreachable: false }
}

async fn load_topic_resources(client: &LoomClient, branch_id: &str) -> Result<TopicResourcesView, UiError> {
    let artifact = match client.branch_artifact(branch_id, MANIFEST_NAME).await {
        Ok(value) => value,
        Err(LoomError::Api { status: 404, .. }) => return Ok(TopicResourcesView::default()),
        Err(error) => return Err(error.into()),
    };
    if artifact.get("meta").and_then(|v| v.get("branch_id")).and_then(|v| v.as_str()) != Some(branch_id) {
        return Err(resource_error("resource manifest name is occupied by a repository-shared artifact"));
    }
    let content = artifact.get("content").and_then(|v| v.as_str())
        .ok_or_else(|| resource_error("resource manifest has no content"))?;
    let mut manifest: TopicResourcesView = serde_json::from_str(content)
        .map_err(|e| resource_error(format!("invalid resource manifest: {e}")))?;
    manifest.revision = artifact.get("meta").and_then(|v| v.get("rev"))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| resource_error("resource manifest has no revision"))?;
    Ok(manifest)
}

async fn resource_context(
    client: &LoomClient,
    mentions: Vec<ResourceMention>,
) -> Result<String, UiError> {
    if mentions.len() > 12 { return Err(resource_error("at most 12 resources can be mentioned in one message")); }
    let mut seen = std::collections::HashSet::new();
    let mut resolved = Vec::new();
    for mention in mentions {
        if !seen.insert((mention.topic_id.clone(), mention.resource_id.clone())) { continue; }
        let topic = client.get_session(&mention.topic_id).await?;
        let manifest = load_topic_resources(client, &topic.branch.id).await?;
        let resource = manifest.resources.into_iter().find(|resource| resource.id == mention.resource_id)
            .ok_or_else(|| resource_error(format!("mentioned resource no longer exists: {}", mention.resource_id)))?;
        resolved.push(serde_json::json!({ "source_topic": mention.topic_id, "resource": resource }));
    }
    if resolved.is_empty() { return Ok(String::new()); }
    let content = serde_json::to_string_pretty(&resolved)
        .map_err(|e| resource_error(format!("serializing resource mentions: {e}")))?;
    Ok(format!("\n\nReferenced topic resources (current Arachne bindings; use these locators to inspect the resources):\n{content}"))
}

async fn save_topic_resources(client: &LoomClient, branch_id: &str, manifest: &TopicResourcesView, expected_revision: i64) -> Result<TopicResourcesView, UiError> {
    let content = serde_json::to_string_pretty(&manifest)
        .map_err(|e| resource_error(format!("serializing resources: {e}")))?;
    let artifact = client.write_branch_artifact(branch_id, MANIFEST_NAME, &content, expected_revision).await?;
    if artifact.get("meta").and_then(|v| v.get("branch_id")).and_then(|v| v.as_str()) != Some(branch_id) {
        return Err(resource_error("resource manifest was not saved on the topic branch"));
    }
    let mut saved = manifest.clone();
    saved.revision = artifact.get("meta").and_then(|v| v.get("rev"))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| resource_error("saved resource manifest has no revision"))?;
    Ok(saved)
}

#[tauri::command]
pub async fn topic_resources(state: State<'_, LoomState>, topic_id: String) -> Result<TopicResourcesView, UiError> {
    let client = state_client(&state).await?;
    let topic = client.get_session(&topic_id).await?;
    load_topic_resources(&client, &topic.branch.id).await
}

#[tauri::command]
pub async fn attach_topic_resource(
    state: State<'_, LoomState>, topic_id: String, resource: ResourceDraft,
    expected_revision: i64,
) -> Result<TopicResourcesView, UiError> {
    let client = state_client(&state).await?;
    let topic = client.get_session(&topic_id).await?;
    let resource = resource.validated(&topic.branch.repo_root, &topic.branch.branch)
        .map_err(resource_error)?;
    let mut manifest = load_topic_resources(&client, &topic.branch.id).await?;
    if manifest.revision != expected_revision { return Err(resource_error("resources changed; reload before editing")); }
    if let Some(existing) = manifest.resources.iter_mut().find(|r| r.id == resource.id) {
        *existing = resource;
    } else {
        manifest.resources.push(resource);
    }
    save_topic_resources(&client, &topic.branch.id, &manifest, expected_revision).await
}

#[tauri::command]
pub async fn detach_topic_resource(
    state: State<'_, LoomState>, topic_id: String, resource_id: String,
    expected_revision: i64,
) -> Result<TopicResourcesView, UiError> {
    let client = state_client(&state).await?;
    let topic = client.get_session(&topic_id).await?;
    let mut manifest = load_topic_resources(&client, &topic.branch.id).await?;
    if manifest.revision != expected_revision { return Err(resource_error("resources changed; reload before editing")); }
    let before = manifest.resources.len();
    manifest.resources.retain(|r| r.id != resource_id);
    if manifest.resources.len() == before { return Err(resource_error("resource not found")); }
    save_topic_resources(&client, &topic.branch.id, &manifest, expected_revision).await
}

async fn resolve_topic_resource(client: &LoomClient, topic_id: &str, resource_id: &str) -> Result<(SessionView, TopicResource), UiError> {
    let topic = client.get_session(topic_id).await?;
    let manifest = load_topic_resources(client, &topic.branch.id).await?;
    let resource = manifest.resources.into_iter().find(|r| r.id == resource_id)
        .ok_or_else(|| resource_error("resource not found"))?;
    Ok((topic, resource))
}

#[tauri::command]
pub async fn read_topic_resource(
    state: State<'_, LoomState>, topic_id: String, resource_id: String,
) -> Result<TopicResourceContent, UiError> {
    let client = state_client(&state).await?;
    let (topic, resource) = resolve_topic_resource(&client, &topic_id, &resource_id).await?;
    let content = match resource.data.kind {
        ResourceKind::File | ResourceKind::DesignDocument => {
            if topic.status == "archived" { return Err(resource_error("topic checkout is archived; recover it to preview this file")); }
            if resource.data.reference.as_deref() != Some(&topic.branch.branch) {
                return Err(resource_error("file reference no longer matches the topic branch"));
            }
            let path = resource.data.path.as_deref().ok_or_else(|| resource_error("resource has no file path"))?;
            crate::resources::validate_relative_path(path).map_err(resource_error)?;
            client.worktree_text(&topic_id, path).await?
        }
        ResourceKind::Artifact => {
            let name = resource.data.path.as_deref().ok_or_else(|| resource_error("resource has no artifact name"))?;
            client.branch_artifact(&topic.branch.id, name).await?
                .get("content").and_then(|v| v.as_str()).ok_or_else(|| resource_error("artifact has no content"))?.to_owned()
        }
        _ => return Err(resource_error("this resource has no text preview")),
    };
    Ok(TopicResourceContent { resource, content })
}

#[tauri::command]
pub async fn open_topic_resource_in_zed(
    state: State<'_, LoomState>, topic_id: String, resource_id: String,
) -> Result<(), UiError> {
    let client = state_client(&state).await?;
    let (topic, resource) = resolve_topic_resource(&client, &topic_id, &resource_id).await?;
    if !matches!(resource.data.kind, ResourceKind::File | ResourceKind::DesignDocument) {
        return Err(resource_error("only repository files can open in Zed"));
    }
    if topic.status == "archived" { return Err(resource_error("topic checkout is archived; recover it before opening a file")); }
    if resource.data.reference.as_deref() != Some(&topic.branch.branch) {
        return Err(resource_error("file reference no longer matches the topic branch"));
    }
    let path = resource.data.path.as_deref().ok_or_else(|| resource_error("resource has no file path"))?;
    crate::resources::validate_relative_path(path).map_err(resource_error)?;
    let host = client.server_host().ok_or_else(|| resource_error("Loom URL has no host"))?;
    let file = std::path::Path::new(&topic.work_dir).join(path);
    let target = zed_target(host, &file.to_string_lossy())?;
    let cli = if std::path::Path::new("/Applications/Zed.app/Contents/MacOS/cli").exists() {
        "/Applications/Zed.app/Contents/MacOS/cli"
    } else { "zed" };
    let status = tokio::process::Command::new(cli).arg(&target).status().await
        .map_err(|e| resource_error(format!("launching zed: {e}")))?;
    if !status.success() { return Err(resource_error(format!("zed exited with {status}"))); }
    Ok(())
}

#[cfg(test)]
mod integration_tests {
    use super::resolve_integration_target;
    use crate::loom::SessionSummaryView;

    fn session(id: &str, branch: &str, repo: &str, parent: Option<&str>, topic: bool) -> SessionSummaryView {
        serde_json::from_value(serde_json::json!({
            "id": id, "status": "running", "profile": "default", "class": "interactive",
            "origin": "user", "created_by": null, "created_at": "2026-01-01T00:00:00Z",
            "last_activity_at": "2026-01-01T00:00:00Z", "placement": null,
            "github_repo": null, "parent_id": null, "parent_session_id": parent,
            "branch": { "id": id, "branch": branch, "name": id, "title": id,
                "repo_root": repo, "tags": if topic { serde_json::json!([{
                    "key":"topic", "value":"true", "note":"", "set_at":"", "set_by":"arachne"
                }]) } else { serde_json::json!([]) }
            }
        })).unwrap()
    }

    #[test]
    fn nested_worker_integrates_to_same_repo_topic() {
        let fleet = vec![
            session("topic", "topic-branch", "/repo", None, true),
            session("parent", "parent-branch", "/repo", Some("topic"), false),
            session("child", "child-branch", "/repo", Some("parent"), false),
        ];
        let target = resolve_integration_target(&fleet, "child").unwrap();
        assert_eq!(target.coordinator_id, "topic");
        assert_eq!(target.target_branch, "topic-branch");
        assert!(resolve_integration_target(&fleet, "topic").is_none());
    }

    #[test]
    fn cross_repo_topic_uses_nearest_same_repo_parent() {
        let fleet = vec![
            session("topic", "other-topic", "/other", None, true),
            session("parent", "parent-branch", "/repo", Some("topic"), false),
            session("child", "child-branch", "/repo", Some("parent"), false),
        ];
        assert_eq!(resolve_integration_target(&fleet, "child").unwrap().target_branch, "parent-branch");
        assert!(resolve_integration_target(&fleet, "parent").is_none());
    }
}
