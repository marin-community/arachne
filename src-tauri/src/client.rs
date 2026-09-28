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
        resp.json::<T>().await.map_err(|e| LoomError::Decode {
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
        let agents: crate::loom::AgentsView =
            self.op("/api/agents/list", &serde_json::json!({})).await?;
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
            .op(
                path,
                &serde_json::json!({ "session": id, "selection": selection }),
            )
            .await?;
        if preview.get("valid").and_then(|v| v.as_bool()) != Some(true) {
            let errors = preview
                .get("errors")
                .and_then(|v| v.as_array())
                .map(|v| {
                    v.iter()
                        .filter_map(|e| e.as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .unwrap_or_else(|| "selection is invalid".to_string());
            return Err(LoomError::Api {
                status: 400,
                method: "POST",
                path: path.into(),
                message: errors,
            });
        }
        self.op(
            "/api/sessions/handoff",
            &serde_json::json!({
                "session": id,
                "selection": selection,
                "expected_profile_revision": preview["profile_revision"],
                "expected_resolver_revision": preview["resolver_revision"],
            }),
        )
        .await
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

    /// `issues.board` — every work item across every repo. `all` includes
    /// closed items and `automation` items claimed by automation-class
    /// sessions (mirroring loom SPA's board fetch); the panel filters to the
    /// topic's slice client-side.
    pub async fn list_issues(&self) -> Result<Vec<crate::loom::IssueView>, LoomError> {
        self.op(
            "/api/issues/board",
            &serde_json::json!({ "all": true, "automation": true }),
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
        self.op("/api/sessions/get", &serde_json::json!({ "session": id }))
            .await
    }

    /// `sessions.changes` — committed and uncommitted changes against the
    /// session's recorded base ref. Feeds the Integrate diff summary.
    pub async fn session_changes(&self, id: &str) -> Result<crate::loom::ChangeSetView, LoomError> {
        self.op(
            "/api/sessions/changes",
            &serde_json::json!({ "session": id }),
        )
        .await
    }

    /// Branch-scoped versioned artifact. A missing manifest is a normal empty
    /// topic; callers distinguish its 404 from other Loom failures.
    pub async fn branch_artifact(
        &self,
        branch: &str,
        name: &str,
    ) -> Result<serde_json::Value, LoomError> {
        self.op(
            "/api/artifacts/get",
            &serde_json::json!({ "branch": branch, "name": name, "repo": false }),
        )
        .await
    }

    pub async fn write_branch_artifact(
        &self,
        branch: &str,
        name: &str,
        content: &str,
        base_rev: i64,
    ) -> Result<serde_json::Value, LoomError> {
        self.write_branch_artifact_titled(
            branch,
            name,
            content,
            base_rev,
            "Arachne topic resources",
        )
        .await
    }

    /// `write_branch_artifact` with an explicit display title — the shared
    /// write path for Arachne's branch artifacts (resources, todos).
    pub async fn write_branch_artifact_titled(
        &self,
        branch: &str,
        name: &str,
        content: &str,
        base_rev: i64,
        title: &str,
    ) -> Result<serde_json::Value, LoomError> {
        self.op(
            "/api/artifacts/write",
            &serde_json::json!({
                "branch": branch, "name": name, "content": content,
                "title": title, "kind": "json",
                "base_rev": base_rev, "repo": false,
            }),
        )
        .await
    }

    /// Read text from Loom's server-side worktree. Never interpret the path
    /// as a Mac-local filename, including when Loom runs on this machine.
    pub async fn worktree_text(&self, session: &str, path: &str) -> Result<String, LoomError> {
        let mut url = self
            .base
            .join("/api/sessions/raw")
            .map_err(|e| LoomError::Connection(format!("joining /api/sessions/raw: {e}")))?;
        url.query_pairs_mut()
            .append_pair("session", session)
            .append_pair("path", path);
        let mut req = self.http.get(url);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        let mut resp = req
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| LoomError::Connection(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            return Err(LoomError::Api {
                status,
                method: "GET",
                path: "/api/sessions/raw".into(),
                message: resp.text().await.unwrap_or_default(),
            });
        }
        const MAX_BYTES: usize = 512 * 1024;
        let mut bytes = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| LoomError::Connection(e.to_string()))?
        {
            if bytes.len() + chunk.len() > MAX_BYTES {
                return Err(LoomError::Decode {
                    path: "/api/sessions/raw".into(),
                    detail: "file exceeds 512 KiB preview limit".into(),
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        String::from_utf8(bytes).map_err(|e| LoomError::Decode {
            path: "/api/sessions/raw".into(),
            detail: format!("file is not UTF-8 text: {e}"),
        })
    }

    /// `branches.list` — every branch loom tracks, including its base ref.
    /// The Land action uses that recorded ref as its default upstream.
    pub async fn list_branches(&self) -> Result<Vec<serde_json::Value>, LoomError> {
        self.op("/api/branches/list", &serde_json::json!({})).await
    }

    /// `repos.list` — the managed-repo allowlist (slug → checkout path). The
    /// launch sheet resolves an `owner/name` slug to a checkout for
    /// `repos.branches`.
    pub async fn list_repos(&self) -> Result<Vec<crate::loom::RepoView>, LoomError> {
        self.op("/api/repos/list", &serde_json::json!({})).await
    }

    /// `repos.branches` — the local branches of a repository checkout and
    /// which one the primary checkout (the main worktree) currently has
    /// checked out. `cwd` is a server-side path (the checkout itself resolves
    /// to its canonical repo root); loopback trust or a user PAT grants it,
    /// an agent session token does not.
    pub async fn repo_branches(
        &self,
        cwd: &str,
    ) -> Result<Vec<crate::loom::RepoBranchView>, LoomError> {
        self.op("/api/repos/branches", &serde_json::json!({ "cwd": cwd }))
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

    /// Change an agent-owned ACP composer selector. Loom returns its refreshed
    /// metadata and also broadcasts a `metadata` chat event to other clients.
    pub async fn set_session_config(
        &self,
        id: &str,
        config_id: &str,
        value: serde_json::Value,
    ) -> Result<crate::loom::AcpMetadataView, LoomError> {
        #[derive(serde::Deserialize)]
        struct ConfigReply {
            metadata: crate::loom::AcpMetadataView,
        }
        let reply: ConfigReply = self
            .op(
                "/api/sessions/config/set",
                &serde_json::json!({ "session": id, "config_id": config_id, "value": value }),
            )
            .await?;
        Ok(reply.metadata)
    }

    /// `sessions.files` — tracked and unignored files in this session's
    /// server-side worktree, ranked by loom for @-mention completion.
    pub async fn session_files(&self, id: &str, query: &str) -> Result<Vec<String>, LoomError> {
        #[derive(serde::Deserialize)]
        struct FilesReply {
            files: Vec<String>,
        }
        let reply: FilesReply = self
            .op(
                "/api/sessions/files",
                &serde_json::json!({ "session": id, "q": query }),
            )
            .await?;
        Ok(reply.files)
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
    pub async fn send_prompt(
        &self,
        id: &str,
        text: &str,
        files: &[String],
    ) -> Result<(), LoomError> {
        let _: serde_json::Value = self
            .op(
                "/api/sessions/prompt/create",
                &serde_json::json!({ "session": id, "text": text, "files": files, "send_now": true }),
            )
            .await?;
        Ok(())
    }

    /// Answer one ACP tool permission using Loom's user-scoped operation.
    pub async fn answer_permission(
        &self,
        session: &str,
        request_id: &str,
        option_id: &str,
    ) -> Result<(), LoomError> {
        let _: serde_json::Value = self
            .op(
                "/api/sessions/permissions/answer",
                &serde_json::json!({
                    "session": session,
                    "request_id": request_id,
                    "option_id": option_id,
                }),
            )
            .await?;
        Ok(())
    }

    /// Raw Scratch upload keeps file bytes on Loom's host, including when
    /// Arachne is connected over Tailscale to a remote runner.
    pub async fn upload_scratch(
        &self,
        session: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<String, LoomError> {
        let path = "/api/sessions/scratch/write";
        let mut url = self
            .base
            .join(path)
            .map_err(|e| LoomError::Connection(format!("joining {path}: {e}")))?;
        url.query_pairs_mut()
            .append_pair("session", session)
            .append_pair("name", name);
        let mut req = self
            .http
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream");
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        let resp = req
            .body(bytes)
            .timeout(Duration::from_secs(120))
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
                path: path.into(),
                message,
            });
        }
        let value: serde_json::Value = resp.json().await.map_err(|e| LoomError::Decode {
            path: path.into(),
            detail: e.to_string(),
        })?;
        value
            .get("path")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| LoomError::Decode {
                path: path.into(),
                detail: "upload response has no path".into(),
            })
    }

    /// Queue an ACP request behind the current turn. Integration and landing
    /// must not interrupt work already in progress in a coordinator thread.
    pub async fn queue_prompt(&self, id: &str, text: &str) -> Result<(), LoomError> {
        let _: serde_json::Value = self
            .op(
                "/api/sessions/prompt/create",
                &serde_json::json!({ "session": id, "text": text, "send_now": false }),
            )
            .await?;
        Ok(())
    }

    /// Start Loom's durable next-turn queue after a lost runtime is restored.
    pub async fn send_queued_prompt(&self, id: &str) -> Result<(), LoomError> {
        let _: serde_json::Value = self
            .op(
                "/api/sessions/prompt/create",
                &serde_json::json!({ "session": id, "text": "", "force_queued": true }),
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

    /// `sessions.adopt` — resume an orphaned session in its existing checkout.
    pub async fn adopt(&self, id: &str) -> Result<crate::loom::SessionView, LoomError> {
        self.op("/api/sessions/adopt", &serde_json::json!({ "session": id }))
            .await
    }

    /// Resume a lost runtime only when an action needs to deliver work to it.
    /// Another client may win the adoption race; in that case its running
    /// session is equally ready to receive the action.
    pub async fn resume_if_orphaned(
        &self,
        id: &str,
    ) -> Result<crate::loom::SessionView, LoomError> {
        let view = self.get_session(id).await?;
        if view.status != "orphaned" {
            return Ok(view);
        }
        match self.adopt(id).await {
            Ok(view) => Ok(view),
            Err(error @ LoomError::Api { status: 409, .. }) => {
                let current = self.get_session(id).await?;
                if current.status == "running" {
                    Ok(current)
                } else {
                    Err(error)
                }
            }
            Err(error) => Err(error),
        }
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
    /// leader chat as a durable `topic` (survives archive and restarts) and
    /// to record integration/landing outcomes. `note` is the one-line
    /// reason shown next to the value in the UI.
    pub async fn set_tag(
        &self,
        session: &str,
        key: &str,
        value: &str,
        note: &str,
    ) -> Result<(), LoomError> {
        let _: serde_json::Value = self
            .op(
                "/api/sessions/tags/set",
                &serde_json::json!({
                    "session": session,
                    "key": key,
                    "value": value,
                    "note": note,
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

    /// `repos.worktrees.ensure` — materialize a branch's worktree when it is
    /// gone (archive removes it, branch survives), idempotently. The
    /// checkout-recovery half of “open this code”: server-side, no agent,
    /// no session resurrection.
    pub async fn ensure_worktree(
        &self,
        cwd: &str,
        branch: &str,
    ) -> Result<crate::loom::RepoWorktreeView, LoomError> {
        self.op(
            "/api/repos/worktrees/ensure",
            &serde_json::json!({ "cwd": cwd, "branch": branch }),
        )
        .await
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
        result.push(if chunk.len() > 1 {
            CHARS[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        result.push(if chunk.len() > 2 {
            CHARS[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    result
}

#[cfg(test)]
mod image_tests {
    use super::*;

    #[test]
    fn validates_relative_paths_and_image_signatures() {
        assert!(validate_image_path("images/screenshot.png").is_ok());
        for path in [
            "",
            "/etc/passwd",
            "../secret.png",
            "a/./b.png",
            "a//b.png",
            "C:\\x.png",
        ] {
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

#[cfg(test)]
mod resume_tests {
    use super::LoomClient;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn session(status: &str) -> serde_json::Value {
        serde_json::json!({
            "id": "session", "status": status, "profile": "default", "class": "interactive",
            "origin": "user", "agent_kind": "codex", "model": "", "effort": "",
            "protocol": "acp", "work_dir": "/tmp/work", "term_session": "runtime",
            "turn_count": 0, "created_by": null, "created_at": "now", "last_activity_at": "now",
            "branch": { "id": "branch", "branch": "topic", "name": "topic", "title": "Topic",
                "repo_root": "/tmp", "tags": [] }
        })
    }

    #[tokio::test]
    async fn resumes_only_orphaned_sessions() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, body) in [
                ("/api/sessions/get", session("orphaned")),
                ("/api/sessions/adopt", session("running")),
                ("/api/sessions/get", session("running")),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 2048];
                let count = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..count])
                    .starts_with(&format!("POST {path} ")));
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = LoomClient::new(&format!("http://{addr}/"), None).unwrap();
        assert_eq!(
            client.resume_if_orphaned("session").await.unwrap().status,
            "running"
        );
        assert_eq!(
            client.resume_if_orphaned("session").await.unwrap().status,
            "running"
        );
        server.await.unwrap();
    }
}
