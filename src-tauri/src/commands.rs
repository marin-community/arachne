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

use futures_util::FutureExt;
use std::sync::Arc;

use base64::Engine as _;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::client::{LoomClient, LoomError};
use crate::landing::LocalLanding;
use crate::loom::{LaunchOptionsView, SessionSummaryView, SessionView};
use crate::resources::{
    binding_key, merge_effective, EffectiveResource, ProjectBinding, ProjectInfo, ProjectsStore,
    ResourceDraft, ResourceKind, ResourceMention, ResourceOrigin, TodoItem, TodoListView,
    TodoTopicView, TopicEffectiveView, TopicResource, TopicResourceContent, TopicResourcesView,
    MANIFEST_NAME, PROJECTS_NAME, TODOS_NAME,
};

#[derive(Default)]
pub struct LoomState {
    pub client: tokio::sync::RwLock<Option<Arc<LoomClient>>>,
    open_generation: std::sync::atomic::AtomicU64,
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
    pub server_url: String,
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
pub async fn save_token(base_url: String, token: String) -> Result<(), UiError> {
    tokio::task::spawn_blocking(move || crate::secret::save(&base_url, &token))
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
pub async fn load_token(
    base_url: String,
    migrate_legacy: Option<bool>,
) -> Result<Option<String>, UiError> {
    tokio::task::spawn_blocking(move || {
        crate::secret::load(&base_url, migrate_legacy.unwrap_or(false))
    })
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

    state
        .open_generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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

/// Managed repositories on the currently connected Loom server.
#[tauri::command]
pub async fn list_repos(
    state: State<'_, LoomState>,
) -> Result<Vec<crate::loom::RepoView>, UiError> {
    state_client(&state)
        .await?
        .list_repos()
        .await
        .map_err(Into::into)
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

fn emit_connection_error(
    app: &AppHandle,
    client: &LoomClient,
    error: LoomError,
) -> Result<(), tauri::Error> {
    let error = UiError::from(error);
    app.emit("loom://error", serde_json::json!({"message": error.message, "unreachable": error.unreachable, "server_url": client.server_url()}))
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
            let _ = emit_connection_error(&app, &client, e);
            return;
        }
    };
    sessions.sort_by(|a, b| a.last_activity_at.cmp(&b.last_activity_at));
    let _ = app.emit(
        "loom://fleet",
        &FleetSnapshot {
            server_url: client.server_url(),
            sessions,
            layout,
        },
    );
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
    let _ = app.emit(
        "loom://fleet",
        &FleetSnapshot {
            server_url: client.server_url(),
            sessions,
            layout,
        },
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
                    let _ = emit_connection_error(&app, &client, e);
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
                    }
                    continue;
                }
            };
            // Every live session gets an event subscription. Loom caps each stream at
            // 64 topics, so shard instead of silently losing older workers' attention.
            use futures_util::StreamExt;
            let topics = fleet_topics(&sessions);
            sessions.sort_by(|a, b| a.last_activity_at.cmp(&b.last_activity_at));
            emit_fleet(&app, &client, sessions).await;
            let mut streams = futures_util::stream::SelectAll::new();
            for chunk in topics.chunks(64) {
                match client.subscribe(chunk).await {
                    Ok(rx) => streams.push(tokio_stream::wrappers::ReceiverStream::new(rx)),
                    Err(e) => {
                        let _ = emit_connection_error(&app, &client, e);
                    }
                }
            }
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    frame = streams.next() => {
                        if frame.is_none() { break; }
                        // Coalesce a burst into one authoritative snapshot; no periodic polling.
                        tokio::select! {
                            _ = cancel.cancelled() => return,
                            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                        }
                        while streams.next().now_or_never().flatten().is_some() {}
                        match client.list_sessions().await {
                            Ok(list) => {
                                let changed_topics = fleet_topics(&list) != topics;
                                if cancel.is_cancelled() { return; }
                                emit_fleet(&app, &client, list).await;
                                if changed_topics { break; }
                            }
                            Err(e) => { let _ = emit_connection_error(&app, &client, e); }
                        }
                    }
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
    live.sort_by(|a, b| a.id.cmp(&b.id));
    let mut topics = vec!["layout".to_owned()];
    topics.extend(
        live.into_iter()
            .map(|session| format!("session:{}", session.id)),
    );
    topics
}

/// Promote a one-off conversation without relaunching it or moving its checkout.
#[tauri::command]
pub async fn convert_to_track(
    app: AppHandle,
    state: State<'_, LoomState>,
    id: String,
) -> Result<(), UiError> {
    let client = state_client(&state).await?;
    let view = client.get_session(&id).await?;
    if view.status == "archived" {
        return Err(resource_error(
            "Reopen the thread before converting it to a track.",
        ));
    }
    client
        .set_tag(
            &id,
            "topic",
            "true",
            "Converted to a track; this conversation is its coordinator",
        )
        .await?;
    if let Ok(list) = client.list_sessions().await {
        emit_fleet(&app, &client, list).await;
    }
    Ok(())
}

/// Open a session and start its chat forwarder. A queued prompt in a lost ACP
/// runtime is work the user already requested, so resume it when opened.
#[tauri::command]
pub async fn open_session(
    app: AppHandle,
    state: State<'_, LoomState>,
    id: String,
) -> Result<SessionView, UiError> {
    let generation = state
        .open_generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        + 1;
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
    let mut chat_cancel = state.chat_cancel.write().await;
    if state
        .open_generation
        .load(std::sync::atomic::Ordering::SeqCst)
        != generation
    {
        return Ok(view);
    }
    if let Some(old) = chat_cancel.take() {
        old.cancel();
    }
    let cancel = CancellationToken::new();
    *chat_cancel = Some(cancel.clone());
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

/// Complete file mentions from a session checkout, or the repository selected
/// before launch. Server-side paths are only read locally for loopback Loom.
#[tauri::command]
pub async fn complete_files(
    state: State<'_, LoomState>,
    id: Option<String>,
    repo: Option<String>,
    query: String,
) -> Result<Vec<String>, UiError> {
    let client = state_client(&state).await?;
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        return client.session_files(&id, &query).await.map_err(Into::into);
    }
    let repo = repo.unwrap_or_default();
    let repo = repo.trim();
    if repo.is_empty() {
        return Ok(Vec::new());
    }
    let managed = client.list_repos().await.unwrap_or_default();
    let root = managed
        .iter()
        .find(|r| r.slug == repo || r.path == repo)
        .map(|r| r.path.as_str())
        .unwrap_or(repo);
    if client.server_host().is_some_and(host_is_loopback) {
        return crate::file_completion::repository_files(std::path::Path::new(root), &query)
            .await
            .map_err(resource_error);
    }
    // Remote repositories use Loom's existing checkout listing; never treat
    // their paths as paths on the Mac running Arachne.
    let sessions = client.list_sessions().await?;
    for session in sessions.iter().filter(|session| {
        session.worktree_present
            && (session.branch.repo_root == root || session.github_repo.as_deref() == Some(repo))
    }) {
        if let Ok(files) = client.session_files(&session.id, &query).await {
            return Ok(files);
        }
    }
    Err(resource_error(
        "no available checkout for repository file completion",
    ))
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

#[tauri::command]
pub async fn answer_permission(
    state: State<'_, LoomState>,
    id: String,
    request_id: String,
    option_id: String,
) -> Result<(), UiError> {
    let client = state_client(&state).await?;
    client
        .answer_permission(&id, &request_id, &option_id)
        .await?;
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

fn launch_repository(repository: &str) -> (Option<String>, String) {
    let repository = repository.trim();
    if std::path::Path::new(repository).is_absolute() {
        (None, repository.to_owned())
    } else {
        (Some(repository.to_owned()), String::new())
    }
}

#[cfg(test)]
mod launch_repository_tests {
    #[test]
    fn local_checkout_uses_cwd_and_managed_repo_uses_slug() {
        assert_eq!(
            super::launch_repository(" /tmp/demo "),
            (None, "/tmp/demo".into())
        );
        assert_eq!(
            super::launch_repository("org/repo"),
            (Some("org/repo".into()), String::new())
        );
    }
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
    // Project bindings the creation form unchecked (design.md: "A new Topic
    // inherits these unless its creation form overrides them"). Recorded as
    // the new topic's initial hides — the binding stays attached to the
    // project and keeps supplying other topics.
    hidden_binding_keys: Option<Vec<String>>,
    as_track: Option<bool>,
    topic_branch: Option<String>,
) -> Result<SessionView, UiError> {
    let client = state_client(&state).await?;
    let (managed_repo, cwd) = launch_repository(&repo);
    let parent = match parent_id.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => Some(client.get_session(id).await?),
        None => None,
    };
    let parent_branch = parent.as_ref().map(|p| p.branch.id.clone());
    if let Some(parent) = &parent {
        let fleet = client.list_sessions().await?;
        prepare_child_autoland(&client, parent, &fleet).await?;
    }
    // A bare prompt (the New thread sheet with no explicit title) sends
    // only `task`, so the title falls back to it like the CLI; the expanded
    // topic card sends an explicit short title and longer body.
    let label = title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .unwrap_or_else(|| task.chars().take(80).collect());
    let canonical = if parent_branch.is_none() && as_track.unwrap_or(true) {
        let branch_label = title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(task.trim());
        Some(allocate_track_branch(&client, &repo, branch_label, topic_branch.as_deref()).await?)
    } else {
        None
    };
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
    // The launch consumes `attachments` (scratch seeding); the names are
    // needed after it, to record the files as the topic's resources.
    let attachment_names: Vec<String> = attachments.iter().map(|file| file.name.clone()).collect();
    let mut view = client
        .launch(&crate::loom::SessionsLaunchInput {
            repo: managed_repo,
            cwd,
            base: base
                .as_deref()
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .map(String::from),
            title: Some(label),
            goal: Some(goal),
            initialization_prompt: canonical
                .as_deref()
                .map(crate::track_policy::initialization_prompt),
            parent_branch: parent_branch.clone(),
            // Empty strings mean "inherit the server default" — filter to
            // None so loom sees omitted-vs-blank as intended.
            profile: profile.filter(|s| !s.is_empty()),
            launch_guidance: Some(if parent_branch.is_some() {
                crate::loom::worker_launch_guidance()
            } else if as_track.unwrap_or(true) {
                crate::loom::topic_launch_guidance()
            } else {
                crate::loom::thread_launch_guidance()
            }),
            agent: agent.filter(|s| !s.is_empty()),
            model: model.filter(|s| !s.is_empty()),
            effort: effort.filter(|s| !s.is_empty()),
            scratch: attachments,
            ..Default::default()
        })
        .await?;
    // Launch attachments are a resource too: the files land at
    // `scratch/<name>` in the fresh worktree (loom seeds them at launch),
    // so they are recorded as File resources of the new branch — the
    // track starts with what you attached, visible in the Resources panel
    // from the first moment (design.md: the effective view). Names are
    // captured before `scratch` consumes the list.
    //
    // Both post-launch manifest edits (these rows, and the initial hides
    // below) go through ONE write so a failure in one cannot strand the
    // other. The branch is brand new — loom keys artifacts by branch id,
    // which `sessions.launch` just created — so the manifest cannot exist
    // yet and revision 0 creates it (base_rev == latest == 0).
    let mut initial_manifest = TopicResourcesView::default();
    for name in &attachment_names {
        let draft = ResourceDraft {
            kind: ResourceKind::File,
            title: name.clone(),
            repository: view.branch.repo_root.clone(),
            // The scratch file is uncommitted worktree content, not a
            // commit: the "ref" that pins it is the branch the launch
            // just created.
            reference: Some(view.branch.branch.clone()),
            path: Some(format!("scratch/{name}")),
            url: None,
        };
        if let Ok(resource) = draft.validated(&view.branch.repo_root, &view.branch.branch) {
            if !initial_manifest
                .resources
                .iter()
                .any(|r| r.id == resource.id)
            {
                initial_manifest.resources.push(resource);
            }
        }
    }
    // The creation form's unchecked project bindings are recorded as the
    // topic's initial hides, matched by reference-free binding key — the
    // binding stays attached to the project and keeps supplying other
    // topics (design.md).
    let hidden_keys = hidden_binding_keys.unwrap_or_default();
    if !hidden_keys.is_empty() {
        initial_manifest.hidden.extend(hidden_keys);
        initial_manifest.hidden.sort();
        initial_manifest.hidden.dedup();
    }
    if !initial_manifest.resources.is_empty() || !initial_manifest.hidden.is_empty() {
        // Best-effort: a failed manifest write must not fail the launch —
        // the attachments are still in Scratch and the goal text; the
        // topic simply inherits everything until the user hides from the
        // panel.
        let _ = save_topic_resources(&client, &view.branch.id, &initial_manifest, 0).await;
    }
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
    if let Some(canonical) = &canonical {
        stamp_new_track_metadata(&client, &mut view, canonical).await;
    } else if parent_branch.is_none() {
        let _ = client
            .set_tag(&view.id, "topic", "false", "Standalone thread")
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
        // Project bindings keep supplying the topic live (design.md:
        // "Project resource bindings may continue to supply shared
        // references to existing Topics") — nothing is copied at creation;
        // the initial hides were folded into the manifest write above.
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
        .or_else(|| {
            std::path::Path::new(&repo)
                .is_dir()
                .then_some(repo.as_str())
        })
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

/// Post-launch failures are recoverable setup blockers, never a failed launch
/// that invites retrying sessions.launch. The opening turn repairs metadata.
async fn stamp_new_track_metadata(client: &LoomClient, view: &mut SessionView, canonical: &str) {
    let mut failures = Vec::new();
    for (key, value, note) in [
        (
            "track_topic_branch",
            canonical,
            "Canonical accepted ref; verify real launch-seed initialization before use",
        ),
        ("topic", "true", "Track coordinator"),
    ] {
        match client.set_tag(&view.id, key, value, note).await {
            Ok(()) => view.branch.tags.push(crate::loom::TagView {
                key: key.into(),
                value: value.into(),
                note: note.into(),
                set_at: String::new(),
                set_by: "arachne".into(),
            }),
            Err(error) => failures.push(format!("{key}: {error}")),
        }
    }
    if !failures.is_empty() {
        let note = format!("Track launched; canonical initialization metadata needs repair for {canonical}. Do not retry launch. {}", failures.join("; "));
        // Retained launch intent makes missing canonical tags fail closed even
        // when every follow-up metadata request fails.
        if !view
            .branch
            .goal
            .contains(crate::track_policy::CANONICAL_INITIALIZATION_MARKER)
        {
            view.branch.goal.push_str(&format!(
                "\n\n{} {canonical}",
                crate::track_policy::CANONICAL_INITIALIZATION_MARKER
            ));
        }
        view.branch.description = note.clone();
        let _ = client
            .set_tag(&view.id, "attention", "blocked", &note)
            .await;
    }
}

/// Inspect server-side refs AND durable Track claims before allocating. A
/// launch race is fenced by exclusive Git ref creation in the opening turn.
pub(crate) async fn allocate_track_branch(
    client: &LoomClient,
    repo: &str,
    title: &str,
    requested: Option<&str>,
) -> Result<String, UiError> {
    if requested
        .filter(|s| !s.is_empty())
        .is_some_and(|s| !crate::track_policy::valid_topic_branch(s))
    {
        return Err(resource_error("Invalid or reserved Track topic branch."));
    }
    let (managed, _) = launch_repository(repo);
    let repo_root = if let Some(slug) = managed {
        client
            .list_repos()
            .await?
            .into_iter()
            .find(|r| r.slug == slug)
            .ok_or_else(|| {
                resource_error(
                    "Managed repository is not available for canonical branch validation.",
                )
            })?
            .path
    } else {
        repo.trim().to_owned()
    };
    let mut occupied: Vec<String> = client
        .repo_branches(&repo_root)
        .await?
        .into_iter()
        .map(|b| b.name)
        .collect();
    for session in client.list_sessions().await? {
        if session.branch.repo_root == repo_root {
            if let Some(claimed) = crate::track_policy::tag(&session.branch, "track_topic_branch")
                .or_else(|| crate::track_policy::pending_topic_branch(&session.branch))
            {
                occupied.push(claimed.into());
            }
        }
    }
    crate::track_policy::choose_topic_branch(requested, title, &occupied).map_err(resource_error)
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
    let base = if fleet
        .iter()
        .find(|s| s.id == parent.id)
        .is_some_and(crate::track_policy::owns_track)
    {
        crate::track_policy::canonical_topic_branch(&parent.branch)
            .ok_or_else(|| resource_error("Invalid canonical Track ref."))?
            .to_owned()
    } else {
        resolve_integration_target(&fleet, &parent_id)
            .map(|target| target.target_branch)
            .unwrap_or_else(|| parent.branch.branch.clone())
    };
    prepare_child_autoland(&client, &parent, &fleet).await?;
    let view = client
        .launch(&crate::loom::SessionsLaunchInput {
            repo: parent.github_repo.clone(),
            cwd: parent.work_dir.clone(),
            base: Some(base),
            title: Some(task.chars().take(80).collect()),
            goal: Some(task),
            parent_branch: Some(parent.branch.id),
            launch_guidance: Some(crate::loom::worker_launch_guidance()),
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
    if should_mark_track(&fleet, &parent_id) {
        let _ = client
            .set_tag(&parent_id, "topic", "true", "joined by child session")
            .await;
    }
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

/// Dismiss a session's attention flag without opening the thread: clear
/// the loud tag axes on its branch. Loom's tag semantics make absence the
/// calm state (there is no stored `ok` tag — `clear` deletes the row), so
/// after this the session leaves every Needs You bucket and its badge
/// falls quiet. Both loud keys are cleared — `attention` (the agent's
/// self-report) and `triage` (an outside assessment) raise the same
/// badge, and a person dismissing one means the other stays gone too.
/// Tool-approval attention (`pending_permissions`) is NOT touched: that is
/// a live ACP request only its options can answer.
#[tauri::command]
pub async fn clear_attention(
    app: AppHandle,
    state: State<'_, LoomState>,
    session: String,
) -> Result<(), UiError> {
    let client = state_client(&state).await?;
    for key in ["attention", "triage"] {
        client.clear_tag(&session, key).await?;
    }
    // Fresh fleet snapshot, pushed so every surface (sidebar badges, Needs
    // You buckets, inspector rows) drops the loud state at once. Tag
    // changes also publish on the session's own SSE topic, but the push
    // here keeps the UI instant even when that stream lags or is capped.
    if let Ok(list) = client.list_sessions().await {
        emit_fleet(&app, &client, list).await;
    }
    Ok(())
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
    let target = zed_target(host, work_dir.as_deref().unwrap_or(&view.work_dir))?;
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
    client
        .ensure_worktree(&repo_root, &branch)
        .await
        .map_err(Into::into)
}

/// The status a PR row needs to show its light, fetched straight from GitHub
/// via the `gh` CLI — the same fields loom's poll loop snapshots. Used for
/// *attached* PR resources (manifest rows carry only a URL), so they light up
/// without waiting for loom's GitHub App credential. Fails soft: the row
/// simply stays unlit, exactly like an unknown live state.
#[derive(Debug, serde::Serialize)]
pub struct PrStatusView {
    pub state: String,
    pub mergeable: Option<String>,
    /// Loom's rollup verdict: `passing` / `failing` / `pending`, or null when
    /// the PR has no checks at all.
    pub checks: Option<String>,
    pub title: Option<String>,
}

/// Parse `https://github.com/<owner>/<repo>/pull/<n>` into its parts.
fn parse_pr_url(url: &str) -> Option<(String, i64)> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("https://www.github.com/"))?;
    let mut parts = rest.trim_end_matches('/').splitn(3, '/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    let tail = parts.next()?;
    let number = tail.strip_prefix("pull/")?.parse().ok()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((format!("{owner}/{repo}"), number))
}

/// One entry of `gh pr view --json statusCheckRollup`, shaped like loom's
/// `CheckJson`: either a modern check run (status + conclusion) or a legacy
/// commit status (state). `__typename` distinguishes them.
#[derive(Debug, serde::Deserialize)]
struct GhCheck {
    #[serde(default)]
    __typename: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(rename = "startedAt", default)]
    started_at: Option<String>,
    #[serde(rename = "workflowName", default)]
    workflow_name: Option<String>,
    #[serde(rename = "checkSuite", default)]
    check_suite: Option<GhCheckSuite>,
}

#[derive(Debug, serde::Deserialize)]
struct GhCheckSuite {
    #[serde(rename = "workflowRun", default)]
    workflow_run: Option<GhWorkflowRun>,
}

#[derive(Debug, serde::Deserialize)]
struct GhWorkflowRun {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    workflow: Option<GhWorkflow>,
}

#[derive(Debug, serde::Deserialize)]
struct GhWorkflow {
    #[serde(default)]
    name: Option<String>,
}

impl GhCheck {
    fn workflow_name(&self) -> &str {
        self.workflow_name
            .as_deref()
            .or_else(|| {
                self.check_suite
                    .as_ref()?
                    .workflow_run
                    .as_ref()?
                    .workflow
                    .as_ref()?
                    .name
                    .as_deref()
            })
            .unwrap_or("")
    }

    fn event(&self) -> &str {
        self.check_suite
            .as_ref()
            .and_then(|suite| suite.workflow_run.as_ref())
            .and_then(|run| run.event.as_deref())
            .unwrap_or("")
    }

    /// Logical check identity, matching `gh pr checks`: job name + workflow +
    /// event, or the context for legacy statuses. Used to keep only the newest
    /// attempt per check.
    fn identity(&self) -> Option<String> {
        if let Some(context) = self.context.as_deref() {
            return Some(format!("context\0{context}"));
        }
        self.name
            .as_deref()
            .map(|name| format!("run\0{name}\0{}\0{}", self.workflow_name(), self.event()))
    }
}

/// Keep only the newest attempt for each logical check (GitHub's rollup
/// includes superseded runs), then roll up to one verdict exactly as loom's
/// poll loop does: any failure ⇒ `failing`, else anything running ⇒ `pending`,
/// else `passing`. None when the PR has no checks at all.
fn rollup_checks(items: &[GhCheck]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let mut newest_first = items.iter().collect::<Vec<_>>();
    newest_first.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    let mut seen = std::collections::HashSet::new();
    let mut any_pending = false;
    let mut any_fail = false;
    for it in newest_first {
        if !it.identity().is_none_or(|identity| seen.insert(identity)) {
            continue;
        }
        if let Some(status) = it.status.as_deref() {
            if status != "COMPLETED" {
                any_pending = true;
                continue;
            }
            match it.conclusion.as_deref().unwrap_or("") {
                "SUCCESS" | "NEUTRAL" | "SKIPPED" | "CANCELLED" => {}
                "ERROR" | "FAILURE" | "TIMED_OUT" | "ACTION_REQUIRED" => any_fail = true,
                _ => any_pending = true,
            }
        } else if let Some(state) = it.state.as_deref() {
            match state {
                "SUCCESS" => {}
                "PENDING" | "EXPECTED" => any_pending = true,
                "FAILURE" | "ERROR" => any_fail = true,
                _ => any_pending = true,
            }
        } else {
            any_pending = true;
        }
    }
    Some(
        if any_fail {
            "failing"
        } else if any_pending {
            "pending"
        } else {
            "passing"
        }
        .to_string(),
    )
}

#[derive(Debug, serde::Deserialize)]
struct GhPrView {
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    mergeable: Option<String>,
    #[serde(rename = "statusCheckRollup", default)]
    status_check_rollup: Vec<GhCheck>,
    #[serde(default)]
    title: Option<String>,
}

#[tauri::command]
/// Resolve the gh CLI by its well-known absolute paths — GUI apps inherit
/// launchd's minimal PATH, which misses homebrew.
fn gh_path() -> &'static str {
    ["/opt/homebrew/bin/gh", "/usr/local/bin/gh"]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
        .unwrap_or("gh")
}

/// One result of the attach picker's GitHub search.
#[derive(Debug, serde::Serialize)]
pub struct GhSearchRow {
    pub number: i64,
    pub title: String,
    /// OPEN / CLOSED / MERGED, GitHub's capitals.
    pub state: String,
    pub url: String,
}

/// Search a repo's issues or PRs for the attach picker: typed words fuzzy
/// match titles (GitHub search), a bare number jumps straight to that item.
/// Closed items are included — merged PRs are often what you want to attach.
#[tauri::command]
pub async fn gh_search(
    repo: String,
    kind: String,
    query: String,
) -> Result<Vec<GhSearchRow>, UiError> {
    let is_pr = kind == "pull_request";
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let gh = gh_path();
    let mut cmd = tokio::process::Command::new(gh);
    cmd.env("GH_PAGER", "").args([
        "search",
        if is_pr { "prs" } else { "issues" },
        "--repo",
        &repo,
        "--json",
        "number,title,state,url",
        "--limit",
        "20",
    ]);
    if let Ok(number) = trimmed.parse::<i64>() {
        // A bare number is a jump: search that exact number (`N in:number`).
        cmd.arg(format!("{number} in:number"));
    } else {
        // No --state filter: open and closed both come back, so merged PRs
        // are attachable. The state rides the row.
        cmd.arg(trimmed);
    }
    let output = cmd
        .output()
        .await
        .map_err(|e| resource_error(format!("launching gh: {e} (is gh installed?)")))?;
    if !output.status.success() {
        return Err(resource_error(format!(
            "gh search failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    #[derive(Debug, serde::Deserialize)]
    struct Row {
        number: i64,
        title: String,
        state: String,
        url: String,
    }
    let rows: Vec<Row> = serde_json::from_slice(&output.stdout)
        .map_err(|e| resource_error(format!("parsing gh output: {e}")))?;
    Ok(rows
        .into_iter()
        .map(|row| GhSearchRow {
            number: row.number,
            title: row.title,
            state: row.state.to_uppercase(),
            url: row.url,
        })
        .collect())
}

#[tauri::command]
pub async fn pr_status(url: String) -> Result<PrStatusView, UiError> {
    let (repo, number) = parse_pr_url(&url)
        .ok_or_else(|| resource_error(format!("not a github.com pull request URL: {url}")))?;
    // GUI apps inherit launchd's minimal PATH, which misses homebrew —
    // where gh lives; gh_path() prefers the well-known absolute locations.
    let output = tokio::process::Command::new(gh_path())
        .env("GH_PAGER", "")
        .args([
            "pr",
            "view",
            "--repo",
            &repo,
            "--json",
            "state,mergeable,statusCheckRollup,title",
        ])
        .arg(&number.to_string())
        .output()
        .await
        .map_err(|e| resource_error(format!("launching gh: {e} (is gh installed and on PATH?)")))?;
    if !output.status.success() {
        return Err(resource_error(format!(
            "gh pr view failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let view: GhPrView = serde_json::from_slice(&output.stdout)
        .map_err(|e| resource_error(format!("parsing gh output: {e}")))?;
    Ok(PrStatusView {
        state: view.state.unwrap_or_default(),
        mergeable: view.mergeable.filter(|v| !v.is_empty()),
        checks: rollup_checks(&view.status_check_rollup),
        title: view.title,
    })
}

#[cfg(test)]
mod zed_tests {
    use super::zed_target;

    #[test]
    fn local_checkout_is_a_path() {
        assert_eq!(zed_target("127.0.0.1", "/tmp/a b").unwrap(), "/tmp/a b");
    }
}

#[cfg(test)]
mod pr_status_tests {
    use super::{parse_pr_url, rollup_checks, GhCheck};

    fn check(name: &str, status: Option<&str>, conclusion: Option<&str>) -> GhCheck {
        GhCheck {
            __typename: Some("CheckRun".into()),
            name: Some(name.into()),
            context: None,
            status: status.map(String::from),
            conclusion: conclusion.map(String::from),
            state: None,
            started_at: Some("2026-01-01T00:00:00Z".into()),
            workflow_name: None,
            check_suite: None,
        }
    }

    #[test]
    fn parses_github_pr_urls() {
        assert_eq!(
            parse_pr_url("https://github.com/acme/app/pull/13"),
            Some(("acme/app".into(), 13))
        );
        assert_eq!(
            parse_pr_url("https://www.github.com/acme/app/pull/13/"),
            Some(("acme/app".into(), 13))
        );
        assert_eq!(parse_pr_url("https://gitlab.com/acme/app/pull/13"), None);
        assert_eq!(parse_pr_url("https://github.com/acme/app/issues/13"), None);
    }

    #[test]
    fn rollup_matches_looms_semantics() {
        // Empty rollup: no verdict at all, not "passing".
        assert_eq!(rollup_checks(&[]), None);
        // All terminal-success: passing.
        assert_eq!(
            rollup_checks(&[
                check("ci", Some("COMPLETED"), Some("SUCCESS")),
                check("lint", Some("COMPLETED"), Some("SKIPPED")),
            ]),
            Some("passing".into())
        );
        // Any failure beats in-progress.
        assert_eq!(
            rollup_checks(&[
                check("ci", Some("COMPLETED"), Some("FAILURE")),
                check("lint", Some("IN_PROGRESS"), None),
            ]),
            Some("failing".into())
        );
        // Running but nothing failed: pending.
        assert_eq!(
            rollup_checks(&[check("ci", Some("IN_PROGRESS"), None)]),
            Some("pending".into())
        );
        // A superseded failed attempt is ignored when a newer attempt exists.
        // The SUCCESS at 00:00:01 is the newest attempt for check "ci", so the
        // FAILURE at 00:00:00 is superseded and does not count.
        let mut passed = check("ci", Some("COMPLETED"), Some("SUCCESS"));
        passed.started_at = Some("2026-01-01T00:00:01Z".into());
        let mut failed = check("ci", Some("COMPLETED"), Some("FAILURE"));
        failed.started_at = Some("2026-01-01T00:00:00Z".into());
        assert_eq!(rollup_checks(&[failed, passed]), Some("passing".into()));
    }
}

#[cfg(test)]
mod zed_tests_more {
    use super::{thread_note_body, zed_target};

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
    Ok(FleetSnapshot {
        server_url: client.server_url(),
        sessions,
        layout,
    })
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

/// A child acquiring children is a local coordinator, not a new canonical Track.
pub fn should_mark_track(fleet: &[SessionSummaryView], parent_id: &str) -> bool {
    fleet
        .iter()
        .find(|s| s.id == parent_id)
        .is_some_and(|s| s.parent_session_id.is_none() && s.parent_id.is_none())
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
    // Only a root is promoted; recursive delegation does not change Track ownership.
    if let Some(parent) = parent_id.as_deref().filter(|p| !p.is_empty()) {
        if let Ok(fleet) = client.list_sessions().await {
            if should_mark_track(&fleet, parent) {
                let _ = client
                    .set_tag(parent, "topic", "true", "joined by child session")
                    .await;
            }
        }
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

/// Resolve acceptance to a marked same-repository Track and its canonical ref.
pub fn resolve_integration_target(
    fleet: &[SessionSummaryView],
    session_id: &str,
) -> Option<IntegrationTarget> {
    let track = crate::track_policy::track_ancestor(fleet, session_id)?;
    Some(IntegrationTarget {
        coordinator_id: track.id.clone(),
        target_branch: crate::track_policy::canonical_topic_branch(&track.branch)?.into(),
        coordinator_name: track.branch.name.clone(),
    })
}

/// Direct results already link to the parent. Nested results link to their
/// immediate parent's channel, so THAT channel must deliver to the Track.
async fn ensure_acceptance_route(
    client: &LoomClient,
    fleet: &[SessionSummaryView],
    source: &SessionSummaryView,
    coordinator_id: &str,
) -> Result<(), UiError> {
    client
        .ensure_delivery_subscription(coordinator_id, coordinator_id)
        .await?;
    let parent = crate::track_policy::immediate_parent(fleet, source).ok_or_else(|| {
        resource_error("Cannot route source results without its immediate parent channel.")
    })?;
    if parent.id != coordinator_id {
        client
            .ensure_delivery_subscription(&parent.id, coordinator_id)
            .await?;
    }
    Ok(())
}

pub(crate) async fn land_thread_with_client(
    client: &LoomClient,
    session_id: &str,
    strategy: Option<&str>,
) -> Result<IntegrationTarget, UiError> {
    let choice = strategy
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("squash");
    let strategy = crate::loom::IntegrationStrategy::parse(choice)
        .filter(|s| matches!(s, crate::loom::IntegrationStrategy::Squash | crate::loom::IntegrationStrategy::Merge | crate::loom::IntegrationStrategy::Rebase | crate::loom::IntegrationStrategy::CherryPick | crate::loom::IntegrationStrategy::Ask))
        .ok_or_else(|| resource_error("Thread Land accepts squash, merge, rebase or cherry-pick; publishing is a separate Track action."))?;
    let fleet = client.list_sessions().await?;
    let mut target = resolve_integration_target(&fleet, session_id).ok_or_else(|| {
        resource_error("No canonical owning Track target is available for this repository. Attach the source to a Track or finish its canonical branch initialization, then retry Thread Land.")
    })?;
    let source = fleet.iter().find(|s| s.id == session_id).unwrap();
    let view = client.get_session(session_id).await?;
    if crate::track_policy::tag(&view.branch, "role") == Some("reviewer") {
        return Err(resource_error(
            "Review threads report findings; accept the implementation thread instead.",
        ));
    }
    let coordinator = client.get_session(&target.coordinator_id).await?;
    if view.branch.repo_root != coordinator.branch.repo_root
        || view.branch.repo_root != source.branch.repo_root
    {
        return Err(resource_error(
            "Source and Track must remain in the same repository.",
        ));
    }
    let owner = fleet.iter().find(|s| s.id == coordinator.id).unwrap();
    if coordinator.status == "archived"
        || crate::track_policy::tag(&coordinator.branch, "topic") == Some("false")
        || !crate::track_policy::owns_track(owner)
    {
        return Err(resource_error(
            "The Track coordinator must be live and own this repository; standalone topic=false roots cannot accept.",
        ));
    }
    target.target_branch = crate::track_policy::canonical_topic_branch(&coordinator.branch)
        .ok_or_else(|| resource_error("The canonical Track ref is invalid."))?
        .into();
    if view.branch.branch == target.target_branch {
        return Err(resource_error(
            "Source and canonical target already use the same branch.",
        ));
    }
    ensure_acceptance_route(client, &fleet, source, &target.coordinator_id).await?;
    // Request identity only: active/uncommitted work still wraps up under this
    // authorization, and the coordinator must review/reverify its final SHA.
    // Bind both delivery stages to one snapshot rather than activity counters
    // that our own delivery can change. Later committed work gets a new key.
    let source_changes = client.session_changes(session_id).await?;
    let source_request_version = source_changes.version;
    let source_request_revision = source_changes.head_oid
        .filter(|revision| !revision.trim().is_empty())
        .ok_or_else(|| resource_error("Cannot bind Thread Land request identity without a source HEAD. Reconcile the source repository before authorizing delivery."))?;
    let request = crate::loom::IntegrationRequest {
        action: "land-thread",
        source_session: view.id.clone(),
        source_branch: view.branch.branch.clone(),
        source_work_dir: view.work_dir.clone(),
        repo_root: view.branch.repo_root.clone(),
        target_session: target.coordinator_id.clone(),
        target_branch: target.target_branch.clone(),
        strategy,
        requested_by: "user",
    };
    // Advisory only. Failed metadata does not prevent the authorized delivery.
    let _ = client
        .set_tag(
            &view.id,
            "landing_request",
            "pending",
            "Thread Land wrap-up/acceptance requested; not readiness",
        )
        .await;
    client
        .deliver_request(
            &view,
            &request.source_wrap_up_prompt(),
            serde_json::json!({ "action": "land-thread-wrap-up", "request": request, "source_request_revision": source_request_revision, "source_request_version": source_request_version }),
        )
        .await?;
    client
        .deliver_request(
            &coordinator,
            &request.to_prompt(&coordinator.branch.name),
            serde_json::json!({ "action": "land-thread", "request": request, "source_request_revision": source_request_revision, "source_request_version": source_request_version }),
        )
        .await?;
    Ok(target)
}

#[tauri::command]
pub async fn land_thread(
    app: AppHandle,
    state: State<'_, LoomState>,
    session_id: String,
    strategy: Option<String>,
) -> Result<IntegrationTarget, UiError> {
    let client = state_client(&state).await?;
    let target = land_thread_with_client(&client, &session_id, strategy.as_deref()).await?;
    let _ = app.emit("loom://integration-sent", &target);
    Ok(target)
}

/// Compatibility alias with identical Thread Land acceptance semantics.
#[tauri::command]
pub async fn integrate_session(
    app: AppHandle,
    state: State<'_, LoomState>,
    session_id: String,
    strategy: Option<String>,
) -> Result<IntegrationTarget, UiError> {
    land_thread(app, state, session_id, strategy).await
}

#[derive(Debug, Clone, Serialize)]
pub struct AutolandPolicyResult {
    pub session: SessionView,
    pub warnings: Vec<String>,
}

pub(crate) async fn set_autoland_with_client(
    client: &LoomClient,
    session_id: &str,
    policy: &str,
) -> Result<AutolandPolicyResult, UiError> {
    let view = client.get_session(session_id).await?;
    let fleet = client.list_sessions().await?;
    let track_policy = fleet
        .iter()
        .find(|s| s.id == view.id)
        .is_some_and(crate::track_policy::owns_track);
    let allowed = if track_policy {
        matches!(policy, "on" | "off")
    } else {
        matches!(policy, "inherit" | "on" | "off")
    };
    if !allowed {
        return Err(resource_error(
            "Track policy accepts on/off; source policy accepts inherit/on/off.",
        ));
    }
    let coordinator_id = if track_policy {
        view.id.clone()
    } else {
        crate::track_policy::track_ancestor(&fleet, session_id)
            .ok_or_else(|| {
                resource_error("Autoland needs a marked same-repository Track ancestor.")
            })?
            .id
            .clone()
    };
    let coordinator = if track_policy {
        view.clone()
    } else {
        client.get_session(&coordinator_id).await?
    };
    if coordinator.status == "archived"
        || crate::track_policy::tag(&coordinator.branch, "topic") == Some("false")
        || coordinator.branch.repo_root != view.branch.repo_root
    {
        return Err(resource_error(
            "Autoland needs a live same-repository Track.",
        ));
    }
    // Persist OFF immediately even if route setup later fails; stale automatic
    // requests are required to read this live tag before every write.
    let key = if track_policy {
        "track_autoland"
    } else {
        "thread_autoland"
    };
    client
        .set_tag(
            session_id,
            key,
            policy,
            "Durable autoland policy; reread live before automatic writes",
        )
        .await?;
    // The acknowledged tag write is durable even when setup/refresh fails.
    // Keep that confirmed state separate from best-effort delivery setup.
    let mut saved = view.clone();
    saved.branch.tags.retain(|tag| tag.key != key);
    saved.branch.tags.push(crate::loom::TagView {
        key: key.into(),
        value: policy.into(),
        note: "Durable autoland policy; reread live before automatic writes".into(),
        set_at: String::new(),
        set_by: "arachne".into(),
    });
    let mut warnings = Vec::new();
    if let Err(error) = setup_autoland_policy(
        client,
        &fleet,
        &coordinator,
        session_id,
        policy,
        view.branch.tags.iter().find(|tag| tag.key == key),
    )
    .await
    {
        warnings.push(format!(
            "Autoland policy {policy} was saved, but setup is incomplete: {}",
            error.message
        ));
    }
    match client.get_session(session_id).await {
        Ok(fresh) => saved = fresh,
        Err(error) => warnings.push(format!(
            "Autoland policy {policy} was saved; session refresh failed: {error}"
        )),
    }
    Ok(AutolandPolicyResult {
        session: saved,
        warnings,
    })
}

async fn setup_autoland_policy(
    client: &LoomClient,
    fleet: &[SessionSummaryView],
    coordinator: &SessionView,
    session_id: &str,
    policy: &str,
    previous_policy_tag: Option<&crate::loom::TagView>,
) -> Result<(), UiError> {
    let coordinator_id = &coordinator.id;
    let canonical = crate::track_policy::canonical_topic_branch(&coordinator.branch)
        .ok_or_else(|| resource_error("Autoland policy was saved, but canonical Track initialization is incomplete or invalid. Repair initialization before acceptance routing can be completed."))?;
    client
        .ensure_delivery_subscription(&coordinator_id, &coordinator_id)
        .await?;
    let mut routed = std::collections::HashSet::new();
    for source in fleet {
        if crate::track_policy::track_ancestor(fleet, &source.id)
            .is_some_and(|t| &t.id == coordinator_id)
        {
            if let Some(parent) = crate::track_policy::immediate_parent(&fleet, source) {
                if &parent.id != coordinator_id && routed.insert(parent.id.clone()) {
                    client
                        .ensure_delivery_subscription(&parent.id, &coordinator_id)
                        .await?;
                }
            }
        }
    }
    // A later On -> Off -> On transition must wake the coordinator again.
    // The previously observed durable tag revision distinguishes that new
    // policy authorization while keeping retries of this envelope stable.
    client.deliver_request(&coordinator, &crate::loom::autoland_policy_prompt(&coordinator, canonical), serde_json::json!({"action":"autoland-policy", "session_id":session_id, "policy":policy, "previous_policy_tag":previous_policy_tag, "track_id":coordinator_id, "target_branch":canonical})).await?;
    Ok(())
}

#[tauri::command]
pub async fn set_autoland(
    app: AppHandle,
    state: State<'_, LoomState>,
    session_id: String,
    policy: String,
) -> Result<AutolandPolicyResult, UiError> {
    let client = state_client(&state).await?;
    let view = set_autoland_with_client(&client, &session_id, &policy).await?;
    if let Ok(fleet) = client.list_sessions().await {
        emit_fleet(&app, &client, fleet).await;
    }
    Ok(view)
}

/// Establish routes and notify the Track BEFORE a future child can report a
/// result. Policy is checked again by the coordinator at every actual write.
async fn prepare_child_autoland(
    client: &LoomClient,
    parent: &SessionView,
    fleet: &[SessionSummaryView],
) -> Result<(), UiError> {
    let track_id = if fleet
        .iter()
        .find(|s| s.id == parent.id)
        .is_some_and(crate::track_policy::owns_track)
    {
        Some(parent.id.as_str())
    } else {
        crate::track_policy::track_ancestor(fleet, &parent.id).map(|t| t.id.as_str())
    };
    let Some(track_id) = track_id else {
        return Ok(());
    };
    let track = if track_id == parent.id {
        parent.clone()
    } else {
        client.get_session(track_id).await?
    };
    // A future child defaults to inherit, independent of its parent's override.
    if crate::track_policy::tag(&track.branch, "track_autoland") != Some("on") {
        return Ok(());
    }
    let canonical = crate::track_policy::canonical_topic_branch(&track.branch)
        .ok_or_else(|| resource_error("Invalid canonical Track ref."))?;
    client
        .ensure_delivery_subscription(track_id, track_id)
        .await?;
    if parent.id != track_id {
        client
            .ensure_delivery_subscription(&parent.id, track_id)
            .await?;
    }
    client.deliver_request(&track, &crate::loom::autoland_policy_prompt(&track, canonical), serde_json::json!({"action":"autoland-child-launch", "parent_session":parent.id, "track_id":track_id, "target_branch":canonical})).await?;
    Ok(())
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
#[cfg(test)]
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

/// The result of a Track Publish action. Agent-mediated strategies send a
/// request (their outcome is reported in the topic thread). A suitable
/// `land-locally` fast path returns the completed squash-merge; queued
/// preparation or repair always returns local=false.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LandResult {
    /// True when this landing happened locally, right now.
    pub local: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub landing: Option<LocalLandingView>,
}

/// Compatibility name for Track Publish; resolve the canonical accepted ref.
/// Worker acceptance uses land_thread instead.
#[tauri::command]
pub async fn land_topic(
    state: State<'_, LoomState>,
    session_id: String,
    strategy: String,
    upstream: Option<String>,
) -> Result<LandResult, UiError> {
    // Compatibility for older callers; publishing always resolves the canonical
    // accepted Track ref through the same path as the current Publish UI.
    crate::publishing::publish_track(state, session_id, strategy, upstream).await
}

#[cfg(test)]
async fn dispatch_landing(
    client: &LoomClient,
    view: &SessionView,
    request: crate::loom::LandingRequest,
) -> Result<LandResult, UiError> {
    let prompt = request.to_prompt(&view.branch.name);
    client.resume_if_orphaned(&view.id).await?;
    if view.protocol == "terminal" {
        client.send_text(&view.id, &prompt, true).await?;
    } else {
        client.queue_prompt(&view.id, &prompt).await?;
    }
    // A request is never a completed landing and must not stamp landed tags.
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
    pub warnings: Vec<String>,
}

impl From<LocalLanding> for LocalLandingView {
    fn from(landing: LocalLanding) -> Self {
        LocalLandingView {
            landed: landing.landed,
            target_branch: landing.target_branch,
            commit: landing.commit,
            commits_squashed: landing.commits_squashed,
            primary_checkout: landing.primary_checkout,
            warnings: landing.warnings,
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
/// branch, or queue preparation/repair and that same chosen landing.
///
/// Preconditions beyond `squash_into_primary_checkout`'s own preflight:
/// the Loom server must be loopback (its `repo_root` paths are local
/// paths; a remote server would make Arachne run git against a
/// server-side path that is meaningless here), and the topic must own a
/// branch — an archived or branchless topic has nothing to land.
#[cfg(test)]
async fn land_topic_locally(
    client: Arc<LoomClient>,
    view: SessionView,
    summary: &SessionSummaryView,
) -> Result<LandResult, UiError> {
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
    let source = crate::landing::inspect_source_checkout(
        std::path::Path::new(&view.work_dir),
        &view.branch.branch,
    )
    .await
    .map_err(resource_error)?;
    crate::landing::ensure_same_repository(
        std::path::Path::new(&view.work_dir),
        std::path::Path::new(&view.branch.repo_root),
    )
    .await
    .map_err(resource_error)?;
    let target = crate::landing::inspect_local_target(
        std::path::Path::new(&view.branch.repo_root),
        &view.branch.branch,
    )
    .await
    .map_err(|e| resource_error(e.to_string()))?;
    // Target inspection runs several git reads; refresh source immediately
    // before selecting the synchronous path so intervening saves aren't omitted.
    let refreshed = crate::landing::inspect_source_checkout(
        std::path::Path::new(&view.work_dir),
        &view.branch.branch,
    )
    .await
    .map_err(resource_error)?;
    let source_moved = refreshed.revision != source.revision;
    let source = refreshed;
    let reason = if !source.status.trim().is_empty() {
        Some("The source checkout has uncommitted edits requiring inspection, validation and intended-source commits.".to_string())
    } else if source_moved {
        Some("The source revision changed during preflight; refresh review and validation before landing.".into())
    } else if target
        .status
        .lines()
        .any(|line| !line.starts_with("??") && !line.starts_with(' '))
    {
        Some("The primary checkout has staged changes; preserve the user's index and working tree during landing.".to_string())
    } else {
        None
    };
    let attempt = if reason.is_none() {
        Some(
            crate::landing::squash_into_primary_checkout(
                std::path::Path::new(&view.branch.repo_root),
                &view.branch.branch,
                &summary_title(summary, &view),
            )
            .await,
        )
    } else {
        None
    };
    let (result, reason) = match attempt {
        Some(Ok(result)) => (Some(result), None),
        Some(Err(crate::landing::LandingError::StagedChanges)) => (
            None,
            Some(
                "The primary checkout acquired staged changes; preserve them while landing.".into(),
            ),
        ),
        Some(Err(crate::landing::LandingError::RepairNeeded { detail })) => (None, Some(detail)),
        Some(Err(error)) => return Err(resource_error(error.to_string())),
        None => (None, reason),
    };
    let Some(result) = result else {
        // Snapshot exact local destination, never recorded base/main. The
        // coordinator must refresh both snapshots before it writes.
        let request = crate::loom::LandingRequest {
            action: "land",
            source_session: view.id.clone(),
            source_work_dir: view.work_dir.clone(),
            source_state: Some(source),
            target_upstream: target.branch.clone(),
            target_origin: "primary-checkout".into(),
            local_target: Some(target),
            repair_reason: reason,
            source_branch: view.branch.branch.clone(),
            repo_root: view.branch.repo_root.clone(),
            strategy: crate::loom::IntegrationStrategy::LandLocally,
            requested_by: "user",
        };
        return dispatch_landing(&client, &view, request).await;
    };
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
    Ok(LandResult {
        local: true,
        landing: Some(LocalLandingView::from(result)),
    })
}

/// A landing title for the squash commit: the topic's name, else the
/// branch's title, else the branch name.
#[cfg(test)]
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

/// Actual canonical comparison for loopback repositories, with explicit remote limits.
#[tauri::command]
pub async fn work_changes(
    state: State<'_, LoomState>,
    session_id: String,
) -> Result<ReviewChanges, UiError> {
    let client = state_client(&state).await?;
    load_review_changes(&client, &session_id).await
}

/// A canonical preview identifies its actual source and comparison, rather than
/// re-labelling Loom's recorded-base changes as accepted-state evidence.
#[derive(Debug, Serialize)]
pub struct ReviewChanges {
    #[serde(flatten)]
    pub changes: crate::loom::ChangeSetView,
    pub source_ref: String,
    pub accepted_source: bool,
    pub comparison: &'static str,
}

async fn load_review_changes(
    client: &LoomClient,
    session_id: &str,
) -> Result<ReviewChanges, UiError> {
    let source = client.get_session(session_id).await?;
    if source.id != session_id {
        return Err(resource_error(
            "The review source identity changed; refresh before comparing.",
        ));
    }
    let comparison = crate::coordination::resolve_review_comparison(client, &source).await?;
    review_changes_for_source(client, &source, &comparison).await
}

async fn review_changes_for_source(
    client: &LoomClient,
    source: &SessionView,
    comparison: &crate::coordination::ReviewComparison,
) -> Result<ReviewChanges, UiError> {
    // This check precedes every filesystem or Git read: server paths are not
    // desktop paths. There is no arbitrary-ref diff endpoint in Loom today.
    if !host_is_loopback(client.server_host().unwrap_or("")) {
        return Ok(ReviewChanges {
            source_ref: comparison.source_ref.clone(),
            accepted_source: comparison.accepted_source,
            comparison: "target-tree",
            changes: crate::loom::ChangeSetView {
                base: serde_json::json!({"state":"unavailable", "reference":comparison.target_ref,
                    "reason":"remote_canonical_preview_unavailable"}),
                ..Default::default()
            },
        });
    }
    local_review_changes(source, comparison).await
}

/// Bounded read-only Git capture. Disable optional index writes, external diff
/// drivers and text conversion; a preview never fetches or runs repo helpers.
async fn review_git(
    root: &std::path::Path,
    args: &[&str],
    cap: usize,
) -> Result<(Vec<u8>, bool), UiError> {
    use std::process::Stdio;
    use tokio::io::{AsyncRead, AsyncReadExt};
    async fn capture(mut reader: impl AsyncRead + Unpin, cap: usize) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut chunk = [0; 8192];
        loop {
            let n = reader.read(&mut chunk).await?;
            if n == 0 {
                return Ok(bytes);
            }
            let keep = n.min((cap + 1).saturating_sub(bytes.len()));
            bytes.extend_from_slice(&chunk[..keep]);
        }
    }
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "diff.autoRefreshIndex=false",
            "-c",
            "color.ui=false",
            "-c",
            "submodule.recurse=false",
        ])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|e| resource_error(e.to_string()))?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let (out, err) = tokio::join!(capture(stdout, cap), capture(stderr, 8192));
        let mut out = out.map_err(|e| resource_error(e.to_string()))?;
        let err = err.map_err(|e| resource_error(e.to_string()))?;
        let truncated = out.len() > cap;
        let status = child
            .wait()
            .await
            .map_err(|e| resource_error(e.to_string()))?;
        if !status.success() {
            return Err(resource_error(format!(
                "Canonical Git comparison failed: {}",
                String::from_utf8_lossy(&err).trim()
            )));
        }
        out.truncate(cap);
        Ok((out, truncated))
    })
    .await
    .map_err(|_| resource_error("Canonical Git comparison timed out."))?;
    output
}

async fn review_oid(root: &std::path::Path, reference: &str) -> Result<String, UiError> {
    let (bytes, truncated) = review_git(
        root,
        &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
        128,
    )
    .await?;
    let oid = String::from_utf8_lossy(&bytes).trim().to_owned();
    if truncated || !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(resource_error(
            "The comparison ref does not resolve to a Git commit.",
        ));
    }
    Ok(oid)
}

fn review_path(raw: &[u8]) -> serde_json::Value {
    serde_json::json!({"display":String::from_utf8_lossy(raw),
        "bytes":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)})
}

fn review_hunks(patch: &[u8], line_budget: &mut usize) -> (Vec<serde_json::Value>, u32, u32, bool) {
    let mut hunks = Vec::new();
    let mut header = String::new();
    let mut lines = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    let (mut additions, mut deletions) = (0, 0);
    let mut truncated = false;
    for line in String::from_utf8_lossy(patch).lines() {
        if line.starts_with("@@ ") {
            if !header.is_empty() {
                hunks.push(serde_json::json!({"header":header,"lines":lines,"truncated":false}));
                lines = Vec::new();
            }
            if hunks.len() >= 50 {
                truncated = true;
                break;
            }
            header = line.to_string();
            let mut parts = line.split_whitespace();
            parts.next();
            old = parts
                .next()
                .and_then(|s| s.trim_start_matches('-').split(',').next()?.parse().ok())
                .unwrap_or(0);
            new = parts
                .next()
                .and_then(|s| s.trim_start_matches('+').split(',').next()?.parse().ok())
                .unwrap_or(0);
        } else if !header.is_empty() {
            let kind = match line.as_bytes().first() {
                Some(b'+') => "addition",
                Some(b'-') => "deletion",
                Some(b' ') => "context",
                _ => continue,
            };
            if *line_budget == 0 || lines.len() >= 1000 {
                truncated = true;
                break;
            }
            *line_budget -= 1;
            let (old_line, new_line) = (
                if kind == "addition" { None } else { Some(old) },
                if kind == "deletion" { None } else { Some(new) },
            );
            if old_line.is_some() {
                old += 1;
            }
            if new_line.is_some() {
                new += 1;
            }
            if kind == "addition" {
                additions += 1;
            }
            if kind == "deletion" {
                deletions += 1;
            }
            let text = line[1..].chars().take(2048).collect::<String>();
            if text.len() < line[1..].len() {
                truncated = true;
            }
            lines.push(serde_json::json!({"kind":kind,"old_line":old_line,"new_line":new_line,"text":text}));
        }
    }
    if !header.is_empty() {
        hunks.push(serde_json::json!({"header":header,"lines":lines,"truncated":truncated}));
    }
    (hunks, additions, deletions, truncated)
}

async fn local_review_changes(
    source: &SessionView,
    comparison: &crate::coordination::ReviewComparison,
) -> Result<ReviewChanges, UiError> {
    use tokio::io::AsyncReadExt;
    let root = std::path::Path::new(if comparison.accepted_source {
        &source.branch.repo_root
    } else {
        &source.work_dir
    });
    if !comparison.accepted_source {
        crate::landing::ensure_same_repository(
            root,
            std::path::Path::new(&source.branch.repo_root),
        )
        .await
        .map_err(resource_error)?;
    }
    let source_git_ref = format!("refs/heads/{}", comparison.source_ref);
    let source_oid = review_oid(root, &source_git_ref).await?;
    // Explicit canonical refs always name local accepted branches. A publishing
    // base can also be a recorded remote-tracking ref (e.g. origin/main).
    let target_git_ref = format!("refs/heads/{}", comparison.target_ref);
    let (target_git_ref, target_oid) = match review_oid(root, &target_git_ref).await {
        Ok(oid) => (target_git_ref, oid),
        Err(error) if !comparison.accepted_source => return Err(error),
        Err(_) => {
            let reference = format!("refs/remotes/{}", comparison.target_ref);
            let oid = review_oid(root, &reference).await?;
            (reference, oid)
        }
    };
    if !comparison.accepted_source {
        let (branch, _) = review_git(root, &["symbolic-ref", "--quiet", "HEAD"], 4096).await?;
        if String::from_utf8_lossy(&branch).trim() != source_git_ref
            || review_oid(root, "HEAD").await? != source_oid
        {
            return Err(resource_error(
                "The worker checkout changed branch or revision; refresh before comparing.",
            ));
        }
    }
    let mut diff_args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--submodule=short",
        "--line-prefix=",
        "--no-renames",
        "--ignore-submodules=dirty",
        "--name-status",
        "-z",
        target_oid.as_str(),
    ];
    if comparison.accepted_source {
        diff_args.push(&source_oid);
    }
    diff_args.push("--");
    let (inventory, inventory_truncated) = review_git(root, &diff_args, 8 * 1024 * 1024).await?;
    if inventory_truncated {
        return Err(resource_error(
            "The canonical change inventory exceeds preview limits.",
        ));
    }
    let mut paths: Vec<(String, Vec<u8>, bool)> = Vec::new();
    let mut parts = inventory.split(|b| *b == 0).filter(|s| !s.is_empty());
    while let Some(status) = parts.next() {
        let path = parts
            .next()
            .ok_or_else(|| resource_error("Incomplete Git change inventory."))?;
        let status = match status.first() {
            Some(b'A') => "added",
            Some(b'D') => "deleted",
            Some(b'T') => "type_changed",
            _ => "modified",
        };
        paths.push((status.into(), path.to_vec(), false));
    }
    if !comparison.accepted_source {
        let (untracked, truncated) = review_git(
            root,
            &["ls-files", "--others", "--exclude-standard", "-z"],
            8 * 1024 * 1024,
        )
        .await?;
        if truncated {
            return Err(resource_error(
                "Untracked file inventory exceeds preview limits.",
            ));
        }
        paths.extend(
            untracked
                .split(|b| *b == 0)
                .filter(|p| !p.is_empty())
                .map(|p| ("added".into(), p.to_vec(), true)),
        );
    }
    let mut totals = crate::loom::ChangeTotalsView {
        files: paths.len().min(u32::MAX as usize) as u32,
        truncated: paths.len() > 100,
        ..Default::default()
    };
    let mut files = Vec::new();
    let mut line_budget = 8000;
    for (status, raw_path, untracked) in paths.into_iter().take(100) {
        let (mut content, mut patch, mut truncated) = ("text", Vec::new(), false);
        if let Ok(path) = std::str::from_utf8(&raw_path) {
            if untracked {
                let full_path = root.join(path);
                let metadata = tokio::fs::symlink_metadata(&full_path)
                    .await
                    .map_err(|e| resource_error(e.to_string()))?;
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    content = "unsupported";
                } else {
                    let file = tokio::fs::File::open(full_path)
                        .await
                        .map_err(|e| resource_error(e.to_string()))?;
                    let mut bytes = Vec::new();
                    file.take(128 * 1024 + 1)
                        .read_to_end(&mut bytes)
                        .await
                        .map_err(|e| resource_error(e.to_string()))?;
                    if bytes.len() > 128 * 1024 {
                        content = "oversize";
                        truncated = true;
                    } else if bytes.contains(&0) {
                        content = "binary";
                    } else {
                        patch.extend_from_slice(b"@@ -0,0 +1 @@\n");
                        for line in String::from_utf8_lossy(&bytes).lines() {
                            patch.push(b'+');
                            patch.extend_from_slice(line.as_bytes());
                            patch.push(b'\n');
                        }
                    }
                }
            } else {
                let mut args = vec![
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                    "--submodule=short",
                    "--line-prefix=",
                    "--no-renames",
                    "--ignore-submodules=dirty",
                    "--unified=3",
                    target_oid.as_str(),
                ];
                if comparison.accepted_source {
                    args.push(&source_oid);
                }
                args.extend(["--", path]);
                (patch, truncated) = review_git(root, &args, 128 * 1024).await?;
                if patch.windows(13).any(|s| s == b"Binary files ") {
                    content = "binary";
                }
            }
        } else {
            content = "unsupported";
        }
        let (hunks, additions, deletions, limited) = review_hunks(&patch, &mut line_budget);
        truncated |= limited;
        totals.additions += additions;
        totals.deletions += deletions;
        totals.truncated |= truncated;
        files.push(serde_json::json!({"status":status,"path":review_path(&raw_path),"old_path":null,
            "sources":[if untracked { "untracked" } else if comparison.accepted_source { "committed" } else { "checkout" }],
            "additions":if content == "text" { Some(additions) } else { None },
            "deletions":if content == "text" { Some(deletions) } else { None },
            "content":content,"hunks":hunks,"truncated":truncated}));
    }
    // Both SHAs must still name the refs used for this snapshot; advanced
    // canonical targets require another comparison, never recorded-base reuse.
    if review_oid(root, &source_git_ref).await? != source_oid
        || review_oid(root, &target_git_ref).await? != target_oid
    {
        return Err(resource_error(
            "Source or canonical comparison advanced during preview; refresh.",
        ));
    }
    if !comparison.accepted_source {
        let (branch, _) = review_git(root, &["symbolic-ref", "--quiet", "HEAD"], 4096).await?;
        if String::from_utf8_lossy(&branch).trim() != source_git_ref
            || review_oid(root, "HEAD").await? != source_oid
        {
            return Err(resource_error(
                "The worker checkout changed during preview; refresh.",
            ));
        }
    }
    Ok(ReviewChanges {
        source_ref: comparison.source_ref.clone(),
        accepted_source: comparison.accepted_source,
        comparison: "target-tree",
        changes: crate::loom::ChangeSetView {
            version: None,
            base: serde_json::json!({"state":"available","reference":comparison.target_ref,"oid":target_oid}),
            head_oid: Some(source_oid),
            truncated: totals.truncated,
            totals,
            files,
        },
    })
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
        // Mentions resolve against the CURRENT effective view — project
        // bindings included (a project's shared reference keeps supplying
        // existing topics) — and against the topic's current accepted
        // branch, not the reference recorded when it was attached.
        let context = load_topic_context(client, &mention.topic_id).await?;
        let resource = context
            .find_effective(&mention.resource_id)
            .map(|row| row.resource)
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

// --- Project bindings --------------------------------------------------------
//
// A Project's resource bindings live in the repo-shared `arachne-projects`
// artifact of the repository they target, keyed by the project's layout
// group id (design.md "Project defaults and resource inheritance"). The
// repo-shared scope is keyed on the branch's canonical repo root, so any
// session/branch id of the repo is a valid access context — a topic's own
// branch id is always available and always right. One artifact per repo →
// one revision, so concurrent edits serialize through `base_rev` exactly
// like the topic manifest and the todo list.

async fn load_projects_store(
    client: &LoomClient,
    branch_id: &str,
) -> Result<ProjectsStore, UiError> {
    let artifact = match client.repo_artifact(branch_id, PROJECTS_NAME).await {
        Ok(value) => value,
        Err(LoomError::Api { status: 404, .. }) => return Ok(ProjectsStore::default()),
        Err(error) => return Err(error.into()),
    };
    // A repo:true read always resolves the shared scope; a reply carrying a
    // branch_id would mean something else claimed the name — refuse rather
    // than read the wrong store.
    if artifact
        .get("meta")
        .and_then(|v| v.get("branch_id"))
        .and_then(|v| v.as_str())
        .is_some()
    {
        return Err(resource_error(
            "projects store name is occupied by a branch-scoped artifact",
        ));
    }
    let content = artifact
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| resource_error("projects store has no content"))?;
    let mut store: ProjectsStore = serde_json::from_str(content)
        .map_err(|e| resource_error(format!("invalid projects store: {e}")))?;
    store.revision = artifact
        .get("meta")
        .and_then(|v| v.get("rev"))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| resource_error("projects store has no revision"))?;
    Ok(store)
}

async fn save_projects_store(
    client: &LoomClient,
    branch_id: &str,
    store: &ProjectsStore,
    expected_revision: i64,
) -> Result<ProjectsStore, UiError> {
    let content = serde_json::to_string_pretty(store)
        .map_err(|e| resource_error(format!("serializing project bindings: {e}")))?;
    let artifact = client
        .write_repo_artifact(
            branch_id,
            PROJECTS_NAME,
            &content,
            expected_revision,
            "Arachne project bindings",
        )
        .await?;
    if artifact
        .get("meta")
        .and_then(|v| v.get("branch_id"))
        .and_then(|v| v.as_str())
        .is_some()
    {
        return Err(resource_error(
            "project bindings were not saved in the repository-shared scope",
        ));
    }
    let mut saved = store.clone();
    saved.revision = artifact
        .get("meta")
        .and_then(|v| v.get("rev"))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| resource_error("saved project bindings have no revision"))?;
    Ok(saved)
}

/// A topic's home project: its placement group when that group is a
/// non-system group (the same model as src/projects.ts `topicProjectId` —
/// placement is a live join, so a deleted group already reads as None).
fn topic_project_of(topic: &SessionView) -> Option<(String, String)> {
    let placement = topic.placement.as_ref()?;
    let id = placement.group_id.as_deref()?;
    if placement.group_system_key.is_some() {
        return None;
    }
    Some((
        id.to_owned(),
        placement
            .group_name
            .clone()
            .unwrap_or_else(|| "Project".into()),
    ))
}

/// Everything the effective view of one topic's resources needs: the
/// session, its manifest, and its project's bindings for the topic's repo.
struct TopicContext {
    topic: SessionView,
    manifest: TopicResourcesView,
    /// The topic's home project, if any, with the store's revision for
    /// revision-checked project-binding edits.
    project: Option<ProjectInfo>,
    /// The project's bindings (empty when unfiled).
    bindings: Vec<ProjectBinding>,
}

impl TopicContext {
    /// The merged rows (design.md: Project → Topic overrides → hides), with
    /// hidden project bindings flagged for the restore affordance.
    fn merged(&self) -> Vec<EffectiveResource> {
        merge_effective(&self.bindings, &self.manifest, &self.topic.branch.branch)
    }

    /// The effective view the panel and the mutation replies render.
    fn effective_view(&self) -> TopicEffectiveView {
        let mut resources = Vec::new();
        let mut hidden_resources = Vec::new();
        for row in self.merged() {
            if row.hidden {
                hidden_resources.push(row);
            } else {
                resources.push(row);
            }
        }
        TopicEffectiveView {
            resources,
            hidden_resources,
            revision: self.manifest.revision,
            project: self.project.clone(),
        }
    }

    /// Resolve one effective (non-hidden) row by resource id.
    fn find_effective(&self, resource_id: &str) -> Option<EffectiveResource> {
        self.merged()
            .into_iter()
            .find(|row| !row.hidden && row.resource.id == resource_id)
    }
}

async fn load_topic_context(client: &LoomClient, topic_id: &str) -> Result<TopicContext, UiError> {
    let topic = client.get_session(topic_id).await?;
    let manifest = load_topic_resources(&client, &topic.branch.id).await?;
    let (project, bindings) = match topic_project_of(&topic) {
        Some((id, name)) => {
            let store = load_projects_store(&client, &topic.branch.id).await?;
            let bindings = store.projects.get(&id).cloned().unwrap_or_default();
            (
                Some(ProjectInfo {
                    id,
                    name,
                    revision: store.revision,
                }),
                bindings,
            )
        }
        None => (None, Vec::new()),
    };
    Ok(TopicContext {
        topic,
        manifest,
        project,
        bindings,
    })
}

/// The Resources panel's full view: the durable manifest plus the topic's
/// live slice — GitHub issues its subtree works, and the PRs of every session
/// in the subtree. Loom has no topic↔issue link, so the issues are scoped
/// here: an issue belongs to the topic when its repo matches and a branch of
/// the topic's subtree claims (or sourced) it. PRs ride the fleet snapshot
/// (`BranchSummaryView.github`, loom's poll loop), so they are always live.
#[derive(Debug, Clone, Serialize)]
pub struct TopicResourcesPanel {
    /// The merged effective view (project bindings inherited, topic
    /// overrides winning, hides flagged) — design.md "Project defaults and
    /// resource inheritance".
    #[serde(flatten)]
    pub effective: TopicEffectiveView,
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
            [
                issue.claimed_branch.as_deref(),
                issue.source_branch.as_deref(),
            ]
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
    let context = load_topic_context(&client, &topic_id).await?;
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
                Ok(board) => topic_issues(board, &context.topic, &subtree),
                Err(_) => Vec::new(),
            };
            (issues, topic_prs(&subtree))
        }
        Err(_) => (Vec::new(), Vec::new()),
    };
    Ok(TopicResourcesPanel {
        effective: context.effective_view(),
        issues,
        prs,
    })
}

/// Attach a resource to a topic (an explicit Topic addition — design.md's
/// effective view). Attaching an object the topic already inherits from its
/// project records the topic's own binding: a real override, so the
/// reference stops following the project's binding. Attaching an object the
/// topic inherited *hidden* un-hides it (the user changed their mind).
// --- Project bindings (store access) ---------------------------------------

/// Resolve a repository reference (slug or path, as typed in the launch
/// sheet) to a branch id of that repository — the access context loom's
/// repo-shared artifact API needs. Any branch of the repo works: the
/// repo-shared scope is keyed on the branch's canonical repo root. Repos
/// with no loom branch yet have no context (and no store to read — the
/// first branch of a repo brings the first opportunity to create one).
async fn repo_branch_context(client: &LoomClient, repo: &str) -> Result<Option<String>, UiError> {
    let repo = repo.trim();
    if repo.is_empty() {
        return Ok(None);
    }
    // Candidate roots: the managed checkout path for a slug, and the raw
    // value itself when it looks like a path. Branch rows record the
    // canonical root loom provisioned, so an exact match is the normal
    // case; the raw value covers paths typed directly into the sheet.
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(repos) = client.list_repos().await {
        if let Some(managed) = repos.iter().find(|r| r.slug == repo) {
            candidates.push(managed.path.clone());
        }
    }
    if repo.contains('/') {
        candidates.push(repo.to_owned());
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    let branches = client.list_branches().await?;
    for candidate in &candidates {
        if let Some(id) = branches
            .iter()
            .filter(|b| b.get("repo_root").and_then(|v| v.as_str()) == Some(candidate.as_str()))
            .find_map(|b| b.get("id").and_then(|v| v.as_str()))
        {
            return Ok(Some(id.to_owned()));
        }
    }
    Ok(None)
}

/// One managed repository, for the repo pickers.
#[derive(Debug, Clone, Serialize)]
pub struct ManagedRepoView {
    pub slug: String,
    pub path: String,
}

/// The project-bindings view the sheet and the project home render.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectBindingsView {
    pub bindings: Vec<ProjectBinding>,
    /// The store's revision for revision-checked edits.
    pub revision: i64,
}

/// Load one project's bindings (design.md "Project defaults and resource
/// inheritance") — the creation form's checklist and the project home's
/// resource list. `repo` is the sheet's repository reference (slug or path).
#[tauri::command]
pub async fn project_bindings(
    state: State<'_, LoomState>,
    project_id: String,
    repo: String,
) -> Result<ProjectBindingsView, UiError> {
    let client = state_client(&state).await?;
    let context = repo_branch_context(&client, &repo)
        .await?
        .ok_or_else(|| resource_error("no loom branch exists for this repository yet"))?;
    let store = load_projects_store(&client, &context).await?;
    Ok(ProjectBindingsView {
        bindings: store.projects.get(&project_id).cloned().unwrap_or_default(),
        revision: store.revision,
    })
}

/// The managed repositories, for pickers that bind a repository by slug
/// (project bindings, topic attachments). Slug + path so the frontend can
/// offer the human-readable name while commands key by either.
#[tauri::command]
pub async fn managed_repos(state: State<'_, LoomState>) -> Result<Vec<ManagedRepoView>, UiError> {
    let client = state_client(&state).await?;
    let mut repos = client.list_repos().await?;
    repos.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(repos
        .into_iter()
        .map(|r| ManagedRepoView {
            slug: r.slug,
            path: r.path,
        })
        .collect())
}

/// Resolve `repo` to the canonical repo root by finding a branch of it —
/// the same root the topic manifests and the project store are keyed on,
/// so a binding's `repository` field matches what topics see.
async fn repo_root_of(client: &LoomClient, repo: &str) -> Result<Option<String>, UiError> {
    let repo = repo.trim();
    if repo.is_empty() {
        return Ok(None);
    }
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(repos) = client.list_repos().await {
        if let Some(managed) = repos.iter().find(|r| r.slug == repo) {
            candidates.push(managed.path.clone());
        }
    }
    if repo.contains('/') {
        candidates.push(repo.to_owned());
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    let branches = client.list_branches().await?;
    for candidate in &candidates {
        if let Some(root) = branches
            .iter()
            .filter(|b| b.get("repo_root").and_then(|v| v.as_str()) == Some(candidate.as_str()))
            .find_map(|b| b.get("repo_root").and_then(|v| v.as_str()))
        {
            return Ok(Some(root.to_owned()));
        }
    }
    Ok(None)
}

/// Add a resource binding to a project (design.md: projects bind
/// repositories, design documents, and other resources; new topics inherit
/// them). The binding's repository is stamped with the repo's canonical root
/// — the sheet only knows a slug or path, never the root topics are keyed on.
#[tauri::command]
pub async fn add_project_binding(
    state: State<'_, LoomState>,
    project_id: String,
    repo: String,
    resource: ResourceDraft,
    expected_revision: i64,
) -> Result<ProjectBindingsView, UiError> {
    let client = state_client(&state).await?;
    let context = repo_branch_context(&client, &repo)
        .await?
        .ok_or_else(|| resource_error("no loom branch exists for this repository yet"))?;
    let repo_root = repo_root_of(&client, &repo)
        .await?
        .ok_or_else(|| resource_error("no loom branch exists for this repository yet"))?;
    let mut draft = resource;
    if draft.kind == ResourceKind::Repository {
        // A repository binding points at its own repo: the picker sends a
        // slug (or path), resolved to the canonical root the same way the
        // host repo is, so identity matches what a topic attach produces.
        draft.repository = repo_root_of(&client, &draft.repository)
            .await?
            .ok_or_else(|| resource_error("no loom branch exists for that repository yet"))?;
    } else {
        draft.repository = repo_root.clone();
    }
    let binding =
        ProjectBinding::validated_for_project(draft, &repo_root).map_err(resource_error)?;
    let mut store = load_projects_store(&client, &context).await?;
    if store.revision != expected_revision {
        return Err(resource_error(
            "project bindings changed; reload before editing",
        ));
    }
    let bindings = store.projects.entry(project_id.clone()).or_default();
    let binding = ProjectBinding {
        created_at: chrono_iso_now(),
        ..binding
    };
    if let Some(existing) = bindings.iter_mut().find(|b| b.id == binding.id) {
        *existing = binding;
    } else {
        bindings.push(binding);
    }
    let saved = save_projects_store(&client, &context, &store, expected_revision).await?;
    Ok(ProjectBindingsView {
        bindings: saved.projects.get(&project_id).cloned().unwrap_or_default(),
        revision: saved.revision,
    })
}

/// Remove a binding from a project. Existing topics keep their own additions
/// and hides; the removed binding simply stops being inherited (live merge,
/// never a copy-at-creation).
#[tauri::command]
pub async fn remove_project_binding(
    state: State<'_, LoomState>,
    project_id: String,
    repo: String,
    binding_id: String,
    expected_revision: i64,
) -> Result<ProjectBindingsView, UiError> {
    let client = state_client(&state).await?;
    let context = repo_branch_context(&client, &repo)
        .await?
        .ok_or_else(|| resource_error("no loom branch exists for this repository yet"))?;
    let mut store = load_projects_store(&client, &context).await?;
    if store.revision != expected_revision {
        return Err(resource_error(
            "project bindings changed; reload before editing",
        ));
    }
    let bindings = store
        .projects
        .get_mut(&project_id)
        .ok_or_else(|| resource_error("project has no bindings"))?;
    let before = bindings.len();
    bindings.retain(|b| b.id != binding_id);
    if bindings.len() == before {
        return Err(resource_error("binding not found"));
    }
    let saved = save_projects_store(&client, &context, &store, expected_revision).await?;
    Ok(ProjectBindingsView {
        bindings: saved.projects.get(&project_id).cloned().unwrap_or_default(),
        revision: saved.revision,
    })
}

/// Attach a resource to a topic (an explicit Topic addition — design.md's
/// effective view). Attaching an object the topic already inherits from its
/// project records the topic's own binding: a real override, so the
/// reference stops following the project's binding. Attaching an object the
/// topic inherited *hidden* un-hides it (the user changed their mind).
#[tauri::command]
pub async fn attach_topic_resource(
    state: State<'_, LoomState>,
    topic_id: String,
    resource: ResourceDraft,
    expected_revision: i64,
) -> Result<TopicEffectiveView, UiError> {
    let client = state_client(&state).await?;
    let context = load_topic_context(&client, &topic_id).await?;
    let topic = &context.topic;
    if context.manifest.revision != expected_revision {
        return Err(resource_error("resources changed; reload before editing"));
    }
    let mut draft = resource;
    if draft.kind == ResourceKind::Repository {
        // A repository attachment names its own repo (a slug or path);
        // canonicalize to the repo root a project binding would record so
        // override/hide matching works across scopes.
        draft.repository = repo_root_of(&client, &draft.repository)
            .await?
            .ok_or_else(|| resource_error("no loom branch exists for that repository yet"))?;
    }
    let resource = draft
        .validated(&topic.branch.repo_root, &topic.branch.branch)
        .map_err(resource_error)?;
    let mut manifest = context.manifest.clone();
    let key = resource.binding_key();
    if let Some(existing) = manifest.resources.iter_mut().find(|r| r.id == resource.id) {
        *existing = resource;
    } else {
        manifest.resources.push(resource);
    }
    // An explicit attach beats a recorded hide of the same backing object.
    manifest.hidden.retain(|hidden| hidden != &key);
    let saved =
        save_topic_resources(&client, &topic.branch.id, &manifest, expected_revision).await?;
    Ok(TopicContext {
        topic: topic.clone(),
        manifest: saved,
        project: context.project,
        bindings: context.bindings,
    }
    .effective_view())
}

/// Detach a topic's own binding. Inherited (project) bindings cannot be
/// detached — hiding them is the topic-level opt-out (design.md: hide, not
/// delete from the project).
#[tauri::command]
pub async fn detach_topic_resource(
    state: State<'_, LoomState>,
    topic_id: String,
    resource_id: String,
    expected_revision: i64,
) -> Result<TopicEffectiveView, UiError> {
    let client = state_client(&state).await?;
    let context = load_topic_context(&client, &topic_id).await?;
    if context.manifest.revision != expected_revision {
        return Err(resource_error("resources changed; reload before editing"));
    }
    let before = context.manifest.resources.len();
    let mut manifest = context.manifest.clone();
    manifest.resources.retain(|r| r.id != resource_id);
    if manifest.resources.len() == before {
        // Not a topic row: either an inherited binding (hide it instead) or
        // nothing — both report the same error to the panel.
        return Err(resource_error(
            "resource not found on this topic; inherited bindings are hidden, not detached",
        ));
    }
    let saved = save_topic_resources(
        &client,
        &context.topic.branch.id,
        &manifest,
        expected_revision,
    )
    .await?;
    Ok(TopicContext {
        topic: context.topic,
        manifest: saved,
        project: context.project,
        bindings: context.bindings,
    }
    .effective_view())
}

/// Hide an inherited project binding for this topic (design.md: allow a
/// Topic to hide an inherited resource without deleting it from the
/// Project). Hiding records the binding's key in the topic manifest's
/// `hidden` list; a later attach of the same object overrides cleanly.
#[tauri::command]
pub async fn hide_topic_resource(
    state: State<'_, LoomState>,
    topic_id: String,
    resource_id: String,
    expected_revision: i64,
) -> Result<TopicEffectiveView, UiError> {
    let client = state_client(&state).await?;
    let context = load_topic_context(&client, &topic_id).await?;
    if context.manifest.revision != expected_revision {
        return Err(resource_error("resources changed; reload before editing"));
    }
    let row = context
        .find_effective(&resource_id)
        .ok_or_else(|| resource_error("resource not found"))?;
    if row.origin != ResourceOrigin::Project {
        return Err(resource_error(
            "only inherited resources can be hidden; detach your own bindings instead",
        ));
    }
    let key = binding_key(&row.resource.data);
    let mut manifest = context.manifest.clone();
    if !manifest.hidden.iter().any(|hidden| hidden == &key) {
        manifest.hidden.push(key);
    }
    let saved = save_topic_resources(
        &client,
        &context.topic.branch.id,
        &manifest,
        expected_revision,
    )
    .await?;
    Ok(TopicContext {
        topic: context.topic,
        manifest: saved,
        project: context.project,
        bindings: context.bindings,
    }
    .effective_view())
}

/// Restore a hidden inherited binding.
#[tauri::command]
pub async fn unhide_topic_resource(
    state: State<'_, LoomState>,
    topic_id: String,
    resource_id: String,
    expected_revision: i64,
) -> Result<TopicEffectiveView, UiError> {
    let client = state_client(&state).await?;
    let context = load_topic_context(&client, &topic_id).await?;
    if context.manifest.revision != expected_revision {
        return Err(resource_error("resources changed; reload before editing"));
    }
    // Hidden rows are kept in the view exactly so this can resolve them.
    let row = context
        .merged()
        .into_iter()
        .find(|row| row.resource.id == resource_id)
        .ok_or_else(|| resource_error("resource not found"))?;
    if !row.hidden {
        return Ok(context.effective_view());
    }
    let key = binding_key(&row.resource.data);
    let mut manifest = context.manifest.clone();
    let before = manifest.hidden.len();
    manifest.hidden.retain(|hidden| hidden != &key);
    if manifest.hidden.len() == before {
        return Ok(context.effective_view());
    }
    let saved = save_topic_resources(
        &client,
        &context.topic.branch.id,
        &manifest,
        expected_revision,
    )
    .await?;
    Ok(TopicContext {
        topic: context.topic,
        manifest: saved,
        project: context.project,
        bindings: context.bindings,
    }
    .effective_view())
}

async fn resolve_topic_resource(
    client: &LoomClient,
    topic_id: &str,
    resource_id: &str,
) -> Result<(SessionView, TopicResource), UiError> {
    // Reads and opens resolve against the effective view — an inherited
    // project binding's file reads from the topic's own branch (the merge
    // retargeted it), and a hidden binding never resolves.
    let context = load_topic_context(client, topic_id).await?;
    let resource = context
        .find_effective(resource_id)
        .map(|row| row.resource)
        .ok_or_else(|| resource_error("resource not found"))?;
    Ok((context.topic, resource))
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

/// Open user Todos across visible Tracks. Discover existing manifests once
/// per repository, then read only branches that actually contain a list.
#[derive(Serialize)]
pub struct UserTodoRow {
    #[serde(flatten)]
    todo: TodoItem,
    revision: i64,
}
#[derive(Serialize)]
pub struct UserTodosView {
    todos: Vec<UserTodoRow>,
    warnings: Vec<String>,
}
fn open_user_todos(list: TodoListView, topic_id: &str) -> Vec<UserTodoRow> {
    list.todos
        .into_iter()
        .filter(|todo| !todo.done && todo.topic_id == topic_id)
        .map(|todo| UserTodoRow {
            todo,
            revision: list.revision,
        })
        .collect()
}

#[cfg(test)]
mod user_todos_tests {
    use super::*;
    #[test]
    fn only_open_owner_todos_carry_the_source_revision() {
        let make = |id: &str, topic: &str, done: bool| TodoItem {
            id: id.into(),
            text: id.into(),
            topic_id: topic.into(),
            done,
            created_at: String::new(),
        };
        let rows = open_user_todos(
            TodoListView {
                revision: 9,
                todos: vec![
                    make("open", "track", false),
                    make("done", "track", true),
                    make("foreign", "other", false),
                ],
            },
            "track",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].todo.id, "open");
        assert_eq!(rows[0].revision, 9);
    }
}

#[tauri::command]
pub async fn user_todos(
    state: State<'_, LoomState>,
    topic_ids: Vec<String>,
) -> Result<UserTodosView, UiError> {
    use futures_util::{stream, StreamExt};
    use std::collections::{HashMap, HashSet};
    let client = state_client(&state).await?;
    let wanted: HashSet<String> = topic_ids.into_iter().collect();
    let sessions = client.list_sessions().await?;
    let topics: Vec<_> = sessions
        .into_iter()
        .filter(|s| wanted.contains(&s.id) && s.status != "archived")
        .collect();
    let mut repositories = HashMap::new();
    let by_branch: HashMap<_, _> = topics
        .iter()
        .map(|s| (s.branch.id.clone(), s.id.clone()))
        .collect();
    for topic in &topics {
        repositories
            .entry(topic.branch.repo_root.clone())
            .or_insert(topic.branch.id.clone());
    }
    let mut warnings = Vec::new();
    let mut existing = HashSet::new();
    for (repo, branch) in repositories {
        match client
            .op::<Vec<serde_json::Value>>(
                "/api/artifacts/list",
                &serde_json::json!({ "branch": branch, "repo": true }),
            )
            .await
        {
            Ok(artifacts) => {
                for artifact in artifacts {
                    if artifact["name"] == TODOS_NAME {
                        if let Some(branch) = artifact["branch_id"]
                            .as_str()
                            .filter(|id| by_branch.contains_key(*id))
                        {
                            existing.insert(branch.to_owned());
                        }
                    }
                }
            }
            Err(error) => warnings.push(format!("Todos unavailable for {repo}: {error}")),
        }
    }
    let pages = stream::iter(existing.into_iter().map(|branch| {
        let client = client.clone();
        async move {
            let result = load_todo_list(&client, &branch).await;
            (branch, result)
        }
    }))
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    let mut todos = Vec::new();
    for (branch, result) in pages {
        match result {
            Ok(list) => {
                if let Some(topic_id) = by_branch.get(&branch) {
                    todos.extend(open_user_todos(list, topic_id));
                }
            }
            Err(error) => warnings.push(format!("Todos unavailable: {}", error.message)),
        }
    }
    todos.sort_by(|a, b| {
        a.todo
            .created_at
            .cmp(&b.todo.created_at)
            .then_with(|| a.todo.id.cmp(&b.todo.id))
    });
    Ok(UserTodosView { todos, warnings })
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

    #[test]
    fn fleet_subscriptions_cover_old_workers_and_do_not_churn_on_activity() {
        let mut sessions: Vec<_> = (0..150)
            .map(|i| session(&format!("s{i:03}"), "branch", "/repo", None, false))
            .collect();
        let topics = super::fleet_topics(&sessions);
        assert_eq!(topics.len(), 151);
        assert!(topics.contains(&"session:s000".to_owned()));
        assert!(topics.contains(&"session:s149".to_owned()));
        assert!(topics.chunks(64).all(|chunk| chunk.len() <= 64));
        sessions[149].last_activity_at = "2099-01-01".into();
        sessions.reverse();
        assert_eq!(super::fleet_topics(&sessions), topics);
        sessions[0].status = "archived".into();
        assert_eq!(super::fleet_topics(&sessions).len(), 150);
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
    fn cross_repo_track_never_accepts_into_unmarked_parent() {
        let fleet = vec![
            session("topic", "other-topic", "/other", None, true),
            session("parent", "parent-branch", "/repo", Some("topic"), false),
            session("child", "child-branch", "/repo", Some("parent"), false),
        ];
        assert!(resolve_integration_target(&fleet, "child").is_none());
        assert!(resolve_integration_target(&fleet, "parent").is_none());
    }
}

#[cfg(test)]
mod resource_panel_tests {
    use super::{topic_issues, topic_prs, topic_subtree};
    use crate::loom::{GithubStatusView, IssueView, SessionSummaryView};

    fn session(
        id: &str,
        parent: Option<&str>,
        branch: &str,
        pr: Option<i64>,
    ) -> SessionSummaryView {
        let github = pr.map(|n| GithubStatusView {
            pr_number: n,
            pr_url: format!("https://github.com/acme/app/pull/{n}"),
            pr_state: "OPEN".into(),
            pr_title: format!("PR {n}"),
            is_draft: false,
            review_decision: None,
            checks: None,
            mergeable: None,
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
        let mut grandchild_value =
            serde_json::to_value(session("grandchild", None, "weaver/grandchild", None)).unwrap();
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
            issue(1, "/repo", Some("weaver/worker"), None), // claimed by subtree
            issue(2, "/repo", None, Some("weaver/topic")),  // sourced by subtree
            issue(3, "/repo", Some("weaver/other"), None),  // another topic's branch
            issue(4, "/repo", None, None),                  // unclaimed backlog
            issue(5, "/other", Some("weaver/topic"), None), // wrong repo
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
        assert_eq!(
            prs.iter().map(|p| p.github.pr_number).collect::<Vec<_>>(),
            [10, 11]
        );
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

#[derive(Serialize)]
pub struct CheckoutFile {
    content: String,
    checkout: String,
}

async fn local_editor_checkout(state: &LoomState, session_id: &str) -> Result<String, UiError> {
    let client = state_client(state).await?;
    if !host_is_loopback(client.server_host().unwrap_or("")) {
        return Err(resource_error(
            "In-app editing currently requires a local Loom server",
        ));
    }
    let view = client.get_session(session_id).await?;
    if view.status == "archived" || view.work_dir.is_empty() {
        return Err(resource_error(
            "Recover this thread's checkout before editing",
        ));
    }
    Ok(view.work_dir)
}

#[tauri::command]
pub async fn read_checkout_file(
    state: State<'_, LoomState>,
    session_id: String,
    path: String,
) -> Result<CheckoutFile, UiError> {
    let checkout = local_editor_checkout(&state, &session_id).await?;
    tokio::task::spawn_blocking(move || {
        let content =
            crate::editor::read(std::path::Path::new(&checkout), &path).map_err(resource_error)?;
        Ok(CheckoutFile { content, checkout })
    })
    .await
    .map_err(|e| resource_error(e.to_string()))?
}

#[tauri::command]
pub async fn save_checkout_file(
    state: State<'_, LoomState>,
    session_id: String,
    path: String,
    checkout: String,
    original: String,
    content: String,
) -> Result<(), UiError> {
    let current = local_editor_checkout(&state, &session_id).await?;
    if current != checkout {
        return Err(resource_error(
            "The thread's checkout moved. Reopen the file before saving.",
        ));
    }
    tokio::task::spawn_blocking(move || {
        crate::editor::save(std::path::Path::new(&current), &path, &original, &content)
            .map_err(resource_error)
    })
    .await
    .map_err(|e| resource_error(e.to_string()))?
}

#[cfg(test)]
mod landing_dispatch_tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    static COUNTER: AtomicU32 = AtomicU32::new(0);
    struct Repo(PathBuf);
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
    fn fixture() -> (Repo, SessionView, SessionSummaryView) {
        let dir = std::env::temp_dir().join(format!(
            "arachne-land-dispatch-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "dev"]);
        git(&dir, &["config", "user.name", "Test"]);
        git(&dir, &["config", "user.email", "test@example.com"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("base.txt"), "base\n").unwrap();
        git(&dir, &["add", "base.txt"]);
        git(&dir, &["commit", "-qm", "base"]);
        git(&dir, &["worktree", "add", "-qb", "weaver/topic", "source"]);
        let source = dir.join("source");
        std::fs::write(source.join("topic.txt"), "topic\n").unwrap();
        git(&source, &["add", "topic.txt"]);
        git(&source, &["commit", "-qm", "topic"]);
        // Exclude the fixture's nested linked checkout from target status.
        std::fs::write(dir.join(".git/info/exclude"), "source/\n").unwrap();
        let value = serde_json::json!({
            "id":"topic-session", "status":"running", "profile":"default", "class":"interactive",
            "origin":"user", "agent_kind":"codex", "model":"", "effort":"", "protocol":"acp",
            "work_dir":source, "term_session":"runtime", "turn_count":0,
            "created_by":null, "created_at":"now", "last_activity_at":"now",
            "branch":{"id":"topic-branch", "branch":"weaver/topic", "name":"Topic", "title":"Topic",
                      "repo_root":dir, "tags":[]}
        });
        (
            Repo(dir),
            serde_json::from_value(value.clone()).unwrap(),
            serde_json::from_value(value).unwrap(),
        )
    }
    async fn mock(
        view: &SessionView,
        paths: Vec<&'static str>,
    ) -> (
        Arc<LoomClient>,
        tokio::task::JoinHandle<Vec<serde_json::Value>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let session = serde_json::to_value(view).unwrap();
        let server = tokio::spawn(async move {
            let mut bodies = Vec::new();
            for path in paths {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let (header_end, length) = loop {
                    let mut buf = [0; 4096];
                    let count = socket.read(&mut buf).await.unwrap();
                    assert_ne!(count, 0);
                    request.extend_from_slice(&buf[..count]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        assert!(headers.starts_with(&format!("POST {path} ")), "{headers}");
                        let len: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|n| n.trim().parse().unwrap())
                            })
                            .unwrap();
                        break (end + 4, len);
                    }
                };
                while request.len() < header_end + length {
                    let mut buf = [0; 4096];
                    let count = socket.read(&mut buf).await.unwrap();
                    assert_ne!(count, 0);
                    request.extend_from_slice(&buf[..count]);
                }
                bodies.push(
                    serde_json::from_slice(&request[header_end..header_end + length]).unwrap(),
                );
                let body = if path == "/api/sessions/get" {
                    session.clone()
                } else {
                    serde_json::json!({"queued":true,"turn":null})
                }
                .to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
            bodies
        });
        (
            Arc::new(LoomClient::new(&format!("http://{addr}/"), None).unwrap()),
            server,
        )
    }
    async fn queued(view: SessionView, summary: &SessionSummaryView) -> String {
        let path = if view.protocol == "terminal" {
            "/api/sessions/send"
        } else {
            "/api/sessions/prompt/create"
        };
        let (client, server) = mock(&view, vec!["/api/sessions/get", path]).await;
        let result = land_topic_locally(client, view, summary).await.unwrap();
        assert!(!result.local);
        assert!(result.landing.is_none());
        let bodies = server.await.unwrap();
        assert_eq!(bodies[1]["session"], "topic-session");
        if path.ends_with("send") {
            assert_eq!(bodies[1]["submit"], true);
        } else {
            assert_eq!(bodies[1]["send_now"], false);
        }
        bodies[1]["text"].as_str().unwrap().to_owned()
    }
    fn request(prompt: &str) -> serde_json::Value {
        serde_json::from_str(
            prompt
                .split("```json\n")
                .nth(1)
                .unwrap()
                .split("\n```")
                .next()
                .unwrap(),
        )
        .unwrap()
    }
    #[tokio::test]
    async fn dirty_editor_edits_queue_complete_chosen_local_landing_without_touching_foreign_work()
    {
        let (repo, view, summary) = fixture();
        let source = Path::new(&view.work_dir);
        std::fs::write(source.join("topic.txt"), "editor save\n").unwrap();
        std::fs::write(source.join("unrelated.txt"), "foreign source\n").unwrap();
        git(source, &["add", "unrelated.txt"]);
        std::fs::write(repo.0.join("base.txt"), "staged target\n").unwrap();
        git(&repo.0, &["add", "base.txt"]);
        std::fs::write(repo.0.join("base.txt"), "unstaged target\n").unwrap();
        std::fs::write(repo.0.join("untracked.txt"), "foreign target\n").unwrap();
        let before_source = git(source, &["status", "--porcelain"]);
        let before_target = git(&repo.0, &["status", "--porcelain"]);
        let before_head = git(&repo.0, &["rev-parse", "HEAD"]);
        let prompt = queued(view.clone(), &summary).await;
        let payload = request(&prompt);
        assert_eq!(payload["strategy"], "land-locally");
        assert_eq!(payload["prepare_then_land"], true);
        assert_eq!(payload["source_thread"], view.id);
        assert_eq!(payload["source_work"], view.work_dir);
        assert_eq!(payload["repository"], view.branch.repo_root);
        assert_eq!(payload["target_resource"], "dev");
        assert_eq!(payload["target_origin"], "primary-checkout");
        assert_eq!(payload["local_target"]["checkout"], view.branch.repo_root);
        assert_eq!(payload["local_target"]["revision"], before_head.trim());
        assert_eq!(payload["source_state"]["status"], before_source);
        assert!(prompt.contains("do not ask for another click"));
        assert!(prompt.contains("never blanket git add/commit"));
        assert!(prompt.contains("Then perform the chosen landing"));
        assert_eq!(git(source, &["status", "--porcelain"]), before_source);
        assert_eq!(git(&repo.0, &["status", "--porcelain"]), before_target);
        assert_eq!(git(&repo.0, &["show", ":base.txt"]), "staged target\n");
        assert_eq!(
            std::fs::read_to_string(repo.0.join("base.txt")).unwrap(),
            "unstaged target\n"
        );
        assert_eq!(git(&repo.0, &["rev-parse", "HEAD"]), before_head);
        assert_eq!(
            std::fs::read_to_string(source.join("topic.txt")).unwrap(),
            "editor save\n"
        );
        assert_eq!(git(source, &["show", ":unrelated.txt"]), "foreign source\n");
        assert_eq!(
            std::fs::read_to_string(repo.0.join("untracked.txt")).unwrap(),
            "foreign target\n"
        );
    }
    #[tokio::test]
    async fn clean_source_lands_immediately_and_tags_only_the_completed_result() {
        let (repo, view, summary) = fixture();
        std::fs::write(repo.0.join("base.txt"), "unrelated unstaged\n").unwrap();
        std::fs::write(repo.0.join("untracked.txt"), "keep\n").unwrap();
        for expected_landed in [true, false] {
            let (client, server) = mock(&view, vec!["/api/sessions/tags/set"]).await;
            let result = land_topic_locally(client, view.clone(), &summary)
                .await
                .unwrap();
            assert!(result.local);
            let landing = result.landing.unwrap();
            assert_eq!(landing.landed, expected_landed);
            assert_eq!(landing.target_branch, "dev");
            assert_eq!(landing.commit.is_some(), expected_landed);
            let bodies = server.await.unwrap();
            assert_eq!(bodies[0]["key"], "integration_result");
        }
        assert_eq!(
            std::fs::read_to_string(repo.0.join("base.txt")).unwrap(),
            "unrelated unstaged\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.0.join("untracked.txt")).unwrap(),
            "keep\n"
        );
        assert_eq!(
            git(&repo.0, &["status", "--porcelain"]),
            " M base.txt\n?? untracked.txt\n"
        );
    }
    #[tokio::test]
    async fn staged_target_queues_preservation_repair_even_for_clean_source() {
        let (repo, mut view, summary) = fixture();
        view.protocol = "terminal".into();
        std::fs::write(repo.0.join("base.txt"), "foreign staged\n").unwrap();
        git(&repo.0, &["add", "base.txt"]);
        let prompt = queued(view, &summary).await;
        assert!(request(&prompt)["repair_reason"]
            .as_str()
            .unwrap()
            .contains("staged changes"));
        assert_eq!(git(&repo.0, &["status", "--porcelain"]), "M  base.txt\n");
        assert!(!repo.0.join("topic.txt").exists());
    }
    #[tokio::test]
    async fn local_conflict_queues_exact_repair_context_after_safe_unwind() {
        let (repo, view, summary) = fixture();
        let source = Path::new(&view.work_dir);
        std::fs::write(source.join("base.txt"), "topic divergence\n").unwrap();
        git(source, &["add", "base.txt"]);
        git(source, &["commit", "-qm", "diverge source"]);
        std::fs::write(repo.0.join("base.txt"), "target divergence\n").unwrap();
        git(&repo.0, &["add", "base.txt"]);
        git(&repo.0, &["commit", "-qm", "diverge target"]);
        std::fs::write(repo.0.join("untracked.txt"), "keep\n").unwrap();
        let head = git(&repo.0, &["rev-parse", "HEAD"]);
        let prompt = queued(view, &summary).await;
        let payload = request(&prompt);
        assert_eq!(payload["target_resource"], "dev");
        assert!(payload["repair_reason"]
            .as_str()
            .unwrap()
            .contains("CONFLICT"));
        assert_eq!(git(&repo.0, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            git(&repo.0, &["status", "--porcelain"]),
            "?? untracked.txt\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.0.join("base.txt")).unwrap(),
            "target divergence\n"
        );
    }
    #[tokio::test]
    async fn true_local_blockers_do_not_dispatch_or_mutate_checkouts() {
        let (repo, view, summary) = fixture();
        std::fs::write(
            Path::new(&view.work_dir).join("topic.txt"),
            "pending editor save\n",
        )
        .unwrap();
        let local = Arc::new(LoomClient::new("http://127.0.0.1:1/", None).unwrap());
        let remote = Arc::new(LoomClient::new("http://remote.example/", None).unwrap());
        assert!(land_topic_locally(remote, view.clone(), &summary)
            .await
            .unwrap_err()
            .message
            .contains("remotely"));
        let mut missing = view.clone();
        missing.work_dir = "".into();
        assert!(land_topic_locally(local.clone(), missing, &summary)
            .await
            .unwrap_err()
            .message
            .contains("unavailable"));
        let source = Path::new(&view.work_dir);
        git(source, &["checkout", "-qb", "changed"]);
        assert!(land_topic_locally(local.clone(), view.clone(), &summary)
            .await
            .unwrap_err()
            .message
            .contains("changed branches"));
        git(source, &["checkout", "-q", "weaver/topic"]);
        git(&repo.0, &["checkout", "-q", "--detach"]);
        assert!(land_topic_locally(local.clone(), view.clone(), &summary)
            .await
            .unwrap_err()
            .message
            .contains("detached"));
        git(&repo.0, &["checkout", "-q", "dev"]);
        std::fs::write(repo.0.join(".git/CHERRY_PICK_HEAD"), "foreign operation\n").unwrap();
        assert!(land_topic_locally(local.clone(), view.clone(), &summary)
            .await
            .unwrap_err()
            .message
            .contains("in progress"));
        assert_eq!(
            std::fs::read_to_string(repo.0.join(".git/CHERRY_PICK_HEAD")).unwrap(),
            "foreign operation\n"
        );
        std::fs::remove_file(repo.0.join(".git/CHERRY_PICK_HEAD")).unwrap();
        let operation = git(source, &["rev-parse", "--git-path", "MERGE_HEAD"]);
        std::fs::write(source.join(operation.trim()), "foreign source operation\n").unwrap();
        assert!(land_topic_locally(local.clone(), view.clone(), &summary)
            .await
            .unwrap_err()
            .message
            .contains("in progress"));
        std::fs::remove_file(source.join(operation.trim())).unwrap();
        let (_other, mut other_view, _) = fixture();
        other_view.branch.repo_root = view.branch.repo_root.clone();
        assert!(land_topic_locally(local, other_view, &summary)
            .await
            .unwrap_err()
            .message
            .contains("different repository"));
        assert_eq!(git(&repo.0, &["status", "--porcelain"]), "");
        assert!(!repo.0.join("topic.txt").exists());
    }
}

#[cfg(test)]
mod thread_landing_contract_tests {
    use super::*;
    use serde_json::{json, Value};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn view(id: &str, protocol: &str) -> SessionView {
        let mut v: SessionView =
            serde_json::from_str(include_str!("../tests/fixtures/launch-response.json")).unwrap();
        v.id = id.into();
        v.protocol = protocol.into();
        v.status = "running".into();
        v.branch.id = format!("{id}-branch-id");
        v.branch.branch = format!("weaver/{id}");
        v.branch.repo_root = "/repo".into();
        v.work_dir = format!("/repo/.worktrees/{id}");
        v.branch.tags.clear();
        v
    }
    fn tagged(v: &mut SessionView, key: &str, value: &str) {
        v.branch.tags.push(crate::loom::TagView {
            key: key.into(),
            value: value.into(),
            note: String::new(),
            set_at: String::new(),
            set_by: "arachne".into(),
        });
    }
    fn summary(view: &SessionView, parent: Option<&str>) -> Value {
        let mut value = serde_json::to_value(view).unwrap();
        value["parent_session_id"] = json!(parent);
        value["parent_id"] = Value::Null;
        value
    }
    fn fixture(protocol: &str) -> (SessionView, SessionView, Value) {
        let mut track = view("track", protocol);
        tagged(&mut track, "topic", "true");
        tagged(&mut track, "track_topic_branch", "arachne/tracks/accepted");
        let parent = view("parent", protocol);
        let worker = view("worker", protocol);
        let fleet = json!([
            summary(&track, None),
            summary(&parent, Some("track")),
            summary(&worker, Some("parent"))
        ]);
        (track, worker, fleet)
    }
    fn sub(channel: &str, session: &str) -> Value {
        json!({"channel_id":channel,"subject_kind":"session","subject_id":session,"mode":"deliver"})
    }
    async fn mock(
        script: Vec<(&'static str, Value)>,
    ) -> (LoomClient, tokio::task::JoinHandle<Vec<(String, Value)>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (path, reply) in script {
                let (mut socket, _) =
                    tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut bytes = Vec::new();
                let (header_end, length) = loop {
                    let mut chunk = [0; 8192];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length: usize = headers
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(str::to_owned)
                            })
                            .unwrap()
                            .parse()
                            .unwrap();
                        break (end + 4, length);
                    }
                };
                while bytes.len() < header_end + length {
                    let mut chunk = [0; 8192];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                }
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                assert!(headers.starts_with(&format!("POST {path} ")), "{headers}");
                let body: Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                let reply = if reply.get("_message_receipt").is_some() {
                    json!({"id":"durable-message", "channel_id":body["channel"], "kind":"message", "body":body["body"].as_str().unwrap().trim(), "payload":body["payload"]})
                } else {
                    reply
                };
                requests.push((path.into(), body));
                if reply.get("_drop_ack").is_some() {
                    continue;
                }
                let (status, response) = if reply.get("_http_error").is_some() {
                    (
                        "500 Internal Server Error",
                        json!({"error":reply["_http_error"]}).to_string(),
                    )
                } else if let Some(raw) = reply["_raw_ack"].as_str() {
                    ("200 OK", raw.into())
                } else {
                    ("200 OK", reply.to_string())
                };
                socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
            }
            requests
        });
        (
            LoomClient::new(&format!("http://{addr}"), None).unwrap(),
            handle,
        )
    }

    #[tokio::test]
    async fn nested_acp_land_queues_wrapup_and_one_complete_canonical_acceptance() {
        let (track, worker, fleet) = fixture("acp");
        let (client, server) = mock(vec![
            ("/api/sessions/summary/list", fleet),
            ("/api/sessions/get", json!(worker)),
            ("/api/sessions/get", json!(track)),
            ("/api/channels/subscription/set", sub("track", "track")),
            ("/api/channels/subscription/set", sub("parent", "track")),
            (
                "/api/sessions/changes",
                json!({"head_oid":"authorized-source-head","base":null}),
            ),
            ("/api/sessions/tags/set", json!({})),
            ("/api/sessions/get", json!(worker)),
            (
                "/api/sessions/prompt/create",
                json!({"queued":true,"turn":null}),
            ),
            ("/api/sessions/get", json!(track)),
            (
                "/api/sessions/prompt/create",
                json!({"queued":true,"turn":null}),
            ),
        ])
        .await;
        let target = land_thread_with_client(&client, "worker", None)
            .await
            .unwrap();
        assert_eq!(target.target_branch, "arachne/tracks/accepted");
        let calls = server.await.unwrap();
        assert_eq!(
            calls[4].1,
            json!({"channel":"parent","session":"track","mode":"deliver"})
        );
        assert_eq!(calls[6].1["key"], "landing_request");
        for i in [8, 10] {
            assert_eq!(calls[i].1["send_now"], false);
        }
        let wrap = calls[8].1["text"].as_str().unwrap();
        assert!(wrap.contains(&worker.work_dir));
        assert!(wrap.contains(&worker.branch.branch));
        let accept = calls[10].1["text"].as_str().unwrap();
        assert!(accept.contains("\"strategy\":\"squash\""));
        assert!(accept.contains("successful intended wrap-up"));
        assert!(accept.contains("review, repair, commit, validate and acceptance"));
        assert!(accept.contains("preserve foreign edits"));
        assert!(accept.contains("arachne/tracks/accepted"));
        assert!(accept.contains("Never push, publish, open PRs"));
    }

    #[tokio::test]
    async fn terminal_land_is_durable_and_never_types_or_interrupts() {
        let (track, worker, fleet) = fixture("terminal");
        let (client, server) = mock(vec![
            ("/api/sessions/summary/list", fleet),
            ("/api/sessions/get", json!(worker)),
            ("/api/sessions/get", json!(track)),
            ("/api/channels/subscription/set", sub("track", "track")),
            ("/api/channels/subscription/set", sub("parent", "track")),
            (
                "/api/sessions/changes",
                json!({"head_oid":"authorized-source-head","base":null}),
            ),
            ("/api/sessions/tags/set", json!({})),
            ("/api/channels/subscription/set", sub("worker", "worker")),
            (
                "/api/channels/messages/create",
                json!({"_message_receipt":true}),
            ),
            ("/api/channels/subscription/set", sub("track", "track")),
            (
                "/api/channels/messages/create",
                json!({"_message_receipt":true}),
            ),
        ])
        .await;
        land_thread_with_client(&client, "worker", Some("merge"))
            .await
            .unwrap();
        let calls = server.await.unwrap();
        assert_eq!(calls[8].1["channel"], "worker");
        assert_eq!(
            calls[8].1["payload"]["source_request_revision"],
            "authorized-source-head"
        );
        assert_eq!(
            calls[10].1["payload"]["source_request_revision"],
            "authorized-source-head"
        );
        assert_eq!(calls[10].1["channel"], "track");
        assert_eq!(calls[8].1["payload"]["request"]["source_session"], "worker");
        assert_eq!(
            calls[10].1["payload"]["request"]["target_branch"],
            "arachne/tracks/accepted"
        );
        assert!(calls.iter().all(|(p, _)| !p.ends_with("/send")
            && !p.ends_with("/interrupt")
            && !p.ends_with("/prompt/create")));
    }

    #[tokio::test]
    async fn terminal_land_retry_keeps_snapshot_keys_and_later_head_gets_new_keys() {
        let (track, worker, fleet) = fixture("terminal");
        let mut replies = Vec::new();
        for (index, (head, version)) in [
            ("first-head", "same-work"),
            ("first-head", "same-work"),
            ("later-head", "later-work"),
            ("later-head", "new-uncommitted-work"),
        ]
        .iter()
        .enumerate()
        {
            replies.extend([
                ("/api/sessions/summary/list", fleet.clone()),
                ("/api/sessions/get", json!(worker)),
                ("/api/sessions/get", json!(track)),
                ("/api/channels/subscription/set", sub("track", "track")),
                ("/api/channels/subscription/set", sub("parent", "track")),
                (
                    "/api/sessions/changes",
                    json!({"head_oid":head,"version":version,"base":null}),
                ),
                ("/api/sessions/tags/set", json!({})),
                ("/api/channels/subscription/set", sub("worker", "worker")),
                (
                    "/api/channels/messages/create",
                    json!({"_message_receipt":true}),
                ),
                ("/api/channels/subscription/set", sub("track", "track")),
                (
                    "/api/channels/messages/create",
                    if index == 0 {
                        json!({"_drop_ack":true})
                    } else {
                        json!({"_message_receipt":true})
                    },
                ),
            ]);
        }
        let (client, server) = mock(replies).await;
        let error = land_thread_with_client(&client, "worker", None)
            .await
            .unwrap_err();
        assert!(error.message.contains("may already have been delivered"));
        land_thread_with_client(&client, "worker", None)
            .await
            .unwrap();
        land_thread_with_client(&client, "worker", None)
            .await
            .unwrap();
        land_thread_with_client(&client, "worker", None)
            .await
            .unwrap();
        let calls = server.await.unwrap();
        for stage in [8, 10] {
            assert_eq!(
                calls[stage].1["idempotency_key"],
                calls[stage + 11].1["idempotency_key"]
            );
            assert_ne!(
                calls[stage + 11].1["idempotency_key"],
                calls[stage + 22].1["idempotency_key"]
            );
            assert_ne!(
                calls[stage + 22].1["idempotency_key"],
                calls[stage + 33].1["idempotency_key"]
            );
        }
        assert_eq!(
            calls[8].1["payload"]["source_request_revision"],
            calls[10].1["payload"]["source_request_revision"]
        );
        assert_eq!(
            calls[32].1["payload"]["source_request_revision"],
            "later-head"
        );
    }

    #[tokio::test]
    async fn missing_source_head_cannot_claim_snapshot_bound_land_delivery() {
        let (track, worker, fleet) = fixture("terminal");
        let (client, server) = mock(vec![
            ("/api/sessions/summary/list", fleet),
            ("/api/sessions/get", json!(worker)),
            ("/api/sessions/get", json!(track)),
            ("/api/channels/subscription/set", sub("track", "track")),
            ("/api/channels/subscription/set", sub("parent", "track")),
            (
                "/api/sessions/changes",
                json!({"head_oid":null,"base":null}),
            ),
        ])
        .await;
        let error = land_thread_with_client(&client, "worker", None)
            .await
            .unwrap_err();
        assert!(error
            .message
            .contains("request identity without a source HEAD"));
        assert_eq!(server.await.unwrap().len(), 6);
    }

    #[tokio::test]
    async fn subscriptions_reject_wrong_channel_subject_or_observe_mode() {
        for (field, wrong) in [
            ("channel_id", "wrong"),
            ("subject_kind", "user"),
            ("subject_id", "caller"),
            ("mode", "observe"),
        ] {
            let mut reply = sub("parent", "track");
            reply[field] = json!(wrong);
            let (client, server) = mock(vec![("/api/channels/subscription/set", reply)]).await;
            assert!(client
                .ensure_delivery_subscription("parent", "track")
                .await
                .is_err());
            let calls = server.await.unwrap();
            assert_eq!(calls[0].1["session"], "track");
        }
    }

    #[tokio::test]
    async fn failed_nested_route_stops_before_wrapup_or_acceptance() {
        let (track, worker, fleet) = fixture("acp");
        let (client, server) = mock(vec![
            ("/api/sessions/summary/list", fleet),
            ("/api/sessions/get", json!(worker)),
            ("/api/sessions/get", json!(track)),
            ("/api/channels/subscription/set", sub("track", "track")),
            ("/api/channels/subscription/set", sub("parent", "human")),
        ])
        .await;
        assert!(land_thread_with_client(&client, "worker", None)
            .await
            .is_err());
        assert_eq!(server.await.unwrap().len(), 5);
    }

    #[tokio::test]
    async fn saved_policy_survives_route_delivery_and_refresh_failures_in_both_directions() {
        for (previous, policy) in [("off", "on"), ("on", "off")] {
            for failure in ["route", "delivery", "refresh"] {
                let (mut track, _, fleet) = fixture("acp");
                tagged(&mut track, "track_autoland", previous);
                let mut replies = vec![
                    ("/api/sessions/get", json!(track)),
                    ("/api/sessions/summary/list", fleet),
                    ("/api/sessions/tags/set", json!({})),
                ];
                if failure == "route" {
                    replies.push((
                        "/api/channels/subscription/set",
                        sub("track", "wrong-subject"),
                    ));
                } else {
                    replies.extend([
                        ("/api/channels/subscription/set", sub("track", "track")),
                        ("/api/channels/subscription/set", sub("parent", "track")),
                        ("/api/sessions/get", json!(track)),
                        (
                            "/api/sessions/prompt/create",
                            if failure == "delivery" {
                                json!({"_drop_ack":true})
                            } else {
                                json!({"queued":true,"turn":null})
                            },
                        ),
                    ]);
                }
                replies.push((
                    "/api/sessions/get",
                    json!({"_http_error":"refresh unavailable"}),
                ));
                let (client, server) = mock(replies).await;
                let result = set_autoland_with_client(&client, "track", policy)
                    .await
                    .unwrap();
                assert_eq!(
                    crate::track_policy::tag(&result.session.branch, "track_autoland"),
                    Some(policy)
                );
                assert!(result.warnings.iter().all(|w| w.contains("was saved")));
                assert_eq!(
                    result.warnings.len(),
                    if failure == "refresh" { 1 } else { 2 }
                );
                if failure == "delivery" {
                    assert!(result.warnings[0].contains("delivery to track is uncertain"));
                }
                let calls = server.await.unwrap();
                assert_eq!(calls[2].1["value"], policy);
            }
        }
    }

    #[tokio::test]
    async fn later_policy_reenable_has_a_new_request_identity() {
        let mut keys = Vec::new();
        for revision in ["off-before-first-enable", "off-after-disable"] {
            let (mut track, _, fleet) = fixture("terminal");
            tagged(&mut track, "track_autoland", "off");
            track.branch.tags.last_mut().unwrap().set_at = revision.into();
            let mut refreshed = track.clone();
            refreshed.branch.tags.last_mut().unwrap().value = "on".into();
            let (client, server) = mock(vec![
                ("/api/sessions/get", json!(track)),
                ("/api/sessions/summary/list", fleet),
                ("/api/sessions/tags/set", json!({})),
                ("/api/channels/subscription/set", sub("track", "track")),
                ("/api/channels/subscription/set", sub("parent", "track")),
                ("/api/channels/subscription/set", sub("track", "track")),
                (
                    "/api/channels/messages/create",
                    json!({"_message_receipt":true}),
                ),
                ("/api/sessions/get", json!(refreshed)),
            ])
            .await;
            let result = set_autoland_with_client(&client, "track", "on")
                .await
                .unwrap();
            assert!(result.warnings.is_empty());
            let calls = server.await.unwrap();
            assert_eq!(
                calls[6].1["payload"]["previous_policy_tag"]["set_at"],
                revision
            );
            keys.push(calls[6].1["idempotency_key"].clone());
        }
        assert_ne!(keys[0], keys[1]);
    }

    #[tokio::test]
    async fn policy_refresh_can_report_a_newer_live_policy() {
        let (track, _, fleet) = fixture("acp");
        let mut live = track.clone();
        tagged(&mut live, "track_autoland", "off");
        let (client, server) = mock(vec![
            ("/api/sessions/get", json!(track)),
            ("/api/sessions/summary/list", fleet),
            ("/api/sessions/tags/set", json!({})),
            ("/api/channels/subscription/set", sub("track", "wrong")),
            ("/api/sessions/get", json!(live)),
        ])
        .await;
        let result = set_autoland_with_client(&client, "track", "on")
            .await
            .unwrap();
        assert_eq!(
            crate::track_policy::tag(&result.session.branch, "track_autoland"),
            Some("off")
        );
        assert_eq!(result.warnings.len(), 1);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn terminal_uncertain_receipts_keep_retry_identity_across_client_restarts() {
        for bad in [
            json!({}),
            json!({"_raw_ack":"broken json"}),
            json!({"_drop_ack":true}),
            json!({"_http_error":"failed after append"}),
        ] {
            let terminal = view("terminal", "terminal");
            let (client, server) = mock(vec![
                (
                    "/api/channels/subscription/set",
                    sub("terminal", "terminal"),
                ),
                ("/api/channels/messages/create", bad),
                (
                    "/api/channels/subscription/set",
                    sub("terminal", "terminal"),
                ),
                (
                    "/api/channels/messages/create",
                    json!({"_message_receipt":true}),
                ),
                (
                    "/api/channels/subscription/set",
                    sub("terminal", "terminal"),
                ),
                (
                    "/api/channels/messages/create",
                    json!({"_message_receipt":true}),
                ),
            ])
            .await;
            let payload =
                json!({"action":"publish","source_revision":"sha1","target_revision":"target1"});
            let error = client
                .deliver_request(&terminal, "complete authorization", payload.clone())
                .await
                .unwrap_err();
            let LoomError::DeliveryUncertain {
                request_key: Some(key),
                ..
            } = &error
            else {
                panic!("{error}")
            };
            assert!(error
                .to_string()
                .contains("may already have been delivered"));
            let restarted = LoomClient::new(&client.server_url(), None).unwrap();
            restarted
                .deliver_request(&terminal, "complete authorization", payload)
                .await
                .unwrap();
            restarted.deliver_request(&terminal, "complete authorization", json!({"action":"publish","source_revision":"sha2","target_revision":"target1"})).await.unwrap();
            let calls = server.await.unwrap();
            assert_eq!(calls[1].1["idempotency_key"], key.as_str());
            assert_eq!(calls[1].1["idempotency_key"], calls[3].1["idempotency_key"]);
            assert_ne!(calls[3].1["idempotency_key"], calls[5].1["idempotency_key"]);
            assert!(calls.iter().all(|(p, _)| !p.ends_with("/send")
                && !p.ends_with("/interrupt")
                && !p.ends_with("/prompt/create")));
        }
    }

    #[tokio::test]
    async fn wrong_terminal_receipts_are_uncertain_not_successful() {
        for field in ["id", "channel_id", "kind", "body", "payload"] {
            let mut receipt = json!({"id":"message","channel_id":"terminal","kind":"message","body":"authorization","payload":{}});
            receipt[field] = json!("wrong");
            if field == "id" {
                receipt[field] = json!("");
            }
            let (client, server) = mock(vec![("/api/channels/messages/create", receipt)]).await;
            assert!(matches!(
                client
                    .send_to_thread("terminal", "authorization", json!({}), "stable-key")
                    .await,
                Err(LoomError::DeliveryUncertain {
                    request_key: Some(_),
                    ..
                })
            ));
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn acp_receipts_require_real_ack_and_never_suggest_safe_retry() {
        for reply in [
            json!({}),
            json!({"queued":"true","turn":null}),
            json!({"queued":true}),
            json!({"queued":true,"turn":-1}),
            json!({"_raw_ack":"broken"}),
            json!({"_drop_ack":true}),
        ] {
            let (client, server) = mock(vec![("/api/sessions/prompt/create", reply)]).await;
            let error = client
                .queue_prompt("worker", "authorization")
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                LoomError::DeliveryUncertain {
                    request_key: None,
                    ..
                }
            ));
            assert!(error.to_string().contains("Reconcile"));
            let calls = server.await.unwrap();
            assert_eq!(calls[0].1["send_now"], false);
            assert!(calls[0].1.get("idempotency_key").is_none());
        }
        for reply in [
            json!({"queued":true,"turn":null}),
            json!({"queued":false,"turn":12}),
        ] {
            let (client, server) = mock(vec![("/api/sessions/prompt/create", reply)]).await;
            client
                .queue_prompt("worker", "authorization")
                .await
                .unwrap();
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn request_route_failure_prevents_terminal_authorization() {
        let (client, server) = mock(vec![(
            "/api/channels/subscription/set",
            sub("terminal", "wrong"),
        )])
        .await;
        let error = client
            .deliver_request(&view("terminal", "terminal"), "authorization", json!({}))
            .await
            .unwrap_err();
        assert!(matches!(error, LoomError::Decode { .. }));
        assert_eq!(server.await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn policy_off_persists_routes_nested_children_and_delivers_live_guard() {
        let (track, _, fleet) = fixture("acp");
        let mut refreshed = track.clone();
        tagged(&mut refreshed, "track_autoland", "off");
        let (client, server) = mock(vec![
            ("/api/sessions/get", json!(track)),
            ("/api/sessions/summary/list", fleet),
            ("/api/sessions/tags/set", json!({})),
            ("/api/channels/subscription/set", sub("track", "track")),
            ("/api/channels/subscription/set", sub("parent", "track")),
            ("/api/sessions/get", json!(track)),
            (
                "/api/sessions/prompt/create",
                json!({"queued":true,"turn":null}),
            ),
            ("/api/sessions/get", json!(refreshed)),
        ])
        .await;
        let result = set_autoland_with_client(&client, "track", "off")
            .await
            .unwrap();
        assert_eq!(
            crate::track_policy::tag(&result.session.branch, "track_autoland"),
            Some("off")
        );
        let calls = server.await.unwrap();
        assert_eq!(calls[2].1["key"], "track_autoland");
        assert_eq!(calls[2].1["value"], "off");
        let prompt = calls[6].1["text"].as_str().unwrap();
        assert!(prompt.contains("reread LIVE Track and source session tags"));
        assert!(prompt.contains("including source commits, repairs"));
        assert!(prompt.contains("stop this stale automatic request before writing"));
        assert!(prompt.contains("source on overrides"));
        assert!(prompt.contains("no scheduler or polling"));
        assert_eq!(calls[6].1["send_now"], false);
    }

    #[tokio::test]
    async fn disabling_persists_live_off_even_if_canonical_initialization_is_incomplete() {
        let (mut track, _, _) = fixture("acp");
        track.branch.tags.retain(|t| t.key != "track_topic_branch");
        track.branch.goal = format!(
            "{} arachne/tracks/pending",
            crate::track_policy::CANONICAL_INITIALIZATION_MARKER
        );
        let (client, server) = mock(vec![
            ("/api/sessions/get", json!(track)),
            ("/api/sessions/summary/list", json!([summary(&track, None)])),
            ("/api/sessions/tags/set", json!({})),
            (
                "/api/sessions/get",
                json!({"_http_error":"refresh unavailable"}),
            ),
        ])
        .await;
        let result = set_autoland_with_client(&client, "track", "off")
            .await
            .unwrap();
        assert!(result.warnings[0].contains("policy off was saved"));
        assert_eq!(
            crate::track_policy::tag(&result.session.branch, "track_autoland"),
            Some("off")
        );
        let calls = server.await.unwrap();
        assert_eq!(calls[2].1["key"], "track_autoland");
        assert_eq!(calls[2].1["value"], "off");
    }

    #[tokio::test]
    async fn source_off_persists_before_incomplete_canonical_setup() {
        let (mut track, worker, _) = fixture("acp");
        track
            .branch
            .tags
            .retain(|tag| tag.key != "track_topic_branch");
        track.branch.goal = format!(
            "{} arachne/tracks/pending",
            crate::track_policy::CANONICAL_INITIALIZATION_MARKER
        );
        let fleet = json!([summary(&track, None), summary(&worker, Some("track"))]);
        let (client, server) = mock(vec![
            ("/api/sessions/get", json!(worker)),
            ("/api/sessions/summary/list", fleet),
            ("/api/sessions/get", json!(track)),
            ("/api/sessions/tags/set", json!({})),
            (
                "/api/sessions/get",
                json!({"_http_error":"refresh unavailable"}),
            ),
        ])
        .await;
        let result = set_autoland_with_client(&client, "worker", "off")
            .await
            .unwrap();
        assert_eq!(
            crate::track_policy::tag(&result.session.branch, "thread_autoland"),
            Some("off")
        );
        assert!(result.warnings[0].contains("initialization is incomplete"));
        let calls = server.await.unwrap();
        assert_eq!(calls[3].1["key"], "thread_autoland");
        assert_eq!(calls[3].1["value"], "off");
    }

    #[tokio::test]
    async fn source_override_routes_to_track_and_returns_refreshed_source() {
        let (track, worker, fleet) = fixture("terminal");
        let mut refreshed = worker.clone();
        tagged(&mut refreshed, "thread_autoland", "on");
        let (client, server) = mock(vec![
            ("/api/sessions/get", json!(worker)),
            ("/api/sessions/summary/list", fleet),
            ("/api/sessions/get", json!(track)),
            ("/api/sessions/tags/set", json!({})),
            ("/api/channels/subscription/set", sub("track", "track")),
            ("/api/channels/subscription/set", sub("parent", "track")),
            ("/api/channels/subscription/set", sub("track", "track")),
            (
                "/api/channels/messages/create",
                json!({"_message_receipt":true}),
            ),
            ("/api/sessions/get", json!(refreshed)),
        ])
        .await;
        let result = set_autoland_with_client(&client, "worker", "on")
            .await
            .unwrap();
        assert_eq!(result.session.id, "worker");
        assert_eq!(
            crate::track_policy::tag(&result.session.branch, "thread_autoland"),
            Some("on")
        );
        let calls = server.await.unwrap();
        assert_eq!(calls[3].1["key"], "thread_autoland");
        assert_eq!(calls[7].1["payload"]["session_id"], "worker");
    }

    #[tokio::test]
    async fn future_nested_launch_installs_parent_route_before_work() {
        let (mut track, _, fleet) = fixture("acp");
        tagged(&mut track, "track_autoland", "on");
        let parent = view("parent", "acp");
        let fleet: Vec<SessionSummaryView> = serde_json::from_value(fleet).unwrap();
        let (client, server) = mock(vec![
            ("/api/sessions/get", json!(track)),
            ("/api/channels/subscription/set", sub("track", "track")),
            ("/api/channels/subscription/set", sub("parent", "track")),
            ("/api/sessions/get", json!(track)),
            (
                "/api/sessions/prompt/create",
                json!({"queued":true,"turn":null}),
            ),
        ])
        .await;
        prepare_child_autoland(&client, &parent, &fleet)
            .await
            .unwrap();
        let calls = server.await.unwrap();
        assert_eq!(calls[2].1["channel"], "parent");
        assert!(calls[4].1["text"]
            .as_str()
            .unwrap()
            .contains("existing or future linked"));
    }

    #[tokio::test]
    async fn launch_branch_allocation_respects_git_refs_and_durable_claims() {
        let (track, _, _) = fixture("acp");
        let (client, server) = mock(vec![
            (
                "/api/repos/branches",
                json!([{"name":"arachne/tracks/api","worktree":null,"current":false}]),
            ),
            ("/api/sessions/summary/list", json!([summary(&track, None)])),
        ])
        .await;
        assert_eq!(
            allocate_track_branch(&client, "/repo", "API", None)
                .await
                .unwrap(),
            "arachne/tracks/api-2"
        );
        server.await.unwrap();
        let (client, server) = mock(vec![
            ("/api/repos/branches", json!([])),
            ("/api/sessions/summary/list", json!([summary(&track, None)])),
        ])
        .await;
        assert!(allocate_track_branch(
            &client,
            "/repo",
            "ignored",
            Some("arachne/tracks/accepted")
        )
        .await
        .is_err());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn launch_durable_first_turn_initializes_real_ref_before_task_and_keeps_metadata_concise()
    {
        let launched = view("new-track", "terminal");
        let (client, server) = mock(vec![
            ("/api/sessions/launch", json!(launched)),
            ("/api/branches/update", json!(launched.branch)),
        ])
        .await;
        let input = crate::loom::SessionsLaunchInput {
            cwd: "/repo".into(),
            goal: Some("Implement API".into()),
            initialization_prompt: Some(crate::track_policy::initialization_prompt(
                "arachne/tracks/api",
            )),
            launch_guidance: Some(crate::loom::topic_launch_guidance()),
            ..Default::default()
        };
        client.launch(&input).await.unwrap();
        let calls = server.await.unwrap();
        let opening = calls[0].1["goal"].as_str().unwrap();
        assert!(opening.starts_with(crate::track_policy::CANONICAL_INITIALIZATION_MARKER));
        assert!(opening.find("git update-ref").unwrap() < opening.find("Implement API").unwrap());
        assert!(opening.contains("Capture the fresh launch seed"));
        assert!(opening.contains("STOP and report"));
        let metadata_goal = calls[1].1["goal"].as_str().unwrap();
        assert!(metadata_goal.starts_with("Implement API"));
        assert!(metadata_goal.contains(crate::track_policy::CANONICAL_INITIALIZATION_MARKER));
        assert!(!metadata_goal.contains("Arachne role guidance"));
        assert!(calls[0].1.get("initialization_prompt").is_none());
    }

    #[tokio::test]
    async fn canonical_tag_failure_is_recoverable_and_cannot_become_legacy_scratch_acceptance() {
        let mut launched = view("new-track", "terminal");
        launched.branch.goal = format!(
            "Implement API\n\n{} arachne/tracks/api",
            crate::track_policy::CANONICAL_INITIALIZATION_MARKER
        );
        let (client, server) = mock(vec![
            (
                "/api/sessions/tags/set",
                json!({"_http_error":"metadata temporarily unavailable"}),
            ),
            ("/api/sessions/tags/set", json!({})),
            ("/api/sessions/tags/set", json!({})),
        ])
        .await;
        stamp_new_track_metadata(&client, &mut launched, "arachne/tracks/api").await;
        assert_eq!(launched.id, "new-track");
        assert!(crate::track_policy::is_track(&launched.branch));
        assert!(crate::track_policy::canonical_topic_branch(&launched.branch).is_none());
        assert!(launched.branch.description.contains("Do not retry launch"));
        let calls = server.await.unwrap();
        assert_eq!(calls[2].1["key"], "attention");
        assert!(calls.iter().all(|(path, _)| !path.ends_with("/launch")));
    }

    #[test]
    fn canonical_resolution_covers_legacy_standalone_cross_repo_cycles_and_pending_initialization()
    {
        let (mut track, worker, fleet) = fixture("acp");
        let fleet: Vec<SessionSummaryView> = serde_json::from_value(fleet).unwrap();
        assert_eq!(
            resolve_integration_target(&fleet, &worker.id)
                .unwrap()
                .target_branch,
            "arachne/tracks/accepted"
        );
        track.branch.tags.clear();
        let parent = view("parent", "acp");
        let summaries = |track: &SessionView| {
            serde_json::from_value::<Vec<SessionSummaryView>>(json!([
                summary(track, None),
                summary(&parent, Some("track")),
                summary(&worker, Some("parent"))
            ]))
            .unwrap()
        };
        assert_eq!(
            resolve_integration_target(&summaries(&track), "worker")
                .unwrap()
                .target_branch,
            "weaver/track"
        );
        tagged(&mut track, "topic", "false");
        assert!(resolve_integration_target(&summaries(&track), "worker").is_none());
        track.branch.tags.clear();
        track.branch.repo_root = "/other".into();
        assert!(resolve_integration_target(&summaries(&track), "worker").is_none());
        track.branch.repo_root = "/repo".into();
        track.branch.goal = format!(
            "{} arachne/tracks/new",
            crate::track_policy::CANONICAL_INITIALIZATION_MARKER
        );
        assert!(resolve_integration_target(&summaries(&track), "worker").is_none());
        let mut cycle = summaries(&track);
        cycle[0].parent_session_id = Some("worker".into());
        assert!(resolve_integration_target(&cycle, "worker").is_none());
    }

    #[tokio::test]
    async fn publishing_choices_are_rejected_without_any_http_write() {
        let client = LoomClient::new("http://127.0.0.1:1", None).unwrap();
        for choice in ["open-pr", "pr-stack", "push", "land-locally"] {
            let error = land_thread_with_client(&client, "worker", Some(choice))
                .await
                .unwrap_err();
            assert!(!error.unreachable);
            assert!(error.message.contains("publishing is a separate"));
        }
    }
}
#[cfg(test)]
mod canonical_comparison_tests {
    use super::*;
    use crate::coordination::{resolve_review_comparison, review_launch_input, ReviewComparison};
    use serde_json::{json, Value};
    use std::path::{Path, PathBuf};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Repository(PathBuf);
    impl Drop for Repository {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn git(root: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().into()
    }
    fn commit(root: &Path, path: &str, contents: &str) {
        std::fs::write(root.join(path), contents).unwrap();
        git(root, &["add", "--", path]);
        git(root, &["commit", "-m", path]);
    }
    fn repo() -> Repository {
        let path = std::env::temp_dir().join(format!(
            "arachne-canonical-review-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-b", "release"]);
        git(&path, &["config", "user.name", "Test"]);
        git(&path, &["config", "user.email", "test@example.invalid"]);
        commit(&path, "seed.txt", "seed\n");
        git(&path, &["branch", "accepted/topic"]);
        Repository(path)
    }
    fn view(id: &str, repo: &str) -> SessionView {
        let mut source: SessionView =
            serde_json::from_str(include_str!("../tests/fixtures/launch-response.json")).unwrap();
        source.id = id.into();
        source.branch.id = format!("branch-{id}");
        source.branch.branch = format!("work/{id}");
        source.branch.repo_root = repo.into();
        source.work_dir = repo.into();
        source
    }
    fn track(repo: &str) -> SessionView {
        let mut track = view("track", repo);
        track.branch.branch = "scratch/coordinator".into();
        track.branch.tags = serde_json::from_value(json!([
            {"key":"topic","value":"true","note":"","set_by":"","set_at":""},
            {"key":"track_topic_branch","value":"accepted/topic","note":"","set_by":"","set_at":""}
        ]))
        .unwrap();
        track
    }
    fn summary(view: &SessionView, parent: Option<&str>) -> Value {
        let mut value = serde_json::to_value(view).unwrap();
        value["parent_session_id"] = json!(parent);
        value["parent_id"] = Value::Null;
        value
    }
    async fn mock(
        script: Vec<(&'static str, Value)>,
    ) -> (LoomClient, tokio::task::JoinHandle<Vec<Value>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (path, reply) in script {
                let (mut socket, _) =
                    tokio::time::timeout(std::time::Duration::from_secs(10), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut bytes = Vec::new();
                let (header_end, length) = loop {
                    let mut chunk = [0; 8192];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        assert!(headers.starts_with(&format!("POST {path} ")), "{headers}");
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(str::to_owned)
                            })
                            .unwrap()
                            .parse()
                            .unwrap();
                        break (end + 4, length);
                    }
                };
                while bytes.len() < header_end + length {
                    let mut chunk = [0; 8192];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                }
                requests
                    .push(serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap());
                let response = reply.to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
            }
            requests
        });
        (
            LoomClient::new(&format!("http://{addr}"), None).unwrap(),
            handle,
        )
    }

    #[tokio::test]
    async fn worker_preview_includes_inherited_changes_checkout_edits_and_advanced_target() {
        let repo = repo();
        let root = &repo.0;
        git(root, &["checkout", "-b", "work/parent", "accepted/topic"]);
        commit(root, "inherited.txt", "unaccepted parent\n");
        git(root, &["checkout", "-b", "work/worker"]);
        commit(root, "worker.txt", "worker\n");
        commit(root, "shared.txt", "worker original\n");
        std::fs::write(root.join("staged.txt"), "staged edit\n").unwrap();
        git(root, &["add", "staged.txt"]);
        std::fs::write(root.join("worker.txt"), "worker manual\n").unwrap();
        std::fs::write(root.join("untracked.txt"), "new edit\n").unwrap();
        let source = view("worker", root.to_str().unwrap());
        let comparison = ReviewComparison {
            source_ref: "work/worker".into(),
            target_ref: "accepted/topic".into(),
            accepted_source: false,
        };
        let before = git(root, &["status", "--porcelain=v1"]);
        let first = local_review_changes(&source, &comparison).await.unwrap();
        let paths: Vec<_> = first
            .changes
            .files
            .iter()
            .map(|f| f["path"]["display"].as_str().unwrap())
            .collect();
        for path in ["inherited.txt", "worker.txt", "staged.txt", "untracked.txt"] {
            assert!(paths.contains(&path), "{paths:?}");
        }
        assert_eq!(git(root, &["status", "--porcelain=v1"]), before);
        let accepted_checkout = root.join("accepted-checkout");
        git(
            root,
            &[
                "worktree",
                "add",
                accepted_checkout.to_str().unwrap(),
                "accepted/topic",
            ],
        );
        commit(&accepted_checkout, "shared.txt", "canonical advanced\n");
        let second = local_review_changes(&source, &comparison).await.unwrap();
        assert_ne!(first.changes.base["oid"], second.changes.base["oid"]);
        assert_eq!(
            second.changes.base["oid"],
            git(root, &["rev-parse", "accepted/topic"])
        );
        let shared = second
            .changes
            .files
            .iter()
            .find(|f| f["path"]["display"] == "shared.txt")
            .unwrap();
        assert!(shared.to_string().contains("canonical advanced"));
        assert!(shared.to_string().contains("worker original"));
        assert_eq!(
            git(
                root,
                &[
                    "status",
                    "--porcelain=v1",
                    "--",
                    "worker.txt",
                    "staged.txt",
                    "untracked.txt"
                ]
            ),
            "A  staged.txt\n M worker.txt\n?? untracked.txt"
        );
    }

    #[tokio::test]
    async fn root_preview_and_reviewer_checkout_use_real_accepted_ref_not_scratch() {
        let repo = repo();
        let root = &repo.0;
        git(root, &["checkout", "accepted/topic"]);
        commit(root, "accepted-only.txt", "accepted\n");
        git(root, &["checkout", "-b", "scratch/coordinator", "release"]);
        commit(root, "scratch-only.txt", "scratch\n");
        std::fs::write(root.join("scratch-dirty.txt"), "dirty\n").unwrap();
        let source = track(root.to_str().unwrap());
        let comparison = ReviewComparison {
            source_ref: "accepted/topic".into(),
            target_ref: "release".into(),
            accepted_source: true,
        };
        let before = git(root, &["status", "--porcelain=v1"]);
        let changes = local_review_changes(&source, &comparison).await.unwrap();
        assert_eq!(changes.source_ref, "accepted/topic");
        assert!(changes.accepted_source);
        assert_eq!(
            changes.changes.head_oid.unwrap(),
            git(root, &["rev-parse", "accepted/topic"])
        );
        assert_eq!(changes.changes.files.len(), 1);
        assert_eq!(
            changes.changes.files[0]["path"]["display"],
            "accepted-only.txt"
        );
        assert_eq!(git(root, &["status", "--porcelain=v1"]), before);
        let input = review_launch_input(&source, &comparison, "Review accepted state.");
        assert_eq!(input.base.as_deref(), Some("accepted/topic"));
        assert_eq!(input.parent_branch.as_deref(), Some("branch-track"));
        let reviewer = root.join("reviewer-checkout");
        git(
            root,
            &[
                "worktree",
                "add",
                "--detach",
                reviewer.to_str().unwrap(),
                input.base.as_deref().unwrap(),
            ],
        );
        assert!(reviewer.join("accepted-only.txt").exists());
        assert!(!reviewer.join("scratch-only.txt").exists());
        assert!(!reviewer.join("scratch-dirty.txt").exists());
        let prompt = input.goal.unwrap();
        assert!(prompt.contains("Publishing comparison base: release"));
        assert!(prompt.contains("git diff <base-SHA> <accepted-SHA>"));
    }

    #[tokio::test]
    async fn canonical_preview_mock_contract_never_calls_recorded_base_changes() {
        let repo = repo();
        git(
            &repo.0,
            &["checkout", "-b", "work/worker", "accepted/topic"],
        );
        commit(&repo.0, "inherited.txt", "intended\n");
        let source = view("worker", repo.0.to_str().unwrap());
        let owner = track(repo.0.to_str().unwrap());
        let (client, server) = mock(vec![
            ("/api/sessions/get", json!(source)),
            (
                "/api/sessions/summary/list",
                json!([summary(&owner, None), summary(&source, Some("track"))]),
            ),
            ("/api/sessions/get", json!(owner)),
        ])
        .await;
        let result = load_review_changes(&client, "worker").await.unwrap();
        assert_eq!(result.changes.base["reference"], "accepted/topic");
        assert_eq!(result.changes.files[0]["path"]["display"], "inherited.txt");
        let requests = server.await.unwrap();
        assert_eq!(requests[0], json!({"session":"worker"}));
        assert_eq!(requests[2], json!({"session":"track"}));
    }

    #[tokio::test]
    async fn review_scope_mock_uses_fresh_canonical_metadata_and_repository_specific_publish_base()
    {
        let source = view("worker", "/provider/repository");
        let owner = track("/provider/repository");
        let mut stale_owner = owner.clone();
        stale_owner.branch.tags[1].value = "old/topic".into();
        let (client, server) = mock(vec![
            (
                "/api/sessions/summary/list",
                json!([summary(&stale_owner, None), summary(&source, Some("track"))]),
            ),
            ("/api/sessions/get", json!(owner)),
        ])
        .await;
        let comparison = resolve_review_comparison(&client, &source).await.unwrap();
        assert_eq!(comparison.target_ref, "accepted/topic");
        assert_eq!(comparison.source_ref, "work/worker");
        assert!(!comparison.accepted_source);
        server.await.unwrap();
        let (client, server) = mock(vec![
            ("/api/sessions/summary/list", json!([summary(&owner, None)])),
            ("/api/branches/list", json!([
                {"branch":"accepted/topic","repo_root":"/other-repo","base_branch":"wrong"},
                {"branch":"accepted/topic","repo_root":"/provider/repository","base_branch":"release"},
                {"branch":"scratch/coordinator","repo_root":"/provider/repository","base_branch":"old"}
            ])),
        ]).await;
        let comparison = resolve_review_comparison(&client, &owner).await.unwrap();
        assert!(comparison.accepted_source);
        assert_eq!(comparison.source_ref, "accepted/topic");
        assert_eq!(comparison.target_ref, "release");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn root_review_uses_newer_fleet_canonical_tag_and_legacy_seed_base() {
        let owner = track("/provider/repository");
        let mut earlier = owner.clone();
        earlier.branch.tags[1].value = "old/topic".into();
        let (client, server) = mock(vec![
            ("/api/sessions/summary/list", json!([summary(&owner, None)])),
            ("/api/branches/list", json!([
                {"branch":"scratch/coordinator","repo_root":"/provider/repository","base_branch":"release"}
            ])),
        ]).await;
        let scope = resolve_review_comparison(&client, &earlier).await.unwrap();
        assert_eq!(scope.source_ref, "accepted/topic");
        assert_eq!(scope.target_ref, "release");
        assert!(scope.accepted_source);
        assert_eq!(
            review_launch_input(&earlier, &scope, "Review accepted state")
                .base
                .as_deref(),
            Some("accepted/topic")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn unresolved_publish_base_or_incomplete_initialization_never_falls_back_to_scratch() {
        let owner = track("/provider/repository");
        let (client, server) = mock(vec![
            ("/api/sessions/summary/list", json!([summary(&owner, None)])),
            ("/api/branches/list", json!([])),
        ])
        .await;
        assert!(resolve_review_comparison(&client, &owner)
            .await
            .err()
            .unwrap()
            .message
            .contains("No publishing base"));
        server.await.unwrap();
        let mut pending = owner;
        pending.branch.tags.clear();
        pending.branch.goal =
            "Arachne canonical Track initialization required: accepted/topic".into();
        let (client, server) = mock(vec![(
            "/api/sessions/summary/list",
            json!([summary(&pending, None)]),
        )])
        .await;
        assert!(resolve_review_comparison(&client, &pending)
            .await
            .err()
            .unwrap()
            .message
            .contains("initialization"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn remote_preview_never_reads_provider_paths_locally_and_keeps_review_scope() {
        let client = LoomClient::new("https://loom.example.invalid", None).unwrap();
        for accepted_source in [false, true] {
            let source = view("worker", "/this-provider-path-must-never-be-read-locally");
            let comparison = ReviewComparison {
                source_ref: if accepted_source {
                    "accepted/topic"
                } else {
                    "work/worker"
                }
                .into(),
                target_ref: "release".into(),
                accepted_source,
            };
            let preview = review_changes_for_source(&client, &source, &comparison)
                .await
                .unwrap();
            assert_eq!(preview.changes.base["state"], "unavailable");
            assert_eq!(
                preview.changes.base["reason"],
                "remote_canonical_preview_unavailable"
            );
            assert_eq!(preview.changes.base["reference"], "release");
            assert_eq!(preview.source_ref, comparison.source_ref);
            assert!(preview.changes.files.is_empty());
            assert!(preview.changes.head_oid.is_none());
            let input = review_launch_input(&source, &comparison, "Review on repository host.");
            assert_eq!(input.base.as_deref(), Some(comparison.source_ref.as_str()));
            assert!(input.goal.unwrap().contains("On the repository host"));
        }
    }

    #[tokio::test]
    async fn missing_canonical_ref_and_changed_worker_checkout_fail_closed() {
        let repo = repo();
        let source = view("worker", repo.0.to_str().unwrap());
        let comparison = ReviewComparison {
            source_ref: "release".into(),
            target_ref: "missing/accepted".into(),
            accepted_source: false,
        };
        assert!(local_review_changes(&source, &comparison).await.is_err());
        git(&repo.0, &["branch", "work/worker"]);
        let comparison = ReviewComparison {
            source_ref: "work/worker".into(),
            target_ref: "accepted/topic".into(),
            accepted_source: false,
        };
        assert!(local_review_changes(&source, &comparison)
            .await
            .err()
            .unwrap()
            .message
            .contains("checkout changed branch"));
    }

    #[tokio::test]
    async fn worker_checkout_in_an_unrelated_repository_cannot_impersonate_canonical_refs() {
        let advertised = repo();
        let unrelated = repo();
        git(&unrelated.0, &["checkout", "-b", "work/worker"]);
        commit(
            &unrelated.0,
            "foreign.txt",
            "foreign repository
",
        );
        let mut source = view("worker", advertised.0.to_str().unwrap());
        source.work_dir = unrelated.0.to_str().unwrap().into();
        let scope = ReviewComparison {
            source_ref: "work/worker".into(),
            target_ref: "accepted/topic".into(),
            accepted_source: false,
        };
        let before = git(&unrelated.0, &["status", "--porcelain=v1"]);
        let error = local_review_changes(&source, &scope).await.err().unwrap();
        assert!(
            error.message.contains("different repository"),
            "{}",
            error.message
        );
        assert_eq!(git(&unrelated.0, &["status", "--porcelain=v1"]), before);
    }

    #[test]
    fn ambient_git_overrides_cannot_redirect_canonical_preview() {
        let unrelated = repo();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "commands::canonical_comparison_tests::worker_preview_includes_inherited_changes_checkout_edits_and_advanced_target", "--nocapture"])
            .env("GIT_DIR", unrelated.0.join(".git"))
            .env("GIT_WORK_TREE", &unrelated.0)
            .env("GIT_INDEX_FILE", unrelated.0.join(".git/index"))
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }

    #[test]
    fn hunk_parser_has_correct_coordinates_and_honest_bounds() {
        let mut budget = 3;
        let (hunks, additions, deletions, truncated) = review_hunks(
            b"@@ -4,2 +4,2 @@\n context\n-old\n+new\n context\n",
            &mut budget,
        );
        assert!(truncated);
        assert_eq!((additions, deletions), (1, 1));
        assert_eq!(hunks[0]["lines"][0]["old_line"], 4);
        assert_eq!(hunks[0]["lines"][1]["old_line"], 5);
        assert_eq!(hunks[0]["lines"][2]["new_line"], 5);
        assert_eq!(hunks[0]["lines"][1]["new_line"], Value::Null);
    }
}
