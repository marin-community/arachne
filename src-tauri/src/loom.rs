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
    /// Title ownership for compare-and-swap renames via `sessions.update`
    /// (`user`/`agent`/`derived`/…). Always present on loom's wire; the
    /// default only keeps older payloads decoding.
    #[serde(default)]
    pub title_provenance: String,
    pub repo_root: String,
    #[serde(default)]
    pub github: Option<GithubStatusView>,
    #[serde(default)]
    pub github_pr: Option<i64>,
    #[serde(default)]
    pub tags: Vec<TagView>,
}

/// The small part of Loom's cached PR snapshot the resource strip needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GithubStatusView {
    pub pr_number: i64,
    pub pr_url: String,
    pub pr_state: String,
    pub checks: Option<String>,
    pub review_decision: Option<String>,
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
// Update (topic metadata: title / goal / description)
// ---------------------------------------------------------------------------

/// Request for `POST /api/sessions/update`. `title` renames require the
/// compare-and-swap fence (`expected_title` + `expected_title_provenance`)
/// observed by the caller, so concurrent edits are rejected rather than
/// silently overwritten. `description` is the agent's current-state message
/// shown beside the attention level — for the topic card it's the durable
/// short description a human writes.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionsUpdateInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_title_provenance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
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
// Worktree changes (integration surface)
// ---------------------------------------------------------------------------

/// `sessions.changes` — committed and uncommitted changes against the
/// recorded base ref. Only the totals feed the UI's integrate affordance (the
/// full per-file hunks stay available for a future review pane).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChangeSetView {
    pub version: Option<String>,
    pub base: serde_json::Value,
    pub head_oid: Option<String>,
    #[serde(default)]
    pub totals: ChangeTotalsView,
    #[serde(default)]
    pub files: Vec<serde_json::Value>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChangeTotalsView {
    #[serde(default)]
    pub files: u32,
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
    #[serde(default)]
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// Integration (spec: docs/integration-and-landing.md)
// ---------------------------------------------------------------------------

/// The integration strategies the Integrate split button offers. Wire-safe
/// mirror of the strategies in the spec; the coordinator's skill is the
/// authority on what each one *means* — these are intent constraints, not
/// raw git commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IntegrationStrategy {
    Squash,
    Merge,
    Rebase,
    CherryPick,
    OpenPr,
    Push,
    Ask,
}

impl IntegrationStrategy {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "squash" => Self::Squash,
            "merge" => Self::Merge,
            "rebase" => Self::Rebase,
            "cherry-pick" => Self::CherryPick,
            "open-pr" => Self::OpenPr,
            "push" => Self::Push,
            "ask" | "ask-coordinator" => Self::Ask,
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Squash => "squash",
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::OpenPr => "open-pr",
            Self::Push => "push",
            Self::Ask => "ask",
        }
    }

    /// The label in the split button's dropdown.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Squash => "Squash into topic",
            Self::Merge => "Merge into topic",
            Self::Rebase => "Rebase onto topic",
            Self::CherryPick => "Cherry-pick commits",
            Self::OpenPr => "Open PR into topic",
            Self::Push => "Push topic branch",
            Self::Ask => "Ask coordinator to decide",
        }
    }
}

/// A structured integration request, as sent to the coordinator thread.
/// The spec: "Integration is an agent action with an explicit strategy"
/// — the button and the sentence "Integrate the Zed worker into the
/// Arachne topic" produce the same payload. The coordinator consumes it
/// with the integration skill (skills/integration.md shipped in this
/// repo) and stays responsible for the git operations, validation, and
/// reporting.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct IntegrationRequest {
    pub action: &'static str,
    pub source_session: String,
    pub source_branch: String,
    pub source_work_dir: String,
    pub repo_root: String,
    pub target_session: String,
    pub target_branch: String,
    pub strategy: IntegrationStrategy,
    pub requested_by: &'static str,
}

impl IntegrationRequest {
    /// Render the request as the prompt the coordinator reads. Structured
    /// payload first (machine-parseable), then a human-readable statement
    /// — the skill instructs the agent to treat both as the same intent.
    pub fn to_prompt(&self, topic_name: &str) -> String {
        let strategy = match self.strategy {
            IntegrationStrategy::Squash => "squash".to_string(),
            IntegrationStrategy::Merge => "merge".to_string(),
            IntegrationStrategy::Rebase => "rebase".to_string(),
            IntegrationStrategy::CherryPick => "cherry-pick".to_string(),
            IntegrationStrategy::OpenPr => "open-pr".to_string(),
            IntegrationStrategy::Push => "push".to_string(),
            IntegrationStrategy::Ask => "decide".to_string(),
        };
        let request = format!(
            "**Integration request**\n\
\n\
Integrate the work from session `{source}` (branch `{source_branch}`, worktree `{source_work}`) \
into this topic's accepted state for repository `{repo}`, using strategy **{strategy}**. \
Apply the integration skill: inspect source and target state, verify the result is complete, \
resolve straightforward conflicts, run validation, update the topic branch, and report the \
outcome concisely. If the conflict needs product judgment, escalate rather than inventing a \
decision.\n\
\n\
```json\n{json}\n```",
            source = self.source_session,
            source_branch = self.source_branch,
            source_work = self.source_work_dir,
            repo = self.repo_root,
            json = serde_json::json!({
                "action": "integrate",
                "source_thread": self.source_session,
                "source_resource": self.source_branch,
                "source_work": self.source_work_dir,
                "repository": self.repo_root,
                "target_thread": self.target_session,
                "target_scope": topic_name,
                "target_resource": self.target_branch,
                "strategy": strategy,
                "requested_by": "user",
            }),
        );
        format!(
            "{request}\n\n## Integration procedure\n\n{}",
            include_str!("../../skills/integration.md")
        )
    }
}

/// A landing request (Topic → upstream), the sibling of IntegrationRequest.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LandingRequest {
    pub action: &'static str,
    pub source_branch: String,
    /// The resolved landing target. For non-PR strategies this is the
    /// primary checkout's currently checked out branch (or `main` when that
    /// could not be resolved); for `open-pr` it is the branch's recorded base.
    pub target_upstream: String,
    /// How `target_upstream` was resolved — `primary-checkout`,
    /// `main-fallback`, `recorded-base`, or `explicit` — so the landing
    /// agent knows what to re-verify before writing.
    pub target_origin: String,
    /// The repository the topic branch lives in; the primary checkout for
    /// landing is resolved from here.
    pub repo_root: String,
    pub strategy: IntegrationStrategy,
    pub requested_by: &'static str,
}

impl LandingRequest {
    pub fn to_prompt(&self, topic_name: &str) -> String {
        let strategy = match self.strategy {
            IntegrationStrategy::Squash => "squash",
            IntegrationStrategy::Merge => "merge",
            IntegrationStrategy::Rebase => "rebase",
            IntegrationStrategy::CherryPick => "cherry-pick",
            IntegrationStrategy::OpenPr => "open-pr",
            IntegrationStrategy::Push => "push",
            IntegrationStrategy::Ask => "decide",
        };
        // How the target was resolved shapes what the landing agent must
        // re-verify: a `primary-checkout` target can go stale when the user
        // switches branches; a `main-fallback` target means the control
        // plane could not read the checkout, so the agent should try to
        // resolve the primary checkout itself before settling for `main`.
        let target = match self.target_origin.as_str() {
            "primary-checkout" => format!(
                "`{upstream}`, the primary checkout's currently checked out branch",
                upstream = self.target_upstream
            ),
            "main-fallback" => format!(
                "`{upstream}` (fallback — the primary checkout's current branch could not be \
                 resolved from the control plane; try resolving it yourself from the repository \
                 before settling for `{upstream}`)",
                upstream = self.target_upstream
            ),
            "recorded-base" => format!(
                "`{upstream}`, the branch's recorded base (the remote's default branch)",
                upstream = self.target_upstream
            ),
            _ => format!(
                "`{upstream}`, as explicitly requested",
                upstream = self.target_upstream
            ),
        };
        let request = format!(
            "**Landing request**\n\
\n\
Land this topic's accepted state: branch `{branch}` for topic **{topic}** in the repository \
located at `{repo}`, using strategy **{strategy}**.\n\
\n\
Target: {target}.\n\
\n\
Apply the integration skill's landing rules: inspect the topic branch state, run required \
validation, create the PR or update the target branch, and report the outcome. Non-PR \
strategies land into the primary checkout's currently checked out branch — the checkout a \
human actually opens — falling back to `main` when it cannot be resolved; re-verify the \
target against the live checkout before writing. Prefer opening a PR when policy is unclear.\n\
\n\
```json\n{json}\n```",
            branch = self.source_branch,
            repo = self.repo_root,
            topic = topic_name,
            strategy = strategy,
            target = target,
            json = serde_json::json!({
                "action": "land",
                "source_thread": topic_name,
                "source_resource": self.source_branch,
                "repository": self.repo_root,
                "target_resource": self.target_upstream,
                "target_origin": self.target_origin,
                "strategy": strategy,
                "requested_by": "user",
            }),
        );
        format!(
            "{request}\n\n## Integration procedure\n\n{}",
            include_str!("../../skills/integration.md")
        )
    }
}

#[cfg(test)]
mod integration_prompt_tests {
    use super::{IntegrationRequest, IntegrationStrategy, LandingRequest};

    #[test]
    fn integration_request_carries_source_target_and_procedure() {
        let prompt = IntegrationRequest {
            action: "integrate",
            source_session: "worker-1".into(),
            source_branch: "worker-branch".into(),
            source_work_dir: "/tmp/worker".into(),
            repo_root: "/tmp/repo".into(),
            target_session: "topic-1".into(),
            target_branch: "topic-branch".into(),
            strategy: IntegrationStrategy::Squash,
            requested_by: "user",
        }
        .to_prompt("Topic");
        assert!(prompt.contains("\"source_work\":\"/tmp/worker\""));
        assert!(prompt.contains("\"target_resource\":\"topic-branch\""));
        assert!(prompt.contains("One active writer per worktree"));
    }

    #[test]
    fn landing_push_is_a_supported_strategy() {
        assert_eq!(
            IntegrationStrategy::parse("push"),
            Some(IntegrationStrategy::Push)
        );
        let prompt = LandingRequest {
            action: "land",
            source_branch: "topic-branch".into(),
            target_upstream: "origin/main".into(),
            target_origin: "recorded-base".into(),
            repo_root: "/tmp/repo".into(),
            strategy: IntegrationStrategy::Push,
            requested_by: "user",
        }
        .to_prompt("Topic");
        assert!(prompt.contains("\"strategy\":\"push\""));
        assert!(prompt.contains("origin/main"));
        assert!(prompt.contains("\"target_origin\":\"recorded-base\""));
    }

    #[test]
    fn landing_prompt_carries_primary_checkout_policy() {
        let prompt = LandingRequest {
            action: "land", source_branch: "weaver/topic".into(),
            target_upstream: "dev".into(), target_origin: "primary-checkout".into(),
            repo_root: "/tmp/repo".into(),
            strategy: IntegrationStrategy::Squash, requested_by: "user",
        }.to_prompt("Topic");
        assert!(prompt.contains("\"target_resource\":\"dev\""));
        assert!(prompt.contains("\"target_origin\":\"primary-checkout\""));
        assert!(prompt.contains("\"repository\":\"/tmp/repo\""));
        // The prompt states the policy, not just the resolved branch.
        assert!(prompt.contains("primary checkout's currently checked out branch"));
        assert!(prompt.contains("falling back to `main`"));
    }

    #[test]
    fn landing_prompt_marks_main_fallback_for_agent_resolution() {
        let prompt = LandingRequest {
            action: "land", source_branch: "weaver/topic".into(),
            target_upstream: "main".into(), target_origin: "main-fallback".into(),
            repo_root: "/tmp/repo".into(),
            strategy: IntegrationStrategy::Merge, requested_by: "user",
        }.to_prompt("Topic");
        assert!(prompt.contains("try resolving it yourself"));
    }
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
    pub effort: Option<String>,
    #[serde(default)]
    pub scratch: Vec<ScratchUpload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "snake_case", deserialize = "camelCase"))]
pub struct ScratchUpload {
    pub name: String,
    pub content_base64: String,
}

/// A project reference from the webview: a layout group id (`null` =
/// ungrouped) plus its display name. Projects are filing only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all(serialize = "snake_case", deserialize = "camelCase"))]
pub struct ProjectRef {
    pub id: Option<String>,
    pub name: String,
}

#[cfg(test)]
mod scratch_upload_tests {
    use super::ScratchUpload;

    #[test]
    fn webview_and_loom_use_their_respective_field_names() {
        let upload: ScratchUpload = serde_json::from_value(serde_json::json!({
            "name": "notes.txt", "contentBase64": "aGk="
        }))
        .unwrap();
        assert_eq!(upload.content_base64, "aGk=");
        let value = serde_json::to_value(upload).unwrap();
        assert_eq!(value["content_base64"], "aGk=");
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchProfileView {
    pub name: String,
    pub description: String,
    pub agent_kind: String,
    pub model: String,
    pub effort: String,
    pub class: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChoiceView {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetadataView {
    pub kind: String,
    pub label: String,
    pub models: Vec<AgentChoiceView>,
    pub efforts: Vec<AgentChoiceView>,
    pub accepts_raw_model: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsView {
    pub agents: Vec<AgentMetadataView>,
    pub default_agent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchOptionsView {
    pub profiles: Vec<LaunchProfileView>,
    pub agents: Vec<AgentMetadataView>,
    pub default_agent: String,
}

// ---------------------------------------------------------------------------
// Managed repositories and their branches
// ---------------------------------------------------------------------------

/// `repos.list` row — a managed repo in the clone allowlist.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RepoView {
    /// Canonical GitHub `owner/name`.
    pub slug: String,
    /// The clone source URL.
    pub remote_url: String,
    /// The managed on-disk checkout path (server-side filesystem path).
    pub path: String,
    pub created_at: String,
}

/// `repos.branches` row — one local git branch of a repo checkout, plus
/// which has a worktree and whether it is the checkout's current branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RepoBranchView {
    pub name: String,
    pub worktree: Option<String>,
    pub current: bool,
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
