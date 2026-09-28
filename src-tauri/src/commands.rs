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

use base64::Engine as _;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::client::{LoomClient, LoomError};
use crate::landing::LocalLanding;
use crate::loom::{LaunchOptionsView, SessionSummaryView, SessionView};
use crate::resources::{
    ResourceDraft, ResourceKind, ResourceMention, TodoItem, TodoListView, TodoTopicView,
    TopicResource, TopicResourceContent, TopicResourcesView, MANIFEST_NAME, TODOS_NAME,
};

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
    pub metadata: crate::loom::AcpMetadataView,
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
    state.client.read().await.clone().ok_or_else(|| UiError {
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

/// `repos.branches` for the launch sheet's Base picker: the local git
/// branches of the repo the sheet is launching against. `repo` may be a
/// managed `owner/name` slug (resolved through `repos.list` to its checkout
/// path) or a plain server-side path — whatever loom's launch accepts, the
/// picker accepts too.
#[tauri::command]
pub async fn repo_branches(
    state: State<'_, LoomState>,
    repo: String,
) -> Result<Vec<crate::loom::RepoBranchView>, UiError> {
    let client = state_client(&state).await?;
    let slug = repo.trim();
    let cwd = match client.list_repos().await {
        Ok(repos) => match repos.iter().find(|r| r.slug == slug) {
            Some(managed) => managed.path.clone(),
            None => slug.to_string(),
        },
        // The allowlist lookup is a convenience, not a gate: a path (or an
        // unlisted slug) still gets handed to loom, which reports the real
        // error if nothing resolves.
        Err(_) => slug.to_string(),
    };
    client.repo_branches(&cwd).await.map_err(Into::into)
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
        .handoff_session(
            &id,
            &profile,
            agent.as_deref(),
            model.as_deref(),
            effort.as_deref(),
        )
        .await
        .map_err(Into::into)
}

/// Fetch summaries + layout together and push them as a fleet snapshot.
async fn emit_fleet(
    app: &AppHandle,
    client: &Arc<LoomClient>,
    mut sessions: Vec<SessionSummaryView>,
) {
    let layout = match client.session_layout().await {
        Ok(l) => l,
        Err(e) => {
            let _ = app.emit("loom://error", UiError::from(e));
            return;
        }
    };
    sessions.sort_by(|a, b| a.last_activity_at.cmp(&b.last_activity_at));
    let _ = app.emit("loom://fleet", &FleetSnapshot { sessions, layout });
}

/// `emit_fleet` with a caller-supplied layout (a mutation command already
/// holds the fresh one) and a best-effort session list fetch. Pushing a
/// snapshot for the mutation's own effect must never fail the command that
/// produced it — loom's `layout` event also publishes it.
async fn emit_fleet_with(
    app: &AppHandle,
    client: &Arc<LoomClient>,
    layout: crate::loom::SessionLayoutView,
) {
    let mut sessions = match client.list_sessions().await {
        Ok(s) => s,
        Err(_) => return,
    };
    sessions.sort_by(|a, b| a.last_activity_at.cmp(&b.last_activity_at));
    let _ = app.emit("loom://fleet", &FleetSnapshot { sessions, layout });
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
    topics.extend(
        live.into_iter()
            .take(63)
            .map(|session| format!("session:{}", session.id)),
    );
    topics
}

/// Open a session and start its chat forwarder. A queued prompt in a lost ACP
/// runtime is work the user already requested, so resume it when opened.
#[tauri::command]
pub async fn open_session(
    app: AppHandle,
    state: State<'_, LoomState>,
    id: String,
) -> Result<SessionView, UiError> {
    let client = state_client(&state).await?;
    let original = client.get_session(&id).await?;
    let view = match resume_queued_on_open(&client, &id, original.clone()).await {
        Ok(view) => view,
        Err(error) => {
            let _ = app.emit(
                "loom://error",
                UiError {
                    message: "Could not resume the queued message. Sending a message will retry."
                        .into(),
                    unreachable: error.is_unreachable(),
                },
            );
            client.get_session(&id).await.unwrap_or(original)
        }
    };

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

async fn resume_queued_on_open(
    client: &LoomClient,
    id: &str,
    mut view: SessionView,
) -> Result<SessionView, LoomError> {
    if view.status != "orphaned" || view.protocol != "acp" {
        return Ok(view);
    }
    let chat = client.session_chat(id, None).await?;
    if !chat
        .pending_prompt
        .as_deref()
        .is_some_and(|prompt| !prompt.trim().is_empty())
    {
        return Ok(view);
    }
    view = client.resume_if_orphaned(id).await?;
    let chat = client.session_chat(id, None).await?;
    if chat.live_turn.is_none()
        && chat
            .pending_prompt
            .as_deref()
            .is_some_and(|prompt| !prompt.trim().is_empty())
    {
        if let Err(error) = client.send_queued_prompt(id).await {
            // A resumed turn can drain the queue between our read and send.
            let latest = client.session_chat(id, None).await?;
            if latest.live_turn.is_none()
                && latest
                    .pending_prompt
                    .as_deref()
                    .is_some_and(|prompt| !prompt.trim().is_empty())
            {
                return Err(error);
            }
        }
    }
    Ok(view)
}

#[cfg(test)]
mod queued_recovery_tests {
    use super::{resume_queued_on_open, SessionView};
    use crate::client::LoomClient;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn opening_an_orphan_with_queued_work_resumes_and_dispatches_it() {
        let base = serde_json::json!({
            "id":"session", "status":"orphaned", "profile":"default", "class":"interactive",
            "origin":"user", "agent_kind":"codex", "model":"", "effort":"", "protocol":"acp",
            "work_dir":"/tmp/work", "term_session":"runtime", "turn_count":0,
            "created_by":null, "created_at":"now", "last_activity_at":"now",
            "branch":{"id":"branch", "branch":"topic", "name":"topic", "title":"Topic",
                      "repo_root":"/tmp", "tags":[]}
        });
        let mut running = base.clone();
        running["status"] = "running".into();
        let view: SessionView = serde_json::from_value(base.clone()).unwrap();
        let queued = serde_json::json!({"blocks":[],"live_turn":null,"pending_prompt":"queued work","older_cursor":null});
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                ("/api/sessions/chat", queued.clone()),
                ("/api/sessions/get", base),
                ("/api/sessions/adopt", running.clone()),
                ("/api/sessions/chat", queued),
                (
                    "/api/sessions/prompt/create",
                    serde_json::json!({"queued":false,"turn":1}),
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 2048];
                let count = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..count])
                    .starts_with(&format!("POST {path} ")));
                let body = body.to_string();
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = LoomClient::new(&format!("http://{addr}/"), None).unwrap();
        assert_eq!(
            resume_queued_on_open(&client, "session", view)
                .await
                .unwrap()
                .status,
            "running"
        );
        server.await.unwrap();
    }
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
        metadata: chat.metadata,
    })
}

/// Set one live ACP selector (model, thinking level, mode, etc.). Its IDs and
/// value shapes come from sessions.chat.metadata.config_options.
#[tauri::command]
pub async fn set_session_config(
    state: State<'_, LoomState>,
    id: String,
    config_id: String,
    value: serde_json::Value,
) -> Result<crate::loom::AcpMetadataView, UiError> {
    let client = state_client(&state).await?;
    client
        .set_session_config(&id, &config_id, value)
        .await
        .map_err(Into::into)
}

/// Complete file mentions from the session's checkout on the loom host.
#[tauri::command]
pub async fn complete_files(
    state: State<'_, LoomState>,
    id: String,
    query: String,
) -> Result<Vec<String>, UiError> {
    let client = state_client(&state).await?;
    client.session_files(&id, &query).await.map_err(Into::into)
}

/// Return a bounded raster image attached to a session as a data URL. Loom
/// resolves `name` inside that session's worktree and authorizes the read.
#[tauri::command]
pub async fn load_session_image(
    state: State<'_, LoomState>,
    id: String,
    name: String,
) -> Result<String, UiError> {
    let client = state_client(&state).await?;
    client.session_image(&id, &name).await.map_err(Into::into)
}

/// Send input to a session: ACP prompt (agents) or terminal text (raw).
#[tauri::command]
pub async fn send_input(
    state: State<'_, LoomState>,
    id: String,
    text: String,
    topic_id: Option<String>,
    resource_ids: Option<Vec<String>>,
    attachments: Option<Vec<crate::loom::ScratchUpload>>,
) -> Result<(), UiError> {
    let client = state_client(&state).await?;
    let mut prompt = text;
    let ids = resource_ids.unwrap_or_default();
    if !ids.is_empty() {
        let topic_id =
            topic_id.ok_or_else(|| resource_error("resource mentions require a topic"))?;
        let mentions = ids
            .into_iter()
            .map(|resource_id| ResourceMention {
                topic_id: topic_id.clone(),
                resource_id,
            })
            .collect();
        prompt.push_str(&resource_context(&client, mentions).await?);
    }
    // An orphan is a recoverable runtime gap, not a different kind of user
    // conversation. Resume before uploading attachments or sending input.
    let view = client.resume_if_orphaned(&id).await?;
    let mut files = Vec::new();
    for (name, bytes) in decode_attachments(attachments.unwrap_or_default())? {
        files.push(client.upload_scratch(&id, &name, bytes).await?);
    }
    if !files.is_empty() {
        prompt.push_str("\n\nAttached files in this session's Scratch directory:\n");
        for path in &files {
            prompt.push_str(&format!("- {path}\n"));
        }
    }
    if view.protocol == "terminal" {
        client.send_text(&id, &prompt, true).await?;
    } else {
        client.send_prompt(&id, &prompt, &files).await?;
    }
    Ok(())
}

fn decode_attachments(
    uploads: Vec<crate::loom::ScratchUpload>,
) -> Result<Vec<(String, Vec<u8>)>, UiError> {
    if uploads.len() > 20 {
        return Err(resource_error("attach at most 20 files"));
    }
    let mut total = 0usize;
    let mut names = std::collections::HashSet::new();
    let mut decoded = Vec::new();
    for upload in uploads {
        let name = &upload.name;
        if name.is_empty()
            || name.trim() != name
            || name == "."
            || name == ".."
            || name.len() > 240
            || name.contains(['/', '\\'])
            || name.chars().any(char::is_control)
            || name.eq_ignore_ascii_case(".gitignore")
            || !names.insert(name.clone())
        {
            return Err(resource_error(format!(
                "invalid or duplicate attachment name: {name}"
            )));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&upload.content_base64)
            .map_err(|e| resource_error(format!("decoding attachment {name}: {e}")))?;
        if bytes.len() > 25 * 1024 * 1024 {
            return Err(resource_error(format!(
                "{name} exceeds Loom's 25 MiB file limit"
            )));
        }
        total += bytes.len();
        if total > 50 * 1024 * 1024 {
            return Err(resource_error(
                "attachments exceed Loom's 50 MiB total limit",
            ));
        }
        decoded.push((name.clone(), bytes));
    }
    Ok(decoded)
}

#[cfg(test)]
mod attachment_tests {
    use super::decode_attachments;
    use crate::loom::ScratchUpload;

    #[test]
    fn decodes_binary_and_rejects_unsafe_names_before_upload() {
        let upload = ScratchUpload {
            name: "image.png".into(),
            content_base64: "AAECA/8=".into(),
        };
        assert_eq!(
            decode_attachments(vec![upload]).unwrap()[0].1,
            [0, 1, 2, 3, 255]
        );
        let bad = ScratchUpload {
            name: "../image.png".into(),
            content_base64: "AA==".into(),
        };
        assert!(decode_attachments(vec![bad]).is_err());
    }
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
    base: Option<String>,
    mentions: Option<Vec<ResourceMention>>,
    attachments: Option<Vec<crate::loom::ScratchUpload>>,
    profile: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    // Project the thread was launched preselected to — the layout group
    // to file the new session into.
    project: Option<crate::loom::ProjectRef>,
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
    // A bare prompt (the New thread sheet with no explicit title) sends
    // only `task`, so the title falls back to it like the CLI; the expanded
    // topic card sends an explicit short title and longer body.
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
    let attachments = attachments.unwrap_or_default();
    // Reject malformed uploads before Loom creates a branch/session. Loom
    // validates the same limits again when it writes Scratch on its host.
    let launch_bytes: usize = decode_attachments(attachments.clone())?
        .iter()
        .map(|(_, bytes)| bytes.len())
        .sum();
    if launch_bytes > 45 * 1024 * 1024 {
        return Err(resource_error(
            "launch attachments exceed the 45 MiB JSON request limit",
        ));
    }
    let mut goal = task;
    if let Some(mentions) = mentions {
        goal.push_str(&resource_context(&client, mentions).await?);
    }
    if !attachments.is_empty() {
        goal.push_str("\n\nAttached files in Scratch:\n");
        for file in &attachments {
            goal.push_str(&format!("- scratch/{}\n", file.name));
        }
    }
    let view = client
        .launch(&crate::loom::SessionsLaunchInput {
            repo: Some(repo),
            base: base
                .as_deref()
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .map(String::from),
            title: Some(label),
            goal: Some(goal),
            parent_branch: parent_branch.clone(),
            // Empty strings mean "inherit the server default" — filter to
            // None so loom sees omitted-vs-blank as intended.
            profile: profile.filter(|s| !s.is_empty()),
            launch_guidance: Some(crate::loom::topic_launch_guidance()),
            agent: agent.filter(|s| !s.is_empty()),
            model: model.filter(|s| !s.is_empty()),
            effort: effort.filter(|s| !s.is_empty()),
            scratch: attachments,
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
    // New root sessions are topics by default: stamp the durable marker
    // so the topic card and inspector recognize them.
    if parent_branch.is_none() {
        let _ = client
            .set_tag(&view.id, "topic", "true", "session is a topic")
            .await;
    }
    // File the new topic into its project when one was preselected (a
    // project is a placement group — filing only, never execution state).
    if let Some(group_id) = project
        .as_ref()
        .and_then(|p| p.id.as_deref())
        .filter(|id| !id.is_empty())
    {
        let _ = client.move_sessions(&[view.id.as_str()], group_id).await;
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

/// Open the macOS file picker at the local checkout of the selected repo.
/// The checkout path comes from Loom's session summaries, rather than from a
/// guessed location on the client machine.
#[tauri::command]
pub async fn pick_topic_files(
    state: State<'_, LoomState>,
    repo: String,
) -> Result<Vec<String>, UiError> {
    let client = state_client(&state).await?;
    let sessions = client.list_sessions().await?;
    let checkout = sessions
        .iter()
        .filter(|s| s.github_repo.as_deref() == Some(repo.as_str()))
        .map(|s| s.branch.repo_root.as_str())
        .find(|path| std::path::Path::new(path).is_dir())
        .or_else(|| std::path::Path::new(&repo).is_dir().then_some(repo.as_str()))
        .ok_or_else(|| UiError {
            message: format!("no local checkout found for {repo}"),
            unreachable: false,
        })?;

    // Pass the path as an argv item so names containing quotes or other
    // AppleScript syntax cannot change the script. A cancelled picker returns
    // -128, which is an ordinary empty selection.
    let script = r#"
on run argv
    set checkout to POSIX file (item 1 of argv) as alias
    set picked to choose file with prompt "Attach files to new topic" default location checkout with multiple selections allowed
    set paths to {}
    repeat with fileItem in picked
        set end of paths to POSIX path of fileItem
    end repeat
    set AppleScript's text item delimiters to (character id 30)
    return paths as text
end run
"#;
    let output = tokio::process::Command::new("osascript")
        .args(["-e", script, "--", checkout])
        .output()
        .await
        .map_err(|e| UiError {
            message: format!("opening file picker: {e}"),
            unreachable: false,
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("(-128)") {
            return Ok(vec![]);
        }
        return Err(UiError {
            message: format!("file picker failed: {}", stderr.trim()),
            unreachable: false,
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .trim_end_matches('\n')
        .split('\u{1e}')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect())
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
    agent: Option<String>,
    model: Option<String>,
    effort: Option<String>,
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
            launch_guidance: Some(crate::loom::topic_launch_guidance()),
            // Launch overrides from the picker: empty strings mean "inherit
            // the server default", so filter them to None (loom's
            // omitted-vs-blank distinction).
            agent: agent.filter(|s| !s.is_empty()),
            model: model.filter(|s| !s.is_empty()),
            effort: effort.filter(|s| !s.is_empty()),
            ..Default::default()
        })
        .await?;
    // Self-heal the parent's topic marker: leaders launched from the CLI (or
    // before this marker existed) should still hold their shape.
    let _ = client
        .set_tag(&parent_id, "topic", "true", "joined by child session")
        .await;
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
pub async fn open_in_zed(
    state: State<'_, LoomState>,
    id: String,
    work_dir: Option<String>,
) -> Result<(), UiError> {
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
    // A caller-supplied path (post `recover_worktree`) wins over the stale
    // session view: the recovery just materialized this checkout server-side.
    let target = zed_target(
        host,
        work_dir.as_deref().unwrap_or(&view.work_dir),
    )?;
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

/// Open a macOS Terminal window at the checkout path.
#[tauri::command]
pub async fn open_in_terminal(path: String) -> Result<(), UiError> {
    // `open -a Terminal <dir>` opens a window at the directory on macOS;
    // -a stays silent when Terminal is missing (unlikely on this target).
    let status = tokio::process::Command::new("open")
        .args(["-a", "Terminal", &path])
        .status()
        .await
        .map_err(|e| UiError {
            message: format!("launching Terminal: {e}"),
            unreachable: false,
        })?;
    if !status.success() {
        return Err(UiError {
            message: format!("open exited with {status}"),
            unreachable: false,
        });
    }
    Ok(())
}

/// Recover a session's checkout after it was archived (or its worktree
/// vanished): `repos.worktrees.ensure` materializes the branch under
/// `.worktrees/<slug>` on the loom server, idempotently. Returns the fresh
/// path so the caller can open it in Zed right away. Recovery never
/// resurrects the session's agent — it just gives the human a checkout.
#[tauri::command]
pub async fn recover_worktree(
    state: State<'_, LoomState>,
    repo_root: String,
    branch: String,
) -> Result<crate::loom::RepoWorktreeView, UiError> {
    let client = state_client(&state).await?;
    client.ensure_worktree(&repo_root, &branch).await.map_err(Into::into)
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

/// Create a project (a placement group in a space). Projects are filing
/// only — a group with no execution state (docs/design.md "User model").
#[tauri::command]
pub async fn create_group(
    app: AppHandle,
    state: State<'_, LoomState>,
    space_id: String,
    name: String,
) -> Result<crate::loom::SessionLayoutView, UiError> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(UiError {
            message: "project name cannot be empty".into(),
            unreachable: false,
        });
    }
    let client = state_client(&state).await?;
    let layout = client
        .create_group(&space_id, &name)
        .await
        .map_err(Into::<UiError>::into)?;
    // The fleet poller's `layout` subscription pushes the snapshot, but the
    // new group should appear immediately — push it from the fresh layout.
    emit_fleet_with(&app, &client, layout.clone()).await;
    Ok(layout)
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
    app: AppHandle,
    state: State<'_, LoomState>,
    session_ids: Vec<String>,
    group_id: String,
) -> Result<crate::loom::SessionLayoutView, UiError> {
    let client = state_client(&state).await?;
    let refs: Vec<&str> = session_ids.iter().map(|s| s.as_str()).collect();
    let layout = client.move_sessions(&refs, &group_id).await?;
    // Loom publishes a layout SSE event, but push the fresh snapshot now so
    // the topic visibly files under its new project heading immediately.
    emit_fleet_with(&app, &client, layout.clone()).await;
    Ok(layout)
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
        let _ = client
            .set_tag(parent, "topic", "true", "joined by child session")
            .await;
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
        if !seen.insert(cur.id.as_str()) {
            break;
        }
        let parent = cur
            .parent_session_id
            .as_deref()
            .and_then(|id| by_id.get(id).copied())
            .or_else(|| {
                cur.parent_id
                    .as_deref()
                    .and_then(|bid| by_branch.get(bid).copied())
            });
        let Some(p) = parent else {
            break;
        };
        if p.branch.repo_root == *repo && p.status != "archived" && !p.branch.branch.is_empty() {
            let target = IntegrationTarget {
                coordinator_id: p.id.clone(),
                target_branch: p.branch.branch.clone(),
                coordinator_name: p.branch.name.clone(),
            };
            if nearest.is_none() {
                nearest = Some(target.clone());
            }
            if p.branch
                .tags
                .iter()
                .any(|t| t.key == "topic" && t.value != "false")
            {
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
        return Err(UiError {
            message: "push is a landing strategy".into(),
            unreachable: false,
        });
    }
    let fleet = client.list_sessions().await?;
    let target = resolve_integration_target(&fleet, &session_id).ok_or_else(|| UiError {
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
        return Err(UiError {
            message: "source and target already use the same branch".into(),
            unreachable: false,
        });
    }
    if coordinator.status == "archived" {
        return Err(UiError {
            message: "the coordinator thread is archived; reopen it before integrating".into(),
            unreachable: false,
        });
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
    client.resume_if_orphaned(&target.coordinator_id).await?;
    // Queue the structured request behind any active ACP turn. Terminal
    // sessions get the text directly because Loom has no terminal prompt queue.
    if coordinator.protocol == "terminal" {
        client
            .send_text(&target.coordinator_id, &prompt, true)
            .await?;
    } else {
        client.queue_prompt(&target.coordinator_id, &prompt).await?;
    }
    // A sent request is not a successful integration. The coordinator's
    // durable conversation records the result after git and validation.
    let _ = app.emit("loom://integration-sent", &target);
    Ok(target)
}

/// How a landing target was resolved — the provenance the prompt and the
/// landing agent need to know what to re-verify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingTargetOrigin {
    /// The primary checkout's currently checked out branch.
    PrimaryCheckout,
    /// `main` — the checkout's branch could not be read; the landing agent
    /// should try to resolve the primary checkout itself before settling.
    MainFallback,
    /// The branch's recorded base (the remote's default branch) — the
    /// open-PR strategy's target.
    RecordedBase,
    /// An explicit override from the UI.
    Explicit,
}

impl LandingTargetOrigin {
    fn as_str(self) -> &'static str {
        match self {
            Self::PrimaryCheckout => "primary-checkout",
            Self::MainFallback => "main-fallback",
            Self::RecordedBase => "recorded-base",
            Self::Explicit => "explicit",
        }
    }
}

/// Resolve the landing target for a non-PR strategy: the primary checkout's
/// currently checked out branch — the checkout a human actually opens —
/// falling back to `main` when it cannot be resolved (no such branch, no
/// readable checkout, or the loom server cannot report it).
///
/// `repo_branches` mirrors loom's `repos.branches` rows, which mark the
/// primary checkout's current branch. Only the row that is both `current`
/// and checked out at `repo_root` (the main working tree) may become the
/// target — a worker worktree's own branch must never win, even if a future
/// loom marked every worktree's branch `current`. The topic's own branch is
/// skipped when matched: landing into it would be a no-op self-merge, so the
/// `main` fallback applies instead.
fn resolve_landing_target(
    repo_branches: &[crate::loom::RepoBranchView],
    repo_root: &str,
    topic_branch: &str,
    explicit: Option<&str>,
) -> Option<(String, LandingTargetOrigin)> {
    if let Some(value) = explicit {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some((trimmed.to_string(), LandingTargetOrigin::Explicit));
        }
    }
    if let Some(branch) = repo_branches
        .iter()
        .find(|b| b.current && b.worktree.as_deref() == Some(repo_root))
    {
        if branch.name != topic_branch {
            return Some((branch.name.clone(), LandingTargetOrigin::PrimaryCheckout));
        }
        // The primary checkout holds the topic branch itself: landing there
        // is a no-op self-merge. Fall through to `main`.
    }
    // No current row at the primary checkout: it is detached or the checkout
    // could not be read. `main` is the fallback the landing agent should try
    // to improve on; when even `main` is absent the caller falls back to the
    // recorded base.
    let main_exists = repo_branches.iter().any(|b| b.name == "main");
    if main_exists || repo_branches.is_empty() {
        return Some(("main".into(), LandingTargetOrigin::MainFallback));
    }
    None
}

/// The result of a Land action. The agent-mediated strategies just send a
/// request (their outcome is reported in the topic thread); `land-locally`
/// runs synchronously and returns the completed squash-merge.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LandResult {
    /// True when this landing happened locally, right now.
    pub local: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub landing: Option<LocalLandingView>,
}

/// Land a topic: move accepted state toward the upstream target. Most
/// strategies send a structured landing request to the topic leader (its
/// coordinator thread); `land-locally` is the deterministic exception —
/// Arachne squash-merges the topic into the primary checkout's current
/// branch itself, with no agent turn, and reports the result
/// synchronously.
#[tauri::command]
pub async fn land_topic(
    state: State<'_, LoomState>,
    session_id: String,
    strategy: String,
    upstream: Option<String>,
) -> Result<LandResult, UiError> {
    let client = state_client(&state).await?;
    let strategy = crate::loom::IntegrationStrategy::parse(&strategy).ok_or_else(|| UiError {
        message: format!("unknown landing strategy {strategy:?}"),
        unreachable: false,
    })?;
    if matches!(
        strategy,
        crate::loom::IntegrationStrategy::CherryPick | crate::loom::IntegrationStrategy::Ask
    ) {
        return Err(UiError {
            message: "choose a concrete landing strategy".into(),
            unreachable: false,
        });
    }
    let view = client.get_session(&session_id).await?;
    if view.status == "archived" {
        return Err(UiError {
            message: "the topic thread is archived; reopen it before landing".into(),
            unreachable: false,
        });
    }
    let fleet = client.list_sessions().await?;
    let summary = fleet
        .iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| UiError {
            message: "topic is no longer in the fleet".into(),
            unreachable: false,
        })?;
    if summary.parent_session_id.is_some() || summary.parent_id.is_some() {
        return Err(UiError {
            message: "landing belongs to a topic; integrate this worker first".into(),
            unreachable: false,
        });
    }
    // The deterministic fast path: `land-locally` runs the squash-merge
    // right here — no agent turn. Loom's API has no git write operations,
    // so Arachne runs git itself, and only where its paths are meaningful:
    // a loopback server runs sessions in checkouts on this same machine.
    // A remote server's `repo_root` is a server-side path, never a local
    // one — remote landings stay with the agent-mediated strategies.
    if strategy.is_local() {
        let landing = land_topic_locally(client, view, summary).await?;
        return Ok(LandResult {
            local: true,
            landing: Some(landing),
        });
    }
    // Resolve the landing target. Non-PR strategies land into the primary
    // checkout's currently checked out branch — the checkout a human
    // actually opens — falling back to `main`; `open-pr` targets the
    // branch's recorded base, the remote's default branch. An explicit
    // `upstream` overrides either. A failed primary-checkout lookup is not
    // fatal for open-PR (which never needs it), so the order matters.
    let branches = client.list_branches().await?;
    let (upstream, target_origin) = if matches!(strategy, crate::loom::IntegrationStrategy::OpenPr)
    {
        let upstream = upstream
            .or_else(|| branches_list_base(&branches, &view.branch.branch))
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| UiError {
                message: "no upstream branch is recorded for this topic".into(),
                unreachable: false,
            })?;
        (upstream, LandingTargetOrigin::RecordedBase)
    } else {
        // Resolve the primary checkout's current branch via `repos.branches`.
        // A failed lookup (scoped token, unreadable checkout, missing repo) is
        // not fatal: the goal is "the primary checkout's current branch, or
        // `main`" — so fall back to `main` with a `main-fallback` origin the
        // landing agent is told to try improving on.
        let repo_branches = client
            .repo_branches(&view.branch.repo_root)
            .await
            .unwrap_or_default();
        match resolve_landing_target(
            &repo_branches,
            &view.branch.repo_root,
            &view.branch.branch,
            upstream.as_deref(),
        ) {
            Some((upstream, origin)) => (upstream, origin),
            // No primary-checkout resolution and no `main` either: fall back
            // to the recorded base so landing still has somewhere to go.
            None => (
                branches_list_base(&branches, &view.branch.branch)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| UiError {
                        message: "no landing target could be resolved for this topic".into(),
                        unreachable: false,
                    })?,
                LandingTargetOrigin::RecordedBase,
            ),
        }
    };

    let request = crate::loom::LandingRequest {
        action: "land",
        source_branch: view.branch.branch.clone(),
        target_upstream: upstream.clone(),
        target_origin: target_origin.as_str().to_string(),
        repo_root: view.branch.repo_root.clone(),
        strategy,
        requested_by: "user",
    };
    let prompt = request.to_prompt(&view.branch.name);
    client.resume_if_orphaned(&view.id).await?;
    if view.protocol == "terminal" {
        client.send_text(&view.id, &prompt, true).await?;
    } else {
        client.queue_prompt(&view.id, &prompt).await?;
    }
    // Do not mark the topic landed before the coordinator reports success.
    Ok(LandResult {
        local: false,
        landing: None,
    })
}

/// The outcome of a `land-locally` landing, returned to the UI so it can
/// confirm synchronously what the agent-mediated strategies can only
/// promise ("follow the result in this thread").
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LocalLandingView {
    pub landed: bool,
    pub target_branch: String,
    pub commit: Option<String>,
    pub commits_squashed: u32,
    /// The primary checkout the squash landed into (absolute path).
    pub primary_checkout: String,
}

impl From<LocalLanding> for LocalLandingView {
    fn from(landing: LocalLanding) -> Self {
        LocalLandingView {
            landed: landing.landed,
            target_branch: landing.target_branch,
            commit: landing.commit,
            commits_squashed: landing.commits_squashed,
            primary_checkout: landing.primary_checkout,
        }
    }
}

/// True when a Loom server host runs sessions on this machine — the
/// precondition for running git against its checkout paths locally.
fn host_is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

/// The deterministic fast path behind the `land-locally` strategy: squash
/// the topic's branch into the primary checkout's currently checked out
/// branch, locally, with no agent turn.
///
/// Preconditions beyond `squash_into_primary_checkout`'s own preflight:
/// the Loom server must be loopback (its `repo_root` paths are local
/// paths; a remote server would make Arachne run git against a
/// server-side path that is meaningless here), and the topic must own a
/// branch — an archived or branchless topic has nothing to land.
async fn land_topic_locally(
    client: Arc<LoomClient>,
    view: SessionView,
    summary: &SessionSummaryView,
) -> Result<LocalLandingView, UiError> {
    let host = client.server_host().unwrap_or("").to_owned();
    if !host_is_loopback(&host) {
        return Err(UiError {
            message: format!(
                "land-locally needs the Loom server on this machine; {host} runs sessions remotely —\n\nuse one of the agent-mediated strategies instead"
            ),
            unreachable: false,
        });
    }
    if view.branch.branch.trim().is_empty() {
        return Err(UiError {
            message: "this topic has no branch to land".into(),
            unreachable: false,
        });
    }
    let result = crate::landing::squash_into_primary_checkout(
        std::path::Path::new(&view.branch.repo_root),
        &view.branch.branch,
        &summary_title(summary, &view),
    )
    .await
    .map_err(|e| UiError {
        message: e.to_string(),
        unreachable: false,
    })?;
    // Record the outcome on the topic's branch — the same durable signal the
    // integration skill stamps for a completed integration, so every fleet
    // surface (dashboards, inspectors) sees the topic as landed without an
    // agent having to report it. A tag failure never fails the landing: the
    // git result stands, as with the agent-mediated flow.
    let note = if result.landed {
        format!(
            "land-locally: squash of {} commit{} into {}",
            result.commits_squashed,
            if result.commits_squashed == 1 {
                ""
            } else {
                "s"
            },
            result.target_branch
        )
    } else {
        format!(
            "land-locally: {} already contained the topic's changes",
            result.target_branch
        )
    };
    let _ = client
        .set_tag(
            &view.id,
            "integration_result",
            result.commit.as_deref().unwrap_or(""),
            &note,
        )
        .await;
    Ok(LocalLandingView::from(result))
}

/// A landing title for the squash commit: the topic's name, else the
/// branch's title, else the branch name.
fn summary_title(summary: &SessionSummaryView, view: &SessionView) -> String {
    if !summary.branch.name.trim().is_empty() {
        return summary.branch.name.clone();
    }
    if !view.branch.title.trim().is_empty() {
        return view.branch.title.clone();
    }
    view.branch.name.clone()
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
fn branches_list_base(branches: &[serde_json::Value], branch: &str) -> Option<String> {
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
        has_commits: changes.head_oid.as_deref()
            != changes.base.get("oid").and_then(|value| value.as_str()),
    })
}

/// The complete bounded Loom change set for the review pane.
#[tauri::command]
pub async fn work_changes(
    state: State<'_, LoomState>,
    session_id: String,
) -> Result<crate::loom::ChangeSetView, UiError> {
    let client = state_client(&state).await?;
    client
        .session_changes(&session_id)
        .await
        .map_err(Into::into)
}

fn resource_error(message: impl Into<String>) -> UiError {
    UiError {
        message: message.into(),
        unreachable: false,
    }
}

async fn load_topic_resources(
    client: &LoomClient,
    branch_id: &str,
) -> Result<TopicResourcesView, UiError> {
    let artifact = match client.branch_artifact(branch_id, MANIFEST_NAME).await {
        Ok(value) => value,
        Err(LoomError::Api { status: 404, .. }) => return Ok(TopicResourcesView::default()),
        Err(error) => return Err(error.into()),
    };
    if artifact
        .get("meta")
        .and_then(|v| v.get("branch_id"))
        .and_then(|v| v.as_str())
        != Some(branch_id)
    {
        return Err(resource_error(
            "resource manifest name is occupied by a repository-shared artifact",
        ));
    }
    let content = artifact
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| resource_error("resource manifest has no content"))?;
    let mut manifest: TopicResourcesView = serde_json::from_str(content)
        .map_err(|e| resource_error(format!("invalid resource manifest: {e}")))?;
    manifest.revision = artifact
        .get("meta")
        .and_then(|v| v.get("rev"))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| resource_error("resource manifest has no revision"))?;
    Ok(manifest)
}

async fn resource_context(
    client: &LoomClient,
    mentions: Vec<ResourceMention>,
) -> Result<String, UiError> {
    if mentions.len() > 12 {
        return Err(resource_error(
            "at most 12 resources can be mentioned in one message",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut resolved = Vec::new();
    for mention in mentions {
        if !seen.insert((mention.topic_id.clone(), mention.resource_id.clone())) {
            continue;
        }
        let topic = client.get_session(&mention.topic_id).await?;
        let manifest = load_topic_resources(client, &topic.branch.id).await?;
        let resource = manifest
            .resources
            .into_iter()
            .find(|resource| resource.id == mention.resource_id)
            .ok_or_else(|| {
                resource_error(format!(
                    "mentioned resource no longer exists: {}",
                    mention.resource_id
                ))
            })?;
        resolved
            .push(serde_json::json!({ "source_topic": mention.topic_id, "resource": resource }));
    }
    if resolved.is_empty() {
        return Ok(String::new());
    }
    let content = serde_json::to_string_pretty(&resolved)
        .map_err(|e| resource_error(format!("serializing resource mentions: {e}")))?;
    Ok(format!("\n\nReferenced topic resources (current Arachne bindings; use these locators to inspect the resources):\n{content}"))
}

async fn save_topic_resources(
    client: &LoomClient,
    branch_id: &str,
    manifest: &TopicResourcesView,
    expected_revision: i64,
) -> Result<TopicResourcesView, UiError> {
    let content = serde_json::to_string_pretty(&manifest)
        .map_err(|e| resource_error(format!("serializing resources: {e}")))?;
    let artifact = client
        .write_branch_artifact(branch_id, MANIFEST_NAME, &content, expected_revision)
        .await?;
    if artifact
        .get("meta")
        .and_then(|v| v.get("branch_id"))
        .and_then(|v| v.as_str())
        != Some(branch_id)
    {
        return Err(resource_error(
            "resource manifest was not saved on the topic branch",
        ));
    }
    let mut saved = manifest.clone();
    saved.revision = artifact
        .get("meta")
        .and_then(|v| v.get("rev"))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| resource_error("saved resource manifest has no revision"))?;
    Ok(saved)
}

/// The Resources panel's full view: the durable manifest plus the topic's
/// live slice — GitHub issues its subtree works, and the PRs of every session
/// in the subtree. Loom has no topic↔issue link, so the issues are scoped
/// here: an issue belongs to the topic when its repo matches and a branch of
/// the topic's subtree claims (or sourced) it. PRs ride the fleet snapshot
/// (`BranchSummaryView.github`, loom's poll loop), so they are always live.
#[derive(Debug, Clone, Serialize)]
pub struct TopicResourcesPanel {
    #[serde(flatten)]
    pub manifest: TopicResourcesView,
    /// Issues the topic's subtree works, from `issues.board`.
    pub issues: Vec<crate::loom::IssueView>,
    /// Every session in the topic's subtree with a PR, coordinator first.
    /// `session_id`/`session_name` label the row; `github` is the snapshot.
    pub prs: Vec<TopicPrRow>,
}

/// One PR row: which thread's PR this is. The coordinator's PR sorts first
/// and keeps no name qualifier; workers are labeled with their thread.
#[derive(Debug, Clone, Serialize)]
pub struct TopicPrRow {
    pub session_id: String,
    pub session_name: String,
    #[serde(flatten)]
    pub github: crate::loom::GithubStatusView,
}

/// The sessions of `topic_id`'s delegation subtree (coordinator first),
/// walking parent_session_id and parent_id links like topicRootOf does.
fn topic_subtree<'a>(
    fleet: &'a [SessionSummaryView],
    topic_id: &str,
) -> Vec<&'a SessionSummaryView> {
    let by_id: std::collections::HashMap<&str, &SessionSummaryView> = fleet
        .iter()
        .map(|session| (session.id.as_str(), session))
        .collect();
    let Some(root) = by_id.get(topic_id) else {
        return Vec::new();
    };
    // parent lookup: parent_session_id by session id, else parent_id (branch
    // id) — the same two links topicRootOf walks in the UI.
    let by_branch: std::collections::HashMap<&str, &'a SessionSummaryView> = fleet
        .iter()
        .map(|session| (session.branch.id.as_str(), session))
        .collect();
    let parent_of = |session: &SessionSummaryView| -> Option<&'a SessionSummaryView> {
        session
            .parent_session_id
            .as_deref()
            .and_then(|id| by_id.get(id).copied())
            .or_else(|| {
                session
                    .parent_id
                    .as_deref()
                    .and_then(|pid| by_branch.get(pid).copied())
            })
    };
    let mut children: std::collections::HashMap<&str, Vec<&'a SessionSummaryView>> =
        std::collections::HashMap::new();
    for session in fleet {
        if let Some(parent) = parent_of(session) {
            if parent.id != session.id {
                children
                    .entry(parent.id.as_str())
                    .or_default()
                    .push(session);
            }
        }
    }
    // BFS from the coordinator, most-recent children first (the fleet list
    // arrives sorted newest-activity-first, so the iteration order keeps
    // the topic's freshest workers nearest the top).
    let mut rows: Vec<&'a SessionSummaryView> = Vec::new();
    let mut queue: std::collections::VecDeque<&'a SessionSummaryView> =
        std::collections::VecDeque::new();
    queue.push_back(root);
    let mut seen = std::collections::HashSet::new();
    while let Some(session) = queue.pop_front() {
        if !seen.insert(session.id.as_str()) {
            continue;
        }
        if let Some(kids) = children.get(session.id.as_str()) {
            for kid in kids {
                if !seen.contains(kid.id.as_str()) {
                    queue.push_back(kid);
                }
            }
        }
        rows.push(session);
    }
    rows
}

/// Scope loom's issue board to one topic: same repo root, and claimed or
/// sourced by a branch name that appears in the topic's subtree. Unclaimed
/// backlog never appears (it belongs to the repo, not this topic).
fn topic_issues(
    board: Vec<crate::loom::IssueView>,
    topic: &SessionView,
    subtree: &[&SessionSummaryView],
) -> Vec<crate::loom::IssueView> {
    let branch_names: std::collections::HashSet<&str> = subtree
        .iter()
        .map(|session| session.branch.branch.as_str())
        .collect();
    board
        .into_iter()
        .filter(|issue| issue.repo_root == topic.branch.repo_root)
        .filter(|issue| {
            [issue.claimed_branch.as_deref(), issue.source_branch.as_deref()]
                .into_iter()
                .flatten()
                .any(|branch| branch_names.contains(branch))
        })
        .collect()
}

/// The PR rows for a topic subtree: every session with a GitHub PR snapshot,
/// coordinator first then most recent. The coordinator's row is the topic's
/// own PR; workers' rows are labeled with their thread.
fn topic_prs(subtree: &[&SessionSummaryView]) -> Vec<TopicPrRow> {
    subtree
        .iter()
        .filter_map(|session| {
            session.branch.github.as_ref().map(|github| TopicPrRow {
                session_id: session.id.clone(),
                session_name: session.branch.name.clone(),
                github: github.clone(),
            })
        })
        .collect()
}

#[tauri::command]
pub async fn topic_resources(
    state: State<'_, LoomState>,
    topic_id: String,
) -> Result<TopicResourcesPanel, UiError> {
    let client = state_client(&state).await?;
    let topic = client.get_session(&topic_id).await?;
    let manifest = load_topic_resources(&client, &topic.branch.id).await?;
    // The live slice rides the same fetch: the fleet snapshot for the
    // subtree's PRs and branches, plus the issue board. A failed board fetch
    // must not take the manifest down with it — the panel degrades to the
    // durable bindings only.
    let fleet = client.list_sessions().await.map(|mut fleet| {
        fleet.sort_by(|a, b| b.last_activity_at.cmp(&a.last_activity_at));
        fleet
    });
    let (issues, prs) = match fleet {
        Ok(fleet) => {
            let subtree = topic_subtree(&fleet, &topic_id);
            let issues = match client.list_issues().await {
                Ok(board) => topic_issues(board, &topic, &subtree),
                Err(_) => Vec::new(),
            };
            (issues, topic_prs(&subtree))
        }
        Err(_) => (Vec::new(), Vec::new()),
    };
    Ok(TopicResourcesPanel {
        manifest,
        issues,
        prs,
    })
}

#[tauri::command]
pub async fn attach_topic_resource(
    state: State<'_, LoomState>,
    topic_id: String,
    resource: ResourceDraft,
    expected_revision: i64,
) -> Result<TopicResourcesView, UiError> {
    let client = state_client(&state).await?;
    let topic = client.get_session(&topic_id).await?;
    let resource = resource
        .validated(&topic.branch.repo_root, &topic.branch.branch)
        .map_err(resource_error)?;
    let mut manifest = load_topic_resources(&client, &topic.branch.id).await?;
    if manifest.revision != expected_revision {
        return Err(resource_error("resources changed; reload before editing"));
    }
    if let Some(existing) = manifest.resources.iter_mut().find(|r| r.id == resource.id) {
        *existing = resource;
    } else {
        manifest.resources.push(resource);
    }
    save_topic_resources(&client, &topic.branch.id, &manifest, expected_revision).await
}

#[tauri::command]
pub async fn detach_topic_resource(
    state: State<'_, LoomState>,
    topic_id: String,
    resource_id: String,
    expected_revision: i64,
) -> Result<TopicResourcesView, UiError> {
    let client = state_client(&state).await?;
    let topic = client.get_session(&topic_id).await?;
    let mut manifest = load_topic_resources(&client, &topic.branch.id).await?;
    if manifest.revision != expected_revision {
        return Err(resource_error("resources changed; reload before editing"));
    }
    let before = manifest.resources.len();
    manifest.resources.retain(|r| r.id != resource_id);
    if manifest.resources.len() == before {
        return Err(resource_error("resource not found"));
    }
    save_topic_resources(&client, &topic.branch.id, &manifest, expected_revision).await
}

async fn resolve_topic_resource(
    client: &LoomClient,
    topic_id: &str,
    resource_id: &str,
) -> Result<(SessionView, TopicResource), UiError> {
    let topic = client.get_session(topic_id).await?;
    let manifest = load_topic_resources(client, &topic.branch.id).await?;
    let resource = manifest
        .resources
        .into_iter()
        .find(|r| r.id == resource_id)
        .ok_or_else(|| resource_error("resource not found"))?;
    Ok((topic, resource))
}

#[tauri::command]
pub async fn read_topic_resource(
    state: State<'_, LoomState>,
    topic_id: String,
    resource_id: String,
) -> Result<TopicResourceContent, UiError> {
    let client = state_client(&state).await?;
    let (topic, resource) = resolve_topic_resource(&client, &topic_id, &resource_id).await?;
    let content = match resource.data.kind {
        ResourceKind::File | ResourceKind::DesignDocument => {
            if topic.status == "archived" {
                return Err(resource_error(
                    "topic checkout is archived; recover it to preview this file",
                ));
            }
            if resource.data.reference.as_deref() != Some(&topic.branch.branch) {
                return Err(resource_error(
                    "file reference no longer matches the topic branch",
                ));
            }
            let path = resource
                .data
                .path
                .as_deref()
                .ok_or_else(|| resource_error("resource has no file path"))?;
            crate::resources::validate_relative_path(path).map_err(resource_error)?;
            client.worktree_text(&topic_id, path).await?
        }
        ResourceKind::Artifact => {
            let name = resource
                .data
                .path
                .as_deref()
                .ok_or_else(|| resource_error("resource has no artifact name"))?;
            client
                .branch_artifact(&topic.branch.id, name)
                .await?
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| resource_error("artifact has no content"))?
                .to_owned()
        }
        _ => return Err(resource_error("this resource has no text preview")),
    };
    Ok(TopicResourceContent { resource, content })
}

#[tauri::command]
pub async fn open_topic_resource_in_zed(
    state: State<'_, LoomState>,
    topic_id: String,
    resource_id: String,
) -> Result<(), UiError> {
    let client = state_client(&state).await?;
    let (topic, resource) = resolve_topic_resource(&client, &topic_id, &resource_id).await?;
    if !matches!(
        resource.data.kind,
        ResourceKind::File | ResourceKind::DesignDocument
    ) {
        return Err(resource_error("only repository files can open in Zed"));
    }
    if topic.status == "archived" {
        return Err(resource_error(
            "topic checkout is archived; recover it before opening a file",
        ));
    }
    if resource.data.reference.as_deref() != Some(&topic.branch.branch) {
        return Err(resource_error(
            "file reference no longer matches the topic branch",
        ));
    }
    let path = resource
        .data
        .path
        .as_deref()
        .ok_or_else(|| resource_error("resource has no file path"))?;
    crate::resources::validate_relative_path(path).map_err(resource_error)?;
    let host = client
        .server_host()
        .ok_or_else(|| resource_error("Loom URL has no host"))?;
    let file = std::path::Path::new(&topic.work_dir).join(path);
    let target = zed_target(host, &file.to_string_lossy())?;
    let cli = if std::path::Path::new("/Applications/Zed.app/Contents/MacOS/cli").exists() {
        "/Applications/Zed.app/Contents/MacOS/cli"
    } else {
        "zed"
    };
    let status = tokio::process::Command::new(cli)
        .arg(&target)
        .status()
        .await
        .map_err(|e| resource_error(format!("launching zed: {e}")))?;
    if !status.success() {
        return Err(resource_error(format!("zed exited with {status}")));
    }
    Ok(())
}

// --- Todos -----------------------------------------------------------------
//
// The durable user todo list lives in one cross-topic branch artifact
// (`arachne-todos`), mirroring the arachne-resources pattern. It is read
// from (and written to) the TOPIC's branch, so each topic's slice rides
// its accepted branch — but a single artifact name per branch is what the
// loom API offers, so the list is stored on the topic branch of whichever
// topic is being viewed, keyed by `topic_id` for filtering. Mutations are
// revision-checked exactly like the resource manifest.

async fn load_todo_list(client: &LoomClient, branch_id: &str) -> Result<TodoListView, UiError> {
    let artifact = match client.branch_artifact(branch_id, TODOS_NAME).await {
        Ok(value) => value,
        Err(LoomError::Api { status: 404, .. }) => return Ok(TodoListView::default()),
        Err(error) => return Err(error.into()),
    };
    if artifact
        .get("meta")
        .and_then(|v| v.get("branch_id"))
        .and_then(|v| v.as_str())
        != Some(branch_id)
    {
        return Err(todo_error(
            "todo list name is occupied by a repository-shared artifact",
        ));
    }
    let content = artifact
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| todo_error("todo list has no content"))?;
    let mut list: TodoListView =
        serde_json::from_str(content).map_err(|e| todo_error(format!("invalid todo list: {e}")))?;
    list.revision = artifact
        .get("meta")
        .and_then(|v| v.get("rev"))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| todo_error("todo list has no revision"))?;
    Ok(list)
}

async fn save_todo_list(
    client: &LoomClient,
    branch_id: &str,
    list: &TodoListView,
    expected_revision: i64,
) -> Result<TodoListView, UiError> {
    let content = serde_json::to_string_pretty(&list)
        .map_err(|e| todo_error(format!("serializing todos: {e}")))?;
    let artifact = client
        .write_branch_artifact_titled(
            branch_id,
            TODOS_NAME,
            &content,
            expected_revision,
            "Arachne topic todos",
        )
        .await?;
    if artifact
        .get("meta")
        .and_then(|v| v.get("branch_id"))
        .and_then(|v| v.as_str())
        != Some(branch_id)
    {
        return Err(todo_error("todo list was not saved on the topic branch"));
    }
    let mut saved = list.clone();
    saved.revision = artifact
        .get("meta")
        .and_then(|v| v.get("rev"))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| todo_error("saved todo list has no revision"))?;
    Ok(saved)
}

fn todo_error(message: impl Into<String>) -> UiError {
    UiError {
        message: message.into(),
        unreachable: false,
    }
}

fn todo_text_ok(text: &str) -> Result<(), UiError> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > 512 {
        return Err(todo_error("todo text must contain 1–512 characters"));
    }
    Ok(())
}

/// A stable identity for a todo: derived from its text and topic so the
/// same todo re-added is idempotent (mirrors the resource locator idea).
/// FNV-1a is implemented inline — std's DefaultHasher is not guaranteed
/// stable across Rust releases, and an id that changes would orphan every
/// saved todo after an app upgrade.
fn todo_id(text: &str, topic_id: &str) -> String {
    fn fnv1a(bytes: &[u8]) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        hash
    }
    let trimmed = text.trim();
    let mixed = fnv1a(trimmed.as_bytes()) ^ fnv1a(topic_id.as_bytes()).rotate_left(17);
    format!("todo:{trimmed}:{mixed:016x}")
}

/// The open topic's todo slice. This is what the inspector's Todos tab
/// renders — items plus the artifact revision for revision-checked edits.
#[tauri::command]
pub async fn topic_todos(
    state: State<'_, LoomState>,
    topic_id: String,
) -> Result<TodoTopicView, UiError> {
    let client = state_client(&state).await?;
    let topic = client.get_session(&topic_id).await?;
    Ok(load_todo_list(&client, &topic.branch.id)
        .await?
        .topic_view(&topic_id))
}

/// Toggle one todo's done state. Because the artifact is one list, the
/// command toggles by id within the full list (the view is topic-scoped,
/// but the id is unique across the list, so a topic's toggle cannot
/// disturb another topic's items).
#[tauri::command]
pub async fn toggle_todo(
    state: State<'_, LoomState>,
    topic_id: String,
    todo_id: String,
    expected_revision: i64,
) -> Result<TodoTopicView, UiError> {
    let client = state_client(&state).await?;
    let topic = client.get_session(&topic_id).await?;
    let mut list = load_todo_list(&client, &topic.branch.id).await?;
    if list.revision != expected_revision {
        return Err(todo_error("todos changed; reload before editing"));
    }
    let todo = list
        .todos
        .iter_mut()
        .find(|t| t.id == todo_id)
        .ok_or_else(|| todo_error("todo not found"))?;
    todo.done = !todo.done;
    let saved = save_todo_list(&client, &topic.branch.id, &list, expected_revision).await?;
    Ok(saved.topic_view(&topic_id))
}

/// Add a todo to this topic's slice.
#[tauri::command]
pub async fn add_todo(
    state: State<'_, LoomState>,
    topic_id: String,
    text: String,
    expected_revision: i64,
) -> Result<TodoTopicView, UiError> {
    let client = state_client(&state).await?;
    let topic = client.get_session(&topic_id).await?;
    todo_text_ok(&text)?;
    let mut list = load_todo_list(&client, &topic.branch.id).await?;
    if list.revision != expected_revision {
        return Err(todo_error("todos changed; reload before editing"));
    }
    let id = todo_id(&text, &topic_id);
    if let Some(existing) = list.todos.iter_mut().find(|t| t.id == id) {
        existing.text = text.trim().to_owned();
        existing.topic_id = topic_id.clone();
    } else {
        list.todos.push(TodoItem {
            id,
            text: text.trim().to_owned(),
            done: false,
            topic_id: topic_id.clone(),
            created_at: chrono_iso_now(),
        });
    }
    let saved = save_todo_list(&client, &topic.branch.id, &list, expected_revision).await?;
    Ok(saved.topic_view(&topic_id))
}

/// Remove a todo from the list.
#[tauri::command]
pub async fn remove_todo(
    state: State<'_, LoomState>,
    topic_id: String,
    todo_id: String,
    expected_revision: i64,
) -> Result<TodoTopicView, UiError> {
    let client = state_client(&state).await?;
    let topic = client.get_session(&topic_id).await?;
    let mut list = load_todo_list(&client, &topic.branch.id).await?;
    if list.revision != expected_revision {
        return Err(todo_error("todos changed; reload before editing"));
    }
    let before = list.todos.len();
    list.todos.retain(|t| t.id != todo_id);
    if list.todos.len() == before {
        return Err(todo_error("todo not found"));
    }
    let saved = save_todo_list(&client, &topic.branch.id, &list, expected_revision).await?;
    Ok(saved.topic_view(&topic_id))
}

/// RFC3339 without pulling a chrono dependency: loom only needs a
/// lexicographically comparable timestamp, and the UTC "Z" form matches
/// the wire format used by `created_at` elsewhere.
fn chrono_iso_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    // Civil-from-days algorithm (Howard Hinnant) — exact for all dates.
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod integration_tests {
    use super::{resolve_integration_target, resolve_landing_target};
    use crate::loom::SessionSummaryView;

    #[test]
    fn loopback_hosts_qualify_for_local_landing() {
        for host in ["localhost", "127.0.0.1", "::1", "[::1]"] {
            assert!(super::host_is_loopback(host), "{host}");
        }
        for host in ["dgx.tail1234.ts.net", "loom.example.com", ""] {
            assert!(!super::host_is_loopback(host), "{host}");
        }
    }

    #[test]
    fn landing_title_prefers_topic_name_then_branch_title() {
        let summary = serde_json::from_value::<SessionSummaryView>(serde_json::json!({
            "id": "s", "status": "running", "profile": "default", "class": "interactive",
            "origin": "user", "created_by": null, "created_at": "2026-01-01T00:00:00Z",
            "last_activity_at": "2026-01-01T00:00:00Z", "placement": null,
            "github_repo": null, "parent_id": null, "parent_session_id": null,
            "branch": { "id": "b", "branch": "weaver/x", "name": "", "title": "Branch title",
                "repo_root": "/repo", "tags": [] }
        }))
        .unwrap();
        let view = serde_json::from_value::<crate::loom::SessionView>(serde_json::json!({
            "id": "s", "status": "running", "profile": "default", "class": "interactive",
            "origin": "user", "created_by": null, "created_at": "2026-01-01T00:00:00Z",
            "last_activity_at": "2026-01-01T00:00:00Z", "turn_count": 0,
            "agent_kind": "claude", "model": "", "effort": "", "protocol": "acp",
            "work_dir": "", "term_session": "", "placement": null,
            "branch": { "id": "b", "branch": "weaver/x", "name": "branch-name", "title": "Branch title",
                "repo_root": "/repo", "tags": [] }
        })).unwrap();
        // Empty topic name falls to the branch's title.
        assert_eq!(super::summary_title(&summary, &view), "Branch title");
    }

    fn repo_branch(
        name: &str,
        worktree: Option<&str>,
        current: bool,
    ) -> crate::loom::RepoBranchView {
        crate::loom::RepoBranchView {
            name: name.into(),
            worktree: worktree.map(String::from),
            current,
        }
    }

    #[test]
    fn landing_prefers_primary_checkout_current_branch() {
        // The primary checkout is on `dev`; a worker worktree holds the topic
        // branch. Non-PR landing targets the primary checkout's branch.
        let rows = vec![
            repo_branch("dev", Some("/repo"), true),
            repo_branch("weaver/topic", Some("/repo/.worktrees/topic"), false),
        ];
        let (target, origin) =
            resolve_landing_target(&rows, "/repo", "weaver/topic", None).unwrap();
        assert_eq!(target, "dev");
        assert_eq!(origin, super::LandingTargetOrigin::PrimaryCheckout);
    }

    #[test]
    fn landing_ignores_worker_worktree_branch() {
        // A worker worktree's branch is never the target, even if it were
        // somehow marked `current`: only a row checked out at the primary
        // checkout path wins. Here nothing matches, so `main` (present as a
        // local branch) is the fallback.
        let rows = vec![
            repo_branch("weaver/other-worker", Some("/repo/.worktrees/other"), true),
            repo_branch("main", Some("/repo"), false),
        ];
        let (target, origin) =
            resolve_landing_target(&rows, "/repo", "weaver/topic", None).unwrap();
        assert_eq!(target, "main");
        assert_eq!(origin, super::LandingTargetOrigin::MainFallback);
    }

    #[test]
    fn landing_falls_back_to_main_when_primary_checkout_detached() {
        // A detached primary checkout reports no `current` row; `main`
        // exists as a local branch and is the fallback the agent may improve on.
        let rows = vec![
            repo_branch("dev", Some("/repo/.worktrees/dev"), false),
            repo_branch("main", Some("/repo"), false),
        ];
        let (target, origin) =
            resolve_landing_target(&rows, "/repo", "weaver/topic", None).unwrap();
        assert_eq!(target, "main");
        assert_eq!(origin, super::LandingTargetOrigin::MainFallback);
    }

    #[test]
    fn landing_empty_rows_still_fall_back_to_main() {
        let (target, origin) = resolve_landing_target(&[], "/repo", "weaver/topic", None).unwrap();
        assert_eq!(target, "main");
        assert_eq!(origin, super::LandingTargetOrigin::MainFallback);
    }

    #[test]
    fn landing_primary_checkout_on_main_is_not_a_fallback() {
        let rows = vec![repo_branch("main", Some("/repo"), true)];
        let (target, origin) =
            resolve_landing_target(&rows, "/repo", "weaver/topic", None).unwrap();
        assert_eq!(target, "main");
        assert_eq!(origin, super::LandingTargetOrigin::PrimaryCheckout);
    }

    #[test]
    fn landing_explicit_upstream_wins() {
        let rows = vec![repo_branch("dev", Some("/repo"), true)];
        let (target, origin) =
            resolve_landing_target(&rows, "/repo", "weaver/topic", Some(" release ")).unwrap();
        assert_eq!(target, "release");
        assert_eq!(origin, super::LandingTargetOrigin::Explicit);
    }

    #[test]
    fn landing_blank_explicit_upstream_falls_through() {
        let rows = vec![repo_branch("dev", Some("/repo"), true)];
        let (target, origin) =
            resolve_landing_target(&rows, "/repo", "weaver/topic", Some("  ")).unwrap();
        assert_eq!(target, "dev");
        assert_eq!(origin, super::LandingTargetOrigin::PrimaryCheckout);
    }

    #[test]
    fn landing_skips_topic_branch_at_primary_checkout() {
        // The primary checkout has the topic's own branch checked out:
        // landing there would be a no-op self-merge, so the `main` fallback
        // applies instead.
        let rows = vec![
            repo_branch("weaver/topic", Some("/repo"), true),
            repo_branch("main", Some("/repo/.worktrees/main"), false),
        ];
        let (target, origin) =
            resolve_landing_target(&rows, "/repo", "weaver/topic", None).unwrap();
        assert_eq!(target, "main");
        assert_eq!(origin, super::LandingTargetOrigin::MainFallback);
    }

    #[test]
    fn landing_without_main_and_without_current_is_none() {
        let rows = vec![repo_branch("dev", Some("/repo"), false)];
        // No current row and no `main`: the caller falls back to the
        // recorded base.
        assert_eq!(
            resolve_landing_target(&rows, "/repo", "weaver/topic", None),
            None
        );
    }

    fn session(
        id: &str,
        branch: &str,
        repo: &str,
        parent: Option<&str>,
        topic: bool,
    ) -> SessionSummaryView {
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
        }))
        .unwrap()
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
        assert_eq!(
            resolve_integration_target(&fleet, "child")
                .unwrap()
                .target_branch,
            "parent-branch"
        );
        assert!(resolve_integration_target(&fleet, "parent").is_none());
    }
}

#[cfg(test)]
mod resource_panel_tests {
    use super::{topic_issues, topic_prs, topic_subtree};
    use crate::loom::{GithubStatusView, IssueView, SessionSummaryView};

    fn session(id: &str, parent: Option<&str>, branch: &str, pr: Option<i64>) -> SessionSummaryView {
        let github = pr.map(|n| GithubStatusView {
            pr_number: n,
            pr_url: format!("https://github.com/acme/app/pull/{n}"),
            pr_state: "OPEN".into(),
            pr_title: format!("PR {n}"),
            is_draft: false,
            review_decision: None,
            checks: None,
        });
        serde_json::from_value(serde_json::json!({
            "id": id, "status": "running", "profile": "default", "class": "interactive",
            "origin": "user", "created_by": null, "created_at": "2026-01-01T00:00:00Z",
            "last_activity_at": "2026-01-01T00:00:00Z", "placement": null,
            "github_repo": null, "parent_session_id": parent, "parent_id": null,
            "branch": { "id": format!("branch-{id}"), "branch": branch, "name": id,
                "title": id, "repo_root": "/repo", "tags": [], "github": github }
        }))
        .unwrap()
    }

    fn issue(id: i64, repo: &str, claimed: Option<&str>, sourced: Option<&str>) -> IssueView {
        IssueView {
            id,
            repo_root: repo.into(),
            github_repo: Some("acme/app".into()),
            source_branch: sourced.map(String::from),
            claimed_branch: claimed.map(String::from),
            title: format!("Issue {id}"),
            status: "open".into(),
            github_issue: Some(id),
            github_state: None,
        }
    }

    fn topic_view() -> crate::loom::SessionView {
        serde_json::from_value(serde_json::json!({
            "id": "topic", "status": "running", "profile": "default", "class": "interactive",
            "origin": "user", "created_by": null, "created_at": "2026-01-01T00:00:00Z",
            "last_activity_at": "2026-01-01T00:00:00Z", "turn_count": 0,
            "agent_kind": "codex", "model": "", "effort": "", "protocol": "acp",
            "work_dir": "", "term_session": "", "placement": null,
            "branch": { "id": "branch-topic", "branch": "weaver/topic", "name": "topic",
                "title": "Topic", "repo_root": "/repo", "tags": [] }
        }))
        .unwrap()
    }

    #[test]
    fn subtree_walks_both_parent_links_and_stops_at_strangers() {
        // topic → worker (by session id) → grandchild (by branch id); an
        // unrelated session and a cycle never enter the walk.
        let fleet = vec![
            session("topic", None, "weaver/topic", None),
            session("worker", Some("topic"), "weaver/worker", None),
            session("stranger", Some("nowhere"), "weaver/stranger", None),
        ];
        let mut grandchild_value = serde_json::to_value(session("grandchild", None, "weaver/grandchild", None)).unwrap();
        grandchild_value["parent_id"] = "branch-worker".into();
        let mut fleet = fleet;
        fleet.push(serde_json::from_value(grandchild_value).unwrap());
        let subtree = topic_subtree(&fleet, "topic");
        let ids: Vec<&str> = subtree.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["topic", "worker", "grandchild"]);
    }

    #[test]
    fn issues_scope_by_repo_and_subtree_branch_names() {
        let fleet = vec![
            session("topic", None, "weaver/topic", None),
            session("worker", Some("topic"), "weaver/worker", None),
        ];
        let subtree = topic_subtree(&fleet, "topic");
        let board = vec![
            issue(1, "/repo", Some("weaver/worker"), None),   // claimed by subtree
            issue(2, "/repo", None, Some("weaver/topic")),     // sourced by subtree
            issue(3, "/repo", Some("weaver/other"), None),     // another topic's branch
            issue(4, "/repo", None, None),                     // unclaimed backlog
            issue(5, "/other", Some("weaver/topic"), None),    // wrong repo
        ];
        let scoped = topic_issues(board, &topic_view(), &subtree);
        assert_eq!(scoped.iter().map(|i| i.id).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn pr_rows_cover_the_whole_subtree_not_just_the_coordinator() {
        let fleet = vec![
            session("topic", None, "weaver/topic", Some(10)),
            session("worker-a", Some("topic"), "weaver/a", Some(11)),
            session("worker-b", Some("topic"), "weaver/b", None),
        ];
        let subtree = topic_subtree(&fleet, "topic");
        let prs = topic_prs(&subtree);
        assert_eq!(prs.iter().map(|p| p.github.pr_number).collect::<Vec<_>>(), [10, 11]);
        // Each row carries which thread the PR belongs to.
        assert_eq!(prs[1].session_id, "worker-a");
        assert_eq!(prs[1].session_name, "worker-a");
    }
}

#[cfg(test)]
mod todo_command_tests {
    use super::{chrono_iso_now, todo_id};

    #[test]
    fn iso_now_is_a_well_formed_utc_stamp() {
        let stamp = chrono_iso_now();
        // Shape: 2026-09-27T12:34:56Z — a real UTC civil date.
        assert_eq!(stamp.len(), 20);
        let (date, time) = stamp.split_once('T').unwrap();
        let parts: Vec<&str> = date.split('-').collect();
        assert_eq!(parts.len(), 3);
        let (y, m, d): (i64, i64, i64) = (
            parts[0].parse().unwrap(),
            parts[1].parse().unwrap(),
            parts[2].parse().unwrap(),
        );
        assert!((2020..=2100).contains(&y));
        assert!((1..=12).contains(&m));
        assert!((1..=31).contains(&d));
        let (hms, z) = time.split_at(8);
        assert_eq!(z, "Z");
        assert_eq!(hms.matches(':').count(), 2);
        assert!(hms
            .split(':')
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_digit())));
    }

    #[test]
    fn todo_identity_is_stable_and_topic_scoped() {
        assert_eq!(
            todo_id("Land Arachne topic", "t1"),
            todo_id(" Land Arachne topic ", "t1")
        );
        assert_ne!(
            todo_id("Land Arachne topic", "t1"),
            todo_id("Land Arachne topic", "t2")
        );
    }
}
