//! Deterministic local landing: squash-merge a topic branch into the primary
//! checkout's currently checked out branch (spec:
//! docs/integration-and-landing.md, "Land locally").
//!
//! The other Land strategies are intent constraints handed to the topic's
//! coordinator; this one is the deterministic fast path — Arachne runs git
//! itself against the repository's primary checkout, the main working tree
//! a human actually opens, with no agent turn. It deliberately does exactly
//! one thing: squash the topic branch onto the checkout's current branch,
//! or refuse with an explanation.
//!
//! It never destroys the checkout's uncommitted state. Unrelated unstaged
//! changes and untracked files survive the landing (git's merge machinery
//! refuses to touch overlapping dirt before writing anything); a failed or
//! conflicted attempt is unwound with `git reset --merge`, which restores
//! HEAD and the index while keeping unstaged changes. Anything that would
//! make the squash commit absorb foreign work — staged changes, an
//! in-progress merge/rebase/cherry-pick — is refused up front rather than
//! reconciled: reconciliation is the agent-mediated strategies' job.

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
    /// The resulting squash commit (short sha), when one was created.
    pub commit: Option<String>,
    /// How many topic commits the squash carries.
    pub commits_squashed: u32,
    /// The primary checkout the landing ran in.
    pub primary_checkout: String,
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

/// Local landing works from committed branch state. Refuse a source checkout
/// with manual edits so an in-app save cannot be silently omitted from a land.
pub async fn ensure_clean_source_checkout(checkout: &Path, branch: &str) -> Result<(), String> {
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
    let status = git(
        checkout,
        &["status", "--porcelain", "--untracked-files=normal"],
        timeout,
    )
    .await?;
    if !status.trim().is_empty() {
        return Err("This thread has uncommitted edits. Use Review → Prepare landing to validate and commit them before landing locally.".into());
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
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
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

/// True when the index differs from HEAD (`git status --porcelain` rows with
/// a staged status). Unstaged modifications and untracked files don't count
/// — the merge machinery refuses to touch those itself when they overlap.
async fn has_staged_changes(dir: &Path) -> Result<bool, String> {
    let status = git(dir, &["status", "--porcelain"], Duration::from_secs(30)).await?;
    Ok(status.lines().any(|line| {
        let index = line.chars().next().unwrap_or(' ');
        index != ' ' && index != '?'
    }))
}

/// The git state files/dirs that mean another operation owns the checkout.
/// A squash commit made mid-cherry-pick would conclude *that* operation, so
/// this is a hard preflight — the unwinding reset must never disturb state
/// that was not ours. (For the primary worktree `.git` is always a real
/// directory, so plain filesystem checks are exact.)
fn in_progress_operation(primary: &Path) -> Option<&'static str> {
    let git_dir = primary.join(".git");
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

/// Unwind after a failed squash: removes the squash's staged result and
/// merge state while keeping unstaged changes (git's `reset --merge`
/// contract; a no-op when the attempt never got started).
async fn unwind(primary: &Path) -> Result<(), String> {
    git(primary, &["reset", "--merge"], Duration::from_secs(60))
        .await
        .map(|_| ())
}

/// Squash-merge `topic_branch` onto the primary checkout's currently
/// checked out branch, creating one commit there.
///
/// `repo_root` may be any directory of the repository (loom reports the
/// canonical repo root, but the primary is resolved here regardless); the
/// landing always runs in the primary checkout itself — the surface a
/// human actually opens — never a worker worktree. The target is whatever
/// branch that checkout has checked out *now*: a deterministic landing
/// must not act on a remembered target that can go stale.
pub async fn squash_into_primary_checkout(
    repo_root: &Path,
    topic_branch: &str,
    title: &str,
) -> Result<LocalLanding, LandingError> {
    let topic_branch = topic_branch.trim();
    if topic_branch.is_empty() {
        return Err(LandingError::Failed {
            detail: "the topic has no branch to land".into(),
        });
    }
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
    let Some(target_branch) = primary.branch else {
        return Err(LandingError::Detached);
    };
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
    if let Some(what) = in_progress_operation(&primary.path) {
        return Err(LandingError::OperationInProgress {
            what: what.to_owned(),
        });
    }
    // Staged changes would ride along inside the squash commit — refuse.
    if has_staged_changes(&primary.path)
        .await
        .map_err(|detail| LandingError::Failed { detail })?
    {
        return Err(LandingError::StagedChanges);
    }
    // Squash-merge the topic onto the checkout's branch. On failure, unwind
    // to the pre-merge state; the reset preserves unstaged changes and
    // removes the squash's staged result, so the checkout is left exactly
    // as it was found.
    if let Err(detail) = git(
        &primary.path,
        &["merge", "--squash", &topic_ref],
        Duration::from_secs(300),
    )
    .await
    {
        let mut message =
            format!("squash merge of {topic_branch} into {target_branch} failed: {detail}");
        if let Err(cleanup) = unwind(&primary.path).await {
            message.push_str(&format!(
                " (cleanup also failed: {cleanup}; run `git reset --merge` in {} yourself)",
                primary.path.display()
            ));
        }
        return Err(LandingError::Failed { detail: message });
    }
    // An up-to-date squash stages nothing: the target already contains the
    // topic's changes. Unwind the leftover squash state and report the no-op.
    if !has_staged_changes(&primary.path)
        .await
        .map_err(|detail| LandingError::Failed { detail })?
    {
        unwind(&primary.path)
            .await
            .map_err(|detail| LandingError::Failed {
                detail: format!("cleaning up the no-op squash failed: {detail}"),
            })?;
        return Ok(LocalLanding {
            landed: false,
            target_branch,
            commit: None,
            commits_squashed: 0,
            primary_checkout: primary.path.display().to_string(),
        });
    }
    // Compose the commit message from the commits being squashed.
    let target_ref = format!("refs/heads/{target_branch}");
    let log = git(
        &primary.path,
        &[
            "log",
            "--reverse",
            "--format=%s",
            &format!("{target_ref}..{topic_ref}"),
        ],
        Duration::from_secs(30),
    )
    .await
    .map_err(|detail| LandingError::Failed {
        detail: format!("listing the topic's commits: {detail}"),
    })?;
    let subjects: Vec<String> = log
        .lines()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    let title = if title.trim().is_empty() {
        topic_branch
    } else {
        title
    };
    let subject = format!("{} (squash into {})", shorten(title, 48), target_branch);
    let mut body = format!(
        "Squash-merged {} commit{} from {} into {} in the primary checkout by Arachne's Land action.",
        subjects.len(),
        if subjects.len() == 1 { "" } else { "s" },
        topic_branch,
        target_branch,
    );
    if !subjects.is_empty() {
        body.push_str("\n\n");
        for s in subjects.iter().take(30) {
            body.push_str(&format!("* {s}\n"));
        }
        if subjects.len() > 30 {
            body.push_str(&format!("* …and {} more\n", subjects.len() - 30));
        }
    }
    // Commit the squash (hooks run: validation is the checkout's own
    // policy). On failure, unwind so no half-state is left behind.
    if let Err(detail) = git(
        &primary.path,
        &["commit", "-m", &subject, "-m", &body],
        Duration::from_secs(600),
    )
    .await
    {
        let mut message = format!("creating the squash commit failed: {detail}");
        if let Err(cleanup) = unwind(&primary.path).await {
            message.push_str(&format!(
                " (cleanup also failed: {cleanup}; run `git reset --merge` in {} yourself)",
                primary.path.display()
            ));
        }
        return Err(LandingError::Failed { detail: message });
    }
    let commit = git(
        &primary.path,
        &["rev-parse", "--short", "HEAD"],
        Duration::from_secs(20),
    )
    .await
    .map_err(|detail| LandingError::Failed {
        detail: format!("reading the new commit: {detail}"),
    })?;
    Ok(LocalLanding {
        landed: true,
        target_branch,
        commit: Some(commit.trim().to_owned()),
        commits_squashed: subjects.len() as u32,
        primary_checkout: primary.path.display().to_string(),
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

    #[tokio::test]
    async fn source_preflight_rejects_uncommitted_editor_changes() {
        let dir = repo_with_topic("source-edits");
        sh(&dir, &["checkout", "-q", "weaver/topic"]);
        assert!(super::ensure_clean_source_checkout(&dir, "weaver/topic")
            .await
            .is_ok());
        std::fs::write(dir.join("topic.txt"), "manual edit\n").unwrap();
        let error = super::ensure_clean_source_checkout(&dir, "weaver/topic")
            .await
            .unwrap_err();
        assert!(error.contains("uncommitted edits"));
        assert_eq!(
            std::fs::read_to_string(dir.join("topic.txt")).unwrap(),
            "manual edit\n"
        );
        assert!(super::ensure_clean_source_checkout(&dir, "another-branch")
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
