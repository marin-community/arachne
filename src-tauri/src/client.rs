//! The Loom HTTP client Arachne runs in its Rust core.
//!
//! Loom's API is operation-shaped, not REST-shaped: nearly everything is
//! `POST /api/<area>/<verb>` with a flat JSON operand body, including reads.
//! This client wraps the handful of operations Arachne's cockpit needs and
//! forwards the server's `{error}` bodies as a `String` on failure.
//!
//! Auth: loopback-trusted when pointed at `127.0.0.1` (no token needed);
//! accepts a bearer token for the remote (DGX over Tailscale) future.

use std::time::Duration;

use serde::de::DeserializeOwned;
use tokio::sync::mpsc;

use crate::loom::EventFrame;

#[derive(Debug, thiserror::Error)]
pub enum LoomError {
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("loom returned {status} for {method} {path}: {message}")]
    Api {
        status: u16,
        method: &'static str,
        path: String,
        message: String,
    },
    #[error("malformed response from {path}")]
    Decode { path: String, detail: String },
}

impl LoomError {
    /// True when the failure is "cannot reach loom at all" rather than "loom
    /// answered with an error". Drives the connection indicator.
    pub fn is_unreachable(&self) -> bool {
        matches!(self, LoomError::Connection(_))
    }
}

pub struct LoomClient {
    http: reqwest::Client,
    base: reqwest::Url,
    token: Option<String>,
}

impl LoomClient {
    /// Host of the authoritative Loom server. For the bootstrap deployment
    /// this is also the runner's SSH host; no host-side path is ever treated
    /// as a local Mac path when the connection is remote.
    pub fn server_host(&self) -> Option<&str> {
        self.base.host_str()
    }

    pub fn new(base_url: &str, token: Option<String>) -> Result<Self, LoomError> {
        let base = reqwest::Url::parse(base_url)
            .map_err(|e| LoomError::Connection(format!("invalid base URL {base_url:?}: {e}")))?;
        // NOTE: no client-wide timeout. A `timeout()` applies per-request
        // including streaming bodies, which would kill the SSE connection
        // every 60s. Per-request timeouts for REST calls are set below.
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| LoomError::Connection(format!("building HTTP client: {e}")))?;
        Ok(Self { http, base, token })
    }

    async fn op<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Result<T, LoomError> {
        let url = self
            .base
            .join(path)
            .map_err(|e| LoomError::Connection(format!("joining {path}: {e}")))?;
        let mut req = self.http.post(url);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        let resp = req
            .json(body)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| LoomError::Connection(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let message = resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
                .unwrap_or_default();
            return Err(LoomError::Api {
                status: status.as_u16(),
                method: "POST",
                path: path.to_string(),
                message,
            });
        }
        resp.json::<T>()
            .await
            .map_err(|e| LoomError::Decode {
                path: path.to_string(),
                detail: e.to_string(),
            })
    }

    /// `GET /api/health` — public, unauthenticated.
    pub async fn health(&self) -> Result<(), LoomError> {
        let url = self
            .base
            .join("/api/health")
            .map_err(|e| LoomError::Connection(format!("joining /api/health: {e}")))?;
        let mut req = self.http.get(url);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        let resp = req
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| LoomError::Connection(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(LoomError::Api {
                status: status.as_u16(),
                method: "GET",
                path: "/api/health".into(),
                message: String::new(),
            });
        }
        Ok(())
    }

    /// Launch controls come from the connected Loom host, including its
    /// installed Codex model catalogue and account-specific profiles.
    pub async fn launch_options(&self) -> Result<crate::loom::LaunchOptionsView, LoomError> {
        let profiles = self
            .op("/api/profiles/list", &serde_json::json!({}))
            .await?;
        let agents: crate::loom::AgentsView = self
            .op("/api/agents/list", &serde_json::json!({}))
            .await?;
        Ok(crate::loom::LaunchOptionsView {
            profiles,
            agents: agents.agents,
            default_agent: agents.default_agent,
        })
    }

    /// Validate a profile/model change against the live resolver, then apply
    /// the same selection with optimistic revision guards. Loom permits this
    /// only for an idle ACP session and preserves the session identity.
    pub async fn handoff_session(
        &self,
        id: &str,
        profile: &str,
        agent: Option<&str>,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Result<crate::loom::SessionView, LoomError> {
        let selection = serde_json::json!({
            "profile": profile,
            "overrides": { "agent": agent, "model": model, "effort": effort }
        });
        let path = "/api/sessions/handoff/resolve";
        let preview: serde_json::Value = self
            .op(path, &serde_json::json!({ "session": id, "selection": selection }))
            .await?;
        if preview.get("valid").and_then(|v| v.as_bool()) != Some(true) {
            let errors = preview.get("errors")
                .and_then(|v| v.as_array())
                .map(|v| v.iter().filter_map(|e| e.as_str()).collect::<Vec<_>>().join("; "))
                .unwrap_or_else(|| "selection is invalid".to_string());
            return Err(LoomError::Api { status: 400, method: "POST", path: path.into(), message: errors });
        }
        self.op(
            "/api/sessions/handoff",
            &serde_json::json!({
                "session": id,
                "selection": selection,
                "expected_profile_revision": preview["profile_revision"],
                "expected_resolver_revision": preview["resolver_revision"],
            }),
        ).await
    }

    /// `sessions.summary.list` — the fleet.
    ///
    /// `archived: true` so finished children still show (dimmed) under their
    /// leader — a topic's history is part of its identity, and the UI
    /// filters placement rows anyway. Without it, archiving a child erases
    /// the leader's topic shape.
    pub async fn list_sessions(&self) -> Result<Vec<crate::loom::SessionSummaryView>, LoomError> {
        self.op(
            "/api/sessions/summary/list",
            &serde_json::json!({ "archived": true }),
        )
        .await
    }

    /// `session_layout.get` — lane spaces/groups.
    pub async fn session_layout(&self) -> Result<crate::loom::SessionLayoutView, LoomError> {
        self.op("/api/session_layout/get", &serde_json::json!({}))
            .await
    }

    /// `session_layout.groups.create` — new lane in a space.
    pub async fn create_group(
        &self,
        space_id: &str,
        name: &str,
    ) -> Result<crate::loom::SessionLayoutView, LoomError> {
        self.op(
            "/api/session_layout/groups/create",
            &serde_json::json!({ "space_id": space_id, "name": name }),
        )
        .await
    }

    /// `session_layout.move` — move sessions into a lane.
    pub async fn move_sessions(
        &self,
        session_ids: &[&str],
        destination_group_id: &str,
    ) -> Result<crate::loom::SessionLayoutView, LoomError> {
        self.op(
            "/api/session_layout/move",
            &serde_json::json!({
                "session_ids": session_ids,
                "destination_group_id": destination_group_id,
            }),
        )
        .await
    }

    /// `session_layout.groups.delete` — remove a lane; its sessions
    /// must move somewhere first (loom never orphans them).
    pub async fn delete_group(
        &self,
        id: &str,
        destination_group_id: &str,
    ) -> Result<crate::loom::SessionLayoutView, LoomError> {
        self.op(
            "/api/session_layout/groups/delete",
            &serde_json::json!({ "id": id, "destination_group_id": destination_group_id }),
        )
        .await
    }

    /// `sessions.get` — one session, including `work_dir` for Open-in-Zed.
    pub async fn get_session(&self, id: &str) -> Result<crate::loom::SessionView, LoomError> {
        self.op(
            "/api/sessions/get",
            &serde_json::json!({ "session": id }),
        )
        .await
    }

    /// `sessions.chat` — the conversation journal. `before` pages older
    /// turns (from the previous page's `older_cursor`); None = newest tail.
    pub async fn session_chat(
        &self,
        id: &str,
        before: Option<&crate::loom::ChatCursorView>,
    ) -> Result<crate::loom::SessionChatView, LoomError> {
        let mut body = serde_json::json!({ "session": id });
        if let Some(c) = before {
            body["before_turn"] = serde_json::json!(c.turn);
            body["before_seq"] = serde_json::json!(c.seq);
        }
        self.op("/api/sessions/chat", &body).await
    }

    /// `sessions.prompt.create` — send input to an ACP session's agent.
    pub async fn send_prompt(&self, id: &str, text: &str) -> Result<(), LoomError> {
        let _: serde_json::Value = self
            .op(
                "/api/sessions/prompt/create",
                &serde_json::json!({ "session": id, "text": text, "send_now": true }),
            )
            .await?;
        Ok(())
    }

    /// A durable inbox item on the destination session's channel. Loom
    /// delivers message-kind items to that session and records the author;
    /// the structured payload lets the UI retain source provenance.
    pub async fn send_to_thread(
        &self,
        destination_id: &str,
        body: &str,
        payload: serde_json::Value,
        idempotency_key: &str,
    ) -> Result<serde_json::Value, LoomError> {
        self.op(
            "/api/channels/messages/create",
            &serde_json::json!({
                "channel": destination_id,
                "body": body,
                "kind": "message",
                "urgency": "normal",
                "payload": payload,
                "idempotency_key": idempotency_key,
            }),
        )
        .await
    }

    /// `sessions.send` — text input to a terminal session; `submit` presses enter.
    pub async fn send_text(&self, id: &str, text: &str, submit: bool) -> Result<(), LoomError> {
        let _: serde_json::Value = self
            .op(
                "/api/sessions/send",
                &serde_json::json!({ "session": id, "text": text, "submit": submit }),
            )
            .await?;
        Ok(())
    }

    /// `sessions.interrupt` — stop the current turn.
    pub async fn interrupt(&self, id: &str) -> Result<(), LoomError> {
        let _: serde_json::Value = self
            .op(
                "/api/sessions/interrupt",
                &serde_json::json!({ "session": id }),
            )
            .await?;
        Ok(())
    }

    /// `sessions.launch` — worktree + terminal + agent, seeded with a task.
    pub async fn launch(
        &self,
        input: &crate::loom::SessionsLaunchInput,
    ) -> Result<crate::loom::SessionView, LoomError> {
        self.op("/api/sessions/launch", input).await
    }

    /// `sessions.reparent` — re-parent a session under another (or detach),
    /// following the parent into its placement group. Returns the fresh summary.
    pub async fn reparent_session(
        &self,
        session: &str,
        parent: Option<&str>,
    ) -> Result<crate::loom::SessionSummaryView, LoomError> {
        self.op(
            "/api/sessions/reparent",
            &serde_json::json!({ "session": session, "parent": parent }),
        )
        .await
    }

    /// `sessions.update` — edit a session's branch-level fields: title
    /// (compare-and-swap fenced), goal, description. Returns the fresh view.
    pub async fn update_session(
        &self,
        input: &crate::loom::SessionsUpdateInput,
    ) -> Result<crate::loom::SessionView, LoomError> {
        self.op("/api/sessions/update", input).await
    }

    /// `sessions.tags.set` — stamp a quiet tag on a session. Used to mark a
    /// leader chat as a durable `topic` (survives archive and restarts).
    pub async fn set_tag(
        &self,
        session: &str,
        key: &str,
        value: &str,
    ) -> Result<(), LoomError> {
        let _: serde_json::Value = self
            .op(
                "/api/sessions/tags/set",
                &serde_json::json!({
                    "session": session,
                    "key": key,
                    "value": value,
                    "by": "arachne",
                }),
            )
            .await?;
        Ok(())
    }

    /// `sessions.archive` — tear down terminal + worktree, keep the branch.
    /// Not yet surfaced in the UI; kept for the next iteration.
    #[allow(dead_code)]
    pub async fn archive(&self, id: &str) -> Result<(), LoomError> {
        let _: serde_json::Value = self
            .op(
                "/api/sessions/archive",
                &serde_json::json!({ "session": id }),
            )
            .await?;
        Ok(())
    }

    // -- SSE ----------------------------------------------------------------

    /// Subscribe to the multiplexed event stream for `topics`.
    ///
    /// Returns a channel of decoded frames. The connection reconnects with
    /// backoff (250ms → 5s cap) until the receiver is dropped. Topic set is
    /// fixed for the subscription's life — SSE cannot change topics
    /// mid-connection, so callers drop and resubscribe when the topic set
    /// changes (exactly what the UI does when switching open sessions).
    pub async fn subscribe(
        &self,
        topics: &[String],
    ) -> Result<mpsc::Receiver<EventFrame>, LoomError> {
        let (tx, rx) = mpsc::channel(256);
        let mut url = self.base.clone();
        url.set_path("/api/events/stream");
        {
            let mut pairs = url.query_pairs_mut();
            let joined = topics.join(",");
            pairs.append_pair("topics", &joined);
        }
        let http = self.http.clone();
        let token = self.token.clone();
        tokio::spawn(async move {
            let mut backoff = Duration::from_millis(250);
            loop {
                let mut req = http.get(url.clone());
                if let Some(token) = &token {
                    req = req.bearer_auth(token);
                }
                let mut ok = false;
                if let Ok(resp) = req.send().await {
                    if resp.status().is_success() {
                        ok = true;
                        backoff = Duration::from_millis(250);
                        // Read the body as bytes and decode SSE textually.
                        use futures_util::StreamExt;
                        let mut stream = resp.bytes_stream();
                        let mut buf = String::new();
                        loop {
                            match stream.next().await {
                                Some(Ok(chunk)) => {
                                    buf.push_str(&String::from_utf8_lossy(&chunk));
                                    // Process complete SSE events: blank-line
                                    // separated; each `data:` line carries one
                                    // JSON frame. Anything left over is a
                                    // partial event that stays buffered for
                                    // the next chunk — never trim to the last
                                    // newline, that would drop a complete
                                    // `data:` line still waiting for its
                                    // terminating blank line.
                                    while let Some(pos) = buf.find("\n\n") {
                                        let event: String = buf.drain(..pos + 2).collect();
                                        for line in event.lines() {
                                            // Keep-alive comments (`:` prefix)
                                            // and other fields are skipped.
                                            if let Some(data) = line.strip_prefix("data:") {
                                                let data = data.strip_prefix(' ').unwrap_or(data);
                                                if let Ok(frame) =
                                                    serde_json::from_str::<EventFrame>(data)
                                                {
                                                    if tx.send(frame).await.is_err() {
                                                        return;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                _ => break,
                            }
                        }
                    }
                }
                let _ = ok;
                if tx.is_closed() {
                    return;
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(5));
            }
        });
        Ok(rx)
    }
}
