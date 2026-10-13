//! Track-owned serial merge intent, stored in Loom's revision-guarded artifact.
//! The coordinator owns claims and Git outcomes; UI commands only order intent.
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::change::Change;
use crate::change_commands::{selected_client, selected_source};
use crate::client::{LoomClient, LoomError};
use crate::commands::{resource_error, LoomState, UiError};
use crate::loom::{SessionSummaryView, SessionView};

pub const QUEUE_NAME: &str = "change-queue.json";
const MAX_WIRE_REVISION: i64 = 9_007_199_254_740_991; // JavaScript safe integer

/// Shared by selected wakes, policy/result wakes and special Track launch prompts.
pub const DRAIN_PROTOCOL: &str = r#"SERIAL CHANGE QUEUE (supersedes immediate/out-of-order acceptance): Before EVERY queue claim/order/promotion/clear/drain mutation, source lifecycle/wrap-up or coordinator instruction/wake, use the selected owning backend's authenticated live GET /api/openapi.json and require exact boolean true at paths["/api/artifacts/write"].post["x-loom-atomic-base-rev"]. Missing/false/string/malformed/unreachable evidence fails closed: updating Loom to transaction-level atomic base_rev CAS is a prerequisite, not source completion or human attention. Never infer support from version, cached metadata, reconnect/replacement, a local mutex, retry or post-write receipt. Use only the owning authenticated client and its current availability/retirement/cancellation lease; no active-server fallback. Reread capability at each mutation boundary, including retries/no-op delivery and before resuming a drain. Runtime, adoption, contested-adoption and route receipts can invalidate prior support without cancelling the lease. Recheck after runtime lookup immediately before adoption, after adoption/contested-adoption reads immediately before source ACP prompt, and after terminal/source/coordinator route setup immediately before message/status dispatch; separately gate the coordinator ACP wake after its durable instruction. No intervening network/helper read may separate the final gate from its guarded dispatch. The owning Track branch artifact change-queue.json is authoritative intent/order, never a Git outcome. Read through artifacts.get with branch=<Track branch ID>, name=change-queue.json, repo=false; require meta.branch_id matches this Track, positive meta.rev and exact version:1/track_session/repo_root/target_branch. Missing artifact means revision 0 empty state. Corrupt/shared/conflicting metadata fails closed. Every artifacts.write uses that fresh revision as base_rev (0 guards creation), branch=<Track branch ID>, repo=false, kind=json and exact content. On 409 reread LIVE state and recompute the bounded mutation; never overwrite with stale state. Preserve concurrent pending entries/order/strategy/requested_by and active.
QueueEntry is {change:exact original Durable Change v1,strategy,requested_by:null|user|autoland}; state is {version:1,track_session,repo_root,target_branch,active:QueueEntry|null,pending:QueueEntry[]}. Use raw server-local source/channel identities. Only the coordinator claims active via fresh revision-CAS, atomically removing that ONE exact entry from pending. Exactly one active acceptance per canonical ref. Reordering/Land promotions never preempt an active claim, including its ongoing review/repair. A wake payload is a hint, not permission to bypass live queue order. If active exists, continue only that acceptance or report its blocker; do not start another.
Register new authenticated explicit outputs deterministically after existing pending order, preserving exact original completion pointers and matching working authorization. Reordering adds unrequested entries with null requested_by and squash; it NEVER authorizes manual acceptance. On a real result wake, authenticate original complete+final intended own-channel result and exact source registry/identity/scope/receipt. Reconcile permitted working-to-range finalization through fresh exact scope review, retaining ID/title/original source identity/base/literal paths and proving pinned working HEAD reaches finalized head; CAS replace only that matching pending declaration while preserving its chosen strategy/requested_by and concurrent order. Never change another declaration or reindex/resume/notify a completed range. Duplicate exact outputs/receipts are idempotent; conflicts stop for reconciliation.
Before claiming, choose the first eligible authorized pending entry in LIVE order. User intent remains manual under autoland Off. Autoland intent and unrequested null entries require LIVE effective Track/source autoland ON plus authenticated successful intended complete+final completion; declaration, reordering and running/sleeping/status are not authorization/readiness. Do not claim an output awaiting intended source completion. Keep failed/unready intent durably pending with clear parent-owned status. Review verdicts, interruptions, failed/checkpoint/help/partial results are not completed implementations. Explicit user interruption requires coordinator judgment, never autoresume. An already claimed completed acceptance retains active during independent review, repair or blockers; only matching actual acceptance completion or explicit reconciled coordinator cancellation may release it.
Immediately before EVERY automatic Git/index/ref/worktree write and resumption, reread LIVE owning Track/source policy/identity/canonical/source revisions and authenticated interruption controls; effectively Off defers writes, and explicit user interruption requires coordinator judgment, never automatic resumption. If parent sessions.get is 403, use permitted LIVE sessions.list(history:true), never borrowed credentials or cached policy. Immediately before EVERY acceptance write reread LIVE queue and verify the matching exact active claim, live policy for automatic work, source registry/ownership/identity/declared scope/original completion/receipt, same-repository owning Track/canonical initialization, canonical ref and exact target revision. Preserve source/target foreign work and coordinator scratch with isolated acceptance. Existing exact successfully accepted outputs require verified receipt/containment no-op history, never reapply them. Never manufacture receipts, successful Git writes or landed history from queue state.
After actual validated acceptance, record ONLY the exact original source change_acceptance:<id> and your typed own-channel actual result with source/target SHA/checks. Then reread LIVE queue and CAS clear ONLY the matching active (preserve concurrent pending edits); on conflict reread/reconcile without replaying Git. Reread LIVE queue/canonical/policy/source evidence and claim the next eligible authorized entry within this result-driven drain. If awaiting source/reviewer/repair results, verify own-channel and immediate-parent result delivery routes before idle, save state and end the turn. No publishing, scheduler or polling. Queue updates and queued requests are not accepted outcomes."#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestedBy {
    User,
    Autoland,
}

// Missing nullable fields are corrupt, rather than an implicit empty claim or
// authorization. In particular an omitted active must never release a slot.
fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueEntry {
    pub change: Change,
    pub strategy: String,
    #[serde(deserialize_with = "required_nullable")]
    pub requested_by: Option<RequestedBy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueState {
    pub version: u32,
    pub track_session: String,
    pub repo_root: String,
    pub target_branch: String,
    #[serde(deserialize_with = "required_nullable")]
    pub active: Option<QueueEntry>,
    pub pending: Vec<QueueEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueView {
    pub queue: QueueState,
    pub revision: i64,
}

impl QueueState {
    pub(crate) fn empty(track: &SessionView) -> Result<Self, UiError> {
        Ok(Self {
            version: 1,
            track_session: track.id.clone(),
            repo_root: track.branch.repo_root.clone(),
            target_branch: crate::track_policy::canonical_topic_branch(&track.branch)
                .ok_or_else(|| resource_error("Canonical Track initialization is unavailable."))?
                .into(),
            active: None,
            pending: vec![],
        })
    }

    fn validate(&self, track: &SessionView) -> Result<(), UiError> {
        let empty = Self::empty(track)?;
        if self.version != 1
            || self.track_session != empty.track_session
            || self.repo_root != empty.repo_root
            || self.target_branch != empty.target_branch
        {
            return Err(resource_error("Change queue Track/repository/canonical identity conflicts; reconcile before proceeding."));
        }
        let mut seen = HashSet::new();
        for entry in self.active.iter().chain(&self.pending) {
            entry.change.validate(false).map_err(resource_error)?;
            if entry.change.repo_root != self.repo_root
                || entry.change.source_session == self.track_session
                || entry.change.source_session.starts_with("loom:")
                || entry.change.source_branch == self.target_branch
                || !valid_strategy(&entry.strategy)
                || !seen.insert(key(&entry.change))
            {
                return Err(resource_error(
                    "Invalid or duplicate Change queue entry; reconcile before proceeding.",
                ));
            }
        }
        if self
            .active
            .as_ref()
            .is_some_and(|e| !e.change.scope.is_range())
        {
            return Err(resource_error(
                "An active queue claim requires intended source completion.",
            ));
        }
        Ok(())
    }

    /// Order conveys no new authorization and cannot remove a pending intent.
    fn reorder(&mut self, order: &[Change]) -> Result<(), UiError> {
        let mut seen = HashSet::new();
        let mut pending = Vec::with_capacity(order.len());
        for change in order {
            if !seen.insert(key(change))
                || self
                    .active
                    .as_ref()
                    .is_some_and(|e| key(&e.change) == key(change))
            {
                return Err(resource_error(
                    "Queue order contains duplicate or active Changes.",
                ));
            }
            if let Some(entry) = self.pending.iter().find(|e| key(&e.change) == key(change)) {
                if &entry.change != change {
                    return Err(resource_error("Pending Change declaration changed; reconcile the queue before reordering."));
                }
                pending.push(entry.clone());
            } else {
                pending.push(QueueEntry {
                    change: change.clone(),
                    strategy: "squash".into(),
                    requested_by: None,
                });
            }
        }
        if self.pending.iter().any(|e| !seen.contains(&key(&e.change))) {
            return Err(resource_error("Queue order must include the complete current pending set; refresh before reordering."));
        }
        self.pending = pending;
        Ok(())
    }

    /// Promote exactly one selection, preserving all other intent and active.
    fn promote(&mut self, change: &Change, strategy: &str) -> Result<(), UiError> {
        if let Some(active) = &self.active {
            if key(&active.change) == key(change) {
                if &active.change != change {
                    return Err(resource_error(
                        "Active Change identity conflicts; reconcile before retry.",
                    ));
                }
                // A retry may wake the coordinator, but cannot rewrite its claim.
                return Ok(());
            }
        }
        let position = self
            .pending
            .iter()
            .position(|e| key(&e.change) == key(change));
        if position.is_some_and(|i| &self.pending[i].change != change) {
            return Err(resource_error(
                "Pending Change declaration changed; reselect and reconcile before Land.",
            ));
        }
        if let Some(i) = position {
            self.pending.remove(i);
        }
        self.pending.insert(
            0,
            QueueEntry {
                change: change.clone(),
                strategy: strategy.into(),
                requested_by: Some(RequestedBy::User),
            },
        );
        Ok(())
    }
}

fn key(change: &Change) -> (&str, &str) {
    (&change.source_session, &change.id)
}

fn valid_strategy(value: &str) -> bool {
    matches!(value, "squash" | "merge" | "rebase" | "cherry-pick")
}

/// One live owning Track per canonical ref: separate artifacts cannot safely
/// serialize writers if conflicting Track metadata claims the same target.
pub(crate) fn verify_canonical_owner(
    fleet: &[SessionSummaryView],
    track: &SessionView,
) -> Result<(), UiError> {
    let target = QueueState::empty(track)?.target_branch;
    if fleet.iter().any(|other| {
        other.id != track.id
            && other.status != "archived"
            && other.branch.repo_root == track.branch.repo_root
            && crate::track_policy::owns_track(other)
            && crate::track_policy::canonical_topic_branch(&other.branch) == Some(target.as_str())
    }) {
        return Err(resource_error("Conflicting live Tracks claim this canonical ref; reconcile ownership before queue mutation."));
    }
    Ok(())
}

/// Resolve on the selected server only. Refresh the owner's full authenticated
/// tags instead of authorizing from a fleet summary's canonical display state.
pub(crate) async fn context(
    client: &LoomClient,
    topic_id: &str,
) -> Result<(SessionView, Vec<SessionSummaryView>), UiError> {
    let mut fleet = client.list_sessions().await?;
    let track = client.get_session(topic_id).await?;
    let summary = fleet
        .iter_mut()
        .find(|s| s.id == topic_id)
        .ok_or_else(|| resource_error("Queue Track is no longer registered."))?;
    if track.id != topic_id
        || track.status == "archived"
        || summary.branch.id != track.branch.id
        || summary.branch.repo_root != track.branch.repo_root
        || summary.branch.branch != track.branch.branch
    {
        return Err(resource_error(
            "Queue Track identity changed; refresh before proceeding.",
        ));
    }
    summary.branch = track.branch.clone();
    if !crate::track_policy::owns_track(summary) {
        return Err(resource_error(
            "Change queue requires its live owning Track.",
        ));
    }
    verify_canonical_owner(&fleet, &track)?;
    Ok((track, fleet))
}

fn decode_artifact(value: &serde_json::Value, track: &SessionView) -> Result<QueueView, UiError> {
    let meta = &value["meta"];
    if meta["branch_id"].as_str() != Some(track.branch.id.as_str()) {
        return Err(resource_error(
            "Change queue artifact is not owned by the Track branch.",
        ));
    }
    let revision = meta["rev"]
        .as_i64()
        .filter(|r| *r > 0 && *r <= MAX_WIRE_REVISION)
        .ok_or_else(|| resource_error("Change queue artifact has no valid revision."))?;
    let queue: QueueState = serde_json::from_str(
        value["content"]
            .as_str()
            .ok_or_else(|| resource_error("Change queue artifact has no text content."))?,
    )
    .map_err(|e| resource_error(format!("Corrupt Change queue: {e}")))?;
    queue.validate(track)?;
    Ok(QueueView { queue, revision })
}

pub(crate) async fn load(client: &LoomClient, track: &SessionView) -> Result<QueueView, UiError> {
    match client.branch_artifact(&track.branch.id, QUEUE_NAME).await {
        Ok(value) => decode_artifact(&value, track),
        Err(LoomError::Api { status: 404, .. }) => Ok(QueueView {
            queue: QueueState::empty(track)?,
            revision: 0,
        }),
        Err(error) => Err(error.into()),
    }
}

/// Independent coordinator writers require transaction-level server CAS.
/// A mutex, a retry or checking a receipt after an overwrite cannot supply it.
/// Capability spelling is coordinated with Loom; accept only exact boolean true.
pub(crate) async fn require_atomic_cas(client: &LoomClient) -> Result<(), UiError> {
    let document = client.openapi_document().await.map_err(|error| {
        let error: UiError = error.into();
        UiError {
            message: format!("Changes queue requires an updated Loom backend with atomic artifact base_rev support; live capability could not be verified. {}", error.message),
            ..error
        }
    })?;
    if document["paths"]["/api/artifacts/write"]["post"]["x-loom-atomic-base-rev"].as_bool()
        != Some(true)
    {
        return Err(resource_error("Changes queue requires a Loom backend update: authenticated GET /api/openapi.json must advertise exact boolean true at paths[\"/api/artifacts/write\"].post[\"x-loom-atomic-base-rev\"]. Update the selected backend before queue mutation or delivery."));
    }
    Ok(())
}

async fn save(
    client: &LoomClient,
    track: &SessionView,
    queue: QueueState,
    revision: i64,
) -> Result<QueueView, UiError> {
    queue.validate(track)?;
    require_atomic_cas(client).await?;
    let content = serde_json::to_string(&queue).map_err(|e| resource_error(e.to_string()))?;
    let value = match client.write_branch_artifact_titled(&track.branch.id, QUEUE_NAME, &content, revision, "Changes").await {
        Ok(value) => value,
        Err(LoomError::Api { status: 409, .. }) => return Err(resource_error("Change queue revision conflict; refresh the queue and retry. No order or promotion was overwritten.")),
        Err(error) => {
            let error: UiError = error.into();
            return Err(UiError {
                message: format!("Change queue persistence failed or is uncertain; refresh to reconcile before retry. Acceptance is not confirmed. {}", error.message),
                ..error
            });
        }
    };
    let saved = decode_artifact(&value, track).map_err(|error| UiError {
        message: format!("Change queue persistence receipt could not be verified; refresh to reconcile the possibly saved request. {}", error.message),
        ..error
    })?;
    if saved.queue != queue || saved.revision != revision + 1 {
        return Err(resource_error("Change queue write receipt did not match intent/revision; refresh to reconcile the possibly saved request."));
    }
    Ok(saved)
}

pub(crate) async fn promote(
    client: &LoomClient,
    track: &SessionView,
    change: &Change,
    strategy: &str,
) -> Result<QueueView, UiError> {
    let view = load(client, track).await?;
    let mut queue = view.queue.clone();
    queue.promote(change, strategy)?;
    if queue == view.queue {
        return Ok(view);
    }
    // No blind retry: another application's order/promotion must be observed.
    save(client, track, queue, view.revision).await
}

pub(crate) async fn change_queue_with_client(
    client: &LoomClient,
    topic_id: &str,
) -> Result<QueueView, UiError> {
    let (track, _) = context(client, topic_id).await?;
    load(client, &track).await
}

pub(crate) async fn reorder_with_client(
    client: &LoomClient,
    topic_id: &str,
    expected_revision: i64,
    order: &[Change],
) -> Result<QueueView, UiError> {
    require_atomic_cas(client).await?;
    let (track, _) = context(client, topic_id).await?;
    let view = load(client, &track).await?;
    if expected_revision < 0 || expected_revision != view.revision {
        return Err(resource_error(
            "Change queue revision conflict; refresh before reordering.",
        ));
    }
    let mut queue = view.queue;
    queue.reorder(order)?;
    queue.validate(&track)?;
    // Authenticate every pending declaration and its original completion on
    // this leased server. Never infer actual source identity from queue content.
    let mut sources = Vec::with_capacity(order.len());
    for change in order {
        let source = selected_source(client, &change.source_session, &change.id, change).await?;
        if change.scope.working_version().is_some() {
            crate::change_commands::verify_working_snapshot(
                change,
                &client.session_changes(&source.id).await?,
            )?;
        }
        if crate::commands::host_is_loopback(client.server_host().unwrap_or("")) {
            crate::change_commands::verify_local_scope(&source, change).await?;
        }
        sources.push(source);
    }
    let (fresh, fleet) = context(client, topic_id).await?;
    if fresh.branch.id != track.branch.id {
        return Err(resource_error("Queue Track branch identity changed."));
    }
    queue.validate(&fresh)?;
    for (change, source) in order.iter().zip(&sources) {
        let owner = crate::commands::resolve_integration_target(&fleet, &change.source_session)
            .ok_or_else(|| resource_error("Queued source has no canonical owning Track."))?;
        if owner.coordinator_id != topic_id || owner.target_branch != queue.target_branch {
            return Err(resource_error(
                "Queued Change belongs to a different Track/backend/repository.",
            ));
        }
        let summary = fleet
            .iter()
            .find(|s| s.id == change.source_session)
            .unwrap();
        if summary.branch.repo_root != change.repo_root
            || summary.branch.branch != change.source_branch
        {
            return Err(resource_error("Queued source registry identity changed."));
        }
        crate::change_commands::recheck_selection(client, source, change).await?;
    }
    save(client, &fresh, queue, expected_revision).await
}

#[tauri::command]
pub async fn change_queue(
    state: State<'_, LoomState>,
    base_url: String,
    topic_id: String,
) -> Result<QueueView, UiError> {
    async {
        let client = selected_client(&state, &base_url).await?;
        change_queue_with_client(&client, &topic_id).await
    }
    .await
    .map_err(|e: UiError| e.for_server(&base_url))
}

#[tauri::command]
pub async fn reorder_change_queue(
    state: State<'_, LoomState>,
    base_url: String,
    topic_id: String,
    expected_revision: i64,
    order: Vec<Change>,
) -> Result<QueueView, UiError> {
    async {
        let client = selected_client(&state, &base_url).await?;
        reorder_with_client(&client, &topic_id, expected_revision, &order).await
    }
    .await
    .map_err(|e: UiError| e.for_server(&base_url))
}
