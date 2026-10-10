//! Durable Track ownership, accepted refs and autoland defaults.
use crate::loom::{BranchSummaryView, SessionSummaryView};

pub const CANONICAL_INITIALIZATION_MARKER: &str =
    "Arachne canonical Track initialization required:";

pub fn tag<'a>(branch: &'a BranchSummaryView, key: &str) -> Option<&'a str> {
    branch
        .tags
        .iter()
        .rev()
        .find(|t| t.key == key)
        .map(|t| t.value.as_str())
}

pub fn is_track(branch: &BranchSummaryView) -> bool {
    tag(branch, "topic") == Some("true")
}

/// A durable launch intent survives failed tag stamping and reserves the ref.
pub fn pending_topic_branch(branch: &BranchSummaryView) -> Option<&str> {
    branch.goal.lines().find_map(|line| {
        line.strip_prefix(CANONICAL_INITIALIZATION_MARKER)
            .map(str::trim)
    })
}

/// A missing tag is the legacy accepted branch. An invalid tag fails closed.
pub fn canonical_topic_branch(branch: &BranchSummaryView) -> Option<&str> {
    if let Some(required) = pending_topic_branch(branch) {
        if tag(branch, "track_topic_initialized") != Some(required)
            || tag(branch, "track_topic_branch") != Some(required)
        {
            return None;
        }
    }
    match tag(branch, "track_topic_branch") {
        Some(name) => valid_topic_branch(name).then_some(name),
        None if pending_topic_branch(branch).is_some() => None,
        None => valid_local_branch(&branch.branch).then_some(branch.branch.as_str()),
    }
}

pub fn effective_autoland(track: &BranchSummaryView, source: &BranchSummaryView) -> bool {
    if track.repo_root != source.repo_root {
        return false;
    }
    match tag(source, "thread_autoland") {
        Some("on") => true,
        Some("off") => false,
        _ => tag(track, "track_autoland") == Some("on"),
    }
}

pub fn immediate_parent<'a>(
    fleet: &'a [SessionSummaryView],
    source: &SessionSummaryView,
) -> Option<&'a SessionSummaryView> {
    source
        .parent_session_id
        .as_deref()
        .and_then(|id| fleet.iter().find(|s| s.id == id))
        .or_else(|| {
            source
                .parent_id
                .as_deref()
                .and_then(|id| fleet.iter().find(|s| s.branch.id == id))
        })
}

/// An unmarked root is a legacy Track unless explicitly standalone. New
/// canonical launches retain an initialization marker until their tags exist.
pub fn owns_track(session: &SessionSummaryView) -> bool {
    is_track(&session.branch)
        || (session.parent_id.is_none()
            && session.parent_session_id.is_none()
            && tag(&session.branch, "topic") != Some("false"))
}

/// Marked same-repository ancestors or legacy roots own acceptance. Never
/// choose an incidental intermediate worker branch or another repository.
pub fn track_ancestor<'a>(
    fleet: &'a [SessionSummaryView],
    source_id: &str,
) -> Option<&'a SessionSummaryView> {
    let source = fleet.iter().find(|s| s.id == source_id)?;
    let mut current = source;
    let mut seen = std::collections::HashSet::new();
    while seen.insert(current.id.as_str()) {
        let parent = immediate_parent(fleet, current)?;
        if seen.contains(parent.id.as_str()) {
            return None;
        }
        if parent.branch.repo_root == source.branch.repo_root
            && parent.status != "archived"
            && owns_track(parent)
        {
            return Some(parent);
        }
        current = parent;
    }
    None
}

/// Git check-ref-format --branch rules plus reserved/pseudo-ref forms.
/// Occupied Loom scratch refs are rejected separately by branch allocation.
pub fn valid_topic_branch(name: &str) -> bool {
    valid_local_branch(name)
}

fn valid_local_branch(name: &str) -> bool {
    !name.is_empty()
        && name != "@"
        && name != "HEAD"
        && !name.starts_with('-')
        && !name.starts_with("refs/")
        && !name.contains("..")
        && !name.contains("@{")
        && !name.contains("//")
        && !name.ends_with('.')
        && !name
            .chars()
            .any(|c| c <= ' ' || c == '\u{7f}' || "~^:?*[\\".contains(c))
        && name
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

pub fn inferred_topic_branch(title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    format!(
        "arachne/tracks/{}",
        if slug.is_empty() { "track" } else { slug }
    )
}

pub fn choose_topic_branch(
    requested: Option<&str>,
    title: &str,
    occupied: &[String],
) -> Result<String, String> {
    if let Some(explicit) = requested.filter(|s| !s.is_empty()) {
        if !valid_topic_branch(explicit) {
            return Err("Invalid or reserved Track topic branch.".into());
        }
        if occupied.iter().any(|b| {
            b == explicit
                || b.starts_with(&format!("{explicit}/"))
                || explicit.starts_with(&format!("{b}/"))
        }) {
            return Err(
                "Track topic branch is already occupied or claimed; choose a fresh branch.".into(),
            );
        }
        return Ok(explicit.into());
    }
    let default = inferred_topic_branch(title);
    for suffix in 1..10_000 {
        let candidate = if suffix == 1 {
            default.clone()
        } else {
            format!("{default}-{suffix}")
        };
        if occupied.iter().all(|b| {
            b != &candidate
                && !b.starts_with(&format!("{candidate}/"))
                && !candidate.starts_with(&format!("{b}/"))
        }) {
            return Ok(candidate);
        }
    }
    Err("Cannot allocate a fresh Track topic branch; choose another name.".into())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Captures the fresh launch HEAD before any substantive work, atomically
/// creates a REAL ref, and leaves primary HEAD and Loom's branch untouched.
pub fn initialization_prompt(branch: &str) -> String {
    let reference = shell_quote(&format!("refs/heads/{branch}"));
    let branch_arg = shell_quote(branch);
    format!("{CANONICAL_INITIALIZATION_MARKER} {branch}\nCanonical Track initialization (required BEFORE substantive work or delegation):\n\nCapture the fresh launch seed with `seed=$(git rev-parse --verify HEAD)` in this new Track checkout before making any commit. Create the real canonical accepted ref {branch:?} at that exact seed using `git update-ref {reference} \"$seed\" \"$(git hash-object --stdin </dev/null | sed 's/./0/g')\"`. This is an exclusive create: if the ref exists or creation fails, STOP and report the collision/failure; never overwrite an existing ref, rename the live Loom branch, checkout another branch, or change primary HEAD. Verify `git rev-parse --verify {reference}` equals the captured seed, then persist `loom sessions tags set track_topic_branch {branch_arg}`, followed by `loom sessions tags set track_topic_initialized {branch_arg}` ONLY after successful exclusive creation and seed verification, BEFORE `loom sessions tags set topic true`. Never stamp track_topic_initialized after a collision or on metadata alone. Reread the LIVE session tags and verify all three exact values; if any metadata write or verification fails, STOP before work/acceptance/publishing and report a recoverable initialization blocker. Preserve the durable canonical initialization marker in the goal; native acceptance/publishing requires its matching track_topic_initialized verification tag, independently of the display/launch tags; never treat missing tags on this new Track as legacy scratch acceptance. Send a durable initialization result with the canonical ref and seed SHA. Metadata alone does not initialize a branch. Do not accept or publish until the real ref is verified. Keep all substantive coordinator work on the original Loom scratch branch; accepted worker results update only this canonical ref.\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn branch() -> BranchSummaryView {
        serde_json::from_value(serde_json::json!({"id":"b", "branch":"weaver/legacy", "name":"b", "title":"b", "repo_root":"/repo"})).unwrap()
    }
    fn tagged(branch: &mut BranchSummaryView, key: &str, value: &str) {
        branch.tags.push(crate::loom::TagView {
            key: key.into(),
            value: value.into(),
            note: String::new(),
            set_at: String::new(),
            set_by: String::new(),
        });
    }
    #[test]
    fn shared_frontend_native_contract_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/track-policy-contract.json"
        ))
        .unwrap();
        for case in fixture["slug"].as_array().unwrap() {
            assert_eq!(
                inferred_topic_branch(case["title"].as_str().unwrap()),
                case["expected"].as_str().unwrap()
            );
        }
        for case in fixture["refs"].as_array().unwrap() {
            assert_eq!(
                valid_topic_branch(case["value"].as_str().unwrap()),
                case["valid"].as_bool().unwrap(),
                "{case}"
            );
        }
        for case in fixture["policy"].as_array().unwrap() {
            let mut track = branch();
            let mut source = branch();
            if let Some(value) = case["track"].as_str() {
                tagged(&mut track, "track_autoland", value);
            }
            if let Some(value) = case["thread"].as_str() {
                tagged(&mut source, "thread_autoland", value);
            }
            assert_eq!(
                effective_autoland(&track, &source),
                case["effective"].as_bool().unwrap(),
                "{case}"
            );
        }
        for invalid in [" topic", "topic ", " ", "\tcustom/topic"] {
            assert!(choose_topic_branch(Some(invalid), "x", &[]).is_err());
        }
    }
    #[test]
    fn initialization_intent_without_tags_never_falls_back_to_legacy_scratch() {
        let mut track = branch();
        assert_eq!(canonical_topic_branch(&track), Some("weaver/legacy"));
        track.goal = format!("Do work\n\n{CANONICAL_INITIALIZATION_MARKER} arachne/tracks/new");
        assert!(canonical_topic_branch(&track).is_none());
        tagged(&mut track, "topic", "true");
        assert!(canonical_topic_branch(&track).is_none());
        tagged(&mut track, "track_topic_branch", "arachne/tracks/new");
        // Display tags stamped by launch cannot authorize a colliding ref.
        assert!(canonical_topic_branch(&track).is_none());
        tagged(&mut track, "track_topic_initialized", "arachne/tracks/other");
        assert!(canonical_topic_branch(&track).is_none());
        tagged(&mut track, "track_topic_initialized", "arachne/tracks/new");
        assert_eq!(canonical_topic_branch(&track), Some("arachne/tracks/new"));
        tagged(&mut track, "track_topic_branch", "arachne/tracks/other");
        assert!(canonical_topic_branch(&track).is_none());
    }
    #[test]
    fn durable_initialization_recipe_creates_real_seed_ref_without_head_or_worktree_mutation() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!(
            "arachne-canonical-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let git = |cwd: &std::path::Path, args: &[&str]| {
            let out = Command::new("git")
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        git(&dir, &["init", "-b", "primary"]);
        git(&dir, &["config", "user.email", "test@example.invalid"]);
        git(&dir, &["config", "user.name", "Test"]);
        git(&dir, &["commit", "--allow-empty", "-m", "seed"]);
        let seed = git(&dir, &["rev-parse", "HEAD"]);
        let scratch = dir.join("scratch");
        git(
            &dir,
            &[
                "worktree",
                "add",
                "-b",
                "weaver/track",
                scratch.to_str().unwrap(),
            ],
        );
        std::fs::write(dir.join("foreign.txt"), "preserve primary dirt").unwrap();
        let prompt = initialization_prompt("arachne/tracks/new");
        let commands: Vec<_> = prompt
            .split('`')
            .enumerate()
            .filter_map(|(i, s)| (i % 2 == 1).then_some(s))
            .collect();
        let capture = commands.iter().find(|s| s.starts_with("seed=")).unwrap();
        let create = commands
            .iter()
            .find(|s| s.starts_with("git update-ref"))
            .unwrap();
        let script = format!("{capture}\n{create}");
        let result = Command::new("sh")
            .args(["-ec", &script])
            .current_dir(&scratch)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(git(&dir, &["rev-parse", "arachne/tracks/new"]), seed);
        assert_eq!(git(&dir, &["symbolic-ref", "HEAD"]), "refs/heads/primary");
        assert_eq!(
            git(&scratch, &["symbolic-ref", "HEAD"]),
            "refs/heads/weaver/track"
        );
        git(
            &scratch,
            &["commit", "--allow-empty", "-m", "substantive scratch work"],
        );
        let scratch_head = git(&scratch, &["rev-parse", "HEAD"]);
        assert_ne!(scratch_head, seed);
        let collision = Command::new("sh")
            .args(["-ec", &script])
            .current_dir(&scratch)
            .output()
            .unwrap();
        assert!(!collision.status.success());
        assert_eq!(git(&dir, &["rev-parse", "arachne/tracks/new"]), seed);
        assert_eq!(git(&scratch, &["rev-parse", "HEAD"]), scratch_head);
        assert_eq!(git(&dir, &["rev-parse", "HEAD"]), seed);
        assert_eq!(
            std::fs::read_to_string(dir.join("foreign.txt")).unwrap(),
            "preserve primary dirt"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn refs_and_duplicate_defaults() {
        for invalid in [
            "HEAD",
            "@",
            "refs/heads/x",
            "-x",
            "x..y",
            "x@{y",
            "x y",
            "x/.y",
            "x.lock",
            "x/",
            "x\\y",
        ] {
            assert!(!valid_topic_branch(invalid), "{invalid}");
        }
        assert!(valid_topic_branch("arachne/tracks/test"));
        assert_eq!(
            inferred_topic_branch(" Fix: API + tags! "),
            "arachne/tracks/fix-api-tags"
        );
        assert_eq!(
            choose_topic_branch(None, "API", &["arachne/tracks/api".into()]).unwrap(),
            "arachne/tracks/api-2"
        );
        assert!(choose_topic_branch(Some("feature"), "", &["feature/child".into()]).is_err());
        assert!(choose_topic_branch(None, "x", &["arachne".into()]).is_err());
    }
    #[test]
    fn policy_is_off_by_default_and_source_override_wins() {
        let mut branch: BranchSummaryView = serde_json::from_value(serde_json::json!({"id":"b","branch":"legacy","name":"b","title":"b","repo_root":"/repo"})).unwrap();
        let source = branch.clone();
        assert!(!effective_autoland(&branch, &source));
        branch.tags.push(crate::loom::TagView {
            key: "track_autoland".into(),
            value: "on".into(),
            note: String::new(),
            set_at: String::new(),
            set_by: String::new(),
        });
        assert!(effective_autoland(&branch, &source));
        let mut source = source;
        source.tags.push(crate::loom::TagView {
            key: "thread_autoland".into(),
            value: "off".into(),
            note: String::new(),
            set_at: String::new(),
            set_by: String::new(),
        });
        assert!(!effective_autoland(&branch, &source));
        source.tags[0].value = "on".into();
        branch.tags[0].value = "off".into();
        assert!(effective_autoland(&branch, &source));
        source.tags[0].value = "invalid".into();
        assert!(!effective_autoland(&branch, &source));
        assert_eq!(canonical_topic_branch(&branch), Some("legacy"));
    }
}
