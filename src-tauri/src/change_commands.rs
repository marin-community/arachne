//! Selected Change commands over existing Loom metadata and delivery APIs.
use tauri::{AppHandle, Emitter, State};

use crate::change::{Change, ChangeRegistry, SELECTED_ACCEPTANCE_GUARD};
use crate::client::LoomClient;
use crate::commands::{resource_error, IntegrationTarget, LoomState, ReviewChanges, UiError};
use crate::loom::{ChangeSetView, IntegrationRequest, IntegrationStrategy, SessionView};

/// Selected actions require their explicit owning backend to be connected and
/// available. Durable declarations and completion/receipt pointers stay local to
/// that client; this boundary never qualifies or rewrites them.
pub(crate) async fn selected_client(
    state: &LoomState,
    base_url: &str,
) -> Result<std::sync::Arc<LoomClient>, UiError> {
    let entry = state.servers.resolve(base_url).await?;
    let cancel = entry.selected_action_token().ok_or_else(|| UiError {
        message: "Selected Loom backend is unavailable; reconnect before proceeding.".into(),
        unreachable: true,
        server_url: Some(entry.client.server_url()),
    })?;
    Ok(std::sync::Arc::new(
        entry.client.for_selected_action(cancel),
    ))
}

/// Selected Prepare cannot cross the gap between a UI preview and generic
/// send_input: authenticate and recheck the selection on this same leased client
/// before any source lifecycle operation or prompt delivery.
pub(crate) async fn selected_preparation_on_server(
    state: &LoomState,
    base_url: &str,
    session_id: &str,
    change_id: &str,
    expected: &Change,
    text: &str,
) -> Result<(), UiError> {
    async {
        if text.trim().is_empty() {
            return Err(resource_error("A bounded selected preparation request is required."));
        }
        let client = selected_client(state, base_url).await?;
        change_changes_with_client(&client, session_id, change_id, expected).await?;
        let mut source = client.get_session(session_id).await?;
        verify_preparation_source(&client, &source, expected).await?;
        if source.status == "orphaned" {
            source = match client.adopt(session_id).await {
                Ok(adopted) => adopted,
                Err(error @ crate::client::LoomError::Api { status: 409, .. }) => {
                    let current = client.get_session(session_id).await?;
                    if current.status != "running" {
                        return Err(error.into());
                    }
                    current
                }
                Err(error) => return Err(error.into()),
            };
            verify_preparation_source(&client, &source, expected).await?;
        }
        recheck_selection(&client, &source, expected).await?;
        if crate::commands::host_is_loopback(client.server_host().unwrap_or("")) {
            verify_local_scope(&source, expected).await?;
        }
        let prompt = format!("{text}\n\nSelected Change preparation ONLY. Exact declaration: {}. Re-read this original identity, exact scope/completion and change:<id>/change_acceptance:<id> before preparing. Preserve siblings and foreign edits; changed scope requires fresh exact review. Do not accept, publish, push, open a PR or stamp integration_ready/integration_result under this preparation request.", serde_json::to_string(expected).unwrap());
        if source.protocol == "terminal" {
            client.send_text(session_id, &prompt, true).await?;
        } else {
            client.send_prompt(session_id, &prompt, &[]).await?;
        }
        Ok(())
    }
    .await
    .map_err(|error: UiError| error.for_server(base_url))
}

/// Validate the fresh runtime view itself before any selected lifecycle write.
/// The generic chat resume helper intentionally has no Change operands.
async fn verify_preparation_source(
    client: &LoomClient,
    source: &SessionView,
    expected: &Change,
) -> Result<(), UiError> {
    expected.verify_source(source).map_err(resource_error)?;
    ChangeRegistry::parse(&source.branch.tags, source)
        .map_err(resource_error)?
        .select(&expected.id, expected)
        .map_err(resource_error)?;
    if expected.scope.working_version().is_some() {
        verify_working_snapshot(expected, &client.session_changes(&source.id).await?)?;
    }
    if crate::commands::host_is_loopback(client.server_host().unwrap_or("")) {
        verify_local_scope(source, expected).await?;
    }
    Ok(())
}

pub(crate) async fn change_changes_on_server(
    state: &LoomState,
    base_url: &str,
    session_id: &str,
    change_id: &str,
    expected: &Change,
) -> Result<ReviewChanges, UiError> {
    async {
        let client = selected_client(state, base_url).await?;
        change_changes_with_client(&client, session_id, change_id, expected).await
    }
    .await
    .map_err(|error: UiError| error.for_server(base_url))
}

/// Queue delivery has a live backend prerequisite that ordinary chat does not.
/// Keep its gates on this leased client, after helper reads and immediately
/// before each lifecycle or instruction dispatch; an earlier gate can go stale
/// even without an outage or replacement of the selected client.
async fn deliver_queue_wrap_up(
    client: &LoomClient,
    source: &SessionView,
    expected: &Change,
    text: &str,
    payload: serde_json::Value,
) -> Result<(), UiError> {
    if source.protocol == "terminal" {
        let key = crate::client::request_key(&source.id, text, &payload);
        client.ensure_request_route(source).await?;
        crate::change_queue::require_atomic_cas(client).await?;
        client
            .send_to_thread(&source.id, text, payload, &key)
            .await?;
    } else {
        let verify_runtime = |runtime: &SessionView| -> Result<(), UiError> {
            expected.verify_source(runtime).map_err(resource_error)?;
            ChangeRegistry::parse(&runtime.branch.tags, runtime)
                .map_err(resource_error)?
                .select(&expected.id, expected)
                .map_err(resource_error)?;
            if runtime.protocol != source.protocol || runtime.status == "archived" {
                return Err(resource_error(
                    "Selected source runtime changed; refresh before delivery.",
                ));
            }
            Ok(())
        };
        let mut runtime = client.get_session(&source.id).await?;
        verify_runtime(&runtime)?;
        if runtime.status == "orphaned" {
            crate::change_queue::require_atomic_cas(client).await?;
            runtime = match client.adopt(&source.id).await {
                Ok(adopted) => adopted,
                Err(error @ crate::client::LoomError::Api { status: 409, .. }) => {
                    let current = client.get_session(&source.id).await?;
                    if current.status != "running" {
                        return Err(error.into());
                    }
                    current
                }
                Err(error) => return Err(error.into()),
            };
            verify_runtime(&runtime)?;
        }
        crate::change_queue::require_atomic_cas(client).await?;
        client.queue_prompt(&source.id, text).await?;
    }
    Ok(())
}

pub(crate) async fn land_change_on_server(
    state: &LoomState,
    base_url: &str,
    session_id: &str,
    change_id: &str,
    expected: &Change,
    strategy: Option<&str>,
) -> Result<IntegrationTarget, UiError> {
    async {
        let client = selected_client(state, base_url).await?;
        land_change_with_client(&client, session_id, change_id, expected, strategy).await
    }
    .await
    .map_err(|error: UiError| error.for_server(base_url))
}

pub(crate) async fn selected_source(
    client: &LoomClient,
    session_id: &str,
    change_id: &str,
    expected: &Change,
) -> Result<SessionView, UiError> {
    let source = client.get_session(session_id).await?;
    if source.id != session_id || expected.source_session != session_id {
        return Err(resource_error("Selected source session identity changed."));
    }
    let registry = ChangeRegistry::parse(&source.branch.tags, &source).map_err(resource_error)?;
    registry
        .select(change_id, expected)
        .map_err(resource_error)?;
    // Eligibility belongs to the declared output and authenticated completion.
    // A source's current role may have changed after producing this range.
    if let Some(completion) = &expected.completion {
        // Existing list API is ascending and page size capped at 500. Bound the
        // original lookup to 10 pages; missing evidence fails closed, never readiness.
        let mut after = 0;
        let mut found = false;
        for _ in 0..10 {
            let messages = client.channel_results(&source.id, after).await?;
            if messages
                .iter()
                .any(|m| m.seq <= after || m.channel_id != source.id)
                || messages.windows(2).any(|m| m[0].seq >= m[1].seq)
            {
                return Err(resource_error("Malformed original completion history."));
            }
            if let Some(message) = messages.iter().find(|m| m.id == completion.message_id) {
                crate::change::verify_completion(expected, message, &source)
                    .map_err(resource_error)?;
                found = true;
                break;
            }
            if messages.len() < 500 {
                break;
            }
            after = messages.last().unwrap().seq;
        }
        if !found {
            return Err(resource_error(
                "Original completion reference not found in bounded own-channel history.",
            ));
        }
    }
    Ok(source)
}

pub(crate) fn verify_working_snapshot(
    change: &Change,
    snapshot: &ChangeSetView,
) -> Result<(), UiError> {
    if let Some(version) = change.scope.working_version() {
        if !snapshot
            .head_oid
            .as_deref()
            .is_some_and(|oid| oid.eq_ignore_ascii_case(change.scope.head_oid()))
            || snapshot.version.as_deref() != Some(version)
        {
            return Err(resource_error(
                "Working Change HEAD or worktree version changed; reselect and review.",
            ));
        }
    }
    Ok(())
}

pub(crate) async fn recheck_selection(
    client: &LoomClient,
    source: &SessionView,
    change: &Change,
) -> Result<(), UiError> {
    let fresh = client.get_session(&source.id).await?;
    change.verify_source(&fresh).map_err(resource_error)?;
    ChangeRegistry::parse(&fresh.branch.tags, &fresh)
        .map_err(resource_error)?
        .select(&change.id, change)
        .map_err(resource_error)?;
    if change.scope.working_version().is_some() {
        verify_working_snapshot(change, &client.session_changes(&source.id).await?)?;
    }
    Ok(())
}

pub(crate) async fn change_changes_with_client(
    client: &LoomClient,
    session_id: &str,
    change_id: &str,
    expected: &Change,
) -> Result<ReviewChanges, UiError> {
    let source = selected_source(client, session_id, change_id, expected).await?;
    if expected.scope.working_version().is_some() {
        verify_working_snapshot(expected, &client.session_changes(session_id).await?)?;
    }
    let preview = if crate::commands::host_is_loopback(client.server_host().unwrap_or("")) {
        local_change_preview(&source, expected).await?
    } else {
        ReviewChanges {
            source_ref: expected.scope.head_oid().into(),
            accepted_source: false,
            comparison: "change-scope",
            changes: ChangeSetView {
                base: serde_json::json!({"state":"unavailable","reference":expected.scope.base_oid(),"reason":"remote_change_preview_unavailable"}),
                ..Default::default()
            },
        }
    };
    recheck_selection(client, &source, expected).await?;
    Ok(preview)
}

#[tauri::command]
pub async fn change_changes(
    state: State<'_, LoomState>,
    base_url: String,
    session_id: String,
    change_id: String,
    expected_change: Change,
) -> Result<ReviewChanges, UiError> {
    change_changes_on_server(&state, &base_url, &session_id, &change_id, &expected_change).await
}

pub fn selected_wrap_up_prompt(request: &IntegrationRequest, change: &Change) -> String {
    format!("Selected Change Land wrap-up. Exact declaration: {}\n\n{}\n\nFinish ONLY the selected bounded output, preserving all siblings and foreign staged/unstaged/untracked/ignored files. Do not expand this request into cumulative branch preparation. For a working declaration retain ID, original source identity/base and exact literal paths, prove pinned working HEAD reaches the finalized head; finalize its intended output to an exact range, review that fresh scope, send the complete final JSON result first with inline completion:null, capture returned message ID, then set ONLY change:<id> with that own-channel completion reference. The coordinator carries this permitted working finalization through fresh exact scope review within the same Land request; no second Prepare click. For an existing completed range retain its immutable declaration AND original completion reference. Its authenticated successful final result already exists: do not resend it, reindex it with a new completion, resume or retask the finished objective merely because this Land request arrived. The coordinator reviews/repairs the selected delta in isolation; report a concrete source issue only if one exists. For a working output, successful final result body or structured payload must be {{version:1,outcome:\"complete\",intent:\"final\",changes:[exact inline declarations with completion:null]}}. No checkpoints/help/partial/interrupted result qualifies.\n\nLIVE QUEUE BACKEND PREREQUISITE: Before processing this queued source wrap-up, authenticate GET /api/openapi.json on the owning backend and require exact boolean true at paths[\"/api/artifacts/write\"].post[\"x-loom-atomic-base-rev\"]. Missing/false/string/malformed/unreachable support requires a Loom update; do not resume or write under this queue request until supported. Never infer support from version, cache, reconnect or replacement or borrow another backend credential. Before EVERY automatic Git/index/ref/worktree write and resumption reread LIVE owning Arachne Track/source policy/identity/canonical/source revisions and authenticated interruption controls; effective Off defers automatic writes, and user interruption requires coordinator judgment, never automatic resumption. Parent get403 may use permitted LIVE sessions.list(history:true), never cached policy. Only the coordinator claims/clears/drains the queue; this source wrap-up never authorizes source acceptance.", serde_json::to_string(change).unwrap(), request.source_wrap_up_prompt())
}

pub fn selected_acceptance_prompt(request: &IntegrationRequest, change: &Change) -> String {
    // Legacy prose retains all ownership/initialization/interruption protections,
    // while selected receipt rules explicitly supersede the legacy blanket stamp.
    format!("Selected Change Land: {}\n\n{}\n\nSELECTED OUTPUT CONTRACT (supersedes any thread-wide scope or integration_result instruction above): {}\nFor the requested working-to-range finalization ONLY, retain ID/original identity/base/exact bounded paths and prove pinned working HEAD reaches finalized head, verify its new own-channel complete final result and freshly review exact finalized scope within this same Land request. Every other declaration change fails closed for reselection. Carry the exact selected declaration through review, repair, commit, validate, retries and acceptance; return a per-output actual outcome. The full request already authorizes completion without a second Prepare click.\n\n{}", serde_json::to_string(change).unwrap(), request.to_prompt("owning Track").replace("Record integration_result on the ORIGINAL source only after actual successful canonical acceptance and validation", "Record only the matching per-Change receipt on the ORIGINAL source after actual successful canonical acceptance and validation"), SELECTED_ACCEPTANCE_GUARD, crate::change_queue::DRAIN_PROTOCOL)
}

pub(crate) async fn land_change_with_client(
    client: &LoomClient,
    session_id: &str,
    change_id: &str,
    expected: &Change,
    strategy: Option<&str>,
) -> Result<IntegrationTarget, UiError> {
    let strategy = IntegrationStrategy::parse(
        strategy
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("squash"),
    )
    .filter(|s| {
        matches!(
            s,
            IntegrationStrategy::Squash
                | IntegrationStrategy::Merge
                | IntegrationStrategy::Rebase
                | IntegrationStrategy::CherryPick
        )
    })
    .ok_or_else(|| {
        resource_error(
            "Change Land accepts squash, merge, rebase or cherry-pick; publishing is separate.",
        )
    })?;
    let source = selected_source(client, session_id, change_id, expected).await?;
    let fleet = client.list_sessions().await?;
    let summary = fleet
        .iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| resource_error("Source no longer registered."))?;
    if summary.branch.repo_root != source.branch.repo_root
        || summary.branch.branch != source.branch.branch
    {
        return Err(resource_error("Registered source identity changed."));
    }
    let mut target = crate::commands::resolve_integration_target(&fleet, session_id)
        .ok_or_else(|| resource_error("No initialized live canonical owning Track."))?;
    let coordinator = client.get_session(&target.coordinator_id).await?;
    let mut owner = fleet
        .iter()
        .find(|s| s.id == target.coordinator_id)
        .unwrap()
        .clone();
    owner.branch = coordinator.branch.clone();
    if coordinator.id != target.coordinator_id
        || coordinator.status == "archived"
        || coordinator.branch.repo_root != source.branch.repo_root
        || !crate::track_policy::owns_track(&owner)
    {
        return Err(resource_error(
            "Canonical Track ownership or identity changed.",
        ));
    }
    target.target_branch = crate::track_policy::canonical_topic_branch(&coordinator.branch)
        .ok_or_else(|| resource_error("Canonical initialization is incomplete or invalid."))?
        .into();
    if target.target_branch == source.branch.branch {
        return Err(resource_error("Source equals canonical target."));
    }
    let snapshot = if expected.scope.working_version().is_some() {
        let snapshot = client.session_changes(session_id).await?;
        verify_working_snapshot(expected, &snapshot)?;
        Some(snapshot)
    } else {
        None
    };
    if crate::commands::host_is_loopback(client.server_host().unwrap_or("")) {
        let git_root = verify_local_scope(&source, expected).await?;
        crate::commands::review_oid(&git_root, &format!("refs/heads/{}", target.target_branch))
            .await?;
    }
    // Unsupported Loom is an update prerequisite before any route/lifecycle or
    // acceptance dispatch, including a saved-intent delivery retry.
    crate::change_queue::require_atomic_cas(client).await?;
    crate::commands::ensure_acceptance_route(client, &fleet, summary, &target.coordinator_id)
        .await?;
    // A completed range already has authenticated final evidence. Its acceptance
    // must not depend on a source runtime or create a fresh source turn.
    if !expected.scope.is_range() {
        client.ensure_request_route(&source).await?;
    }
    client.ensure_request_route(&coordinator).await?;
    // Detect registry and live owner changes caused during route/preparation reads.
    recheck_selection(client, &source, expected).await?;
    let fresh_fleet = client.list_sessions().await?;
    let fresh_target = crate::commands::resolve_integration_target(&fresh_fleet, session_id)
        .ok_or_else(|| resource_error("Owning Track changed during selected authorization."))?;
    let fresh_coordinator = client.get_session(&target.coordinator_id).await?;
    let fresh_summary = fresh_fleet
        .iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| resource_error("Selected source disappeared from registry."))?;
    let fresh_owner = fresh_fleet
        .iter()
        .find(|s| s.id == target.coordinator_id)
        .ok_or_else(|| resource_error("Owning Track disappeared from registry."))?;
    let mut live_owner = fresh_owner.clone();
    live_owner.branch = fresh_coordinator.branch.clone();
    if fresh_summary.branch.repo_root != expected.repo_root
        || fresh_summary.branch.branch != expected.source_branch
        || crate::track_policy::immediate_parent(&fresh_fleet, fresh_summary).map(|s| &s.id)
            != crate::track_policy::immediate_parent(&fleet, summary).map(|s| &s.id)
        || !crate::track_policy::owns_track(&live_owner)
        || fresh_target.coordinator_id != target.coordinator_id
        || fresh_target.target_branch != target.target_branch
        || fresh_coordinator.id != target.coordinator_id
        || fresh_coordinator.status == "archived"
        || fresh_coordinator.branch.repo_root != source.branch.repo_root
        || crate::track_policy::canonical_topic_branch(&fresh_coordinator.branch)
            != Some(target.target_branch.as_str())
    {
        return Err(resource_error(
            "Canonical ownership/initialization changed; refresh selected Land.",
        ));
    }
    crate::change_queue::verify_canonical_owner(&fresh_fleet, &fresh_coordinator)?;
    let request = IntegrationRequest {
        action: "land-change",
        source_session: source.id.clone(),
        source_branch: source.branch.branch.clone(),
        source_work_dir: source.work_dir.clone(),
        repo_root: source.branch.repo_root.clone(),
        target_session: target.coordinator_id.clone(),
        target_branch: target.target_branch.clone(),
        strategy,
        requested_by: "user",
    };
    // Immutable selection itself supplies retry identity. Branch activity must not
    // create a fresh authorization for a pinned range or collide with a sibling.
    let source_request_revision = expected.scope.head_oid();
    let source_request_version = snapshot.as_ref().and_then(|s| s.version.as_deref());
    let payload = |action| serde_json::json!({"action":action,"request":request,"expected_change":expected,"change_id":change_id,"source_request_revision":source_request_revision,"source_request_version":source_request_version});
    let queued =
        crate::change_queue::promote(client, &fresh_coordinator, expected, strategy.as_str())
            .await?;
    let queued_error = |stage: &str, error: UiError| UiError {
        message: format!(
            "Change remains queued at revision {}; {stage} delivery failed or is uncertain. \
             Acceptance is not confirmed. Repeat the identical Land request to retry delivery \
             (reconcile uncertain ACP delivery first). {}",
            queued.revision, error.message
        ),
        ..error
    };
    if !expected.scope.is_range() {
        crate::change_queue::require_atomic_cas(client)
            .await
            .map_err(|e| queued_error("Source atomic capability", e))?;
        deliver_queue_wrap_up(
            client,
            &source,
            expected,
            &selected_wrap_up_prompt(&request, expected),
            payload("land-change-wrap-up"),
        )
        .await
        .map_err(|e| queued_error("Source wrap-up", e))?;
    }
    crate::change_queue::require_atomic_cas(client)
        .await
        .map_err(|e| queued_error("Coordinator atomic capability", e))?;
    // An ordinary channel message cancels ACP's live turn in Loom. Persist an
    // idempotent status instruction, then queue its wake behind that turn.
    // Terminal coordinators retain the existing durable inbox delivery path.
    let prompt = selected_acceptance_prompt(&request, expected);
    let payload = payload("land-change");
    let key = crate::client::request_key(&coordinator.id, &prompt, &payload);
    client
        .ensure_delivery_subscription(&coordinator.id, &coordinator.id)
        .await
        .map_err(|e| queued_error("Coordinator route", e.into()))?;
    crate::change_queue::require_atomic_cas(client)
        .await
        .map_err(|e| queued_error("Coordinator dispatch capability", e))?;
    if coordinator.protocol == "terminal" {
        client
            .send_to_thread(&coordinator.id, &prompt, payload, &key)
            .await
            .map_err(|e| queued_error("Coordinator queue wake", e.into()))?;
    } else {
        client
            .record_request(&coordinator.id, &prompt, payload, &key)
            .await
            .map_err(|e| queued_error("Coordinator durable instruction", e.into()))?;
        // This existing prompt API has no idempotent receipt; duplicate wakes
        // cannot duplicate acceptance because live CAS claims/receipts govern it.
        crate::change_queue::require_atomic_cas(client)
            .await
            .map_err(|e| queued_error("Coordinator ACP wake capability", e))?;
        client
            .queue_prompt(&coordinator.id, &prompt)
            .await
            .map_err(|e| queued_error("Coordinator ACP queue wake", e.into()))?;
    }
    Ok(target)
}

#[tauri::command]
pub async fn land_change(
    app: AppHandle,
    state: State<'_, LoomState>,
    base_url: String,
    session_id: String,
    change_id: String,
    expected_change: Change,
    strategy: Option<String>,
) -> Result<IntegrationTarget, UiError> {
    let target = land_change_on_server(
        &state,
        &base_url,
        &session_id,
        &change_id,
        &expected_change,
        strategy.as_deref(),
    )
    .await?;
    let _ = app.emit(
        "loom://integration-sent",
        serde_json::json!({
            "server_url": crate::routing::normalize_server_url(&base_url).unwrap(),
            "coordinator_id": target.coordinator_id,
            "coordinator_name": target.coordinator_name,
            "target_branch": target.target_branch,
        }),
    );
    Ok(target)
}

/// Resolve only the declared repository's Git store, never an incidental
/// enclosing repository or a replacement checkout at the original path.
async fn local_scope_root(
    source: &SessionView,
    change: &Change,
) -> Result<std::path::PathBuf, UiError> {
    use crate::commands::review_git;
    async fn git_directory(
        root: &std::path::Path,
        arg: &str,
    ) -> Result<std::path::PathBuf, UiError> {
        let (bytes, truncated) = review_git(
            root,
            &["rev-parse", "--path-format=absolute", arg],
            64 * 1024,
        )
        .await?;
        let value = std::str::from_utf8(&bytes).map_err(|e| resource_error(e.to_string()))?;
        // Preserve literal spaces in repository names; Git adds only a newline.
        let value = value.strip_suffix('\n').unwrap_or(value);
        if truncated || value.is_empty() || !std::path::Path::new(value).is_absolute() {
            return Err(resource_error("Invalid declared repository Git directory."));
        }
        tokio::fs::canonicalize(value)
            .await
            .map_err(|e| resource_error(e.to_string()))
    }
    change.verify_source(source).map_err(resource_error)?;
    let repo = std::path::Path::new(&source.branch.repo_root);
    let repo_path = tokio::fs::canonicalize(repo)
        .await
        .map_err(|e| resource_error(e.to_string()))?;
    let (bare, truncated) = review_git(repo, &["rev-parse", "--is-bare-repository"], 16).await?;
    let identity = match (bare.as_slice(), truncated) {
        (b"true\n", false) => git_directory(repo, "--absolute-git-dir").await?,
        (b"false\n", false) => git_directory(repo, "--show-toplevel").await?,
        _ => return Err(resource_error("Invalid declared repository identity.")),
    };
    if identity != repo_path {
        return Err(resource_error(
            "Declared repository root is not the Git repository root.",
        ));
    }
    let common = git_directory(repo, "--git-common-dir").await?;
    let checkout = std::path::Path::new(&source.work_dir);
    match tokio::fs::symlink_metadata(checkout).await {
        Ok(_) => {
            crate::landing::ensure_same_repository(checkout, repo)
                .await
                .map_err(resource_error)?;
            if git_directory(checkout, "--show-toplevel").await?
                != tokio::fs::canonicalize(checkout)
                    .await
                    .map_err(|e| resource_error(e.to_string()))?
            {
                return Err(resource_error(
                    "Original checkout path is not a Git worktree root.",
                ));
            }
            let (branch, truncated) =
                review_git(checkout, &["symbolic-ref", "--quiet", "HEAD"], 4096).await?;
            if truncated
                || String::from_utf8_lossy(&branch).trim()
                    != format!("refs/heads/{}", change.source_branch)
            {
                return Err(resource_error("Original checkout branch changed."));
            }
        }
        // A completed range's authenticated declaration preserves original
        // worktree identity after archival removes the linked checkout. Read
        // immutable objects and retained refs in its declared common store;
        // never recover the source or adopt a foreign path in its place.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && change.scope.is_range() => {}
        Err(error) => {
            return Err(resource_error(format!(
                "Original checkout unavailable: {error}"
            )))
        }
    }
    Ok(if change.scope.is_range() {
        common
    } else {
        checkout.to_path_buf()
    })
}

pub(crate) async fn verify_local_scope(
    source: &SessionView,
    change: &Change,
) -> Result<std::path::PathBuf, UiError> {
    use crate::commands::{review_git, review_oid};
    let git_root = local_scope_root(source, change).await?;
    let root = git_root.as_path();
    let branch_ref = format!("refs/heads/{}", change.source_branch);
    for oid in [change.scope.base_oid(), change.scope.head_oid()] {
        if !review_oid(root, oid).await?.eq_ignore_ascii_case(oid) {
            return Err(resource_error(
                "Pinned Change object does not resolve exactly.",
            ));
        }
    }
    review_git(
        root,
        &[
            "merge-base",
            "--is-ancestor",
            change.scope.base_oid(),
            change.scope.head_oid(),
        ],
        128,
    )
    .await?;
    if change.scope.is_range() {
        // Branch may advance; equality to current HEAD would destroy immutable selection.
        review_git(
            root,
            &[
                "merge-base",
                "--is-ancestor",
                change.scope.head_oid(),
                &branch_ref,
            ],
            128,
        )
        .await?;
    } else if !review_oid(root, "HEAD")
        .await?
        .eq_ignore_ascii_case(change.scope.head_oid())
    {
        return Err(resource_error("Working Change HEAD changed."));
    }
    Ok(git_root)
}

pub(crate) async fn local_change_preview(
    source: &SessionView,
    change: &Change,
) -> Result<ReviewChanges, UiError> {
    use crate::commands::{review_git, review_hunks, review_path};
    let git_root = verify_local_scope(source, change).await?;
    let root = git_root.as_path();
    let committed = change.scope.is_range();
    // Working scope includes the bounded committed/uncommitted output since
    // its explicit base, guarded by the pinned HEAD and worktree version.
    let base = change.scope.base_oid();
    let mut args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-renames",
        "--ignore-submodules=dirty",
        "--name-status",
        "-z",
        base,
    ];
    if committed {
        args.push(change.scope.head_oid());
    }
    args.push("--");
    args.extend(change.scope.paths().iter().map(String::as_str));
    let (inventory, truncated) = review_git(root, &args, 8 * 1024 * 1024).await?;
    if truncated {
        return Err(resource_error(
            "Selected change inventory exceeds preview limits.",
        ));
    }
    let mut paths = Vec::new();
    let mut items = inventory.split(|b| *b == 0).filter(|p| !p.is_empty());
    while let Some(status) = items.next() {
        let path = items
            .next()
            .ok_or_else(|| resource_error("Incomplete selected inventory."))?;
        let status = match status.first() {
            Some(b'A') => "added",
            Some(b'D') => "deleted",
            Some(b'T') => "type_changed",
            _ => "modified",
        };
        paths.push((status, path.to_vec(), false));
    }
    if !committed {
        let mut args = vec!["ls-files", "--others", "--exclude-standard", "-z", "--"];
        args.extend(change.scope.paths().iter().map(String::as_str));
        let (untracked, truncated) = review_git(root, &args, 8 * 1024 * 1024).await?;
        if truncated {
            return Err(resource_error(
                "Selected untracked inventory exceeds preview limits.",
            ));
        }
        paths.extend(
            untracked
                .split(|b| *b == 0)
                .filter(|p| !p.is_empty())
                .map(|p| ("added", p.to_vec(), true)),
        );
    }
    let mut totals = crate::loom::ChangeTotalsView {
        files: paths.len().min(u32::MAX as usize) as u32,
        truncated: paths.len() > 100,
        ..Default::default()
    };
    let mut files = Vec::new();
    let mut line_budget = 8000;
    for (status, path, untracked) in paths.into_iter().take(100) {
        let mut content = "text";
        let mut patch = Vec::new();
        let mut truncated = false;
        if let Ok(path) = std::str::from_utf8(&path) {
            if untracked {
                use tokio::io::AsyncReadExt;
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
                    "--no-renames",
                    "--ignore-submodules=dirty",
                    "--unified=3",
                    base,
                ];
                if committed {
                    args.push(change.scope.head_oid());
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
        files.push(serde_json::json!({"status":status,"path":review_path(&path),"old_path":null,"sources":[if committed {"committed"} else if untracked {"untracked"} else {"checkout"}],"content":content,"hunks":hunks,"additions":if content == "text" {Some(additions)} else {None},"deletions":if content == "text" {Some(deletions)} else {None},"truncated":truncated}));
    }
    if verify_local_scope(source, change).await? != git_root {
        return Err(resource_error(
            "Declared repository Git store changed during preview.",
        ));
    }
    Ok(ReviewChanges {
        source_ref: change.scope.head_oid().into(),
        accepted_source: false,
        comparison: "change-scope",
        changes: ChangeSetView {
            version: change.scope.working_version().map(String::from),
            base: serde_json::json!({"state":"available","reference":base,"oid":base}),
            head_oid: Some(change.scope.head_oid().into()),
            truncated: totals.truncated,
            totals,
            files,
        },
    })
}

#[cfg(test)]
#[path = "change_tests.rs"]
mod tests;
