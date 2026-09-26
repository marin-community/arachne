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

    /// `sessions.summary.list` — the fleet.
    pub async fn list_sessions(&self) -> Result<Vec<crate::loom::SessionSummaryView>, LoomError> {
        self.op("/api/sessions/summary/list", &serde_json::json!({}))
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

    /// `sessions.chat` — the conversation journal.
    pub async fn session_chat(&self, id: &str) -> Result<crate::loom::SessionChatView, LoomError> {
        self.op(
            "/api/sessions/chat",
            &serde_json::json!({ "session": id }),
        )
        .await
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
