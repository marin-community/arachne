//! Deterministic local landing: squash-merge a topic branch into the primary
//! checkout's currently checked out branch (spec:
//! docs/integration-and-landing.md, "Land locally").
//!
//! Clean suitable sources land immediately. Dirty sources, target staging and
//! safely unwound conflicts are handed to the coordinator for preparation and
//! the chosen landing. Target/access blockers remain actionable errors.
//!
//! Unrelated unstaged and untracked target changes survive the deterministic
//! path. Staged target changes are never absorbed into a squash commit. A
//! failed attempt is recoverable only after isolated-index rollback succeeds.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

/// One completed local landing, reported to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct LocalLanding {
    /// Whether a new squash commit was created. `false` means the target
    /// branch already contained the topic's changes — nothing was done.
    pub landed: bool,
    /// The primary checkout's branch the topic landed into.
    pub target_branch: String,
    /// Full verified resulting revision, including for a pinned no-op.
    /// None means the write completed but neither Git stdout nor HEAD supplied its revision.
    pub commit: Option<String>,
    /// How many topic commits the squash carries.
    pub commits_squashed: u32,
    /// The primary checkout the landing ran in.
    pub primary_checkout: String,
    /// Limitations after a completed write; these never turn completion into failure.
    pub warnings: Vec<String>,
}

/// Why a local landing could not happen. Every message is user-facing.
#[derive(Debug, thiserror::Error)]
pub enum LandingError {
    #[error("could not read the repository at {path}: {detail}")]
    RepoUnreachable { path: String, detail: String },
    #[error("the repository at {path} has no working checkout (bare)")]
    Bare { path: String },
    #[error("the primary checkout has a detached HEAD; check out the branch to land into first")]
    Detached,
    #[error("the primary checkout is already on the topic branch {branch}; landing there would be a no-op self-merge")]
    SelfMerge { branch: String },
    #[error("branch {branch} does not exist in this repository")]
    BranchMissing { branch: String },
    #[error("the primary checkout has staged changes; commit or unstage them before landing")]
    StagedChanges,
    #[error("a {what} is already in progress in the primary checkout; conclude it before landing")]
    OperationInProgress { what: String },
    /// Recoverable only after the attempted squash has been safely unwound.
    #[error("{detail}")]
    RepairNeeded { detail: String },
    #[error("{detail}")]
    Failed { detail: String },
}

/// The repository's main working tree — the primary checkout — as reported
/// by `git worktree list --porcelain`'s first record.
struct PrimaryWorktree {
    path: PathBuf,
    /// The branch it has checked out; `None` when detached.
    branch: Option<String>,
    /// A bare repository has no working tree to land into at all.
    bare: bool,
}

/// Capture source changes for preparation instead of silently omitting editor
/// saves from committed branch state. Branch switches/foreign operations block.
#[derive(Debug, Clone, Serialize)]
pub struct SourceCheckoutState {
    pub revision: String,
    pub status: String,
}

pub async fn inspect_source_checkout(
    checkout: &Path,
    branch: &str,
) -> Result<SourceCheckoutState, String> {
    if checkout.as_os_str().is_empty() || !checkout.is_dir() {
        return Err(
            "The source checkout is unavailable; refresh the thread before landing.".into(),
        );
    }
    let timeout = Duration::from_secs(15);
    let actual = git(
        checkout,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        timeout,
    )
    .await?;
    if actual.trim() != branch {
        return Err(
            "The source checkout changed branches. Refresh the thread before landing.".into(),
        );
    }
    // Resolve state paths through git: linked worktrees have a .git file.
    for name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
        "BISECT_LOG",
    ] {
        let state_path = git(checkout, &["rev-parse", "--git-path", name], timeout).await?;
        if checkout.join(state_path.trim()).exists() {
            return Err(format!("An unrelated Git operation ({name}) is in progress in the source checkout; conclude it before landing."));
        }
    }
    let revision = git(checkout, &["rev-parse", "HEAD"], timeout).await?;
    let status = git(
        checkout,
        &["status", "--porcelain", "--untracked-files=normal"],
        timeout,
    )
    .await?;
    Ok(SourceCheckoutState {
        revision: revision.trim().into(),
        status,
    })
}

/// Do not pair a source checkout with an unrelated repository's branch of
/// the same name. Worktrees share this canonical Git directory.
pub async fn ensure_same_repository(source: &Path, repo: &Path) -> Result<(), String> {
    let args = ["rev-parse", "--path-format=absolute", "--git-common-dir"];
    let source_dir = git(source, &args, Duration::from_secs(15)).await?;
    let repo_dir = git(repo, &args, Duration::from_secs(15)).await?;
    let source_dir = std::fs::canonicalize(source_dir.trim()).map_err(|e| e.to_string())?;
    let repo_dir = std::fs::canonicalize(repo_dir.trim()).map_err(|e| e.to_string())?;
    if source_dir != repo_dir {
        return Err("The source checkout belongs to a different repository; refresh the thread before landing.".into());
    }
    Ok(())
}

/// Run git in `dir`, returning stdout on success. Failures carry git's own
/// words (stderr preferred), collapsed to one bounded line for toasts.
///
/// The ambient `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE` overrides are
/// cleared so the checkout Arachne writes to is exactly the one `-C`
/// names, never one an inherited environment silently re-points at.
async fn git(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    git_with_index(dir, args, timeout, None).await
}

async fn git_with_index(
    dir: &Path,
    args: &[&str],
    timeout: Duration,
    index: Option<&Path>,
) -> Result<String, String> {
    // Per-fixture fault injection avoids process-global PATH mutations in
    // concurrent temporary-Git tests. Production always invokes ordinary Git.
    #[cfg(test)]
    let test_git = dir.join(".git/arachne-test-git");
    #[cfg(test)]
    let executable = if test_git.exists() {
        test_git.as_os_str()
    } else {
        std::ffi::OsStr::new("git")
    };
    #[cfg(not(test))]
    let executable = "git";
    let mut command = tokio::process::Command::new(executable);
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| format!("git {} timed out", args.join(" ")))?
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Err(match (stderr.is_empty(), stdout.is_empty()) {
        (true, true) => format!("git {} failed", args.join(" ")),
        (false, true) => collapse(&stderr),
        (true, false) => collapse(&stdout),
        (false, false) => collapse(&format!("{stderr} {stdout}")),
    })
}

/// Collapse whitespace and bound the length, keeping git's message readable.
fn collapse(s: &str) -> String {
    let mut out: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.chars().count() > 400 {
        out = out.chars().take(400).collect();
        out.push('…');
    }
    out
}

/// Flatten a title onto one line, bounded so the commit subject stays sane.
fn shorten(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        let mut cut: String = flat.chars().take(max.saturating_sub(1)).collect();
        cut.push('…');
        cut
    } else {
        flat
    }
}

/// The first record of `git worktree list --porcelain` is the main working
/// tree — the primary checkout. Its `branch refs/heads/<name>` line tells
/// what it has checked out; a `detached` line means no branch; a `bare`
/// line means there is no working tree at all.
fn parse_primary_worktree(listing: &str) -> Option<PrimaryWorktree> {
    let first = listing.split("\n\n").next()?;
    let path = first.lines().find_map(|l| l.strip_prefix("worktree "))?;
    let branch = first.lines().find_map(|l| {
        l.strip_prefix("branch ")
            .map(|r| r.trim_start_matches("refs/heads/").to_owned())
    });
    let bare = first.lines().any(|l| l.trim() == "bare");
    Some(PrimaryWorktree {
        path: PathBuf::from(path),
        branch,
        bare,
    })
}

fn status_has_staged_changes(status: &str) -> bool {
    status.lines().any(|line| {
        let index = line.chars().next().unwrap_or(' ');
        index != ' ' && index != '?'
    })
}

/// Cooperate with ordinary Git writers via the REAL primary index.lock, while
/// merge, hooks and rollback use a private copy. The live index is never a
/// source of squash content after acquisition. Git hooks inherit the private
/// index, as they do for Git's own temporary-index commits.
struct OperationIndex {
    path: PathBuf,
    real: PathBuf,
    lock: PathBuf,
    owns_lock: bool,
    owns_index: bool,
}

impl OperationIndex {
    fn acquire(real: PathBuf) -> Result<Self, String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let lock = real.with_extension("lock");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .map_err(|e| {
                format!("cannot reserve the primary Git index (another writer may own it): {e}")
            })?;
        let path = real.with_file_name(format!(
            "arachne-landing-index-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut operation = Self {
            path,
            real,
            lock,
            owns_lock: true,
            owns_index: false,
        };
        let mut destination = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&operation.path)
            .map_err(|e| format!("creating the isolated index: {e}"))?;
        operation.owns_index = true;
        let mut source = std::fs::File::open(&operation.real)
            .map_err(|e| format!("reading the primary index: {e}"))?;
        std::io::copy(&mut source, &mut destination)
            .map_err(|e| format!("copying the primary index: {e}"))?;
        Ok(operation)
    }

    async fn git(&self, dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
        git_with_index(dir, args, timeout, Some(&self.path)).await
    }

    fn install(&mut self) -> Result<(), String> {
        std::fs::copy(&self.path, &self.lock)
            .map_err(|e| format!("preparing the completed index: {e}"))?;
        std::fs::rename(&self.lock, &self.real)
            .map_err(|e| format!("installing the completed index: {e}"))?;
        self.owns_lock = false;
        Ok(())
    }
}

impl Drop for OperationIndex {
    fn drop(&mut self) {
        if self.owns_index {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(self.path.with_extension("lock"));
        }
        if self.owns_lock {
            let _ = std::fs::remove_file(&self.lock);
        }
    }
}

/// The git state files/dirs that mean another operation owns the checkout.
/// A squash commit made mid-cherry-pick would conclude *that* operation, so
/// this is a hard preflight — the unwinding reset must never disturb state
/// that was not ours. Resolve the actual Git directory, including separate
/// Git directories, before checking these paths.
fn in_progress_operation(git_dir: &Path) -> Option<&'static str> {
    for (name, what) in [
        ("MERGE_HEAD", "merge"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
        ("BISECT_LOG", "bisect"),
    ] {
        if git_dir.join(name).exists() {
            return Some(what);
        }
    }
    None
}

/// Roll back only the operation index and its worktree changes. reset --merge
/// refuses overlapping unstaged edits, and the real index stays byte-for-byte
/// untouched. Never reset a foreign index or discard post-merge editor saves.
async fn unwind(primary: &Path, index: &OperationIndex, revision: &str) -> Result<(), String> {
    let head = git(
        primary,
        &["rev-parse", "--verify", "HEAD"],
        Duration::from_secs(20),
    )
    .await?;
    if head.trim() != revision {
        return Err("The primary HEAD changed; refusing to reset another writer's revision".into());
    }
    index
        .git(
            primary,
            &["reset", "--merge", revision],
            Duration::from_secs(60),
        )
        .await
        .map(|_| ())
}

/// Verified local destination, captured before either a squash or dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalLandingTarget {
    pub branch: String,
    pub checkout: String,
    pub revision: String,
    pub status: String,
}

pub async fn inspect_local_target(
    repo_root: &Path,
    topic_branch: &str,
) -> Result<LocalLandingTarget, LandingError> {
    // Resolve the primary checkout (the main working tree).
    let listing = git(
        repo_root,
        &["worktree", "list", "--porcelain"],
        Duration::from_secs(20),
    )
    .await
    .map_err(|detail| LandingError::RepoUnreachable {
        path: repo_root.display().to_string(),
        detail,
    })?;
    let Some(primary) = parse_primary_worktree(&listing) else {
        return Err(LandingError::RepoUnreachable {
            path: repo_root.display().to_string(),
            detail: "could not parse `git worktree list`".into(),
        });
    };
    if primary.bare {
        return Err(LandingError::Bare {
            path: repo_root.display().to_string(),
        });
    }
    let target_branch = git(
        &primary.path,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        Duration::from_secs(20),
    )
    .await
    .map_err(|_| LandingError::Detached)?
    .trim()
    .to_owned();
    if primary.branch.as_deref() != Some(target_branch.as_str()) {
        return Err(LandingError::RepairNeeded {
            detail: "The primary branch changed during inspection; refresh before landing.".into(),
        });
    }
    if target_branch == topic_branch {
        return Err(LandingError::SelfMerge {
            branch: topic_branch.to_owned(),
        });
    }
    let topic_ref = format!("refs/heads/{topic_branch}");
    git(
        &primary.path,
        &["rev-parse", "-q", "--verify", &topic_ref],
        Duration::from_secs(20),
    )
    .await
    .map_err(|_| LandingError::BranchMissing {
        branch: topic_branch.to_owned(),
    })?;
    // The checkout must be ours to drive: another in-progress operation
    // would make the squash commit conclude *that* operation instead.
    let git_dir = git(
        &primary.path,
        &["rev-parse", "--absolute-git-dir"],
        Duration::from_secs(20),
    )
    .await
    .map_err(|detail| LandingError::Failed { detail })?;
    if let Some(what) = in_progress_operation(Path::new(git_dir.trim())) {
        return Err(LandingError::OperationInProgress {
            what: what.to_owned(),
        });
    }
    let revision = git(
        &primary.path,
        &["rev-parse", "HEAD"],
        Duration::from_secs(20),
    )
    .await
    .map_err(|detail| LandingError::Failed { detail })?;
    let status = git(
        &primary.path,
        &["status", "--porcelain", "--untracked-files=normal"],
        Duration::from_secs(20),
    )
    .await
    .map_err(|detail| LandingError::Failed { detail })?;
    Ok(LocalLandingTarget {
        branch: target_branch,
        checkout: primary.path.display().to_string(),
        revision: revision.trim().into(),
        status,
    })
}

/// Compatibility entry point: capture fresh pins and use the guarded API.
pub async fn squash_into_primary_checkout(
    repo_root: &Path,
    topic_branch: &str,
    title: &str,
) -> Result<LocalLanding, LandingError> {
    let topic_branch = topic_branch.trim();
    let target = inspect_local_target(repo_root, topic_branch).await?;
    let revision = git(
        Path::new(&target.checkout),
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{topic_branch}^{{commit}}"),
        ],
        Duration::from_secs(20),
    )
    .await
    .map_err(|detail| LandingError::Failed { detail })?;
    let mut landing = squash_into_primary_checkout_pinned(
        repo_root,
        topic_branch,
        revision.trim(),
        &target,
        title,
    )
    .await?;
    // Keep the historical no-op display contract for existing callers. The
    // pinned API exposes the known target revision for publication evidence.
    if !landing.landed {
        landing.commit = None;
    }
    Ok(landing)
}

/// Land exactly the reviewed source SHA into exactly the inspected primary
/// destination. Changed pins queue repair before any worktree/index writes.
/// Normal primary Git writers are excluded for the entire operation, including
/// hooks and rollback; merge consumes an immutable SHA, never a branch ref.
pub async fn squash_into_primary_checkout_pinned(
    repo_root: &Path,
    source_branch: &str,
    expected_full_sha: &str,
    expected_target: &LocalLandingTarget,
    title: &str,
) -> Result<LocalLanding, LandingError> {
    let repair = |detail| LandingError::RepairNeeded { detail };
    if source_branch.is_empty()
        || source_branch != source_branch.trim()
        || ![40, 64].contains(&expected_full_sha.len())
        || !expected_full_sha.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(repair(
            "The source branch or immutable revision is invalid; refresh before landing.".into(),
        ));
    }
    let primary = Path::new(&expected_target.checkout);
    // Resolve the real index via Git (also works with a separate Git directory).
    let index_path = git(
        primary,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        Duration::from_secs(20),
    )
    .await
    .map_err(repair)?;
    let mut index = OperationIndex::acquire(PathBuf::from(index_path.trim())).map_err(repair)?;
    let target = inspect_local_target(repo_root, source_branch)
        .await
        .map_err(|e| repair(format!("The landing target changed or is unavailable: {e}")))?;
    if &target != expected_target {
        return Err(repair(
            "The primary checkout branch, HEAD or status changed; refresh before landing.".into(),
        ));
    }
    if status_has_staged_changes(&target.status) {
        return Err(LandingError::StagedChanges);
    }
    let topic_ref = format!("refs/heads/{source_branch}^{{commit}}");
    let source_revision = git(
        primary,
        &["rev-parse", "--verify", &topic_ref],
        Duration::from_secs(20),
    )
    .await
    .map_err(repair)?;
    if source_revision.trim() != expected_full_sha {
        return Err(repair(
            "The canonical source revision changed; refresh before landing.".into(),
        ));
    }
    // All fallible preparation precedes merge, so it cannot leave squash dirt.
    let log = git(
        primary,
        &[
            "log",
            "--reverse",
            "--format=%s",
            &format!("{}..{expected_full_sha}", target.revision),
        ],
        Duration::from_secs(30),
    )
    .await
    .map_err(repair)?;
    let subjects: Vec<&str> = log.lines().filter(|s| !s.is_empty()).collect();
    let title = if title.trim().is_empty() {
        source_branch
    } else {
        title
    };
    let subject = format!("{} (squash into {})", shorten(title, 48), target.branch);
    let mut body = format!("Squash-merged {} commit{} from {} into {} in the primary checkout by Arachne's Land action.",
        subjects.len(), if subjects.len() == 1 { "" } else { "s" }, source_branch, target.branch);
    if !subjects.is_empty() {
        body.push_str("\n\n");
        for s in subjects.iter().take(30) {
            body.push_str(&format!("* {s}\n"));
        }
        if subjects.len() > 30 {
            body.push_str(&format!("* …and {} more\n", subjects.len() - 30));
        }
    }
    // Inspect again at the write boundary. Reads never refresh the locked live
    // index. Git itself guards tracked/untracked/ignored worktree collisions.
    let boundary = inspect_local_target(repo_root, source_branch)
        .await
        .map_err(|e| repair(format!("The landing target changed: {e}")))?;
    let source_revision = git(
        primary,
        &["rev-parse", "--verify", &topic_ref],
        Duration::from_secs(20),
    )
    .await
    .map_err(repair)?;
    if boundary != target || source_revision.trim() != expected_full_sha {
        return Err(repair(
            "The source or primary checkout changed before landing; refresh before landing.".into(),
        ));
    }
    let attempt = async {
        index
            .git(
                primary,
                &[
                    "merge",
                    "--squash",
                    "--no-overwrite-ignore",
                    expected_full_sha,
                ],
                Duration::from_secs(300),
            )
            .await?;
        let staged = index
            .git(
                primary,
                &["diff", "--cached", "--name-only"],
                Duration::from_secs(30),
            )
            .await?;
        if staged.is_empty() {
            return Ok(None);
        }
        let current_branch = git(
            primary,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
            Duration::from_secs(20),
        )
        .await?;
        let current_head = git(
            primary,
            &["rev-parse", "--verify", "HEAD"],
            Duration::from_secs(20),
        )
        .await?;
        if current_branch.trim() != target.branch || current_head.trim() != target.revision {
            return Err("The primary branch or HEAD changed before committing".into());
        }
        let stdout = index
            .git(
                primary,
                &[
                    "-c",
                    &format!("core.abbrev={}", expected_full_sha.len()),
                    "commit",
                    "-m",
                    &subject,
                    "-m",
                    &body,
                ],
                Duration::from_secs(600),
            )
            .await?;
        Ok::<Option<String>, String>(Some(stdout))
    }
    .await;
    let commit_stdout = match attempt {
        Ok(stdout) => stdout,
        Err(detail) => {
            let message = format!("squash landing failed: {detail}");
            return match unwind(primary, &index, &target.revision).await {
                Ok(()) => Err(repair(message)),
                Err(cleanup) => Err(LandingError::Failed { detail: format!("{message}; isolated rollback failed: {cleanup}. Preserve checkout edits and repair manually.") }),
            };
        }
    };
    if commit_stdout.is_none() {
        unwind(primary, &index, &target.revision)
            .await
            .map_err(|detail| LandingError::Failed { detail })?;
        return Ok(LocalLanding {
            landed: false,
            target_branch: target.branch,
            commit: Some(target.revision),
            commits_squashed: 0,
            primary_checkout: target.checkout,
            warnings: vec![],
        });
    }
    // Commit succeeded. From here on, every failure is a completion warning,
    // never an ordinary error that could authorize a duplicate attempt.
    let mut warnings = vec![];
    // Git's own final commit summary contains the full object ID because we
    // requested full abbreviation. Keep this known outcome if a post-commit
    // hook or read failure makes HEAD unavailable or moves it elsewhere.
    let known_commit = commit_stdout.as_deref().and_then(|stdout| {
        stdout.lines().rev().find_map(|line| {
            let summary = line.strip_prefix('[')?.split_once(']')?.0;
            let sha = summary.split_whitespace().last()?;
            (sha.len() == expected_full_sha.len() && sha.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| sha.to_owned())
        })
    });
    let commit = match git(
        primary,
        &["rev-parse", "--verify", "HEAD"],
        Duration::from_secs(20),
    )
    .await
    {
        Ok(revision)
            if known_commit
                .as_deref()
                .is_some_and(|known| known != revision.trim()) =>
        {
            warnings.push("The squash commit completed, but HEAD subsequently changed; reporting the completed commit.".into());
            known_commit
        }
        Ok(revision) => Some(revision.trim().to_owned()),
        Err(detail) => {
            warnings.push(format!(
                "The squash commit completed, but its revision could not be read: {detail}"
            ));
            known_commit
        }
    };
    if let Err(detail) = index.install() {
        warnings.push(detail);
    }
    Ok(LocalLanding {
        landed: true,
        target_branch: target.branch,
        commit,
        commits_squashed: subjects.len() as u32,
        primary_checkout: target.checkout,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        collapse, parse_primary_worktree, shorten, squash_into_primary_checkout, LandingError,
    };
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    // --- pure helpers ------------------------------------------------------

    #[test]
    fn primary_worktree_is_the_first_record_with_its_branch() {
        let listing = "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\n\
worktree /repo/.worktrees/topic\nHEAD def\nbranch refs/heads/weaver/topic\n";
        let primary = parse_primary_worktree(listing).unwrap();
        assert_eq!(primary.path, Path::new("/repo"));
        assert_eq!(primary.branch.as_deref(), Some("main"));
        assert!(!primary.bare);
    }

    #[test]
    fn detached_primary_reports_no_branch() {
        let listing = "worktree /repo\nHEAD abc\ndetached\n";
        let primary = parse_primary_worktree(listing).unwrap();
        assert_eq!(primary.branch, None);
    }

    #[test]
    fn bare_repository_has_no_checkout() {
        let listing = "worktree /repo\nHEAD abc\nbare\n";
        let primary = parse_primary_worktree(listing).unwrap();
        assert!(primary.bare);
        assert_eq!(primary.branch, None);
    }

    #[test]
    fn collapse_bounds_and_flattens_git_output() {
        assert_eq!(collapse("a\n  b\tc"), "a b c");
        let collapsed = collapse(&"x".repeat(500));
        assert_eq!(collapsed.chars().count(), 401); // 400 + ellipsis
    }

    #[test]
    fn titles_flatten_to_one_bounded_line() {
        assert_eq!(shorten("Multi\nline  title", 48), "Multi line title");
        assert_eq!(shorten(&"x".repeat(60), 48).chars().count(), 48);
        assert_eq!(shorten("short", 48), "short");
    }

    // --- against real git --------------------------------------------------
    // These exercise the actual contract: squash, preserve dirt, unwind on
    // conflict, refuse unsafe states. They need `git` on PATH and a
    // writable temp dir; each gets a unique directory to run in parallel.

    fn temp_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "arachne-landing-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn sh(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .expect("git binary");
        assert!(
            out.status.success(),
            "git {:?} in {dir:?}: {}{}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repo on `main` with one commit, plus a `weaver/topic` branch with
    /// one commit adding `topic.txt`. Returns the repo dir, on `main`.
    fn repo_with_topic(name: &str) -> PathBuf {
        let dir = temp_repo(name);
        sh(&dir, &["init", "-q", "-b", "main"]);
        sh(&dir, &["config", "user.email", "test@example.com"]);
        sh(&dir, &["config", "user.name", "Arachne Test"]);
        sh(&dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("base.txt"), "base\n").unwrap();
        sh(&dir, &["add", "."]);
        sh(&dir, &["commit", "-q", "-m", "base"]);
        sh(&dir, &["checkout", "-q", "-b", "weaver/topic"]);
        std::fs::write(dir.join("topic.txt"), "topic\n").unwrap();
        sh(&dir, &["add", "."]);
        sh(&dir, &["commit", "-q", "-m", "topic work"]);
        sh(&dir, &["checkout", "-q", "main"]);
        dir
    }

    fn status(dir: &Path) -> String {
        String::from_utf8_lossy(
            &std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["status", "--porcelain"])
                .output()
                .unwrap()
                .stdout,
        )
        .to_string()
    }

    async fn pins(dir: &Path) -> (String, super::LocalLandingTarget) {
        let target = super::inspect_local_target(dir, "weaver/topic")
            .await
            .unwrap();
        let source = super::git(dir, &["rev-parse", "weaver/topic"], Duration::from_secs(20))
            .await
            .unwrap();
        (source.trim().into(), target)
    }

    #[cfg(unix)]
    fn hook(dir: &Path, name: &str, script: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(".git/hooks").join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[tokio::test]
    async fn ignored_collision_queues_repair_preserving_exact_bytes() {
        let dir = repo_with_topic("ignored-collision");
        sh(&dir, &["checkout", "-q", "weaver/topic"]);
        std::fs::write(dir.join("private.env"), "accepted content\n").unwrap();
        sh(&dir, &["add", "private.env"]);
        sh(&dir, &["commit", "-q", "-m", "accepted environment"]);
        sh(&dir, &["checkout", "-q", "main"]);
        std::fs::write(dir.join(".git/info/exclude"), "private.env\n").unwrap();
        let foreign = b"foreign secret\0\xff\n";
        std::fs::write(dir.join("private.env"), foreign).unwrap();
        let (source, target) = pins(&dir).await;
        let before_index = std::fs::read(dir.join(".git/index")).unwrap();
        let error = super::squash_into_primary_checkout_pinned(
            &dir,
            "weaver/topic",
            &source,
            &target,
            "land",
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, LandingError::RepairNeeded { .. }),
            "{error}"
        );
        assert_eq!(std::fs::read(dir.join("private.env")).unwrap(), foreign);
        assert_eq!(std::fs::read(dir.join(".git/index")).unwrap(), before_index);
        assert_eq!(pins(&dir).await.1, target);
        assert!(!dir.join(".git/index.lock").exists());
        assert!(!dir.join("topic.txt").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn refuses_advanced_source_pin_without_writing() {
        let dir = repo_with_topic("source-pin");
        let (source, target) = pins(&dir).await;
        sh(&dir, &["checkout", "-q", "weaver/topic"]);
        std::fs::write(dir.join("later.txt"), "later\n").unwrap();
        sh(&dir, &["add", "later.txt"]);
        sh(&dir, &["commit", "-q", "-m", "source advances"]);
        sh(&dir, &["checkout", "-q", "main"]);
        let error = super::squash_into_primary_checkout_pinned(
            &dir,
            "weaver/topic",
            &source,
            &target,
            "land",
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, LandingError::RepairNeeded { .. }),
            "{error}"
        );
        assert!(error.to_string().contains("changed"));
        assert_eq!(pins(&dir).await.1, target);
        assert!(!dir.join("topic.txt").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn refuses_changed_target_head_branch_and_status_without_writing() {
        for change in ["head", "branch", "unstaged", "staged", "untracked"] {
            let dir = repo_with_topic(change);
            let (source, target) = pins(&dir).await;
            match change {
                "head" => sh(
                    &dir,
                    &["commit", "-q", "--allow-empty", "-m", "target advances"],
                ),
                "branch" => sh(&dir, &["checkout", "-q", "-b", "other"]),
                "unstaged" => std::fs::write(dir.join("base.txt"), b"foreign\0\xff").unwrap(),
                "staged" => {
                    std::fs::write(dir.join("base.txt"), b"foreign staged\0\xff").unwrap();
                    sh(&dir, &["add", "base.txt"]);
                }
                _ => std::fs::write(dir.join("foreign.txt"), b"foreign\0\xff").unwrap(),
            }
            let before = pins(&dir).await.1;
            let before_index = std::fs::read(dir.join(".git/index")).unwrap();
            let error = super::squash_into_primary_checkout_pinned(
                &dir,
                "weaver/topic",
                &source,
                &target,
                "land",
            )
            .await
            .unwrap_err();
            assert!(
                matches!(error, LandingError::RepairNeeded { .. }),
                "{change}: {error}"
            );
            assert_eq!(pins(&dir).await.1, before);
            assert_eq!(std::fs::read(dir.join(".git/index")).unwrap(), before_index);
            assert!(!dir.join("topic.txt").exists());
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn foreign_staging_is_blocked_while_hooks_use_isolated_index() {
        let dir = repo_with_topic("foreign-staging");
        let foreign = b"foreign\0\xff\n";
        std::fs::write(dir.join("foreign.txt"), foreign).unwrap();
        hook(
            &dir,
            "pre-commit",
            r#"
            test -f .git/index.lock || exit 20
            test "$GIT_INDEX_FILE" != "$PWD/.git/index" || exit 21
            env -u GIT_INDEX_FILE git add foreign.txt 2>.git/foreign-staging-error
            result=$?
            echo "$result" >.git/foreign-staging-result
            test "$result" -ne 0 || exit 22
            git diff --cached --name-only >.git/hook-staged-paths
        "#,
        );
        let out = squash_into_primary_checkout(&dir, "weaver/topic", "land")
            .await
            .unwrap();
        assert!(out.landed);
        assert_eq!(out.commit.as_ref().unwrap().len(), 40);
        assert!(
            std::fs::read_to_string(dir.join(".git/foreign-staging-error"))
                .unwrap()
                .contains("index.lock")
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(".git/hook-staged-paths")).unwrap(),
            "topic.txt\n"
        );
        assert_eq!(std::fs::read(dir.join("foreign.txt")).unwrap(), foreign);
        assert_eq!(status(&dir), "?? foreign.txt\n");
        let tree = super::git(
            &dir,
            &["ls-tree", "--name-only", "HEAD"],
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        assert!(!tree.contains("foreign.txt"));
        assert!(!dir.join(".git/index.lock").exists());
        // After release, the foreign edit can still be staged normally.
        sh(&dir, &["add", "foreign.txt"]);
        assert_eq!(status(&dir), "A  foreign.txt\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hook_rejection_rolls_back_only_operation_preserving_foreign_dirt() {
        let dir = repo_with_topic("hook-rejection");
        std::fs::write(dir.join("base.txt"), b"foreign tracked\0\xff").unwrap();
        std::fs::write(dir.join("foreign.txt"), b"foreign untracked\0\xff").unwrap();
        std::fs::write(dir.join(".git/info/exclude"), "ignored.txt\n").unwrap();
        std::fs::write(dir.join("ignored.txt"), b"foreign ignored\0\xff").unwrap();
        let (source, target) = pins(&dir).await;
        let before_index = std::fs::read(dir.join(".git/index")).unwrap();
        hook(
            &dir,
            "pre-commit",
            "test -n \"$GIT_INDEX_FILE\" || exit 20\nexit 1",
        );
        let error = super::squash_into_primary_checkout_pinned(
            &dir,
            "weaver/topic",
            &source,
            &target,
            "land",
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, LandingError::RepairNeeded { .. }),
            "{error}"
        );
        assert_eq!(pins(&dir).await.1, target);
        assert_eq!(std::fs::read(dir.join(".git/index")).unwrap(), before_index);
        assert_eq!(
            std::fs::read(dir.join("base.txt")).unwrap(),
            b"foreign tracked\0\xff"
        );
        assert_eq!(
            std::fs::read(dir.join("foreign.txt")).unwrap(),
            b"foreign untracked\0\xff"
        );
        assert_eq!(
            std::fs::read(dir.join("ignored.txt")).unwrap(),
            b"foreign ignored\0\xff"
        );
        assert!(!dir.join("topic.txt").exists());
        assert!(!dir.join(".git/index.lock").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn existing_real_index_lock_is_preserved_and_blocks_landing() {
        let dir = repo_with_topic("existing-lock");
        let (source, target) = pins(&dir).await;
        let lock_bytes = b"foreign writer lock\0\xff";
        std::fs::write(dir.join(".git/index.lock"), lock_bytes).unwrap();
        let before_index = std::fs::read(dir.join(".git/index")).unwrap();
        let error = super::squash_into_primary_checkout_pinned(
            &dir,
            "weaver/topic",
            &source,
            &target,
            "land",
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, LandingError::RepairNeeded { .. }),
            "{error}"
        );
        assert_eq!(
            std::fs::read(dir.join(".git/index.lock")).unwrap(),
            lock_bytes
        );
        assert_eq!(std::fs::read(dir.join(".git/index")).unwrap(), before_index);
        assert!(!dir.join("topic.txt").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn merge_uses_immutable_source_when_branch_advances_at_merge_invocation() {
        let dir = repo_with_topic("immutable-merge");
        let (source, target) = pins(&dir).await;
        sh(&dir, &["checkout", "-q", "weaver/topic"]);
        std::fs::write(dir.join("later.txt"), "unreviewed later change\n").unwrap();
        sh(&dir, &["add", "later.txt"]);
        sh(&dir, &["commit", "-q", "-m", "unreviewed later change"]);
        let advanced = super::git(&dir, &["rev-parse", "HEAD"], Duration::from_secs(20))
            .await
            .unwrap();
        sh(&dir, &["checkout", "-q", "main"]);
        sh(&dir, &["update-ref", "refs/heads/weaver/topic", &source]);
        use std::os::unix::fs::PermissionsExt;
        let wrapper = dir.join(".git/arachne-test-git");
        std::fs::write(
            &wrapper,
            format!(
                r#"#!/bin/sh
            if [ "$3" = merge ]; then
                git -C "$2" update-ref refs/heads/weaver/topic {}
                printf '%s\n' "$6" >"$2/.git/merge-revision"
            fi
            exec git "$@"
        "#,
                advanced.trim()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = super::squash_into_primary_checkout_pinned(
            &dir,
            "weaver/topic",
            &source,
            &target,
            "land",
        )
        .await
        .unwrap();
        assert!(out.landed);
        assert_eq!(pins(&dir).await.0, advanced.trim());
        assert_eq!(
            std::fs::read_to_string(dir.join(".git/merge-revision"))
                .unwrap()
                .trim(),
            source
        );
        assert!(!dir.join("later.txt").exists());
        let tree = super::git(
            &dir,
            &["ls-tree", "--name-only", "HEAD"],
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        assert_eq!(tree, "base.txt\ntopic.txt\n");
        assert_eq!(out.commits_squashed, 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rollback_refuses_to_discard_post_merge_editor_save() {
        let dir = repo_with_topic("postmerge-editor");
        let (source, target) = pins(&dir).await;
        hook(
            &dir,
            "pre-commit",
            "printf 'foreign editor save' >topic.txt\nexit 1",
        );
        let error = super::squash_into_primary_checkout_pinned(
            &dir,
            "weaver/topic",
            &source,
            &target,
            "land",
        )
        .await
        .unwrap_err();
        assert!(matches!(error, LandingError::Failed { .. }), "{error}");
        assert_eq!(
            std::fs::read(dir.join("topic.txt")).unwrap(),
            b"foreign editor save"
        );
        assert_eq!(pins(&dir).await.1.revision, target.revision);
        assert!(!dir.join(".git/index.lock").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn post_commit_head_read_failure_still_reports_completion() {
        use std::os::unix::fs::PermissionsExt;
        for suppress_summary in [false, true] {
            let dir = repo_with_topic("postcommit-read");
            let (source, target) = pins(&dir).await;
            // The real commit command and hooks succeed. Only the helper's
            // subsequent HEAD read fails; no shared PATH/global env is changed.
            let wrapper = dir.join(".git/arachne-test-git");
            std::fs::write(&wrapper, r#"#!/bin/sh
                if [ -f "$2/.git/read-head-fault" ] && [ "$3" = rev-parse ] && [ "$4" = --verify ] && [ "$5" = HEAD ]; then
                    echo 'injected post-commit HEAD read failure' >&2
                    exit 1
                fi
                if [ -f "$2/.git/suppress-commit-summary" ] && [ "$3" = -c ] && [ "$5" = commit ]; then
                    git "$@" >/dev/null
                    exit $?
                fi
                exec git "$@"
            "#).unwrap();
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
            if suppress_summary {
                std::fs::write(dir.join(".git/suppress-commit-summary"), "").unwrap();
            }
            hook(&dir, "post-commit", "touch .git/read-head-fault");
            let out = super::squash_into_primary_checkout_pinned(
                &dir,
                "weaver/topic",
                &source,
                &target,
                "land",
            )
            .await
            .unwrap();
            assert!(out.landed);
            assert!(!out.warnings.is_empty());
            std::fs::remove_file(dir.join(".git/read-head-fault")).unwrap();
            let new_target = pins(&dir).await.1;
            assert_ne!(new_target.revision, target.revision);
            if suppress_summary {
                assert_eq!(out.commit, None);
            } else {
                assert_eq!(out.commit, Some(new_target.revision));
            }
            assert_eq!(new_target.status, "");
            assert_eq!(
                std::fs::read_to_string(dir.join("topic.txt")).unwrap(),
                "topic\n"
            );
            assert_eq!(
                super::git(
                    &dir,
                    &["rev-list", "--count", "main"],
                    Duration::from_secs(20)
                )
                .await
                .unwrap()
                .trim(),
                "2"
            );
            assert!(!dir.join(".git/index.lock").exists());
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[tokio::test]
    async fn source_preflight_reports_uncommitted_editor_changes() {
        let dir = repo_with_topic("source-edits");
        sh(&dir, &["checkout", "-q", "weaver/topic"]);
        assert!(super::inspect_source_checkout(&dir, "weaver/topic")
            .await
            .is_ok());
        std::fs::write(dir.join("topic.txt"), "manual edit\n").unwrap();
        let state = super::inspect_source_checkout(&dir, "weaver/topic")
            .await
            .unwrap();
        assert!(state.status.contains(" M topic.txt"));
        assert!(!state.revision.is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.join("topic.txt")).unwrap(),
            "manual edit\n"
        );
        assert!(super::inspect_source_checkout(&dir, "another-branch")
            .await
            .is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn lands_into_primary_checkout_preserving_unrelated_dirt() {
        let dir = repo_with_topic("land");
        std::fs::write(dir.join("dirt.txt"), "unrelated").unwrap();
        let out = squash_into_primary_checkout(&dir, "weaver/topic", "Add topic work")
            .await
            .unwrap();
        assert!(out.landed);
        assert_eq!(out.target_branch, "main");
        assert!(out.commit.is_some());
        assert_eq!(out.commits_squashed, 1);
        // The topic's change is on main as one new commit…
        assert!(dir.join("topic.txt").exists());
        assert_eq!(
            super::git(
                &dir,
                &["rev-list", "--count", "main"],
                Duration::from_secs(30)
            )
            .await
            .unwrap()
            .trim(),
            "2"
        );
        // …the unrelated untracked file survived, and the subject names the
        // squash.
        assert!(dir.join("dirt.txt").exists());
        let log = super::git(&dir, &["log", "-1", "--format=%s"], Duration::from_secs(30))
            .await
            .unwrap();
        assert!(log.contains("Add topic work (squash into main)"), "{log}");
    }

    #[tokio::test]
    async fn already_landed_topic_reports_a_no_op() {
        let dir = repo_with_topic("noop");
        let first = squash_into_primary_checkout(&dir, "weaver/topic", "Add topic work")
            .await
            .unwrap();
        assert!(first.landed);
        let second = squash_into_primary_checkout(&dir, "weaver/topic", "Add topic work")
            .await
            .unwrap();
        assert!(!second.landed);
        assert_eq!(second.commit, None);
        assert_eq!(second.commits_squashed, 0);
        assert_eq!(
            super::git(
                &dir,
                &["rev-list", "--count", "main"],
                Duration::from_secs(30)
            )
            .await
            .unwrap()
            .trim(),
            "2"
        );
        // No leftover squash state from the no-op attempt.
        assert_eq!(status(&dir), "");
    }

    #[tokio::test]
    async fn pinned_no_op_returns_the_known_target_revision() {
        let dir = repo_with_topic("pinned-noop");
        squash_into_primary_checkout(&dir, "weaver/topic", "land")
            .await
            .unwrap();
        let (source, target) = pins(&dir).await;
        let before_index = std::fs::read(dir.join(".git/index")).unwrap();
        let out = super::squash_into_primary_checkout_pinned(
            &dir,
            "weaver/topic",
            &source,
            &target,
            "land",
        )
        .await
        .unwrap();
        assert!(!out.landed);
        assert_eq!(out.commit, Some(target.revision.clone()));
        assert_eq!(out.commits_squashed, 0);
        assert!(out.warnings.is_empty());
        assert_eq!(pins(&dir).await.1, target);
        assert_eq!(std::fs::read(dir.join(".git/index")).unwrap(), before_index);
        assert!(!dir.join(".git/SQUASH_MSG").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn conflicts_are_unwound_and_dirt_preserved() {
        let dir = repo_with_topic("conflict");
        // Diverge the topic and main on the same file, so the squash
        // conflicts — the deterministic path must refuse, not resolve.
        sh(&dir, &["checkout", "-q", "weaver/topic"]);
        std::fs::write(dir.join("base.txt"), "topic's version\n").unwrap();
        sh(&dir, &["add", "."]);
        sh(&dir, &["commit", "-q", "-m", "topic diverges"]);
        sh(&dir, &["checkout", "-q", "main"]);
        std::fs::write(dir.join("base.txt"), "main's version\n").unwrap();
        sh(&dir, &["add", "."]);
        sh(&dir, &["commit", "-q", "-m", "main diverges"]);
        std::fs::write(dir.join("untracked.txt"), "keep me").unwrap();
        let err = squash_into_primary_checkout(&dir, "weaver/topic", "Add topic work")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("failed"), "{err}");
        // The checkout is back to its own state: main's content, the
        // uncommitted untracked file, nothing staged, no merge leftovers.
        assert_eq!(
            std::fs::read_to_string(dir.join("base.txt")).unwrap(),
            "main's version\n"
        );
        assert_eq!(status(&dir), "?? untracked.txt\n");
        assert!(!dir.join("topic.txt").exists());
    }

    #[tokio::test]
    async fn refuses_detached_primary() {
        let dir = repo_with_topic("detached");
        sh(&dir, &["checkout", "-q", "--detach"]);
        let err = squash_into_primary_checkout(&dir, "weaver/topic", "t")
            .await
            .unwrap_err();
        assert!(matches!(err, LandingError::Detached), "{err}");
    }

    #[tokio::test]
    async fn refuses_self_merge_when_topic_is_checked_out() {
        let dir = repo_with_topic("selfmerge");
        sh(&dir, &["checkout", "-q", "weaver/topic"]);
        let err = squash_into_primary_checkout(&dir, "weaver/topic", "t")
            .await
            .unwrap_err();
        assert!(
            matches!(err, LandingError::SelfMerge { .. } if err.to_string().contains("weaver/topic")),
            "{err}"
        );
    }

    #[tokio::test]
    async fn refuses_missing_branch() {
        let dir = repo_with_topic("missing");
        let err = squash_into_primary_checkout(&dir, "weaver/gone", "t")
            .await
            .unwrap_err();
        assert!(
            matches!(err, LandingError::BranchMissing { .. } if err.to_string().contains("weaver/gone")),
            "{err}"
        );
    }

    #[tokio::test]
    async fn refuses_staged_changes() {
        let dir = repo_with_topic("staged");
        std::fs::write(dir.join("base.txt"), "staged\n").unwrap();
        sh(&dir, &["add", "."]);
        let err = squash_into_primary_checkout(&dir, "weaver/topic", "t")
            .await
            .unwrap_err();
        assert!(matches!(err, LandingError::StagedChanges), "{err}");
        // Refusal touched nothing: the staged change is still staged.
        assert_eq!(status(&dir), "M  base.txt\n");
    }

    #[tokio::test]
    async fn refuses_when_another_operation_owns_the_checkout() {
        let dir = repo_with_topic("inprogress");
        // Start a conflicting merge in the primary checkout and leave it
        // unresolved: MERGE_HEAD exists.
        sh(&dir, &["checkout", "-q", "-b", "other"]);
        std::fs::write(dir.join("base.txt"), "other's version\n").unwrap();
        sh(&dir, &["add", "."]);
        sh(&dir, &["commit", "-q", "-m", "other diverges"]);
        sh(&dir, &["checkout", "-q", "main"]);
        std::fs::write(dir.join("base.txt"), "main's version\n").unwrap();
        sh(&dir, &["add", "."]);
        sh(&dir, &["commit", "-q", "-m", "main diverges"]);
        let merge = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["merge", "other"])
            .output()
            .unwrap();
        assert!(!merge.status.success(), "fixture merge should conflict");
        let err = squash_into_primary_checkout(&dir, "weaver/topic", "t")
            .await
            .unwrap_err();
        assert!(
            matches!(err, LandingError::OperationInProgress { ref what } if what == "merge"),
            "{err}"
        );
        // The in-progress merge is untouched — ours to refuse, not unwind.
        assert!(dir.join(".git").join("MERGE_HEAD").exists());
    }

    #[tokio::test]
    async fn lands_from_any_directory_of_the_repository() {
        // loom may report the repo root, but a linked worktree's path must
        // resolve the same primary checkout.
        let dir = repo_with_topic("linked");
        sh(&dir, &["worktree", "add", "-q", "--detach", ".wt", "main"]);
        let out = squash_into_primary_checkout(&dir.join(".wt"), "weaver/topic", "Add topic work")
            .await
            .unwrap();
        assert!(out.landed);
        // The squash commit landed in the main working tree, not the
        // linked one — the linked checkout stays clean and unmoved.
        assert!(dir.join("topic.txt").exists());
        assert_eq!(status(&dir.join(".wt")), "");
    }
}
