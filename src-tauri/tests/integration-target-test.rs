// Integration-target resolution tests (spec: docs/integration-and-landing.md,
// "Integration target"): the coordinator is the nearest ancestor scope with a
// writable canonical Resource — concretely, the nearest ancestor with the
// durable `topic` marker, else a root with children (de-facto leader), else
// the worker itself. These tests pin that rule against fixture fleets.
use arachne_lib::loom::SessionSummaryView;

fn row(
    id: &str,
    branch_id: &str,
    parent_session: Option<&str>,
    parent_branch: Option<&str>,
    tags: &[(&str, &str)],
) -> SessionSummaryView {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "status": "running",
        "profile": "default",
        "class": "interactive",
        "origin": "agent",
        "created_at": "2026-09-26T00:00:00Z",
        "last_activity_at": "2026-09-26T00:00:00Z",
        "branch": {
            "id": branch_id,
            "branch": format!("weaver/{id}"),
            "name": id,
            "title": id,
            "repo_root": "/r",
            "tags": tags.iter().map(|(k, v)| serde_json::json!({
                "key": k, "note": "", "value": v,
                "set_at": "2026-09-26T00:00:00Z", "set_by": "arachne"
            })).collect::<Vec<_>>()
        },
        "parent_session_id": parent_session,
        "parent_id": parent_branch
    }))
    .expect("fixture row")
}

#[test]
fn worker_finds_marked_topic_ancestor() {
    // topic leader ← worker ← grandchild: the marked leader wins from any depth.
    let fleet = vec![
        row("leader", "b-leader", None, None, &[("topic", "true")]),
        row("worker", "b-worker", Some("leader"), None, &[]),
        row("grandchild", "b-gc", Some("worker"), None, &[]),
    ];
    let t =
        arachne_lib::commands::resolve_integration_target(&fleet, "grandchild").expect("resolve");
    assert_eq!(t.coordinator_id, "leader");
    assert_eq!(t.target_branch, "weaver/leader");
}

#[test]
fn chain_dead_ends_at_root_with_children() {
    // No topic marker anywhere: the root with children is the de-facto leader.
    let fleet = vec![
        row("root", "b-root", None, None, &[]),
        row("worker", "b-worker", Some("root"), None, &[]),
    ];
    let t = arachne_lib::commands::resolve_integration_target(&fleet, "worker").expect("resolve");
    assert_eq!(t.coordinator_id, "root");
}

#[test]
fn topic_leader_has_no_target_above_itself() {
    // A marked leader consolidating its own workers: there is no separate
    // coordinator — a thread cannot integrate into itself (origin/main rule).
    let fleet = vec![
        row("leader", "b-leader", None, None, &[("topic", "true")]),
        row("worker", "b-worker", Some("leader"), None, &[]),
    ];
    assert!(arachne_lib::commands::resolve_integration_target(&fleet, "leader").is_none());
}

#[test]
fn branch_id_parent_link_resolves_too() {
    // Older loom rows carry parent_id (branch id) instead of parent_session_id.
    let fleet = vec![
        row("leader", "b-leader", None, None, &[("topic", "true")]),
        row("worker", "b-worker", None, Some("b-leader"), &[]),
    ];
    let t = arachne_lib::commands::resolve_integration_target(&fleet, "worker").expect("resolve");
    assert_eq!(t.coordinator_id, "leader");
}
