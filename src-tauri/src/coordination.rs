//! Bounded review launches over Loom's existing session primitive.
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::client::LoomClient;
use crate::commands::{LoomState, UiError};
use crate::loom::{reviewer_launch_guidance, SessionView, SessionsLaunchInput};

/// Exact Git scope shared by previews and independent reviewer launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewComparison {
    pub source_ref: String,
    pub target_ref: String,
    pub accepted_source: bool,
}

/// Use the same recorded publishing base policy as Track Publish. A canonical
/// ref can have its own metadata; otherwise retain the coordinator's seed base.
fn publishing_base(
    source: &SessionView,
    canonical: &str,
    branches: &[serde_json::Value],
) -> Option<String> {
    [canonical, source.branch.branch.as_str()]
        .into_iter()
        .find_map(|name| {
            branches
                .iter()
                .find(|row| {
                    row["branch"].as_str() == Some(name)
                        && row["repo_root"].as_str() == Some(source.branch.repo_root.as_str())
                })
                .and_then(|row| row["base_branch"].as_str())
                .filter(|base| !base.trim().is_empty())
                .map(String::from)
        })
}

pub async fn resolve_review_comparison(
    client: &LoomClient,
    source: &SessionView,
) -> Result<ReviewComparison, UiError> {
    let fleet = client.list_sessions().await?;
    let summary = fleet.iter().find(|s| s.id == source.id).ok_or_else(|| {
        review_error("The source is no longer in the fleet; refresh before reviewing.")
    })?;
    // The later fleet snapshot owns canonical tags and parentage. Original
    // worker/repository identity may not silently change between reads.
    if summary.branch.repo_root != source.branch.repo_root
        || summary.branch.branch != source.branch.branch
    {
        return Err(review_error(
            "The source repository or branch changed; refresh before reviewing.",
        ));
    }
    let accepted_source = crate::track_policy::owns_track(summary);
    let (source_ref, target_ref) = if accepted_source {
        let canonical =
            crate::track_policy::canonical_topic_branch(&summary.branch).ok_or_else(|| {
                review_error("Canonical Track initialization is incomplete or invalid.")
            })?;
        let branches = client.list_branches().await?;
        let target = publishing_base(source, canonical, &branches)
            .ok_or_else(|| review_error("No publishing base is recorded for this Track; choose a publishing destination before reviewing."))?;
        (canonical.to_string(), target)
    } else {
        let owner = crate::track_policy::track_ancestor(&fleet, &source.id).ok_or_else(|| {
            review_error("No same-repository owning Track is available for review.")
        })?;
        let track = client.get_session(&owner.id).await?;
        if track.id != owner.id
            || track.branch.repo_root != source.branch.repo_root
            || track.status == "archived"
        {
            return Err(review_error(
                "The owning Track identity or repository changed; refresh before reviewing.",
            ));
        }
        let mut live_owner = owner.clone();
        live_owner.branch = track.branch.clone();
        if !crate::track_policy::owns_track(&live_owner) {
            return Err(review_error(
                "The resolved coordinator no longer owns a Track; refresh before reviewing.",
            ));
        }
        let canonical =
            crate::track_policy::canonical_topic_branch(&track.branch).ok_or_else(|| {
                review_error("Canonical Track initialization is incomplete or invalid.")
            })?;
        (source.branch.branch.clone(), canonical.to_string())
    };
    if !crate::track_policy::valid_topic_branch(&source_ref)
        || !crate::track_policy::valid_topic_branch(&target_ref)
        || source_ref == target_ref
    {
        return Err(review_error(
            "Review requires distinct valid source and comparison branch refs.",
        ));
    }
    Ok(ReviewComparison {
        source_ref,
        target_ref,
        accepted_source,
    })
}

fn review_error(message: &str) -> UiError {
    UiError {
        message: message.into(),
        unreachable: false,
    }
}

/// Fork the original worker or the Track's accepted ref with bounded context.
pub fn review_launch_input(
    source: &SessionView,
    comparison: &ReviewComparison,
    brief: &str,
) -> SessionsLaunchInput {
    let destination = if comparison.accepted_source {
        format!("Canonical accepted Track source: {}. Publishing comparison base: {}. On the repository host, resolve both refs with git rev-parse, inspect git diff <base-SHA> <accepted-SHA> and record the exact SHAs. Review committed accepted state only; coordinator scratch commits and checkout edits are outside this review. Re-read both refs and record their current revisions.", comparison.source_ref, comparison.target_ref)
    } else {
        format!("Canonical accepted Track target: {}. On the repository host, resolve the original worker and canonical target with git rev-parse and inspect git diff <canonical-SHA> <worker-SHA>, plus the worker checkout edits separately. Re-read its current revision; compare the original worker branch against this ref, including inherited unaccepted changes. Inspect any uncommitted source edits read-only and report their coverage separately.", comparison.target_ref)
    };
    SessionsLaunchInput {
        repo: source.github_repo.clone(),
        cwd: source.branch.repo_root.clone(),
        base: Some(comparison.source_ref.clone()),
        parent_branch: Some(source.branch.id.clone()),
        title: Some(format!("Review: {}", source.branch.title)),
        goal: Some(format!(
            "Independent review of session {}.\nObjective: {}\nRepository: {}\nCandidate branch: {}\nOriginal session checkout: {}\n{}\n\n{}\n\nYour fresh checkout contains the committed review source. Report exactly what was reviewed; never claim checkout edits were reviewed merely because a preview showed them. The source and comparison refs named above are the launch scope; any earlier refs in the brief or preview are historical evidence. Re-verify live canonical metadata before comparing. Record source and target revisions and validation evidence. Return actionable findings to the original worker and publish a concise durable result. Do not modify the source checkout, integrate, land, push, or open a PR.",
            source.id, source.branch.goal, source.branch.repo_root, comparison.source_ref,
            source.work_dir, destination, brief.trim(),
        )),
        launch_guidance: Some(reviewer_launch_guidance()),
        ..Default::default()
    }
}

#[derive(Serialize)]
pub struct ReviewLaunch {
    session: SessionView,
    warnings: Vec<String>,
}

#[tauri::command]
pub async fn review_session(
    app: AppHandle,
    state: State<'_, LoomState>,
    source_id: String,
    brief: String,
    profile: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    effort: Option<String>,
) -> Result<ReviewLaunch, UiError> {
    if brief.trim().is_empty() {
        return Err(UiError {
            message: "A bounded review brief is required.".into(),
            unreachable: false,
        });
    }
    let client = state.client.read().await.clone().ok_or_else(|| UiError {
        message: "Connect to Loom to launch a reviewer.".into(),
        unreachable: true,
    })?;
    let source = client.get_session(&source_id).await?;
    if source.id != source_id {
        return Err(review_error(
            "The server returned a different source; refresh before reviewing.",
        ));
    }
    if source.branch.branch.is_empty() || source.branch.repo_root.is_empty() {
        return Err(UiError {
            message: "The candidate needs a repository and branch to review.".into(),
            unreachable: false,
        });
    }
    let comparison = resolve_review_comparison(&client, &source).await?;
    let mut input = review_launch_input(&source, &comparison, &brief);
    input.profile = profile.filter(|v| !v.is_empty());
    input.agent = agent.filter(|v| !v.is_empty());
    input.model = model.filter(|v| !v.is_empty());
    input.effort = effort.filter(|v| !v.is_empty());
    let mut session = client.launch(&input).await?;
    let mut warnings = Vec::new();
    // A tag failure cannot turn a completed launch into a retry/duplicate launch.
    for (key, value) in [("role", "reviewer"), ("review_source", source_id.as_str())] {
        match client
            .set_tag(&session.id, key, value, "Independent candidate review")
            .await
        {
            Ok(()) => session.branch.tags.push(crate::loom::TagView {
                key: key.into(),
                value: value.into(),
                note: "Independent candidate review".into(),
                set_at: String::new(),
                set_by: "arachne".into(),
            }),
            Err(error) => warnings.push(format!(
                "Reviewer launched, but {key} could not be recorded: {error}"
            )),
        }
    }
    let _ = app.emit("loom://review-launched", &session.id);
    Ok(ReviewLaunch { session, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewer_forks_source_with_independent_role_and_bounded_context() {
        let mut source: SessionView =
            serde_json::from_str(include_str!("../tests/fixtures/launch-response.json")).unwrap();
        source.branch.goal = "Fix manual edit preservation".into();
        let input = review_launch_input(
            &source,
            &ReviewComparison {
                source_ref: source.branch.branch.clone(),
                target_ref: "weaver/track".into(),
                accepted_source: false,
            },
            "Check regression coverage. HEAD abc.",
        );
        assert_eq!(input.base.as_deref(), Some("weaver/decode-probe"));
        assert_eq!(input.parent_branch.as_deref(), Some("decode-probe-branch"));
        let guidance = input.launch_guidance.unwrap();
        assert!(guidance.root.contains("independent reviewer"));
        assert_eq!(guidance.child, guidance.root);
        let goal = input.goal.unwrap();
        assert!(goal.contains("Fix manual edit preservation"));
        assert!(goal.contains("weaver/track"));
        assert!(goal.contains("HEAD abc"));
        assert!(goal.contains("uncommitted source edits read-only"));
        assert!(input.scratch.is_empty());
    }

    #[test]
    fn worker_review_compares_canonical_accepted_ref_while_forking_candidate() {
        let mut source: SessionView =
            serde_json::from_str(include_str!("../tests/fixtures/launch-response.json")).unwrap();
        source.id = "source".into();
        let mut track = source.clone();
        track.id = "track".into();
        track.branch.branch = "weaver/coordinator-scratch".into();
        track.branch.tags = serde_json::from_value(serde_json::json!([
            {"key":"topic","value":"true","note":"","set_at":"","set_by":""},
            {"key":"track_topic_branch","value":"arachne/tracks/accepted","note":"","set_at":"","set_by":""}
        ])).unwrap();
        let root = serde_json::to_value(&track).unwrap();
        let mut worker = serde_json::to_value(&source).unwrap();
        worker["parent_session_id"] = serde_json::json!("track");
        let fleet: Vec<crate::loom::SessionSummaryView> =
            serde_json::from_value(serde_json::json!([root, worker])).unwrap();
        let target = crate::commands::resolve_integration_target(&fleet, "source").unwrap();
        let input = review_launch_input(
            &source,
            &ReviewComparison {
                source_ref: source.branch.branch.clone(),
                target_ref: target.target_branch,
                accepted_source: false,
            },
            "Review intended changes.",
        );
        assert_eq!(input.base.as_deref(), Some(source.branch.branch.as_str()));
        let prompt = input.goal.unwrap();
        assert!(prompt.contains("Canonical accepted Track target: arachne/tracks/accepted"));
        assert!(!prompt.contains("weaver/coordinator-scratch"));
    }

    #[test]
    fn review_of_local_or_archived_source_uses_repo_root_and_never_invents_main() {
        let mut source: SessionView =
            serde_json::from_str(include_str!("../tests/fixtures/launch-response.json")).unwrap();
        source.github_repo = None;
        source.status = "archived".into();
        source.work_dir = String::new();
        let input = review_launch_input(
            &source,
            &ReviewComparison {
                source_ref: source.branch.branch.clone(),
                target_ref: "release".into(),
                accepted_source: false,
            },
            "Review the retained branch.",
        );
        assert_eq!(input.cwd, source.branch.repo_root);
        assert!(input.repo.is_none());
        assert!(input
            .goal
            .unwrap()
            .contains("Canonical accepted Track target: release"));
    }
}
