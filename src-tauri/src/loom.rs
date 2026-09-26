//! Wire types for the Loom API views Arachne consumes.
//!
//! Mirrors the SPA's generated types (`crates/loom/frontend/src/api/generated.ts`
//! in the loom repo) and its local block-payload reading (`types.ts`). The
//! journal block `payload` is untyped JSON on the wire — loom stores whatever
//! the ACP adapter produced, keyed by `kind` — so `blocks.rs` narrows it here,
//! client-side, the same way loom's own browser code does.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

// NOTE: warnings about unused helpers (attention, short_name, archive,
// LayoutSnapshot) are suppressed — they're the next UI iteration's surface.

/// A branch tag. The well-known key `attention` carries the session's
/// attention level (`ok | attention | blocked`); absence means calm.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TagView {
    pub key: String,
    pub note: String,
    #[serde(default)]
    pub value: String,
    pub set_at: String,
    pub set_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BranchSummaryView {
    pub id: String,
    pub branch: String,
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub description: String,
    pub repo_root: String,
    #[serde(default)]
    pub tags: Vec<TagView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionPlacementView {
    pub space_id: Option<String>,
    pub space_name: Option<String>,
    pub group_id: Option<String>,
    pub group_name: Option<String>,
    pub group_system_key: Option<String>,
    pub rank: Option<i64>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionSummaryView {
    pub id: String,
    pub status: String,
    pub profile: String,
    pub class: String,
    pub origin: String,
    pub created_by: Option<String>,
    pub created_at: String,
    pub last_activity_at: String,
    pub branch: BranchSummaryView,
    pub placement: Option<SessionPlacementView>,
    pub github_repo: Option<String>,
    pub parent_id: Option<String>,
    pub parent_session_id: Option<String>,
}

impl SessionSummaryView {
    /// The session's attention level from its branch tags, or `"ok"` when no
    /// attention tag is set (absence means calm). `blocked` and `attention`
    /// escalate to the operator; everything else is calm.
    #[allow(dead_code)]
    pub fn attention(&self) -> &str {
        self.branch
            .tags
            .iter()
            .find(|t| t.key == "attention")
            .map(|t| t.value.as_str())
            .unwrap_or("ok")
    }

    /// Short label for the fleet list: branch name without the `weaver/` prefix.
    #[allow(dead_code)]
    pub fn short_name(&self) -> &str {
        self.branch
            .branch
            .strip_prefix("weaver/")
            .unwrap_or(&self.branch.branch)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionView {
    pub id: String,
    pub status: String,
    pub profile: String,
    pub class: String,
    pub origin: String,
    pub agent_kind: String,
    pub model: String,
    pub effort: String,
    pub protocol: String,
    pub work_dir: String,
    pub term_session: String,
    /// The managed `owner/name` slug when the session launched against a
    /// managed repo (delegation's source of truth). `null` for cwd-forked
    /// sessions — delegation then forks from the parent's checkout.
    #[serde(default)]
    pub github_repo: Option<String>,
    pub turn_count: i64,
    pub created_by: Option<String>,
    pub created_at: String,
    pub last_activity_at: String,
    pub branch: BranchSummaryView,
    #[serde(default)]
    pub placement: Option<SessionPlacementView>,
}

// ---------------------------------------------------------------------------
// Session layout (lanes — filing, not topics)
// ---------------------------------------------------------------------------

/// `session_layout.get` — spaces, groups (lanes in Arachne's UI), and
/// placement defaults. Groups carry `session_ids` including archived rows;
/// the fleet summary is the authority on which sessions are active.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionLayoutView {
    pub revision: i64,
    pub spaces: Vec<SessionSpaceView>,
    #[serde(default)]
    pub defaults: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionSpaceView {
    pub id: String,
    pub name: String,
    pub rank: i64,
    pub system_key: Option<String>,
    #[serde(default)]
    pub groups: Vec<SessionGroupView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionGroupView {
    pub id: String,
    pub space_id: String,
    pub name: String,
    pub rank: i64,
    pub system_key: Option<String>,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default)]
    pub session_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// Chat journal
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatBlockView {
    pub seq: i64,
    pub turn: i64,
    pub kind: String,
    pub created_at: String,
    /// Untyped on the wire; narrowed by `kind` — see `blocks.rs`.
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionChatView {
    pub blocks: Vec<ChatBlockView>,
    pub live_turn: Option<i64>,
    pub pending_prompt: Option<String>,
    pub older_cursor: Option<ChatCursorView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatCursorView {
    pub seq: i64,
    pub turn: i64,
}

// ---------------------------------------------------------------------------
// Launch
// ---------------------------------------------------------------------------

/// Request for `POST /api/sessions/launch`. `cwd` is a server-side path;
/// the server ignores it whenever `repo` is present — passing both lets
/// delegation work whether or not the parent launched against a managed
/// repo.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionsLaunchInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Filled from the parent session's work_dir on delegation.
    #[serde(default)]
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    /// The parent session's branch id, for dashboard-launched delegation.
    /// Sets origin=agent + parent_session_id so the child nests under the
    /// parent in the sidebar and inherits its placement group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

// ---------------------------------------------------------------------------
// SSE event stream
// ---------------------------------------------------------------------------

/// One frame from `GET /api/events/stream` — the default `message` event with
/// `{topic, event, data}` JSON. Topics: `layout`, `session:<id>`, `chat:<id>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFrame {
    pub topic: String,
    pub event: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

/// The `layout` topic's `session_layout` event payload: the full fleet list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub struct LayoutSnapshot {
    pub spaces: Vec<serde_json::Value>,
}
