//! Explicit Track publishing. Acceptance/autoland never calls this module.
//!
//! An explicit canonical tag names committed accepted state, independently of
//! the coordinator's scratch checkout. Only legacy Tracks prepare that checkout.

use std::{path::Path, sync::Arc, time::Duration};

use serde::Serialize;
use tauri::State;

use crate::{
    client::LoomClient,
    commands::{LandResult, LoomState, UiError},
    landing::{self, LandingError, LocalLandingTarget, SourceCheckoutState},
    loom::{SessionSummaryView, SessionView},
};

fn error(message: impl Into<String>) -> UiError {
    UiError {
        message: message.into(),
        unreachable: false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Strategy {
    SinglePr,
    Stack,
    Local,
    Squash,
    Merge,
    Rebase,
    Push,
}

impl Strategy {
    fn parse(value: &str) -> Result<Self, UiError> {
        match value {
            "open-pr" => Ok(Self::SinglePr),
            "pr-stack" => Ok(Self::Stack),
            "land-locally" => Ok(Self::Local),
            "squash" => Ok(Self::Squash),
            "merge" => Ok(Self::Merge),
            "rebase" => Ok(Self::Rebase),
            "push" => Ok(Self::Push),
            _ => Err(error(format!(
                "unknown publish strategy {value:?}; choose a concrete publish strategy"
            ))),
        }
    }
    fn wire(self) -> &'static str {
        match self {
            Self::SinglePr => "open-pr",
            Self::Stack => "pr-stack",
            Self::Local => "land-locally",
            Self::Squash => "squash",
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::Push => "push",
        }
    }
}

fn local_client(client: &LoomClient) -> bool {
    matches!(
        client.server_host(),
        Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
    )
}

// Share short-ref syntax with Track creation, acceptance and frontend fixtures.
fn valid_branch(value: &str) -> bool {
    crate::track_policy::valid_topic_branch(value)
}

fn canonical_source(view: &SessionView) -> Result<(String, bool), UiError> {
    if crate::track_policy::tag(&view.branch, "topic") == Some("false") {
        return Err(error(
            "Publish belongs to a root Track; accept this Thread into its Track first.",
        ));
    }
    let tags: Vec<_> = view
        .branch
        .tags
        .iter()
        .filter(|tag| tag.key == "track_topic_branch")
        .collect();
    if tags.len() > 1 {
        return Err(error("The Track has conflicting canonical branch tags; repair its metadata before publishing."));
    }
    let explicit = !tags.is_empty();
    let branch = crate::track_policy::canonical_topic_branch(&view.branch).ok_or_else(|| {
        error("The Track's canonical topic branch is invalid or not initialized; finish canonical initialization before publishing. Never substitute coordinator scratch.")
    })?;
    Ok((branch.into(), explicit))
}

/// Only the user-selected Publish action invokes this command.
#[tauri::command]
pub async fn publish_track(
    state: State<'_, LoomState>,
    session_id: String,
    strategy: String,
    upstream: Option<String>,
) -> Result<LandResult, UiError> {
    let client = state.client.read().await.clone().ok_or_else(|| UiError {
        message: "not connected to loom".into(),
        unreachable: true,
    })?;
    publish(&client, &session_id, &strategy, upstream).await
}

async fn publish(
    client: &Arc<LoomClient>,
    id: &str,
    strategy: &str,
    upstream: Option<String>,
) -> Result<LandResult, UiError> {
    let strategy = Strategy::parse(strategy)?;
    let view = client.get_session(id).await?;
    if view.id != id {
        return Err(error(
            "The server returned a different Track; refresh before publishing.",
        ));
    }
    if view.status == "archived" {
        return Err(error("The Track is archived; reopen it before publishing."));
    }
    let fleet = client.list_sessions().await?;
    let summary = fleet
        .iter()
        .find(|session| session.id == id)
        .ok_or_else(|| error("The Track is no longer in the fleet; refresh before publishing."))?;
    if summary.parent_id.is_some() || summary.parent_session_id.is_some() {
        return Err(error(
            "Publish belongs to a root Track; accept this Thread into its Track first.",
        ));
    }
    let (source, explicit) = canonical_source(&view)?;
    if strategy == Strategy::Local {
        if upstream.as_deref().is_some_and(|s| !s.trim().is_empty()) {
            return Err(error("land-locally always uses the primary checkout's CURRENT branch; clear the upstream override."));
        }
        return publish_local(client, &view, summary, &source, explicit).await;
    }
    let repo_branches = client.repo_branches(&view.branch.repo_root).await?;
    if !repo_branches.iter().any(|branch| branch.name == source) {
        return Err(error(format!("Canonical accepted branch {source} is not initialized in the Track repository. Initialize it from the Track seed and verify it before publishing; never substitute coordinator scratch.")));
    }
    let upstream = upstream.filter(|s| !s.trim().is_empty());
    let (target, origin) = if let Some(target) = upstream {
        (target, "explicit")
    } else if matches!(strategy, Strategy::SinglePr | Strategy::Stack) {
        let branches = client.list_branches().await?;
        let base = |name: &str| {
            branches
                .iter()
                .find(|row| {
                    row["branch"].as_str() == Some(name)
                        && row["repo_root"]
                            .as_str()
                            .is_none_or(|repo| repo == view.branch.repo_root)
                })
                .and_then(|row| row["base_branch"].as_str())
                .filter(|base| !base.trim().is_empty())
                .map(String::from)
        };
        (
            base(&source)
                .or_else(|| base(&view.branch.branch))
                .ok_or_else(|| {
                    error("No PR base is recorded; choose an explicit upstream branch.")
                })?,
            "recorded-base",
        )
    } else {
        if let Some(current) = repo_branches.iter().find(|branch| {
            branch.current && branch.worktree.as_deref() == Some(view.branch.repo_root.as_str())
        }) {
            (current.name.clone(), "primary-checkout")
        } else if repo_branches
            .iter()
            .any(|branch| branch.name == "main" && branch.name != source)
        {
            ("main".into(), "main-fallback")
        } else {
            return Err(error("No current primary target branch or main fallback is available; check out a branch or choose an explicit upstream."));
        }
    };
    if !valid_branch(&target) || target == source {
        return Err(error(
            "Choose a valid upstream branch different from the canonical accepted branch.",
        ));
    }
    let mut request =
        PublishRequest::new(&view, source.clone(), explicit, strategy, target, origin);
    request.source_checkout = repo_branches
        .iter()
        .find(|branch| branch.name == source)
        .and_then(|branch| branch.worktree.clone());
    if local_client(client) {
        request.target_revision = git(
            Path::new(&view.branch.repo_root),
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}^{{commit}}", request.target_upstream),
            ],
        )
        .await
        .ok();
        request.source_revision =
            Some(verify_ref(Path::new(&view.branch.repo_root), &source).await?);
        if !explicit {
            landing::ensure_same_repository(
                Path::new(&view.work_dir),
                Path::new(&view.branch.repo_root),
            )
            .await
            .map_err(error)?;
            request.legacy_source_state = Some(
                landing::inspect_source_checkout(Path::new(&view.work_dir), &source)
                    .await
                    .map_err(error)?,
            );
            request.source_checkout = Some(view.work_dir.clone());
        }
        // A dirty legacy source is intentionally prepared by the coordinator;
        // its final accepted history must be derived after preparation.
        if strategy == Strategy::Stack
            && request
                .legacy_source_state
                .as_ref()
                .is_none_or(|s| s.status.trim().is_empty())
        {
            request.stack = stack_plan(
                Path::new(&view.branch.repo_root),
                request.source_revision.as_deref().unwrap(),
                &request.target_upstream,
            )
            .await?;
        }
    }
    dispatch(client, &view, request).await
}

async fn git(repo: &Path, args: &[&str]) -> Result<String, UiError> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .map_err(|_| error("Git publish preflight timed out; refresh and retry."))?
        .map_err(|e| error(format!("Git publish preflight failed: {e}")))?;
    if !output.status.success() {
        return Err(error(format!(
            "Git publish preflight: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

async fn verify_ref(repo: &Path, branch: &str) -> Result<String, UiError> {
    git(repo, &["check-ref-format", "--branch", branch]).await?;
    git(repo, &["rev-parse", "--verify", &format!("refs/heads/{branch}^{{commit}}")]).await
        .map_err(|_| error(format!("Canonical accepted branch {branch} is missing or uninitialized. Initialize it from the Track seed before publishing; coordinator scratch is never a substitute.")))
}

#[derive(Debug, Serialize)]
struct StackEntry {
    order: usize,
    accepted_commit: String,
    head_ref: String,
    base_ref: String,
}

async fn stack_plan(repo: &Path, revision: &str, target: &str) -> Result<Vec<StackEntry>, UiError> {
    let base = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{target}^{{commit}}"),
        ],
    )
    .await?;
    let commits = git(
        repo,
        &[
            "rev-list",
            "--reverse",
            "--first-parent",
            &format!("{base}..{revision}"),
        ],
    )
    .await?;
    let commits: Vec<_> = commits.lines().filter(|line| !line.is_empty()).collect();
    if commits.len() < 2 {
        return Err(error("PR stack needs at least two accepted changes; choose Single PR for this accepted history."));
    }
    let prefix = format!("arachne/publish-stack/{}", &revision[..12]);
    let mut previous = target.to_owned();
    Ok(commits
        .iter()
        .enumerate()
        .map(|(index, commit)| {
            let head = format!("{prefix}/{}", index + 1);
            let entry = StackEntry {
                order: index + 1,
                accepted_commit: (*commit).into(),
                head_ref: head.clone(),
                base_ref: previous.clone(),
            };
            previous = head;
            entry
        })
        .collect())
}

#[derive(Serialize)]
struct PublishRequest {
    action: &'static str,
    requested_by: &'static str,
    authorization: &'static str,
    source_session: String,
    repo_root: String,
    source_branch: String,
    source_ref: String,
    source_revision: Option<String>,
    source_checkout: Option<String>,
    coordinator_scratch_checkout: String,
    explicit_canonical: bool,
    legacy_source_state: Option<SourceCheckoutState>,
    strategy: &'static str,
    target_upstream: String,
    target_revision: Option<String>,
    target_origin: String,
    local_target: Option<LocalLandingTarget>,
    repair_reason: Option<String>,
    stack: Vec<StackEntry>,
}

impl PublishRequest {
    fn new(
        view: &SessionView,
        source: String,
        explicit: bool,
        strategy: Strategy,
        target: String,
        origin: &str,
    ) -> Self {
        Self {
            action: "publish",
            requested_by: "user",
            authorization: "explicit-chosen-publish",
            source_session: view.id.clone(),
            repo_root: view.branch.repo_root.clone(),
            source_ref: format!("refs/heads/{source}"),
            source_branch: source,
            source_revision: None,
            source_checkout: None,
            coordinator_scratch_checkout: view.work_dir.clone(),
            explicit_canonical: explicit,
            legacy_source_state: None,
            strategy: strategy.wire(),
            target_upstream: target,
            target_revision: None,
            target_origin: origin.into(),
            local_target: None,
            repair_reason: None,
            stack: Vec::new(),
        }
    }
    fn prompt(&self) -> String {
        let mode = match self.strategy {
            "open-pr" => "Single PR: validate the entire committed canonical diff against the chosen upstream, push the canonical head safely, create one actual PR with that head/base, and report its actual URL and head SHA.",
            "pr-stack" => "PR stack: derive an ordered list of accepted commits/branches from canonical history only. The stack array, when present, is an initial cumulative plan: each accepted_commit is a committed canonical ancestor, each head_ref is a separate cumulative branch, and each base_ref is the upstream for entry 1 or the preceding entry's head_ref. Reverify the plan against live history; if missing, construct this same structured plan with order, accepted_commit, head_ref and base_ref before writing. Preserve merge dependencies and the complete canonical final tree. Verify each head/base diff is nonempty and validate each dependent change. Create branches at their accepted commits without overwriting occupied unrelated refs; safely push them in dependency order, create an actual separate PR for EACH entry using its explicit dependent head/base, then report an ordered array of distinct actual PR URLs, head/base refs, SHAs and validation results. At least two separate PRs are required. Never describe a single PR or a list of proposed links as a stack; if fewer than two valid accepted units exist, report a blocker and require a fresh Single PR choice.",
            "land-locally" => "Land locally: squash committed canonical accepted state into the primary checkout's CURRENT branch, as captured in local_target. Reverify exact checkout, branch, target revision/status and canonical revision immediately before writing. If target changed, stop for fresh review. Preserve ALL foreign staged, unstaged and untracked target edits; use a separate temporary index/checkout when needed, never absorb foreign staging into the commit or use destructive resets/stashes. Validate/review and repair conflicts, then complete this same chosen squash in one request. Never push or open a PR. Report the actual target commit and checks.",
            "squash" => "Squash committed canonical changes into the chosen target, validate and report the actual target commit; do not push or open PRs.",
            "merge" => "Merge the committed canonical ref into the chosen target, validate and report the actual merge commit; do not push or open PRs.",
            "rebase" => "Rebase accepted commits onto the chosen target in an isolated publishing branch/checkout; preserve the canonical accepted ref and coordinator scratch, validate and report the resulting target commits. Do not push or open PRs.",
            "push" => "Push the committed canonical ref to the explicitly chosen upstream destination without force; verify the actual remote ref and SHA, and report them. Do not open PRs or land into another local branch.",
            _ => unreachable!(),
        };
        let preparation = if self.explicit_canonical {
            "The source is ONLY the committed canonical source_ref. Coordinator scratch commits, staged, unstaged and untracked edits are NOT accepted state: never include, commit, merge or publish them. A source checkout may differ from coordinator scratch or be absent; use an isolated checkout of source_revision when necessary. Do not initialize a missing explicit canonical ref from scratch; report a blocker and require Track seed initialization."
        } else {
            "Legacy Track without a canonical tag: its coordinator branch remains canonical. Inspect/review and validate intended staged, unstaged and untracked source edits, commit only intended changes preserving foreign work, and refresh the committed canonical revision before performing this same chosen publish. Do not require a second Prepare click."
        };
        format!("Explicit Track Publish request\n\n```json\n{}\n```\n\nThis request authorizes ONLY the chosen {} action now; acceptance/autoland never authorizes publishing. Re-read LIVE Track marker, parentage and track_topic_branch before writing. If canonical metadata or accepted history changed, preserve this same chosen Publish authorization, perform fresh review and validation of the LIVE canonical accepted ref, refresh source/ref/revision and target snapshots, then continue this chosen operation. Never write the stale snapshot or substitute coordinator scratch. Stop for true repository/ownership/access blockers. For a new Track retaining the initialization goal marker, require the LIVE track_topic_initialized verification tag to exactly match the canonical ref; display tags or a coincidentally existing branch are insufficient. Verify the canonical ref really exists as a commit in this exact repository and matches source_revision when recorded, not metadata alone; perform fresh review before continuing if accepted history moved. One active writer per checkout; queue behind current work and establish child result channel wake delivery before waiting for any required acceptance.\n\n{preparation}\n\nInspect, review, repair and validate the accepted changes and chosen destination. For primary-checkout/main-fallback destinations, resolve the primary current branch again before writing; a main fallback is advisory and never authorizes a different destination without fresh review. {mode}\n\nA queued request is NOT a successful publish. Only after the actual chosen operation succeeds, stamp publish_result on this Track with actual commit/ref/PR URLs and report a typed result with chosen strategy, canonical source ref/SHA, target, actual outputs and checks. Never stamp integration_result for publishing. On partial stack failure, report each actual successful PR and each failure without stamping whole-stack success. No automatic or follow-up publish without explicit user choice.",
            serde_json::to_string(self).expect("publish request serialization"), self.strategy)
    }
}

async fn dispatch(
    client: &LoomClient,
    view: &SessionView,
    request: PublishRequest,
) -> Result<LandResult, UiError> {
    let prompt = request.prompt();
    client
        .deliver_request(
            view,
            &prompt,
            serde_json::to_value(&request).expect("publish request"),
        )
        .await?;
    Ok(LandResult {
        local: false,
        landing: None,
    })
}

async fn publish_local(
    client: &Arc<LoomClient>,
    view: &SessionView,
    summary: &SessionSummaryView,
    source: &str,
    explicit: bool,
) -> Result<LandResult, UiError> {
    if !local_client(client) {
        return Err(error("land-locally requires Loom on this machine. Connect to a local Loom server or choose an agent-mediated publish strategy for the remote repository."));
    }
    let repo = Path::new(&view.branch.repo_root);
    let revision = verify_ref(repo, source).await?;
    let legacy = if explicit {
        None
    } else {
        landing::ensure_same_repository(Path::new(&view.work_dir), repo)
            .await
            .map_err(error)?;
        Some(
            landing::inspect_source_checkout(Path::new(&view.work_dir), source)
                .await
                .map_err(error)?,
        )
    };
    let target = landing::inspect_local_target(repo, source)
        .await
        .map_err(|e| error(e.to_string()))?;
    // Re-read legacy editor saves after target inspection so a new save is
    // prepared rather than silently omitted from this one-click operation.
    let legacy = if legacy.is_some() {
        Some(
            landing::inspect_source_checkout(Path::new(&view.work_dir), source)
                .await
                .map_err(error)?,
        )
    } else {
        None
    };
    let refreshed = verify_ref(repo, source).await?;
    let summary_source = crate::track_policy::canonical_topic_branch(&summary.branch);
    let metadata_changed = summary_source != Some(source)
        || summary.branch.repo_root != view.branch.repo_root
        || crate::track_policy::tag(&summary.branch, "topic") == Some("false");
    let reason = if metadata_changed {
        Some("Canonical Track metadata changed between session and fleet reads; refresh the LIVE owning Track/source and review before this same chosen publish.".into())
    } else if revision != refreshed {
        Some(
            "Canonical accepted revision moved during preflight; refresh review and validation."
                .into(),
        )
    } else if legacy
        .as_ref()
        .is_some_and(|state| !state.status.trim().is_empty())
    {
        Some("Legacy canonical source has uncommitted edits; prepare intended changes and validate before the chosen publish.".into())
    } else if target
        .status
        .lines()
        .any(|line| !line.starts_with("??") && !line.starts_with(' '))
    {
        Some("Primary target has staged changes; preserve the entire foreign index and working tree during repair.".into())
    } else {
        None
    };
    let title = if !summary.branch.name.trim().is_empty() {
        &summary.branch.name
    } else {
        &view.branch.title
    };
    // The first session/fleet reads selected a destination; immediately before
    // synchronous Git writes, check that LIVE metadata still selects that ref.
    let reason = if reason.is_none() {
        let live = client.get_session(&view.id).await?;
        let fleet = client.list_sessions().await?;
        let owner = fleet.iter().find(|session| session.id == view.id);
        let unchanged = live.id == view.id
            && live.status != "archived"
            && live.branch.repo_root == view.branch.repo_root
            && canonical_source(&live)
                .is_ok_and(|(branch, mode)| branch == source && mode == explicit)
            && owner.is_some_and(|session| {
                session.parent_id.is_none()
                    && session.parent_session_id.is_none()
                    && session.branch.repo_root == view.branch.repo_root
                    && crate::track_policy::canonical_topic_branch(&session.branch) == Some(source)
                    && crate::track_policy::tag(&session.branch, "topic") != Some("false")
            });
        (!unchanged).then(|| "Canonical Track metadata changed before synchronous publication; reread LIVE ownership/ref and refresh review before the chosen publish.".into())
    } else {
        reason
    };
    let (completed, reason) = if reason.is_none() {
        match landing::squash_into_primary_checkout_pinned(repo, source, &refreshed, &target, title)
            .await
        {
            Ok(result) => (Some(result), None),
            Err(LandingError::StagedChanges) => (
                None,
                Some(
                    "Target acquired staged changes; preserve foreign staging during repair."
                        .into(),
                ),
            ),
            Err(LandingError::RepairNeeded { detail }) => (None, Some(detail)),
            Err(e) => return Err(error(e.to_string())),
        }
    } else {
        (None, reason)
    };
    if let Some(mut result) = completed {
        // The squash has already completed. A follow-up read cannot turn that
        // success into an ordinary failure inviting a duplicate Publish click.
        let target_sha = if result.landed {
            result.commit.clone()
        } else {
            Some(target.revision.clone())
        };
        let note = format!(
            "land-locally: published refs/heads/{source} at {refreshed} into {} at {}",
            result.target_branch,
            target_sha
                .as_deref()
                .unwrap_or("revision unavailable after completed Git operation")
        );
        // Git success stands even if metadata delivery fails, as in the existing
        // local helper. The UI still receives the completed Git result.
        if let Some(target_sha) = target_sha {
            if let Err(error) = client
                .set_tag(&view.id, "publish_result", &target_sha, &note)
                .await
            {
                result.warnings.push(format!(
                    "Git publication completed, but its result tag could not be confirmed: {error}"
                ));
            }
        }
        return Ok(LandResult {
            local: true,
            landing: Some(result.into()),
        });
    }
    let mut request = PublishRequest::new(
        view,
        source.into(),
        explicit,
        Strategy::Local,
        target.branch.clone(),
        "primary-checkout",
    );
    request.source_revision = Some(refreshed);
    request.source_checkout = if explicit {
        // Resolve the canonical checkout separately; scratch is not the source.
        canonical_checkout(repo, source).await?
    } else {
        Some(view.work_dir.clone())
    };
    request.legacy_source_state = legacy;
    request.target_revision = Some(target.revision.clone());
    request.local_target = Some(target);
    request.repair_reason = reason;
    dispatch(client, view, request).await
}

async fn canonical_checkout(repo: &Path, source: &str) -> Result<Option<String>, UiError> {
    let listing = git(repo, &["worktree", "list", "--porcelain"]).await?;
    let branch_line = format!("branch refs/heads/{source}");
    Ok(listing
        .split("\n\n")
        .find(|record| record.lines().any(|line| line == branch_line))
        .and_then(|record| {
            record
                .lines()
                .find_map(|line| line.strip_prefix("worktree "))
        })
        .map(String::from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU32, Ordering},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    static COUNTER: AtomicU32 = AtomicU32::new(0);
    struct Repo(PathBuf);
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn run(repo: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
    fn write(repo: &Path, name: &str, content: &str) {
        std::fs::write(repo.join(name), content).unwrap();
    }
    fn commit(repo: &Path, name: &str, content: &str) {
        write(repo, name, content);
        run(repo, &["add", name]);
        run(repo, &["commit", "-qm", name]);
    }
    fn fixture(explicit: bool) -> (Repo, SessionView, SessionSummaryView) {
        let root = std::env::temp_dir().join(format!(
            "arachne-publishing-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        run(&root, &["init", "-q", "-b", "current-dev"]);
        run(&root, &["config", "user.name", "Test"]);
        run(&root, &["config", "user.email", "test@example.com"]);
        run(&root, &["config", "commit.gpgsign", "false"]);
        commit(&root, "foreign.txt", "base\n");
        commit(&root, "conflict.txt", "base\n");
        run(
            &root,
            &["worktree", "add", "-qb", "coordinator-scratch", "scratch"],
        );
        let scratch = root.join("scratch");
        if explicit {
            run(
                &root,
                &["worktree", "add", "-qb", "accepted/topic", "accepted"],
            );
            commit(&root.join("accepted"), "accepted-one.txt", "one\n");
            commit(&root.join("accepted"), "accepted-two.txt", "two\n");
            commit(&scratch, "scratch-only.txt", "never publish\n");
        } else {
            commit(&scratch, "accepted-one.txt", "one\n");
        }
        std::fs::write(root.join(".git/info/exclude"), "scratch/\naccepted/\n").unwrap();
        let mut tags = vec![
            json!({"key":"topic", "value":"true", "note":"", "set_at":"now", "set_by":"user"}),
        ];
        if explicit {
            tags.push(json!({"key":"track_topic_branch", "value":"accepted/topic", "note":"", "set_at":"now", "set_by":"user"}));
        }
        let value = json!({
            "id":"track-session", "status":"running", "profile":"default", "class":"interactive", "origin":"user",
            "agent_kind":"codex", "model":"", "effort":"", "protocol":"acp", "work_dir":scratch,
            "term_session":"runtime", "turn_count":0, "created_by":null, "created_at":"now", "last_activity_at":"now",
            "branch":{"id":"track-branch", "branch":"coordinator-scratch", "name":"Track", "title":"Track", "repo_root":root, "tags":tags},
            "parent_id":null, "parent_session_id":null
        });
        (
            Repo(root),
            serde_json::from_value(value.clone()).unwrap(),
            serde_json::from_value(value).unwrap(),
        )
    }
    fn branches(repo: &Repo, accepted_checkout: bool) -> Value {
        json!([
            {"name":"current-dev", "current":true, "worktree":repo.0},
            {"name":"accepted/topic", "current":false, "worktree":if accepted_checkout {Some(repo.0.join("accepted"))} else {None}},
            {"name":"coordinator-scratch", "current":false, "worktree":repo.0.join("scratch")}
        ])
    }
    type Reply = (&'static str, Value);
    async fn mock(replies: Vec<Reply>) -> (Arc<LoomClient>, tokio::task::JoinHandle<Vec<Value>>) {
        mock_at(replies, "127.0.0.1:0").await
    }
    async fn mock_at(
        replies: Vec<Reply>,
        bind: &str,
    ) -> (Arc<LoomClient>, tokio::task::JoinHandle<Vec<Value>>) {
        let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut requests: Vec<Value> = vec![];
            for (path, mut body) in replies {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(10), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut bytes = Vec::new();
                let (start, len) = loop {
                    let mut chunk = [0; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        assert!(
                            headers.starts_with(&format!("POST {path} ")),
                            "unexpected request: {headers}"
                        );
                        let len: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|len| len.trim().parse().unwrap())
                            })
                            .unwrap();
                        break (end + 4, len);
                    }
                };
                while bytes.len() < start + len {
                    let mut chunk = [0; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                }
                requests.push(serde_json::from_slice(&bytes[start..start + len]).unwrap());
                // Controlled Git change exactly at a metadata response proves
                // preflight races without relying on process timing.
                if let Some(change) = body.get("_test_git_before_reply") {
                    let args: Vec<_> = change["args"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|arg| arg.as_str().unwrap())
                        .collect();
                    run(Path::new(change["repo"].as_str().unwrap()), &args);
                }
                if body["_test_echo_receipt"] == true {
                    let sent = requests.last().unwrap();
                    body = json!({"id":"message", "channel_id":sent["channel"],
                        "kind":"message", "body":sent["body"].as_str().unwrap().trim(),
                        "payload":sent["payload"]});
                }
                let status = body["_test_status"].as_u64().unwrap_or(200);
                let body = body.to_string();
                socket.write_all(format!("HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
            requests
        });
        (
            Arc::new(LoomClient::new(&format!("http://{address}/"), None).unwrap()),
            server,
        )
    }
    fn initial(view: &SessionView, summary: &SessionSummaryView) -> Vec<Reply> {
        vec![
            ("/api/sessions/get", json!(view)),
            ("/api/sessions/summary/list", json!([summary])),
        ]
    }
    fn queue(replies: &mut Vec<Reply>, view: &SessionView) {
        if view.protocol == "terminal" {
            replies.push((
                "/api/channels/subscription/set",
                json!({"channel_id":view.id,
                "subject_kind":"session", "subject_id":view.id, "mode":"deliver"}),
            ));
            replies.push((
                "/api/channels/messages/create",
                json!({"_test_echo_receipt":true}),
            ));
        } else {
            replies.push(("/api/sessions/get", json!(view)));
            replies.push((
                "/api/sessions/prompt/create",
                json!({"queued":true,"turn":null}),
            ));
        }
    }
    fn payload(body: &Value, terminal: bool) -> (Value, String) {
        if terminal {
            return (
                body["payload"].clone(),
                body["body"].as_str().unwrap().into(),
            );
        }
        assert_eq!(body["send_now"], false);
        let text = body["text"].as_str().unwrap();
        let json = text
            .split("```json\n")
            .nth(1)
            .unwrap()
            .split("\n```")
            .next()
            .unwrap();
        (serde_json::from_str(json).unwrap(), text.into())
    }

    #[tokio::test]
    async fn tagged_local_publishes_accepted_commits_ignoring_dirty_scratch_and_accepted_checkout()
    {
        let (repo, view, summary) = fixture(true);
        write(&repo.0.join("scratch"), "scratch-dirty.txt", "scratch\n");
        write(
            &repo.0.join("scratch"),
            "scratch-only.txt",
            "unstaged scratch\n",
        );
        run(&repo.0.join("scratch"), &["add", "scratch-dirty.txt"]);
        write(
            &repo.0.join("accepted"),
            "accepted-one.txt",
            "not accepted yet\n",
        );
        write(&repo.0, "foreign.txt", "foreign unstaged\n");
        write(&repo.0, "untracked.txt", "foreign untracked\n");
        let source_sha = run(&repo.0, &["rev-parse", "accepted/topic"]);
        let scratch_status = run(&repo.0.join("scratch"), &["status", "--porcelain"]);
        let accepted_status = run(&repo.0.join("accepted"), &["status", "--porcelain"]);
        let mut replies = initial(&view, &summary);
        replies.push(("/api/sessions/get", json!(view)));
        replies.push(("/api/sessions/summary/list", json!([summary])));
        replies.push(("/api/sessions/tags/set", json!({})));
        let (client, server) = mock(replies).await;
        let result = publish(&client, &view.id, "land-locally", None)
            .await
            .unwrap();
        assert!(result.local);
        let landing = result.landing.unwrap();
        assert!(landing.landed);
        assert_eq!(landing.target_branch, "current-dev");
        assert_eq!(landing.commits_squashed, 2);
        assert_eq!(run(&repo.0, &["show", "HEAD:accepted-one.txt"]), "one");
        assert!(!repo.0.join("scratch-only.txt").exists());
        assert_eq!(
            std::fs::read_to_string(repo.0.join("foreign.txt")).unwrap(),
            "foreign unstaged\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.0.join("untracked.txt")).unwrap(),
            "foreign untracked\n"
        );
        assert_eq!(
            run(&repo.0.join("scratch"), &["status", "--porcelain"]),
            scratch_status
        );
        assert_eq!(
            run(&repo.0.join("accepted"), &["status", "--porcelain"]),
            accepted_status
        );
        assert_eq!(run(&repo.0, &["rev-parse", "accepted/topic"]), source_sha);
        let requests = server.await.unwrap();
        let tag = requests.last().unwrap();
        assert_eq!(tag["key"], "publish_result");
        assert_eq!(tag["value"], run(&repo.0, &["rev-parse", "HEAD"]));
        assert!(tag["note"].as_str().unwrap().contains(&source_sha));
    }

    #[tokio::test]
    async fn committed_canonical_ref_needs_no_source_checkout_or_scratch() {
        let (repo, mut view, summary) = fixture(true);
        run(&repo.0, &["worktree", "remove", "accepted"]);
        view.work_dir = "/not/a/local/checkout".into();
        let mut replies = initial(&view, &summary);
        replies.push(("/api/sessions/get", json!(view)));
        replies.push(("/api/sessions/summary/list", json!([summary])));
        replies.push(("/api/sessions/tags/set", json!({})));
        let (client, server) = mock(replies).await;
        assert!(
            publish(&client, &view.id, "land-locally", None)
                .await
                .unwrap()
                .local
        );
        assert_eq!(run(&repo.0, &["show", "HEAD:accepted-two.txt"]), "two");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn unmarked_legacy_root_publishes_its_existing_accepted_branch() {
        let (repo, mut view, mut summary) = fixture(false);
        view.branch.tags.retain(|tag| tag.key != "topic");
        summary.branch = view.branch.clone();
        let scratch_head = run(&repo.0.join("scratch"), &["rev-parse", "HEAD"]);
        let mut replies = initial(&view, &summary);
        replies.push(("/api/sessions/get", json!(view)));
        replies.push(("/api/sessions/summary/list", json!([summary])));
        replies.push(("/api/sessions/tags/set", json!({})));
        let (client, server) = mock(replies).await;
        let result = publish(&client, &view.id, "land-locally", None)
            .await
            .unwrap();
        assert!(result.local);
        assert_eq!(run(&repo.0, &["show", "HEAD:accepted-one.txt"]), "one");
        assert_eq!(
            run(&repo.0.join("scratch"), &["rev-parse", "HEAD"]),
            scratch_head
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.last().unwrap()["key"], "publish_result");
    }

    #[tokio::test]
    async fn stale_fleet_canonical_metadata_queues_review_without_publishing_old_selection() {
        let (repo, view, mut summary) = fixture(true);
        run(&repo.0, &["branch", "accepted/replacement"]);
        summary
            .branch
            .tags
            .iter_mut()
            .find(|tag| tag.key == "track_topic_branch")
            .unwrap()
            .value = "accepted/replacement".into();
        let head = run(&repo.0, &["rev-parse", "HEAD"]);
        let mut replies = initial(&view, &summary);
        queue(&mut replies, &view);
        let (client, server) = mock(replies).await;
        let result = publish(&client, &view.id, "land-locally", None)
            .await
            .unwrap();
        assert!(!result.local);
        let requests = server.await.unwrap();
        let (request, prompt) = payload(requests.last().unwrap(), false);
        assert!(request["repair_reason"]
            .as_str()
            .unwrap()
            .contains("metadata changed"));
        assert!(prompt.contains("preserve this same chosen Publish authorization"));
        assert_eq!(run(&repo.0, &["rev-parse", "HEAD"]), head);
        assert!(!requests.iter().any(|body| body["key"] == "publish_result"));
    }

    #[tokio::test]
    async fn live_canonical_change_before_write_queues_fresh_review_without_target_changes() {
        let (repo, view, summary) = fixture(true);
        run(&repo.0, &["branch", "accepted/replacement"]);
        let mut live = view.clone();
        live.branch
            .tags
            .iter_mut()
            .find(|tag| tag.key == "track_topic_branch")
            .unwrap()
            .value = "accepted/replacement".into();
        let mut owner = summary.clone();
        owner.branch = live.branch.clone();
        let head = run(&repo.0, &["rev-parse", "HEAD"]);
        let mut replies = initial(&view, &summary);
        replies.push(("/api/sessions/get", json!(live)));
        replies.push(("/api/sessions/summary/list", json!([owner])));
        queue(&mut replies, &view);
        let (client, server) = mock(replies).await;
        assert!(
            !publish(&client, &view.id, "land-locally", None)
                .await
                .unwrap()
                .local
        );
        let requests = server.await.unwrap();
        let (request, _) = payload(requests.last().unwrap(), false);
        assert!(request["repair_reason"]
            .as_str()
            .unwrap()
            .contains("metadata changed"));
        assert_eq!(run(&repo.0, &["rev-parse", "HEAD"]), head);
        assert!(!requests.iter().any(|body| body["key"] == "publish_result"));
    }

    #[tokio::test]
    async fn accepted_ref_advance_during_final_metadata_check_never_silently_publishes_new_content()
    {
        let (repo, view, summary) = fixture(true);
        let original = run(&repo.0, &["rev-parse", "accepted/topic"]);
        commit(
            &repo.0.join("accepted"),
            "late-accepted.txt",
            "not reviewed
",
        );
        let advanced = run(&repo.0, &["rev-parse", "accepted/topic"]);
        run(
            &repo.0,
            &[
                "update-ref",
                "refs/heads/accepted/topic",
                &original,
                &advanced,
            ],
        );
        let head = run(&repo.0, &["rev-parse", "HEAD"]);
        let mut response = json!(view);
        response["_test_git_before_reply"] = json!({"repo":repo.0,
            "args":["update-ref", "refs/heads/accepted/topic", advanced, original]});
        let mut replies = initial(&view, &summary);
        replies.push(("/api/sessions/get", response));
        replies.push(("/api/sessions/summary/list", json!([summary])));
        queue(&mut replies, &view);
        let (client, server) = mock(replies).await;
        assert!(
            !publish(&client, &view.id, "land-locally", None)
                .await
                .unwrap()
                .local
        );
        let requests = server.await.unwrap();
        let (request, _) = payload(requests.last().unwrap(), false);
        assert!(request["repair_reason"]
            .as_str()
            .unwrap()
            .contains("changed"));
        assert_eq!(run(&repo.0, &["rev-parse", "HEAD"]), head);
        assert!(!repo.0.join("late-accepted.txt").exists());
        assert!(!requests.iter().any(|body| body["key"] == "publish_result"));
    }

    #[tokio::test]
    async fn primary_ref_advance_during_final_metadata_check_queues_without_a_squash() {
        let (repo, view, summary) = fixture(true);
        let original = run(&repo.0, &["rev-parse", "HEAD"]);
        let tree = run(&repo.0, &["rev-parse", "HEAD^{tree}"]);
        let advanced = run(
            &repo.0,
            &[
                "commit-tree",
                &tree,
                "-p",
                &original,
                "-m",
                "concurrent primary update",
            ],
        );
        let mut response = json!(view);
        response["_test_git_before_reply"] = json!({"repo":repo.0,
            "args":["update-ref", "refs/heads/current-dev", advanced, original]});
        let mut replies = initial(&view, &summary);
        replies.push(("/api/sessions/get", response));
        replies.push(("/api/sessions/summary/list", json!([summary])));
        queue(&mut replies, &view);
        let (client, server) = mock(replies).await;
        assert!(
            !publish(&client, &view.id, "land-locally", None)
                .await
                .unwrap()
                .local
        );
        let requests = server.await.unwrap();
        assert_eq!(run(&repo.0, &["rev-parse", "HEAD"]), advanced);
        assert!(!repo.0.join("accepted-one.txt").exists());
        assert!(!requests.iter().any(|body| body["key"] == "publish_result"));
    }

    #[tokio::test]
    async fn ignored_foreign_collision_queues_preservation_repair_without_overwriting() {
        let (repo, view, summary) = fixture(true);
        commit(
            &repo.0.join("accepted"),
            "private.env",
            "accepted example
",
        );
        let excludes = repo.0.join(".git/info/exclude");
        let mut contents = std::fs::read_to_string(&excludes).unwrap();
        contents.push_str(
            "private.env
",
        );
        std::fs::write(excludes, contents).unwrap();
        write(
            &repo.0,
            "private.env",
            "foreign secret bytes
",
        );
        let head = run(&repo.0, &["rev-parse", "HEAD"]);
        let mut replies = initial(&view, &summary);
        replies.push(("/api/sessions/get", json!(view)));
        replies.push(("/api/sessions/summary/list", json!([summary])));
        queue(&mut replies, &view);
        let (client, server) = mock(replies).await;
        assert!(
            !publish(&client, &view.id, "land-locally", None)
                .await
                .unwrap()
                .local
        );
        let requests = server.await.unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.0.join("private.env")).unwrap(),
            "foreign secret bytes
"
        );
        assert_eq!(run(&repo.0, &["rev-parse", "HEAD"]), head);
        assert_eq!(run(&repo.0, &["status", "--porcelain"]), "");
        assert!(!requests.iter().any(|body| body["key"] == "publish_result"));
    }

    #[tokio::test]
    async fn uncertain_terminal_publish_receipt_preserves_stable_retry_identity_without_success() {
        let (repo, mut view, summary) = fixture(true);
        view.protocol = "terminal".into();
        write(
            &repo.0,
            "foreign.txt",
            "foreign staging
",
        );
        run(&repo.0, &["add", "foreign.txt"]);
        let head = run(&repo.0, &["rev-parse", "HEAD"]);
        let mut keys = Vec::new();
        for _ in 0..2 {
            let mut replies = initial(&view, &summary);
            queue(&mut replies, &view);
            replies.last_mut().unwrap().1 = json!({}); // authorization received, receipt malformed
            let (client, server) = mock(replies).await;
            let error = publish(&client, &view.id, "land-locally", None)
                .await
                .unwrap_err();
            assert!(error.message.contains("may already"), "{}", error.message);
            let requests = server.await.unwrap();
            let sent = requests.last().unwrap();
            keys.push(sent["idempotency_key"].as_str().unwrap().to_owned());
            assert!(!requests.iter().any(|body| body["key"] == "publish_result"));
        }
        assert_eq!(keys[0], keys[1]);
        assert_eq!(run(&repo.0, &["rev-parse", "HEAD"]), head);
        assert_eq!(run(&repo.0, &["show", ":foreign.txt"]), "foreign staging");
    }

    #[tokio::test]
    async fn uncertain_acp_publish_receipt_is_reported_as_uncertain_not_confirmed_queue() {
        let (repo, view, summary) = fixture(true);
        write(
            &repo.0,
            "foreign.txt",
            "foreign staging
",
        );
        run(&repo.0, &["add", "foreign.txt"]);
        let mut replies = initial(&view, &summary);
        queue(&mut replies, &view);
        replies.last_mut().unwrap().1 = json!({});
        let (client, server) = mock(replies).await;
        let error = publish(&client, &view.id, "land-locally", None)
            .await
            .unwrap_err();
        assert!(error.message.contains("may already"), "{}", error.message);
        let requests = server.await.unwrap();
        assert!(!requests.iter().any(|body| body["key"] == "publish_result"));
    }

    #[tokio::test]
    async fn terminal_publish_requires_verified_delivery_route_before_authorization_dispatch() {
        let (repo, mut view, summary) = fixture(true);
        view.protocol = "terminal".into();
        write(
            &repo.0,
            "foreign.txt",
            "foreign staging
",
        );
        run(&repo.0, &["add", "foreign.txt"]);
        let mut replies = initial(&view, &summary);
        replies.push((
            "/api/channels/subscription/set",
            json!({"channel_id":view.id,
            "subject_kind":"session", "subject_id":view.id, "mode":"observe"}),
        ));
        let (client, server) = mock(replies).await;
        assert!(publish(&client, &view.id, "land-locally", None)
            .await
            .is_err());
        let requests = server.await.unwrap();
        assert_eq!(requests.last().unwrap()["mode"], "deliver");
        assert!(!requests.iter().any(|body| body.get("body").is_some()));
    }

    #[tokio::test]
    async fn completed_git_publish_survives_failed_result_metadata_delivery() {
        let (repo, view, summary) = fixture(true);
        let mut replies = initial(&view, &summary);
        replies.push(("/api/sessions/get", json!(view)));
        replies.push(("/api/sessions/summary/list", json!([summary])));
        replies.push((
            "/api/sessions/tags/set",
            json!({"_test_status":503,"message":"metadata unavailable"}),
        ));
        let (client, server) = mock(replies).await;
        let result = publish(&client, &view.id, "land-locally", None)
            .await
            .unwrap();
        assert!(result.local);
        let landing = result.landing.unwrap();
        assert!(landing.landed);
        assert_eq!(
            landing.commit.as_deref(),
            Some(run(&repo.0, &["rev-parse", "HEAD"]).as_str())
        );
        assert!(landing
            .warnings
            .iter()
            .any(|warning| warning.contains("completed") && warning.contains("tag")));
        assert_eq!(run(&repo.0, &["show", "HEAD:accepted-one.txt"]), "one");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn foreign_staging_queues_exact_canonical_target_context_without_any_mutation() {
        for terminal in [false, true] {
            let (repo, mut view, summary) = fixture(true);
            if terminal {
                view.protocol = "terminal".into();
            }
            write(&repo.0, "foreign.txt", "foreign staged\n");
            run(&repo.0, &["add", "foreign.txt"]);
            write(&repo.0, "foreign.txt", "foreign unstaged\n");
            write(&repo.0, "untracked.txt", "keep\n");
            let status = run(&repo.0, &["status", "--porcelain"]);
            let head = run(&repo.0, &["rev-parse", "HEAD"]);
            let source = run(&repo.0, &["rev-parse", "accepted/topic"]);
            let mut replies = initial(&view, &summary);
            queue(&mut replies, &view);
            let (client, server) = mock(replies).await;
            let result = publish(&client, &view.id, "land-locally", None)
                .await
                .unwrap();
            assert!(!result.local);
            assert!(result.landing.is_none());
            let requests = server.await.unwrap();
            let last = requests.last().unwrap();
            let (request, prompt) = payload(last, terminal);
            assert_eq!(request["action"], "publish");
            assert_eq!(request["source_ref"], "refs/heads/accepted/topic");
            assert_eq!(request["source_revision"], source);
            assert_eq!(request["source_checkout"], json!(repo.0.join("accepted")));
            assert_eq!(request["target_upstream"], "current-dev");
            assert_eq!(request["local_target"]["revision"], head);
            assert_eq!(request["local_target"]["checkout"], json!(repo.0));
            assert!(request["repair_reason"]
                .as_str()
                .unwrap()
                .contains("staged"));
            assert!(prompt.contains("temporary index/checkout"));
            assert!(prompt.contains("never absorb foreign staging"));
            assert!(prompt.contains("Never stamp integration_result"));
            if terminal {
                assert_eq!(last["kind"], "message");
                assert_eq!(last["channel"], view.id);
            }
            assert_eq!(run(&repo.0, &["rev-parse", "HEAD"]), head);
            assert_eq!(run(&repo.0, &["status", "--porcelain"]), status);
            assert_eq!(run(&repo.0, &["show", ":foreign.txt"]), "foreign staged");
            assert_eq!(
                std::fs::read_to_string(repo.0.join("foreign.txt")).unwrap(),
                "foreign unstaged\n"
            );
            assert_eq!(
                std::fs::read_to_string(repo.0.join("untracked.txt")).unwrap(),
                "keep\n"
            );
        }
    }

    #[tokio::test]
    async fn recoverable_conflict_unwinds_then_queues_complete_chosen_publish() {
        let (repo, view, summary) = fixture(true);
        commit(
            &repo.0.join("accepted"),
            "conflict.txt",
            "canonical conflict\n",
        );
        commit(&repo.0, "conflict.txt", "target conflict\n");
        write(&repo.0, "foreign.txt", "unrelated edit\n");
        write(&repo.0, "untracked.txt", "keep\n");
        let status = run(&repo.0, &["status", "--porcelain"]);
        let head = run(&repo.0, &["rev-parse", "HEAD"]);
        let mut replies = initial(&view, &summary);
        replies.push(("/api/sessions/get", json!(view)));
        replies.push(("/api/sessions/summary/list", json!([summary])));
        queue(&mut replies, &view);
        let (client, server) = mock(replies).await;
        let result = publish(&client, &view.id, "land-locally", None)
            .await
            .unwrap();
        assert!(!result.local);
        assert!(result.landing.is_none());
        let requests = server.await.unwrap();
        let (request, prompt) = payload(requests.last().unwrap(), false);
        assert!(request["repair_reason"]
            .as_str()
            .unwrap()
            .contains("failed"));
        assert_eq!(request["strategy"], "land-locally");
        assert_eq!(request["local_target"]["revision"], head);
        assert!(prompt.contains("repair conflicts, then complete this same chosen squash"));
        assert_eq!(run(&repo.0, &["status", "--porcelain"]), status);
        assert_eq!(run(&repo.0, &["rev-parse", "HEAD"]), head);
        assert!(!repo.0.join(".git/MERGE_HEAD").exists());
    }

    #[tokio::test]
    async fn legacy_dirty_source_queues_oneclick_preparation_and_clean_source_lands() {
        let (repo, view, summary) = fixture(false);
        write(&repo.0.join("scratch"), "intended-new.txt", "prepare me\n");
        let head = run(&repo.0, &["rev-parse", "HEAD"]);
        let mut replies = initial(&view, &summary);
        queue(&mut replies, &view);
        let (client, server) = mock(replies).await;
        let result = publish(&client, &view.id, "land-locally", None)
            .await
            .unwrap();
        assert!(!result.local);
        let requests = server.await.unwrap();
        let (request, prompt) = payload(requests.last().unwrap(), false);
        assert_eq!(request["explicit_canonical"], false);
        assert_eq!(request["source_ref"], "refs/heads/coordinator-scratch");
        assert!(request["legacy_source_state"]["status"]
            .as_str()
            .unwrap()
            .contains("intended-new.txt"));
        assert!(prompt.contains("commit only intended changes preserving foreign work"));
        assert!(prompt.contains("Do not require a second Prepare click"));
        assert_eq!(run(&repo.0, &["rev-parse", "HEAD"]), head);
        std::fs::remove_file(repo.0.join("scratch/intended-new.txt")).unwrap();
        let mut replies = initial(&view, &summary);
        replies.push(("/api/sessions/get", json!(view)));
        replies.push(("/api/sessions/summary/list", json!([summary])));
        replies.push(("/api/sessions/tags/set", json!({})));
        let (client, server) = mock(replies).await;
        assert!(
            publish(&client, &view.id, "land-locally", None)
                .await
                .unwrap()
                .local
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn single_pr_uses_canonical_source_and_queues_without_premature_result() {
        let (repo, view, summary) = fixture(true);
        let head = run(&repo.0, &["rev-parse", "HEAD"]);
        let mut replies = initial(&view, &summary);
        replies.push(("/api/repos/branches", branches(&repo, true)));
        replies.push((
            "/api/branches/list",
            json!([
                {"branch":"accepted/topic", "repo_root":repo.0, "base_branch":"current-dev"},
                {"branch":"coordinator-scratch", "repo_root":repo.0, "base_branch":"wrong-base"}
            ]),
        ));
        queue(&mut replies, &view);
        let (client, server) = mock(replies).await;
        let result = publish(&client, &view.id, "open-pr", None).await.unwrap();
        assert!(!result.local);
        assert!(result.landing.is_none());
        let requests = server.await.unwrap();
        let (request, prompt) = payload(requests.last().unwrap(), false);
        assert_eq!(request["source_ref"], "refs/heads/accepted/topic");
        assert_eq!(request["target_upstream"], "current-dev");
        assert_eq!(request["source_checkout"], json!(repo.0.join("accepted")));
        assert_eq!(request["authorization"], "explicit-chosen-publish");
        assert!(request["stack"].as_array().unwrap().is_empty());
        assert!(prompt.contains("Single PR:"));
        assert!(prompt.contains("Coordinator scratch commits"));
        assert!(prompt
            .contains("Only after the actual chosen operation succeeds, stamp publish_result"));
        assert_eq!(run(&repo.0, &["rev-parse", "HEAD"]), head);
    }

    #[tokio::test]
    async fn stack_has_ordered_canonical_commits_explicit_dependent_refs_and_actual_pr_results() {
        let (repo, view, summary) = fixture(true);
        let source = run(&repo.0, &["rev-parse", "accepted/topic"]);
        let target = run(&repo.0, &["rev-parse", "HEAD"]);
        let before = run(&repo.0, &["show-ref", "--heads"]);
        let mut replies = initial(&view, &summary);
        replies.push(("/api/repos/branches", branches(&repo, true)));
        queue(&mut replies, &view);
        let (client, server) = mock(replies).await;
        let result = publish(&client, &view.id, "pr-stack", Some("current-dev".into()))
            .await
            .unwrap();
        assert!(!result.local);
        assert!(result.landing.is_none());
        let requests = server.await.unwrap();
        let (request, prompt) = payload(requests.last().unwrap(), false);
        let stack = request["stack"].as_array().unwrap();
        assert_eq!(stack.len(), 2);
        assert_eq!(stack[0]["order"], 1);
        assert_eq!(stack[0]["base_ref"], "current-dev");
        assert_eq!(stack[1]["base_ref"], stack[0]["head_ref"]);
        assert_ne!(stack[0]["head_ref"], stack[1]["head_ref"]);
        assert_eq!(stack[1]["accepted_commit"], source);
        assert_eq!(
            stack[0]["accepted_commit"],
            run(&repo.0, &["rev-parse", "accepted/topic~1"])
        );
        assert!(prompt.contains("actual separate PR for EACH entry"));
        assert!(prompt.contains("ordered array of distinct actual PR URLs"));
        assert!(prompt.contains("validate each dependent change"));
        assert!(prompt.contains("Never describe a single PR"));
        assert_eq!(run(&repo.0, &["show-ref", "--heads"]), before);
        assert_eq!(run(&repo.0, &["rev-parse", "HEAD"]), target);
    }

    #[tokio::test]
    async fn stack_rejects_one_change_instead_of_pretending_single_pr_is_stack() {
        let (repo, view, summary) = fixture(true);
        run(&repo.0.join("accepted"), &["reset", "--hard", "HEAD~1"]);
        let mut replies = initial(&view, &summary);
        replies.push(("/api/repos/branches", branches(&repo, true)));
        let (client, server) = mock(replies).await;
        let e = publish(&client, &view.id, "pr-stack", Some("current-dev".into()))
            .await
            .unwrap_err();
        assert!(e.message.contains("at least two accepted changes"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn root_track_marker_and_initialized_canonical_are_required() {
        for condition in ["child", "standalone", "pending", "unverified", "mismatched-verification", "invalid", "missing"] {
            let (repo, mut view, mut summary) = fixture(true);
            match condition {
                "child" => summary.parent_session_id = Some("parent".into()),
                "standalone" => {
                    view.branch
                        .tags
                        .iter_mut()
                        .find(|tag| tag.key == "topic")
                        .unwrap()
                        .value = "false".into()
                }
                "pending" => {
                    view.branch
                        .tags
                        .retain(|tag| tag.key != "track_topic_branch");
                    view.branch.goal = format!(
                        "{} accepted/topic",
                        crate::track_policy::CANONICAL_INITIALIZATION_MARKER
                    );
                }
                "unverified" | "mismatched-verification" => {
                    // Existing real ref plus launch/display tags cannot prove
                    // this new Track created it, especially after a collision.
                    view.branch.goal = format!(
                        "{} accepted/topic",
                        crate::track_policy::CANONICAL_INITIALIZATION_MARKER
                    );
                    if condition == "mismatched-verification" {
                        view.branch.tags.push(crate::loom::TagView {
                            key: "track_topic_initialized".into(),
                            value: "other-track/ref".into(),
                            note: String::new(),
                            set_at: String::new(),
                            set_by: String::new(),
                        });
                    }
                }
                "invalid" => {
                    view.branch
                        .tags
                        .iter_mut()
                        .find(|tag| tag.key == "track_topic_branch")
                        .unwrap()
                        .value = "../evil".into()
                }
                "missing" => {
                    run(&repo.0, &["worktree", "remove", "accepted"]);
                    run(&repo.0, &["branch", "-D", "accepted/topic"]);
                }
                _ => unreachable!(),
            }
            let head = run(&repo.0, &["rev-parse", "HEAD"]);
            let (client, server) = mock(initial(&view, &summary)).await;
            assert!(publish(&client, &view.id, "land-locally", None)
                .await
                .is_err());
            server.await.unwrap();
            assert_eq!(run(&repo.0, &["rev-parse", "HEAD"]), head);
        }
    }

    #[tokio::test]
    async fn remote_local_error_is_actionable_and_target_override_is_rejected() {
        let (_repo, view, summary) = fixture(true);
        let remote = Arc::new(LoomClient::new("https://remote.invalid", None).unwrap());
        let e = publish_local(&remote, &view, &summary, "accepted/topic", true)
            .await
            .unwrap_err();
        assert!(e.message.contains("Connect to a local Loom"));
        let (client, server) = mock(initial(&view, &summary)).await;
        let e = publish(&client, &view.id, "land-locally", Some("main".into()))
            .await
            .unwrap_err();
        assert!(e.message.contains("CURRENT branch"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn optional_strategies_are_explicit_queued_operations_only() {
        for strategy in ["squash", "merge", "rebase", "push"] {
            let (repo, view, summary) = fixture(true);
            let head = run(&repo.0, &["rev-parse", "HEAD"]);
            let mut replies = initial(&view, &summary);
            replies.push(("/api/repos/branches", branches(&repo, true)));
            queue(&mut replies, &view);
            let (client, server) = mock(replies).await;
            let result = publish(&client, &view.id, strategy, None).await.unwrap();
            assert!(!result.local);
            let requests = server.await.unwrap();
            let (request, prompt) = payload(requests.last().unwrap(), false);
            assert_eq!(request["strategy"], strategy);
            assert_eq!(request["source_ref"], "refs/heads/accepted/topic");
            assert_eq!(request["target_upstream"], "current-dev");
            assert!(prompt.contains("explicit-chosen-publish"));
            assert_eq!(run(&repo.0, &["rev-parse", "HEAD"]), head);
        }
    }

    #[tokio::test]
    async fn local_noop_is_completed_publish_with_actual_target_sha() {
        let (repo, view, summary) = fixture(true);
        run(&repo.0, &["merge", "--ff-only", "accepted/topic"]);
        let head = run(&repo.0, &["rev-parse", "HEAD"]);
        let mut replies = initial(&view, &summary);
        replies.push(("/api/sessions/get", json!(view)));
        replies.push(("/api/sessions/summary/list", json!([summary])));
        replies.push(("/api/sessions/tags/set", json!({})));
        let (client, server) = mock(replies).await;
        let result = publish(&client, &view.id, "land-locally", None)
            .await
            .unwrap();
        assert!(result.local);
        assert!(!result.landing.unwrap().landed);
        let requests = server.await.unwrap();
        assert_eq!(requests.last().unwrap()["key"], "publish_result");
        assert_eq!(requests.last().unwrap()["value"], head);
        assert_eq!(run(&repo.0, &["rev-parse", "HEAD"]), head);
    }

    #[tokio::test]
    async fn remote_stack_carries_structured_derivation_and_never_uses_local_server_paths() {
        let (_repo, mut view, mut summary) = fixture(true);
        view.branch.repo_root = "/remote/only/repo".into();
        view.work_dir = "/remote/only/scratch".into();
        summary.branch = view.branch.clone();
        let remote_branches = json!([
            {"name":"accepted/topic", "current":false, "worktree":"/remote/only/accepted"},
            {"name":"main", "current":true, "worktree":"/remote/only/repo"}
        ]);
        let mut replies = initial(&view, &summary);
        replies.push(("/api/repos/branches", remote_branches));
        queue(&mut replies, &view);
        let (client, server) = mock_at(replies, "127.0.0.2:0").await;
        assert!(!local_client(&client));
        let result = publish(&client, &view.id, "pr-stack", Some("main".into()))
            .await
            .unwrap();
        assert!(!result.local);
        assert!(result.landing.is_none());
        let requests = server.await.unwrap();
        let (request, prompt) = payload(requests.last().unwrap(), false);
        assert_eq!(request["source_ref"], "refs/heads/accepted/topic");
        assert_eq!(request["source_checkout"], "/remote/only/accepted");
        assert!(request["source_revision"].is_null());
        assert!(prompt.contains("construct this same structured plan with order, accepted_commit, head_ref and base_ref before writing"));
        assert!(prompt.contains("Verify the canonical ref really exists as a commit"));
        assert!(prompt.contains("At least two separate PRs are required"));
    }

    #[tokio::test]
    async fn remote_missing_canonical_metadata_blocks_dispatch() {
        let (_repo, view, summary) = fixture(true);
        let mut replies = initial(&view, &summary);
        replies.push((
            "/api/repos/branches",
            json!([{"name":"coordinator-scratch", "current":false, "worktree":null}]),
        ));
        let (client, server) = mock_at(replies, "127.0.0.2:0").await;
        let e = publish(&client, &view.id, "open-pr", Some("main".into()))
            .await
            .unwrap_err();
        assert!(e.message.contains("not initialized"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn optional_main_fallback_remains_advisory_and_local_never_falls_back() {
        let (repo, view, summary) = fixture(true);
        run(&repo.0, &["branch", "main"]);
        run(&repo.0, &["checkout", "--detach", "-q"]);
        let mut replies = initial(&view, &summary);
        replies.push((
            "/api/repos/branches",
            json!([
                {"name":"accepted/topic", "current":false, "worktree":repo.0.join("accepted")},
                {"name":"main", "current":false, "worktree":null}
            ]),
        ));
        queue(&mut replies, &view);
        let (client, server) = mock(replies).await;
        assert!(
            !publish(&client, &view.id, "merge", None)
                .await
                .unwrap()
                .local
        );
        let requests = server.await.unwrap();
        let (request, prompt) = payload(requests.last().unwrap(), false);
        assert_eq!(request["target_upstream"], "main");
        assert_eq!(request["target_origin"], "main-fallback");
        assert!(prompt.contains("main fallback is advisory"));
        let (client, server) = mock(initial(&view, &summary)).await;
        let e = publish(&client, &view.id, "land-locally", None)
            .await
            .unwrap_err();
        assert!(e.message.contains("detached HEAD"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn staged_target_can_queue_without_any_canonical_checkout() {
        let (repo, view, summary) = fixture(true);
        run(&repo.0, &["worktree", "remove", "accepted"]);
        write(&repo.0, "foreign.txt", "staged\n");
        run(&repo.0, &["add", "foreign.txt"]);
        let mut replies = initial(&view, &summary);
        queue(&mut replies, &view);
        let (client, server) = mock(replies).await;
        assert!(
            !publish(&client, &view.id, "land-locally", None)
                .await
                .unwrap()
                .local
        );
        let requests = server.await.unwrap();
        let (request, prompt) = payload(requests.last().unwrap(), false);
        assert!(request["source_checkout"].is_null());
        assert_eq!(request["source_ref"], "refs/heads/accepted/topic");
        assert!(prompt.contains("use an isolated checkout of source_revision"));
        assert_eq!(run(&repo.0, &["show", ":foreign.txt"]), "staged");
    }

    #[test]
    fn ref_and_strategy_validation_fail_closed() {
        for value in [
            "",
            "HEAD",
            "@",
            "refs/heads/x",
            "-x",
            "a..b",
            "a@{b",
            "a.lock",
            "a/",
            "a//b",
            ".a",
            "a/.b",
            "a b",
            "a\\b",
            "a~b",
            "a:b",
            "a/topic.lock",
            "topic.",
            "a?b",
            "a*b",
            "a[b",
        ] {
            assert!(!valid_branch(value), "accepted invalid branch {value}");
        }
        // Reference cases agree with tests/fixtures/track-policy-contract.json (f6a097d).
        for value in ["arachne/tracks/working-thread", "custom/topic", "topic"] {
            assert!(valid_branch(value));
        }
        assert!(valid_branch("arachne/tracks/topic"));
        assert!(valid_branch("feature/über"));
        for value in ["ask", "cherry-pick", "prepare", "unknown"] {
            assert!(Strategy::parse(value).is_err());
        }
        let (_repo, mut view, _) = fixture(true);
        let tag = view
            .branch
            .tags
            .iter()
            .find(|tag| tag.key == "track_topic_branch")
            .unwrap()
            .clone();
        view.branch.tags.push(tag);
        assert!(canonical_source(&view).is_err());
    }
}
