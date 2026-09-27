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

use futures_util::StreamExt;
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
    #[error("invalid image attachment: {0}")]
    InvalidImage(String),
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
            .redirect(reqwest::redirect::Policy::none())
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

    /// Fetch a worktree image through Loom's authenticated `sessions.raw`
    /// download route. The webview only receives a bounded raster data URL;
    /// it never sees the bearer token or an arbitrary local file path.
    pub async fn session_image(&self, id: &str, path: &str) -> Result<String, LoomError> {
        validate_image_path(path)?;
        const ROUTE: &str = "/api/sessions/raw";
        const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
        let mut url = self
            .base
            .join(ROUTE)
            .map_err(|e| LoomError::Connection(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("session", id)
            .append_pair("path", path);
        let mut request = self.http.get(url).timeout(Duration::from_secs(30));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|e| LoomError::Connection(e.to_string()))?;
        if !response.status().is_success() {
            return Err(LoomError::Api {
                status: response.status().as_u16(),
                method: "GET",
                path: ROUTE.into(),
                message: String::new(),
            });
        }
        let mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim)
            .map(str::to_owned)
            .ok_or_else(|| LoomError::InvalidImage("missing image content type".into()))?;
        if !matches!(
            mime.as_str(),
            "image/png"
                | "image/jpeg"
                | "image/gif"
                | "image/webp"
                | "image/avif"
                | "image/bmp"
                | "image/x-icon"
        ) {
            return Err(LoomError::InvalidImage(
                "unsupported image content type".into(),
            ));
        }
        if response
            .content_length()
            .is_some_and(|len| len > MAX_IMAGE_BYTES as u64)
        {
            return Err(LoomError::InvalidImage("image exceeds 10 MiB".into()));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| LoomError::Connection(e.to_string()))?;
            if chunk.len() > MAX_IMAGE_BYTES - bytes.len() {
                return Err(LoomError::InvalidImage("image exceeds 10 MiB".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !has_image_signature(&mime, &bytes) {
            return Err(LoomError::InvalidImage(
                "image bytes do not match content type".into(),
            ));
        }
        Ok(format!("data:{mime};base64,{}", encode_base64(&bytes)))
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

fn validate_image_path(path: &str) -> Result<(), LoomError> {
    if path.is_empty()
        || path.len() > 1024
        || path.starts_with('/')
        || path.bytes().any(|b| b == b'\\' || b == b'\0' || b == b':')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(LoomError::InvalidImage(
            "expected a worktree-relative path".into(),
        ));
    }
    Ok(())
}

fn has_image_signature(mime: &str, bytes: &[u8]) -> bool {
    match mime {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(&b"WEBP"[..]),
        "image/avif" => {
            bytes.get(4..8) == Some(&b"ftyp"[..])
                && (bytes.get(8..12) == Some(&b"avif"[..])
                    || bytes.get(8..12) == Some(&b"avis"[..]))
        }
        "image/bmp" => bytes.starts_with(b"BM"),
        "image/x-icon" => bytes.starts_with(b"\0\0\x01\0"),
        _ => false,
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | (chunk.get(2).copied().unwrap_or(0) as u32);
        result.push(CHARS[((n >> 18) & 63) as usize] as char);
        result.push(CHARS[((n >> 12) & 63) as usize] as char);
        result.push(if chunk.len() > 1 { CHARS[((n >> 6) & 63) as usize] as char } else { '=' });
        result.push(if chunk.len() > 2 { CHARS[(n & 63) as usize] as char } else { '=' });
    }
    result
}

#[cfg(test)]
mod image_tests {
    use super::*;

    #[test]
    fn validates_relative_paths_and_image_signatures() {
        assert!(validate_image_path("images/screenshot.png").is_ok());
        for path in ["", "/etc/passwd", "../secret.png", "a/./b.png", "a//b.png", "C:\\x.png"] {
            assert!(validate_image_path(path).is_err(), "{path}");
        }
        assert!(has_image_signature("image/png", b"\x89PNG\r\n\x1a\n"));
        assert!(!has_image_signature("image/png", b"<script>"));
    }

    #[test]
    fn base64_encodes_without_a_new_dependency() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
    }
}
