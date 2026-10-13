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

// Versioned SHA-256 fingerprint of the complete authorization envelope. Do
// not use RandomState/DefaultHasher: its algorithm is not a stable wire format.
fn request_key(destination: &str, text: &str, payload: &serde_json::Value) -> String {
    let envelope = serde_json::to_vec(&serde_json::json!([destination, text.trim(), payload]))
        .expect("request envelope serialization");
    format!("arachne-request-v1-{}", sha256(&envelope))
}

fn sha256(input: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut bytes = input.to_vec();
    bytes.push(0x80);
    while bytes.len() % 64 != 56 {
        bytes.push(0);
    }
    bytes.extend_from_slice(&(input.len() as u64 * 8).to_be_bytes());
    for chunk in bytes.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(word.try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let t2 = s0.wrapping_add((a & b) ^ (a & c) ^ (b & c));
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (value, add) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *value = value.wrapping_add(add);
        }
    }
    state.iter().map(|v| format!("{v:08x}")).collect()
}

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
    #[error("request delivery to {session_id} is uncertain: {detail}. The request may already have been delivered. Reconcile the destination inbox/queue before retrying; do not issue a new authorization")]
    DeliveryUncertain {
        session_id: String,
        path: String,
        request_key: Option<String>,
        detail: String,
    },
}

impl LoomError {
    /// True when the failure is "cannot reach loom at all" rather than "loom
    /// answered with an error". Drives the connection indicator.
    pub fn is_unreachable(&self) -> bool {
        matches!(self, LoomError::Connection(_))
    }

    fn after_dispatch(mut self, session_id: &str, path: &str, key: Option<&str>) -> Self {
        // A timeout, invalid receipt or server failure can follow a successful
        // durable write, including a delivery-layer 4xx after append. Only
        // authentication/access denial occurs before the operation runs.
        // The ACP prompt API has no idempotency operand.
        if let Self::DeliveryUncertain { request_key, .. } = &mut self {
            if request_key.is_none() {
                *request_key = key.map(str::to_owned);
            }
            return self;
        }
        if matches!(
            self,
            Self::Api {
                status: 401 | 403,
                ..
            }
        ) {
            return self;
        }
        Self::DeliveryUncertain {
            session_id: session_id.into(),
            path: path.into(),
            request_key: key.map(str::to_owned),
            detail: self.to_string(),
        }
    }
}

pub struct LoomClient {
    http: reqwest::Client,
    base: reqwest::Url,
    token: Option<String>,
    selected_action: Option<tokio_util::sync::CancellationToken>,
}

impl LoomClient {
    /// Host of the authoritative Loom server. For the bootstrap deployment
    /// this is also the runner's SSH host; no host-side path is ever treated
    /// as a local Mac path when the connection is remote.
    pub fn server_url(&self) -> String {
        self.base.as_str().trim_end_matches('/').to_owned()
    }

    pub fn server_host(&self) -> Option<&str> {
        self.base.host_str()
    }

    pub fn new(base_url: &str, token: Option<String>) -> Result<Self, LoomError> {
        let normalized = crate::routing::normalize_server_url(base_url)?;
        let base = reqwest::Url::parse(&normalized)
            .map_err(|e| LoomError::Connection(format!("invalid base URL {base_url:?}: {e}")))?;
        // NOTE: no client-wide timeout. A `timeout()` applies per-request
        // including streaming bodies, which would kill the SSE connection
        // every 60s. Per-request timeouts for REST calls are set below.
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| LoomError::Connection(format!("building HTTP client: {e}")))?;
        Ok(Self {
            http,
            base,
            token,
            selected_action: None,
        })
    }

    /// A private action client keeps the original transport/credentials and a
    /// lease canceled by owning-backend outage or client retirement. It never
    /// looks up a replacement client halfway through selected authorization.
    pub(crate) fn for_selected_action(&self, cancel: tokio_util::sync::CancellationToken) -> Self {
        Self {
            http: self.http.clone(),
            base: self.base.clone(),
            token: self.token.clone(),
            selected_action: Some(cancel),
        }
    }

    pub(crate) async fn op<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Result<T, LoomError> {
        if let Some(cancel) = &self.selected_action {
            let started = std::sync::atomic::AtomicBool::new(false);
            let request = async {
                started.store(true, std::sync::atomic::Ordering::Relaxed);
                self.op_unfenced(path, body).await
            };
            let invalidated = || {
                let error = LoomError::Connection("Selected Loom backend became unavailable or was replaced; refresh before proceeding.".into());
                if started.load(std::sync::atomic::Ordering::Relaxed)
                    && matches!(
                        path,
                        "/api/sessions/launch"
                            | "/api/sessions/adopt"
                            | "/api/sessions/send"
                            | "/api/sessions/prompt/create"
                            | "/api/channels/messages/create"
                    )
                {
                    let value = serde_json::to_value(body).unwrap_or_default();
                    let destination = value["session"]
                        .as_str()
                        .or_else(|| value["channel"].as_str())
                        .unwrap_or("new reviewer");
                    error.after_dispatch(destination, path, None)
                } else {
                    error
                }
            };
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(invalidated()),
                result = request => {
                    if cancel.is_cancelled() {
                        return Err(invalidated());
                    }
                    result
                }
            }
        } else {
            self.op_unfenced(path, body).await
        }
    }

    async fn op_unfenced<T: DeserializeOwned>(
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

    /// Read bounded original history without changing a user's read marker.
    pub async fn channel_results(
        &self,
        channel: &str,
        after: i64,
    ) -> Result<Vec<crate::change::ResultMessage>, LoomError> {
        self.op(
            "/api/channels/messages/list",
            &serde_json::json!({
                "channel": channel, "after": after, "limit": 500, "kinds": ["result"], "peek": true
            }),
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

    /// Artifact page keys are canonical branch ids or legacy session keys.
    /// sessions.get resolves either when a session exists. A terminal or
    /// sessionless branch still owns artifacts, so only its 404 falls back
    /// to the branch API; authentication and connection failures propagate.
    pub async fn linked_artifact(
        &self,
        key: &str,
        name: &str,
    ) -> Result<serde_json::Value, LoomError> {
        match self.get_session(key).await {
            Ok(session) => self.branch_artifact(&session.branch.id, name).await,
            Err(LoomError::Api { status: 404, .. }) => self.branch_artifact(key, name).await,
            Err(error) => Err(error),
        }
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

    /// Repository-shared versioned artifact (`repo: true`) — the scope the
    /// per-repo `arachne-projects` store lives in. `branch` is only the
    /// access context (any branch id of the repo resolves it; loom keys
    /// repo-shared artifacts on the branch's canonical `repo_root`), so the
    /// same store is reachable from any session of the repository.
    pub async fn repo_artifact(
        &self,
        branch: &str,
        name: &str,
    ) -> Result<serde_json::Value, LoomError> {
        self.op(
            "/api/artifacts/get",
            &serde_json::json!({ "branch": branch, "name": name, "repo": true }),
        )
        .await
    }

    /// Write the repository-shared artifact (`repo: true`), revision-checked
    /// like every Arachne store (`base_rev` optimistic concurrency).
    pub async fn write_repo_artifact(
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
                "base_rev": base_rev, "repo": true,
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

    /// New chat input reopens a landed conversation, while bounded acceptance
    /// records remain as historical evidence. Clear only after delivery succeeds.
    pub async fn send_chat_input(
        &self,
        view: &crate::loom::SessionView,
        text: &str,
        files: &[String],
    ) -> Result<(), LoomError> {
        if view.protocol == "terminal" {
            self.send_text(&view.id, text, true).await?;
        } else {
            self.send_prompt(&view.id, text, files).await?;
        }
        if view
            .branch
            .tags
            .iter()
            .any(|tag| tag.key == "integration_result")
        {
            // The message was delivered: a cleanup failure must not invite a
            // retry that would send it twice. Timestamp-based UI gating also
            // prevents a stale landed badge when the server rejects cleanup.
            if let Err(error) = self.clear_tag(&view.id, "integration_result").await {
                eprintln!("could not clear landed tag after chat input: {error}");
            }
        }
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
        const PATH: &str = "/api/sessions/prompt/create";
        let reply: serde_json::Value = self
            .op(
                PATH,
                &serde_json::json!({ "session": id, "text": text, "send_now": false }),
            )
            .await
            .map_err(|e| e.after_dispatch(id, PATH, None))?;
        // queued=false is valid: an idle ACP runtime can start immediately.
        // Neither acknowledgment says the requested operation has completed.
        if !reply["queued"].is_boolean()
            || !reply.as_object().is_some_and(|v| v.contains_key("turn"))
            || !(reply["turn"].is_null() || reply["turn"].as_i64().is_some_and(|v| v >= 0))
        {
            return Err(LoomError::Decode {
                path: PATH.into(),
                detail:
                    "prompt receipt must contain queued boolean and turn null/nonnegative integer"
                        .into(),
            }
            .after_dispatch(id, PATH, None));
        }
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
        const PATH: &str = "/api/channels/messages/create";
        let reply: serde_json::Value = self
            .op(
                PATH,
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
            .map_err(|e| e.after_dispatch(destination_id, PATH, Some(idempotency_key)))?;
        if !reply["id"].as_str().is_some_and(|id| !id.trim().is_empty())
            || reply["channel_id"] != destination_id
            || reply["kind"] != "message"
            || reply["body"] != body.trim()
            || reply["payload"] != payload
        {
            return Err(LoomError::Decode {
                path: PATH.into(),
                detail: "message receipt did not match destination, kind, body and payload or had no durable id".into(),
            }.after_dispatch(destination_id, PATH, Some(idempotency_key)));
        }
        Ok(reply)
    }

    /// Install an actual session delivery route and verify the server did not
    /// silently subscribe the human caller or fall back to observe mode.
    pub async fn ensure_delivery_subscription(
        &self,
        channel: &str,
        session: &str,
    ) -> Result<(), LoomError> {
        const PATH: &str = "/api/channels/subscription/set";
        let reply: serde_json::Value = self
            .op(
                PATH,
                &serde_json::json!({
                    "channel": channel, "session": session, "mode": "deliver"
                }),
            )
            .await?;
        if reply["channel_id"] != channel
            || reply["subject_kind"] != "session"
            || reply["subject_id"] != session
            || reply["mode"] != "deliver"
        {
            return Err(LoomError::Decode { path: PATH.into(), detail: "delivery subscription did not match requested channel, session subject and deliver mode".into() });
        }
        Ok(())
    }

    /// Verify the own-channel route before terminal request dispatch. ACP
    /// requests use the prompt API and need no channel subscription.
    pub async fn ensure_request_route(
        &self,
        view: &crate::loom::SessionView,
    ) -> Result<(), LoomError> {
        if view.protocol == "terminal" {
            self.ensure_delivery_subscription(&view.id, &view.id)
                .await?;
        }
        Ok(())
    }

    /// Terminal requests are durable inbox messages, never input into an
    /// active shell. Identical authorization content keeps the same retry key
    /// across client restarts. Callers must include verified source/target
    /// revisions in the payload to distinguish later changed authorizations.
    /// ACP requests queue behind the current turn; uncertain ACP receipt must
    /// be reconciled, because that API has no retry-safe identity.
    pub async fn deliver_request(
        &self,
        view: &crate::loom::SessionView,
        text: &str,
        payload: serde_json::Value,
    ) -> Result<(), LoomError> {
        let key = request_key(&view.id, text, &payload);
        self.deliver_request_with_key(view, text, payload, &key)
            .await
    }

    /// Use a caller-owned durable action identity when available. Reusing a
    /// key with different authorization content is rejected by receipt checks.
    pub async fn deliver_request_with_key(
        &self,
        view: &crate::loom::SessionView,
        text: &str,
        payload: serde_json::Value,
        key: &str,
    ) -> Result<(), LoomError> {
        self.ensure_request_route(view).await?;
        if view.protocol == "terminal" {
            self.send_to_thread(&view.id, text, payload, key).await?;
        } else {
            self.resume_if_orphaned(&view.id).await?;
            self.queue_prompt(&view.id, text).await?;
        }
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
        // Older Loom servers ignore launch_guidance. Seed the role in the
        // durable first turn as well, then retain the concise user objective
        // in branch metadata. Guidance-aware servers still receive the field
        // for child inheritance and restarts.
        let mut request = input.clone();
        if let Some(guidance) = &input.launch_guidance {
            request.goal = Some(format!(
                "{}\n\n## Arachne role guidance\n\n{}",
                input.goal.as_deref().unwrap_or_default(),
                guidance.root
            ));
        }
        if let Some(initialization) = &input.initialization_prompt {
            request.goal = Some(format!(
                "{initialization}\n\n{}",
                request.goal.as_deref().unwrap_or_default()
            ));
        }
        let mut launched: crate::loom::SessionView =
            self.op("/api/sessions/launch", &request).await?;
        if input.launch_guidance.is_some() || input.initialization_prompt.is_some() {
            // sessions.update rewrites goal.txt, which terminal agents may
            // not have read yet. branches.update changes metadata without
            // replacing the durable launch prompt (including profile notes).
            // Metadata cleanup is not a launch failure: never invite a retry
            // that would create a duplicate already-running worker.
            if let Ok(branch) = self
                .op::<crate::loom::BranchSummaryView>(
                    "/api/branches/update",
                    &serde_json::json!({
                        "branch": launched.branch.id,
                        // A missing canonical tag on a NEW Track must never
                        // masquerade as a legacy scratch destination.
                        "goal": match &input.initialization_prompt {
                            Some(initialization) => format!("{}\n\n{}", input.goal.as_deref().unwrap_or_default(), initialization.lines().next().unwrap_or_default()),
                            None => input.goal.clone().unwrap_or_default(),
                        },
                    }),
                )
                .await
            {
                launched.branch = branch;
            }
        }
        Ok(launched)
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

    /// `sessions.tags.delete` — clear a tag, returning that axis to its
    /// calm/default (absent) state. The loud keys (`attention`, `triage`)
    /// carry the session's needs-a-person flag: clearing them is how a
    /// person dismisses attention without opening the thread. Idempotent —
    /// loom's clear is a no-op when the key is absent.
    pub async fn clear_tag(&self, session: &str, key: &str) -> Result<(), LoomError> {
        let _: serde_json::Value = self
            .op(
                "/api/sessions/tags/delete",
                &serde_json::json!({
                    "session": session,
                    "key": key,
                    "by": "arachne",
                }),
            )
            .await?;
        Ok(())
    }

    /// `sessions.archive` — tear down terminal + worktree, keep the branch.
    pub async fn archive(&self, id: &str) -> Result<(), LoomError> {
        let _: serde_json::Value = self
            .op(
                "/api/sessions/archive",
                &serde_json::json!({ "session": id }),
            )
            .await?;
        Ok(())
    }

    /// `sessions.recover` — rebuild an archived checkout and resume its agent.
    pub async fn recover(&self, id: &str) -> Result<crate::loom::SessionView, LoomError> {
        self.op(
            "/api/sessions/recover",
            &serde_json::json!({ "session": id }),
        )
        .await
    }

    /// Recover when the fetched session is archived; leave already-live runtimes alone.
    /// The status check and recovery are separate requests, so another client can
    /// restore the session between them.
    pub async fn unarchive(&self, id: &str) -> Result<crate::loom::SessionView, LoomError> {
        let current = self.get_session(id).await?;
        if current.status == "archived" {
            self.recover(id).await
        } else {
            Ok(current)
        }
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
    /// Synthetic connection/unavailable and connection/resync frames expose
    /// transport transitions even when no server-side event is published.
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
            let mut reported_unavailable = false;
            loop {
                let mut req = http.get(url.clone());
                if let Some(token) = &token {
                    req = req.bearer_auth(token);
                }
                let response = tokio::select! {
                    _ = tx.closed() => return,
                    response = req.send() => response,
                };
                let error = match response {
                    Ok(resp) if resp.status().is_success() => {
                        backoff = Duration::from_millis(250);
                        reported_unavailable = false;
                        // Reconcile even when no session/layout events were published
                        // during the outage. Headers suffice; do not await a frame.
                        if tx
                            .send(EventFrame {
                                topic: "connection".into(),
                                event: "resync".into(),
                                data: serde_json::Value::Null,
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                        let mut stream = resp.bytes_stream();
                        let mut buf = String::new();
                        loop {
                            match tokio::select! {
                                _ = tx.closed() => return,
                                frame = stream.next() => frame,
                            } {
                                Some(Ok(chunk)) => {
                                    buf.push_str(&String::from_utf8_lossy(&chunk));
                                    while let Some(pos) = buf.find("\n\n") {
                                        let event: String = buf.drain(..pos + 2).collect();
                                        for line in event.lines() {
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
                                Some(Err(error)) => {
                                    break LoomError::Connection(format!(
                                        "event stream failed: {error}"
                                    ))
                                }
                                None => {
                                    break LoomError::Connection(
                                        "event stream closed; reconnecting".into(),
                                    )
                                }
                            }
                        }
                    }
                    Ok(resp) => LoomError::Api {
                        status: resp.status().as_u16(),
                        method: "GET",
                        path: "/api/events/stream".into(),
                        message: format!("event subscription rejected: {}", resp.status()),
                    },
                    Err(error) => LoomError::Connection(error.to_string()),
                };
                // ReceiverStream remains open across retries. Surface the first
                // failure immediately rather than waiting for REST or a new event.
                if !reported_unavailable {
                    if tx.send(EventFrame {
                        topic: "connection".into(),
                        event: "unavailable".into(),
                        data: serde_json::json!({"message": error.to_string(), "unreachable": error.is_unreachable()}),
                    }).await.is_err() { return; }
                    reported_unavailable = true;
                }
                tokio::select! {
                    _ = tx.closed() => return,
                    _ = tokio::time::sleep(backoff) => {},
                }
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
mod request_key_tests {
    use super::*;

    #[test]
    fn fingerprint_matches_sha256_vectors_and_changed_authorizations() {
        for (input, expected) in [
            (
                "",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                "abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
        ] {
            assert_eq!(sha256(input.as_bytes()), expected);
        }
        assert_eq!(
            sha256(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        let payload = serde_json::json!({"source_revision":"sha1"});
        let key = request_key("session", "authorization", &payload);
        assert_eq!(key, request_key("session", " authorization ", &payload));
        assert_ne!(key, request_key("other", "authorization", &payload));
        assert_ne!(key, request_key("session", "different strategy", &payload));
        assert_ne!(
            key,
            request_key(
                "session",
                "authorization",
                &serde_json::json!({"source_revision":"sha2"})
            )
        );
        assert!(key.len() <= 255);
    }
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

    async fn artifact_server(
        key: &'static str,
        session_reply: (&'static str, serde_json::Value),
        artifact_reply: Option<(&'static str, &'static str, serde_json::Value)>,
    ) -> (LoomClient, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut replies = vec![(
            "/api/sessions/get",
            serde_json::json!({"session": key}),
            session_reply.0,
            session_reply.1,
        )];
        if let Some((branch, status, body)) = artifact_reply {
            replies.push((
                "/api/artifacts/get",
                serde_json::json!({"branch": branch, "name": "track-brief", "repo": false}),
                status,
                body,
            ));
        }
        let server = tokio::spawn(async move {
            for (path, expected, status, body) in replies {
                let (mut socket, _) =
                    tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut bytes = Vec::new();
                let (header_end, length) = loop {
                    let mut chunk = [0u8; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                        let length: usize = header
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse()
                            .unwrap();
                        break (end + 4, length);
                    }
                };
                while bytes.len() < header_end + length {
                    let mut chunk = [0u8; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                }
                assert!(String::from_utf8_lossy(&bytes[..header_end])
                    .starts_with(&format!("POST {path} ")));
                let request: serde_json::Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                assert_eq!(request, expected);
                let body = body.to_string();
                socket.write_all(format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len(),
                ).as_bytes()).await.unwrap();
            }
        });
        (
            LoomClient::new(&format!("http://{addr}/api/"), None).unwrap(),
            server,
        )
    }

    #[tokio::test]
    async fn linked_artifact_reads_legacy_archived_session_branch() {
        let artifact =
            serde_json::json!({"meta": {"title": "Track brief"}, "content": "Correct branch"});
        let (client, server) = artifact_server(
            "linked-worker",
            ("200 OK", session("archived")),
            Some(("branch", "200 OK", artifact.clone())),
        )
        .await;
        assert_eq!(
            client
                .linked_artifact("linked-worker", "track-brief")
                .await
                .unwrap(),
            artifact
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn linked_artifact_reads_canonical_track_branch_key() {
        // artifacts.url returns the branch id even though the live session differs.
        let mut track = session("running");
        track["id"] = "fwn0ypj5".into();
        track["branch"]["id"] = "99ne4k2x".into();
        let artifact = serde_json::json!({"content": "Canonical Track brief"});
        let (client, server) = artifact_server(
            "99ne4k2x",
            ("200 OK", track),
            Some(("99ne4k2x", "200 OK", artifact.clone())),
        )
        .await;
        assert_eq!(
            client
                .linked_artifact("99ne4k2x", "track-brief")
                .await
                .unwrap(),
            artifact
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn linked_artifact_reads_canonical_branch_without_active_session() {
        // require_session cannot resolve a branch whose session is done/error/archived,
        // but require_branch used by artifacts.get can still resolve it.
        let artifact = serde_json::json!({"content": "Durable Track brief"});
        let (client, server) = artifact_server(
            "99ne4k2x",
            (
                "404 Not Found",
                serde_json::json!({"error": "session not found"}),
            ),
            Some(("99ne4k2x", "200 OK", artifact.clone())),
        )
        .await;
        assert_eq!(
            client
                .linked_artifact("99ne4k2x", "track-brief")
                .await
                .unwrap(),
            artifact
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn linked_artifact_preserves_missing_artifact_error() {
        let (client, server) = artifact_server(
            "linked-worker",
            ("200 OK", session("archived")),
            Some((
                "branch",
                "404 Not Found",
                serde_json::json!({"error": "artifact not found"}),
            )),
        )
        .await;
        let error = client
            .linked_artifact("linked-worker", "track-brief")
            .await
            .unwrap_err();
        assert!(
            matches!(error, super::LoomError::Api { status: 404, ref message, .. } if message == "artifact not found")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn linked_artifact_does_not_fall_back_on_permission_errors() {
        let (client, server) = artifact_server(
            "99ne4k2x",
            (
                "403 Forbidden",
                serde_json::json!({"error": "access denied"}),
            ),
            None,
        )
        .await;
        let error = client
            .linked_artifact("99ne4k2x", "track-brief")
            .await
            .unwrap_err();
        assert!(
            matches!(error, super::LoomError::Api { status: 403, ref message, .. } if message == "access denied")
        );
        server.await.unwrap();
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

#[cfg(test)]
mod unarchive_tests {
    use super::{LoomClient, LoomError};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn session(status: &str) -> serde_json::Value {
        let mut view: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/launch-response.json")).unwrap();
        view["status"] = status.into();
        view
    }

    async fn mock_server(
        replies: Vec<(&'static str, &'static str, serde_json::Value)>,
    ) -> (LoomClient, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (path, status, body) in replies {
                let (mut socket, _) =
                    tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                        .await
                        .expect("timed out waiting for the expected unarchive request")
                        .unwrap();
                let mut bytes = Vec::new();
                let (header_end, length) = loop {
                    let mut chunk = [0u8; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                        let length: usize = header
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse()
                            .unwrap();
                        break (end + 4, length);
                    }
                };
                while bytes.len() < header_end + length {
                    let mut chunk = [0u8; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                }
                let header = String::from_utf8_lossy(&bytes[..header_end]);
                assert!(header.starts_with(&format!("POST {path} ")), "{header}");
                let request: serde_json::Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                assert_eq!(
                    request,
                    serde_json::json!({"session": "decode-probe-session"})
                );
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = LoomClient::new(&format!("http://{addr}/"), None).unwrap();
        (client, server)
    }

    #[tokio::test]
    async fn archived_session_returns_the_recovered_checkout_and_runtime() {
        let mut archived = session("archived");
        archived["worktree_present"] = false.into();
        archived["term_session"] = "".into();
        let mut recovered = session("running");
        recovered["work_dir"] = "/tmp/restored-checkout".into();
        recovered["worktree_present"] = true.into();
        recovered["term_session"] = "restored-runtime".into();
        let (client, server) = mock_server(vec![
            ("/api/sessions/get", "200 OK", archived),
            ("/api/sessions/recover", "200 OK", recovered.clone()),
        ])
        .await;

        let view = client.unarchive("decode-probe-session").await.unwrap();
        server.await.unwrap();
        assert_eq!(view.id, "decode-probe-session");
        assert_eq!(view.status, "running");
        assert_eq!(view.work_dir, "/tmp/restored-checkout");
        assert!(view.worktree_present);
        assert_eq!(view.term_session, "restored-runtime");
        assert_eq!(view.branch.id, recovered["branch"]["id"].as_str().unwrap());
    }

    #[tokio::test]
    async fn already_live_sessions_are_returned_without_recovery_or_restart() {
        let statuses = ["running", "idle", "orphaned"];
        let (client, server) = mock_server(
            statuses
                .iter()
                .map(|status| ("/api/sessions/get", "200 OK", session(status)))
                .collect(),
        )
        .await;

        for status in statuses {
            let expected = session(status);
            let view = client.unarchive("decode-probe-session").await.unwrap();
            assert_eq!(view.status, status);
            assert_eq!(
                view.term_session,
                expected["term_session"].as_str().unwrap()
            );
            assert_eq!(view.work_dir, expected["work_dir"].as_str().unwrap());
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn recovery_api_failure_is_propagated() {
        let (client, server) = mock_server(vec![
            ("/api/sessions/get", "200 OK", session("archived")),
            (
                "/api/sessions/recover",
                "500 Internal Server Error",
                serde_json::json!({"error": "checkout recovery failed"}),
            ),
        ])
        .await;

        let error = client.unarchive("decode-probe-session").await.unwrap_err();
        server.await.unwrap();
        match error {
            LoomError::Api {
                status,
                method,
                path,
                message,
            } => {
                assert_eq!(status, 500);
                assert_eq!(method, "POST");
                assert_eq!(path, "/api/sessions/recover");
                assert_eq!(message, "checkout recovery failed");
            }
            error => panic!("expected recovery API error, got {error:?}"),
        }
    }
}

#[cfg(test)]
mod launch_tests {
    use super::LoomClient;
    use crate::loom::{SessionLaunchGuidance, SessionsLaunchInput};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn launch_with_legacy_server(
        guidance: SessionLaunchGuidance,
        metadata_fails: bool,
    ) -> (crate::loom::SessionView, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let goal_file = std::env::temp_dir().join(format!(
            "arachne-launch-prompt-{}-{}.txt",
            std::process::id(),
            addr.port()
        ));
        let server_goal_file = goal_file.clone();
        let server = tokio::spawn(async move {
            for index in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let (header_end, length) = loop {
                    let mut chunk = [0u8; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                        let length: usize = header
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse()
                            .unwrap();
                        break (end + 4, length);
                    }
                };
                while bytes.len() < header_end + length {
                    let mut chunk = [0u8; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                }
                let header = String::from_utf8_lossy(&bytes[..header_end]);
                let request: serde_json::Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                let mut reply: serde_json::Value =
                    serde_json::from_str(include_str!("../tests/fixtures/launch-response.json"))
                        .unwrap();
                let status;
                if index == 0 {
                    assert!(header.starts_with("POST /api/sessions/launch "));
                    // Simulate a server that ignores the unknown guidance field.
                    assert!(request["goal"]
                        .as_str()
                        .unwrap()
                        .starts_with("Fix the editor"));
                    assert_eq!(request["base"], "main");
                    // Loom writes the goal with launch-profile instructions.
                    // A terminal agent may read it only after launch returns.
                    std::fs::write(
                        &server_goal_file,
                        format!(
                            "Profile instructions\n\n{}",
                            request["goal"].as_str().unwrap()
                        ),
                    )
                    .unwrap();
                    reply["branch"]["goal"] = request["goal"].clone();
                    status = "200 OK";
                } else {
                    assert_eq!(request["goal"], "Fix the editor");
                    if metadata_fails {
                        reply = serde_json::json!({"error":"metadata temporarily unavailable"});
                        status = "500 Internal Server Error";
                    } else {
                        reply["branch"]["goal"] = request["goal"].clone();
                        if header.starts_with("POST /api/sessions/update ") {
                            // Match Loom's session-update side effect so a
                            // regression loses guidance just as the agent does.
                            std::fs::write(&server_goal_file, request["goal"].as_str().unwrap())
                                .unwrap();
                        } else {
                            assert!(header.starts_with("POST /api/branches/update "));
                            assert_eq!(request["branch"], "decode-probe-branch");
                            reply = reply["branch"].clone();
                        }
                        status = "200 OK";
                    }
                }
                let body = reply.to_string();
                let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = LoomClient::new(&format!("http://{addr}/"), None).unwrap();
        let launched = client
            .launch(&SessionsLaunchInput {
                goal: Some("Fix the editor".into()),
                base: Some("main".into()),
                launch_guidance: Some(guidance),
                ..Default::default()
            })
            .await
            .unwrap();
        server.await.unwrap();
        let opening_turn = std::fs::read_to_string(&goal_file).unwrap();
        std::fs::remove_file(goal_file).unwrap();
        (launched, opening_turn)
    }

    #[tokio::test]
    async fn new_track_preserves_delegation_prompt_after_metadata_cleanup() {
        let guidance = crate::loom::topic_launch_guidance();
        let (launched, opening_turn) = launch_with_legacy_server(guidance.clone(), false).await;
        assert_eq!(launched.branch.goal, "Fix the editor");
        assert!(opening_turn.starts_with("Profile instructions\n\nFix the editor"));
        assert!(opening_turn.contains(&guidance.root));
        assert!(opening_turn.contains("Aggressively delegate substantial implementation"));
        assert!(opening_turn.contains("loom sessions launch"));
    }

    #[tokio::test]
    async fn worker_and_reviewer_prompts_survive_metadata_cleanup() {
        for guidance in [
            crate::loom::worker_launch_guidance(),
            crate::loom::reviewer_launch_guidance(),
        ] {
            let (launched, opening_turn) = launch_with_legacy_server(guidance.clone(), false).await;
            assert_eq!(launched.branch.goal, "Fix the editor");
            assert!(opening_turn.contains(&guidance.root));
            assert!(opening_turn.contains("Profile instructions"));
        }
    }

    #[tokio::test]
    async fn metadata_failure_does_not_fail_or_repeat_a_completed_launch() {
        let guidance = crate::loom::topic_launch_guidance();
        let (launched, opening_turn) = launch_with_legacy_server(guidance.clone(), true).await;
        assert_eq!(launched.id, "decode-probe-session");
        assert!(launched.branch.goal.contains(&guidance.root));
        assert!(opening_turn.contains(&guidance.root));
    }
}

#[cfg(test)]
mod stream_transport_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn next(rx: &mut mpsc::Receiver<EventFrame>) -> EventFrame {
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn failed_connect_and_stream_close_signal_unavailable_and_quiet_recovery_signals_resync()
    {
        let reserved = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = reserved.local_addr().unwrap();
        drop(reserved);
        let client = LoomClient::new(&format!("http://{addr}"), None).unwrap();
        let mut rx = client.subscribe(&["layout".into()]).await.unwrap();
        let failed = next(&mut rx).await;
        assert_eq!(
            (&*failed.topic, &*failed.event),
            ("connection", "unavailable")
        );
        assert_eq!(failed.data["unreachable"], true);
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 1024];
            let count = socket.read(&mut bytes).await.unwrap();
            assert!(String::from_utf8_lossy(&bytes[..count])
                .starts_with("GET /api/events/stream?topics=layout "));
            // No session/layout data at all: transport headers alone signal recovery.
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
            socket
        });
        let recovered = next(&mut rx).await;
        assert_eq!(
            (&*recovered.topic, &*recovered.event),
            ("connection", "resync")
        );
        let mut socket = server.await.unwrap();
        socket.write_all(b"0\r\n\r\n").await.unwrap();
        let lost = next(&mut rx).await;
        assert_eq!((&*lost.topic, &*lost.event), ("connection", "unavailable"));
        assert_eq!(lost.data["unreachable"], true);
        drop(rx);
    }
}

#[cfg(test)]
mod chat_input_tests {
    use super::LoomClient;
    use crate::loom::{SessionView, TagView};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn send(protocol: &str, landed: bool, send_ok: bool, cleanup_ok: bool) {
        let mut view: SessionView =
            serde_json::from_str(include_str!("../tests/fixtures/launch-response.json")).unwrap();
        view.protocol = protocol.into();
        if landed {
            view.branch.tags.push(TagView {
                key: "integration_result".into(),
                value: "sha".into(),
                note: String::new(),
                set_at: String::new(),
                set_by: "arachne".into(),
            });
        }
        let path = if protocol == "terminal" {
            "/api/sessions/send"
        } else {
            "/api/sessions/prompt/create"
        };
        let mut replies = vec![(path, send_ok)];
        if landed && send_ok {
            replies.push(("/api/sessions/tags/delete", cleanup_ok));
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut bodies = Vec::<serde_json::Value>::new();
            for (path, ok) in replies {
                let (mut socket, _) =
                    tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut bytes = Vec::new();
                let (end, length) = loop {
                    let mut chunk = [0u8; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..end]);
                        assert!(header.starts_with(&format!("POST {path} ")));
                        let length = header
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(|n| n.parse::<usize>().unwrap())
                            })
                            .unwrap();
                        break (end + 4, length);
                    }
                };
                while bytes.len() < end + length {
                    let mut chunk = [0u8; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                }
                bodies.push(serde_json::from_slice(&bytes[end..end + length]).unwrap());
                let (status, body) = if ok {
                    ("200 OK", "{}")
                } else {
                    ("500 Internal Server Error", "{\"error\":\"failed\"}")
                };
                socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
            bodies
        });
        let client = LoomClient::new(&format!("http://{addr}/"), None).unwrap();
        let files = vec!["/scratch/image.png".into()];
        assert_eq!(
            client
                .send_chat_input(&view, "Follow up", &files)
                .await
                .is_ok(),
            send_ok
        );
        let bodies = server.await.unwrap();
        assert_eq!(bodies[0]["session"], view.id);
        assert_eq!(bodies[0]["text"], "Follow up");
        if protocol == "terminal" {
            assert_eq!(bodies[0]["submit"], true);
        } else {
            assert_eq!(bodies[0]["files"], serde_json::json!(files));
        }
        if landed && send_ok {
            assert_eq!(
                bodies[1],
                serde_json::json!({ "session": view.id, "key": "integration_result", "by": "arachne" })
            );
        }
    }

    #[tokio::test]
    async fn new_chat_input_clears_landed_after_delivery_for_both_protocols() {
        for protocol in ["acp", "terminal"] {
            send(protocol, true, true, true).await;
        }
    }

    #[tokio::test]
    async fn failed_delivery_preserves_landed_and_cleanup_failure_does_not_repeat_input() {
        for protocol in ["acp", "terminal"] {
            send(protocol, true, false, true).await;
            send(protocol, true, true, false).await;
            send(protocol, false, true, true).await;
        }
    }
}
