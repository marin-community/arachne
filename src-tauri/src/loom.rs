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

/// The branch's GitHub PR snapshot, maintained by loom's poll loop. Only
/// the fields the resource strip renders.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GithubStatusView {
    pub pr_number: i64,
    pub pr_url: String,
    pub pr_state: String,
    pub pr_title: String,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    pub review_decision: Option<String>,
    /// One-word CI rollup (e.g. `success`), when known.
    #[serde(default)]
    pub checks: Option<String>,
    /// GitHub's merge fit: `MERGEABLE` / `CONFLICTING` / `UNKNOWN`.
    #[serde(default)]
    pub mergeable: Option<String>,
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
    /// The branch's latest GitHub pull-request snapshot (link, review
    /// decision, check rollup), or `null` when GitHub polling is off, the
    /// repo has no remote PR, or `gh` is unavailable. Maintained by the
    /// loom poll loop.
    #[serde(default)]
    pub github: Option<GithubStatusView>,
    #[serde(default)]
    pub github_pr: Option<i64>,
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

/// ACP's current model context and provider-reported cumulative session cost.
/// These are not cumulative input/output/cache token totals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpUsageView {
    pub used: u64,
    pub size: u64,
    #[serde(default)]
    pub cost: Option<AcpCostView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpCostView {
    pub amount: f64,
    pub currency: String,
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
    /// Old Loom servers omit this; each entry is an unanswered ACP request.
    #[serde(default)]
    pub pending_permissions: Vec<PendingPermissionView>,
    /// Latest ACP context report; absent before a provider reports usage.
    #[serde(default)]
    pub usage: Option<AcpUsageView>,
    /// When the newest `user_message` block was journaled (the last time a
    /// person or a delivery on their behalf steered the conversation), or
    /// `None` when the journal holds no user input — or when an older loom
    /// predates the field (`#[serde(default)]` there). Unlike
    /// `last_activity_at`, it does not advance while the agent is merely
    /// streaming, so topic ordering by it stays still.
    #[serde(default)]
    pub last_user_message_at: Option<String>,
    /// Some Loom summaries omit checkout identity. The client resolves missing
    /// paths for declared-output rows through authenticated sessions.get.
    #[serde(default)]
    pub work_dir: String,
    #[serde(default)]
    pub worktree_present: bool,
    /// Adapter-owned evidence; never accepted from the summary wire response.
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub change_identity_problem: Option<ChangeIdentityProblem>,
    pub branch: BranchSummaryView,
    pub placement: Option<SessionPlacementView>,
    pub github_repo: Option<String>,
    pub parent_id: Option<String>,
    pub parent_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeIdentityProblem {
    Unavailable,
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPermissionView {
    pub request_id: String,
    pub title: String,
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
    /// Latest ACP context report; reset to None on a provider handoff.
    #[serde(default)]
    pub usage: Option<AcpUsageView>,
    pub work_dir: String,
    pub term_session: String,
    /// True when the session's `work_dir` still exists on the server.
    /// Archive removes the worktree while keeping the branch, so this
    /// decides whether "Open in Zed" is live or needs recovery through
    /// `repos.worktrees.ensure` first.
    #[serde(default)]
    pub worktree_present: bool,
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
// Repos / worktrees
// ---------------------------------------------------------------------------

/// `repos.worktrees.ensure` — a branch's (possibly just-recovered)
/// checkout. Idempotent: `created` distinguishes a materialized worktree
/// from one already on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RepoWorktreeView {
    pub repo_root: String,
    pub branch: String,
    pub path: String,
    pub created: bool,
}

// ---------------------------------------------------------------------------
// Issues (loom work items)
// ---------------------------------------------------------------------------

/// `issues.board` row — loom's work items, each optionally linked to a GitHub
/// issue. `claimed_branch`/`source_branch` are branch *names*: issue claims key
/// by (repo_root, branch name), not branch id. `github_state` is only present
/// on the single-issue `issues.get` (the board does no GitHub fan-out).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct IssueView {
    pub id: i64,
    pub repo_root: String,
    #[serde(default)]
    pub github_repo: Option<String>,
    /// Branch the issue was created from (provenance).
    #[serde(default)]
    pub source_branch: Option<String>,
    /// Branch currently working it; `None` is the unclaimed repo backlog.
    #[serde(default)]
    pub claimed_branch: Option<String>,
    pub title: String,
    pub status: String,
    #[serde(default)]
    pub github_issue: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_state: Option<serde_json::Value>,
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
    #[serde(default)]
    pub metadata: AcpMetadataView,
}

/// ACP-owned composer capabilities. Preserve JSON for config and mode
/// variants so adapters can add fields without requiring an Arachne release.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AcpMetadataView {
    #[serde(default)]
    pub commands: Vec<serde_json::Value>,
    #[serde(default)]
    pub config_options: Vec<serde_json::Value>,
    #[serde(default)]
    pub modes: Vec<serde_json::Value>,
    #[serde(default)]
    pub steering_supported: bool,
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
    /// Land deterministically in Arachne itself: squash-merge the topic's
    /// branch into the primary checkout's current branch, no agent turn
    /// (spec: docs/integration-and-landing.md, "Land locally").
    LandLocally,
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
            "land-locally" => Self::LandLocally,
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
            Self::LandLocally => "land-locally",
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
            Self::LandLocally => "Squash into local checkout",
            Self::Ask => "Ask coordinator to decide",
        }
    }

    /// True for the strategies carried out deterministically by Arachne
    /// itself rather than requested from an agent thread.
    pub fn is_local(&self) -> bool {
        matches!(self, Self::LandLocally)
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
    pub fn source_wrap_up_prompt(&self) -> String {
        format!("Thread Land wrap-up requested for session {} in repository {}, original branch {}, original worktree {}. Finish the active intended task first; this request is queued behind active work. Preserve your original session/repository/branch/worktree identity. Inspect and commit only intended source changes, preserving foreign edits, staged/unstaged changes and untracked files; validate the intended result. Send a typed durable result on your OWN channel with successful intended completion, exact source revision, validation and exclusions. Checkpoint/help/partial/interrupted results do not qualify. Inspect control-channel notices and authenticated actor provenance before continuing; an explicit user interruption never authorizes automatic resumption or acceptance. Session-authored interruptions must not be described as user interruptions. The Track coordinator {} will inspect, review/repair and accept into canonical ref {} using {}. Do not publish, push, open a PR, or accept the work yourself. landing_request=pending is only advisory, never readiness or successful acceptance.", self.source_session, self.repo_root, self.source_branch, self.source_work_dir, self.target_session, self.target_branch, self.strategy.as_str())
    }

    /// Complete acceptance authorization; no second Prepare click is needed.
    pub fn to_prompt(&self, topic_name: &str) -> String {
        let automatic = self.requested_by == "autoland";
        let guard = if automatic {
            format!("{} {} This automatic request applies only to explicit qualified Changes on an actual durable result wake; undeclared historical Threads have no acceptance obligation.", autoland_guard(), crate::change::SELECTED_ACCEPTANCE_GUARD)
        } else {
            "This is an explicit manual acceptance request; automatic policy does not revoke this user action.".to_string()
        };
        let receipt_instruction = if automatic {
            "Record only the matching change_acceptance:<id> on the ORIGINAL source after actual successful canonical acceptance and validation; never blanket integration_result"
        } else {
            "Record integration_result on the ORIGINAL source only after actual successful canonical acceptance and validation"
        };
        format!("Thread Land acceptance request for Track {topic_name}.\n\n```json\n{}\n```\n\nWait for the source's successful intended wrap-up result on its durable own channel or linked result notice; checkpoint/help/partial/interrupted results and landing_request=pending do not qualify. Interruption notices are ordinary system-authored parent-channel messages, never typed completion results. Inspect notice context and authenticated actor provenance: explicit user interruption requires coordinator judgment and never authorizes automatic child resumption or acceptance; session-authored interruption must not be called a user interruption. Source session status, sleeping, completion, integration_state and integration_ready are never evidence of intended success. If the result has not arrived, retain this request durably and wait on the installed result delivery route; do not poll or use a scheduler.\n\nInspect the exact source session/repository/original branch/worktree and canonical target ref named above. Verify the LIVE Track is still the same-repository owning Track ancestor (marked Track, or legacy unmarked root unless topic=false) and its LIVE track_topic_branch still agrees with this target (a legacy missing tag retains its existing coordinator branch). Verify the canonical ref exists and initialization succeeded before accepting; a new Track retaining its initialization marker requires LIVE track_topic_initialized to match the canonical ref, not display tags alone. Preserve worker identity and the coordinator Loom scratch branch. Review and repair the intended changes, commit only intended source changes, preserve foreign edits and staged/unstaged/untracked files in every checkout, and validate. One active writer per worktree: wait for source wrap-up before writing, or use an isolated checkout. Delegate only when useful through visible Loom children with typed own-channel results.\n\n{guard}\n\nImmediately before EVERY acceptance write after preparation or delegation, reverify the exact source and canonical target repository/branch/revision and refresh review/validation if either changed. Apply the chosen acceptance strategy into the canonical accepted ref only (squash is the default). Complete review, repair, commit, validate and acceptance without another Prepare click. Never push, publish, open PRs, modify primary HEAD or substitute local main. {receipt_instruction}, then send a typed durable outcome with source/target SHAs, strategy, checks and exclusions on your own channel. A queued request is not a completed acceptance.",
            serde_json::json!({
                "action": self.action, "source_thread": self.source_session,
                "source_resource": self.source_branch, "source_work": self.source_work_dir,
                "repository": self.repo_root, "target_thread": self.target_session,
                "target_scope": topic_name, "target_resource": self.target_branch,
                "strategy": self.strategy.as_str(), "requested_by": self.requested_by,
                "prepare_then_accept": true,
            }))
    }
}

pub fn autoland_guard() -> &'static str {
    "AUTOLAND LIVE WRITE GUARD: Immediately before EVERY automatic Git/ref/index/worktree write, including source commits, repairs and final canonical acceptance, reread LIVE Track and source session tags from Loom; never use the policy snapshot in this queued prompt. Track track_autoland is ON only for exact on, absent/invalid means OFF. Source thread_autoland=on overrides Track OFF; off overrides Track ON; absent/invalid/inherit inherits Track. If effectively OFF, stop this stale automatic request before writing and report deferred/cancelled acceptance. Recheck after delegation and on every resumed turn. Before writes also reverify the LIVE canonical target and source identity; skip exact explicit Changes already successfully accepted by matching change_acceptance:<id>, never blanket integration_result. Only authenticated own-channel complete+final explicit Change results qualify; exclude zero-output review verdicts, checkpoint, help, partial and interrupted work. Source role/title/status never disqualifies an otherwise authenticated explicitly declared implementation range. Inspect control-channel interruption notices and authenticated actor provenance before every automatic write or resumption; these are ordinary system-authored messages, never typed completion results. An explicit user interruption requires coordinator judgment: do not automatically resume that child or accept its work, even under an older queued request or prior complete result. A session-authored interruption must not be called a user interruption. Pending interruption remains incomplete until explicitly resolved with appropriate authorization and fresh completion/review evidence. Autoland never publishes."
}

pub fn autoland_policy_prompt(track: &SessionView, target_branch: &str) -> String {
    format!("Durable autoland policy setup for Track {} in repository {}, canonical accepted ref {}. Subscribe/retain result delivery on your own channel and installed immediate-parent channels for nested sources. On existing or future linked actual durable result wakes, operate ONLY on explicit Change outputs, not historical discovery or tags alone. A result with changes:[] requires no Land. Resolve the ORIGINAL source identity and SAME-repository owning Track ancestor (marked Track, or legacy unmarked root unless topic=false), re-read LIVE canonical ownership/initialization and policy. Read the linked ORIGINAL kind=result with author_kind=session, author_id=source_session, channel_id=source_session and exact source_message_id; require JSON body or structured payload {{version:1,outcome:\"complete\",intent:\"final\",changes:[inline declarations with completion:null]}}. Missing original reference in bounded history fails closed. Complete+final ONLY; failed/help/checkpoint/partial/interrupted results and zero-output review verdicts do not qualify. Source role/title/status never disqualifies an otherwise authenticated explicitly declared implementation range. Process authenticated inline range outputs BEFORE tag indexing: attach that original message channel_id/message_id as completion, validate each exact ID/source identity/scope, and re-read registry. If change:<id> is missing because result delivery preceded indexing, index/reconcile this exact authenticated inline output through existing sessions.tags.set within this same wake; no second wake, no polling, no silently lost output. A matching working declaration may finalize to this range ONLY while retaining ID, original source identity, original base and exact bounded paths, proving pinned working HEAD reaches the finalized head and freshly reviewing the exact new scope within this same request/wake. Index that permitted finalization once authenticated; never overwrite any other changed/conflicting declaration or mismatched receipt, and stop for reconciliation. Identical duplicate IDs and repeat deliveries are idempotent; conflicting duplicate metadata fails closed. Never manufacture explicit declarations from historical Threads, branches or non-explicit legacy completions, never generate fresh pending deltas from repeated historical results, and never retask historical sources. Each new explicit ID/range remains independently eligible despite legacy integration_result. If effectively ON, review/repair/validate and accept ONLY the eligible selected delta using squash default; preserve foreign staged/unstaged/untracked/ignored edits and use isolated acceptance worktrees. Never ask for another Prepare click. Off policy does not disable source on overrides. Never treat sleeping/working/completion or advisory tags as readiness. {} {} Use existing durable result wakeups; no scheduler or polling loops, and no automatic resumption. Never publish or change primary HEAD or Loom scratch branches. Report actual per-output receipt and typed own-channel outcome, never blanket integration_result.\n\n{}", track.id, track.branch.repo_root, target_branch, crate::change::SELECTED_ACCEPTANCE_GUARD, autoland_guard(), crate::change_queue::DRAIN_PROTOCOL)
}

/// A landing request (Topic → upstream), the sibling of IntegrationRequest.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LandingRequest {
    pub action: &'static str,
    pub source_session: String,
    pub source_work_dir: String,
    pub source_state: Option<crate::landing::SourceCheckoutState>,
    pub local_target: Option<crate::landing::LocalLandingTarget>,
    pub repair_reason: Option<String>,
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
            IntegrationStrategy::LandLocally => "land-locally",
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
This click authorizes one complete preparation-and-landing operation using the chosen \
strategy; do not ask for another click or confirmation for routine preparation. Inspect the \
intended user changes in the source checkout (including editor saves, staged, unstaged and \
untracked files), review and repair straightforward issues, and run required validation. \
Delegate substantial independent review or repair through visible Loom children using \
`loom sessions launch` with explicit repository and base; require typed results on their own \
channels and verify parentage and wake delivery before waiting. Commit only intended source \
changes after validation, preserving unrelated manual work; never blanket git add/commit \
foreign edits. If intended versus unrelated changes require a real decision, report it. \
Re-verify the source branch, revision, status and destination immediately before landing. \
Then perform the chosen landing without stopping at a prepared/ready report. For land-locally, \
squash into the primary checkout's CURRENT branch; never substitute recorded base or main, \
never push or open a PR. Preserve all staged, unstaged and untracked target changes; use an \
isolated checkout for reconciliation and advance/update the target safely, without absorbing \
foreign staged changes or using destructive resets/stashes. A changed target requires fresh \
validation; unrelated in-progress Git operations are blockers. Other non-PR strategies honor \
an explicit destination; otherwise use the primary checkout's current branch, falling back to `main` \
only when it cannot be resolved. Re-verify the target against the live checkout. Report the actual \
completed commit/PR or no-op, exact source/target revisions, validation and remaining git \
status. Stamp integration_result only after actual successful landing and validation; queued \
preparation is not landed or verified readiness.\n\
\n\
```json\n{json}\n```",
            branch = self.source_branch,
            repo = self.repo_root,
            topic = topic_name,
            strategy = strategy,
            target = target,
            json = serde_json::json!({
                "action": "land",
                "source_thread": self.source_session,
                "source_work": self.source_work_dir,
                "source_state": self.source_state,
                "local_target": self.local_target,
                "repair_reason": self.repair_reason,
                "prepare_then_land": true,
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
    fn interrupted_work_never_qualifies_for_automatic_acceptance_or_resumption() {
        let request = IntegrationRequest {
            action: "integrate",
            source_session: "worker-1".into(),
            source_branch: "worker-branch".into(),
            source_work_dir: "/tmp/worker".into(),
            repo_root: "/tmp/repo".into(),
            target_session: "topic-1".into(),
            target_branch: "topic-branch".into(),
            strategy: IntegrationStrategy::Squash,
            requested_by: "autoland",
        };
        let automatic = request.to_prompt("Topic");
        for prompt in [automatic, request.source_wrap_up_prompt()] {
            assert!(prompt.contains("authenticated actor provenance"));
            assert!(prompt.contains("user interruption"));
            assert!(prompt.contains("resumption"));
            assert!(
                prompt.contains("Session-authored interruptions")
                    || prompt.contains("session-authored interruption")
            );
        }
        let guard = super::autoland_guard();
        assert!(guard.contains("ordinary system-authored messages, never typed completion results"));
        assert!(guard.contains("do not automatically resume that child or accept its work"));
        assert!(guard.contains("older queued request or prior complete result"));
        assert!(guard.contains("appropriate authorization and fresh completion/review evidence"));
        assert!(
            include_str!("../../skills/coordinator.md").contains("authenticated actor provenance")
        );
        assert!(include_str!("../../skills/worker.md").contains("explicit user interruption"));
    }

    #[test]
    fn landing_push_is_a_supported_strategy() {
        assert_eq!(
            IntegrationStrategy::parse("push"),
            Some(IntegrationStrategy::Push)
        );
        let prompt = LandingRequest {
            action: "land",
            source_session: "topic-1".into(),
            source_work_dir: "/tmp/source".into(),
            source_state: None,
            local_target: None,
            repair_reason: None,
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
            action: "land",
            source_session: "topic-1".into(),
            source_work_dir: "/tmp/source".into(),
            source_state: None,
            local_target: None,
            repair_reason: None,
            source_branch: "weaver/topic".into(),
            target_upstream: "dev".into(),
            target_origin: "primary-checkout".into(),
            repo_root: "/tmp/repo".into(),
            strategy: IntegrationStrategy::Squash,
            requested_by: "user",
        }
        .to_prompt("Topic");
        assert!(prompt.contains("\"target_resource\":\"dev\""));
        assert!(prompt.contains("\"target_origin\":\"primary-checkout\""));
        assert!(prompt.contains("\"repository\":\"/tmp/repo\""));
        // The prompt states the policy, not just the resolved branch.
        assert!(prompt.contains("primary checkout's currently checked out branch"));
        assert!(prompt.contains("falling back to `main`"));
    }

    #[test]
    fn every_landing_strategy_authorizes_complete_preparation_and_chosen_landing() {
        for strategy in [
            IntegrationStrategy::OpenPr,
            IntegrationStrategy::Squash,
            IntegrationStrategy::Merge,
            IntegrationStrategy::Rebase,
            IntegrationStrategy::Push,
            IntegrationStrategy::LandLocally,
        ] {
            let prompt = LandingRequest {
                action: "land",
                source_session: "topic-1".into(),
                source_work_dir: "/tmp/source".into(),
                source_state: None,
                local_target: None,
                repair_reason: None,
                source_branch: "weaver/topic".into(),
                target_upstream: "dev".into(),
                target_origin: "primary-checkout".into(),
                repo_root: "/tmp/repo".into(),
                strategy,
                requested_by: "user",
            }
            .to_prompt("Topic");
            assert!(prompt.contains("\"prepare_then_land\":true"));
            assert!(prompt.contains("\"source_thread\":\"topic-1\""));
            assert!(prompt.contains("\"source_work\":\"/tmp/source\""));
            assert!(prompt.contains("Commit only intended source"));
            assert!(prompt.contains("Then perform the chosen landing"));
            assert!(prompt.contains("do not ask for another click"));
            assert!(prompt.contains("visible Loom children"));
            assert!(
                prompt.contains("Stamp integration_result only after actual successful landing")
            );
        }
    }

    #[test]
    fn landing_prompt_marks_main_fallback_for_agent_resolution() {
        let prompt = LandingRequest {
            action: "land",
            source_session: "topic-1".into(),
            source_work_dir: "/tmp/source".into(),
            source_state: None,
            local_target: None,
            repair_reason: None,
            source_branch: "weaver/topic".into(),
            target_upstream: "main".into(),
            target_origin: "main-fallback".into(),
            repo_root: "/tmp/repo".into(),
            strategy: IntegrationStrategy::Merge,
            requested_by: "user",
        }
        .to_prompt("Topic");
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
    pub launch_guidance: Option<SessionLaunchGuidance>,
    /// Local prompt prefix, retained in the first durable turn on old servers.
    #[serde(skip)]
    pub initialization_prompt: Option<String>,
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

#[derive(Debug, Clone, Serialize)]
pub struct SessionLaunchGuidance {
    pub root: String,
    pub child: String,
}

pub fn topic_launch_guidance() -> SessionLaunchGuidance {
    SessionLaunchGuidance {
        root: format!("{}\n\nRevised Thread Land policy (supersedes earlier acceptance defaults): accepted state is LIVE track_topic_branch, or the existing Loom branch only for a legacy Track with no tag. New Tracks require verified real ref initialization before acceptance or publishing. Manual Land includes complete wrap-up, review/repair and canonical acceptance. Autoland defaults OFF, source overrides inherit/on/off, and is driven by durable successful result wakeups only. {}\n\n{}", include_str!("../../skills/coordinator.md"), autoland_guard(), crate::change_queue::DRAIN_PROTOCOL),
        child: include_str!("../../skills/worker.md").into(),
    }
}

pub fn worker_launch_guidance() -> SessionLaunchGuidance {
    let worker = include_str!("../../skills/worker.md");
    SessionLaunchGuidance {
        root: worker.into(),
        child: worker.into(),
    }
}

pub fn reviewer_launch_guidance() -> SessionLaunchGuidance {
    SessionLaunchGuidance {
        root: include_str!("../../skills/reviewer.md").into(),
        // Some Loom versions choose child guidance on a parented launch.
        child: include_str!("../../skills/reviewer.md").into(),
    }
}

/// One-off conversations stay direct; the durable marker permits later promotion.
pub fn thread_launch_guidance() -> SessionLaunchGuidance {
    SessionLaunchGuidance {
        root: format!("Arachne standalone thread: handle this one-off task directly. Delegation is optional. The durable session tag topic=false denotes a standalone thread; topic=true means this conversation is a Track. Check that marker when beginning subsequent work; when promoted, follow the coordinator guidance below.\n\n{}", topic_launch_guidance().root),
        child: worker_launch_guidance().child,
    }
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

#[cfg(test)]
mod launch_guidance_tests {
    use super::{topic_launch_guidance, SessionsLaunchInput};

    #[test]
    fn topic_guidance_reaches_loom_launch_request() {
        let request = SessionsLaunchInput {
            goal: Some("Fix the editor".into()),
            launch_guidance: Some(topic_launch_guidance()),
            ..Default::default()
        };
        let json = serde_json::to_value(request).unwrap();
        let root = json["launch_guidance"]["root"].as_str().unwrap();
        let child = json["launch_guidance"]["child"].as_str().unwrap();
        assert!(root.contains("Track coordinator"));
        assert!(child.contains("Track worker"));
        // The coordinator is a PM, not a coder: delegation is the default,
        // not an option weighed against overhead.
        assert!(root.contains("project manager"));
        assert!(root.contains("Aggressively delegate"));
        assert!(root.contains("context management"));
        assert!(root.contains("definition of done"));
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
