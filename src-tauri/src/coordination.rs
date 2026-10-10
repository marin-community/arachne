//! Bounded review launches over Loom's existing session primitive.
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::commands::{resolve_integration_target, LoomState, UiError};
use crate::loom::{reviewer_launch_guidance, SessionView, SessionsLaunchInput};

/// A review forks the candidate, not the accepted Track, and receives no history.
pub fn review_launch_input(
    source: &SessionView,
    target: Option<&str>,
    brief: &str,
) -> SessionsLaunchInput {
    let destination = target.map(|branch| format!("Canonical accepted Track target: {branch}. Re-read its current revision; compare against this ref, preserving the candidate branch."))
        .unwrap_or_else(|| "No integration target has been selected. Inspect the recorded base and repository policy; report the comparison used.".into());
    SessionsLaunchInput {
        repo: source.github_repo.clone(),
        cwd: source.branch.repo_root.clone(),
        base: Some(source.branch.branch.clone()),
        parent_branch: Some(source.branch.id.clone()),
        title: Some(format!("Review: {}", source.branch.title)),
        goal: Some(format!(
            "Independent review of session {}.\nObjective: {}\nRepository: {}\nCandidate branch: {}\nSource checkout: {}\n{}\n\n{}\n\nYour fresh checkout contains committed candidate changes. Inspect any uncommitted source edits read-only and report exactly what was reviewed. Record source and target revisions and validation evidence. Return actionable findings to the original worker and publish a concise durable result. Do not modify the source checkout, integrate, land, push, or open a PR.",
            source.id, source.branch.goal, source.branch.repo_root, source.branch.branch,
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
    if source.branch.branch.is_empty() || source.branch.repo_root.is_empty() {
        return Err(UiError {
            message: "The candidate needs a repository and branch to review.".into(),
            unreachable: false,
        });
    }
    let fleet = client.list_sessions().await?;
    let target = resolve_integration_target(&fleet, &source_id);
    let mut input = review_launch_input(
        &source,
        target.as_ref().map(|t| t.target_branch.as_str()),
        &brief,
    );
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
            Some("weaver/track"),
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
        let target = resolve_integration_target(&fleet, "source").unwrap();
        let input = review_launch_input(
            &source,
            Some(&target.target_branch),
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
        let input = review_launch_input(&source, None, "Review the retained branch.");
        assert_eq!(input.cwd, source.branch.repo_root);
        assert!(input.repo.is_none());
        assert!(input
            .goal
            .unwrap()
            .contains("No integration target has been selected"));
    }
}
