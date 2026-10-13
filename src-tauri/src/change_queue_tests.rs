//! Native queue tests share disposable Git fixtures, never the live Track.
use super::*;
use crate::change_queue::{self as queue, QueueEntry, QueueState, RequestedBy};

fn entry(change: &Change, by: Option<RequestedBy>, strategy: &str) -> QueueEntry {
    QueueEntry {
        change: change.clone(),
        strategy: strategy.into(),
        requested_by: by,
    }
}
fn pending(track: &SessionView, changes: &[Change]) -> QueueState {
    let mut queue = QueueState::empty(track).unwrap();
    queue.pending = changes.iter().map(|c| entry(c, None, "squash")).collect();
    queue
}
fn context_script(s: &SessionView, t: &SessionView) -> Vec<(&'static str, Value)> {
    vec![
        ("/api/sessions/summary/list", fleet(s, t)),
        ("/api/sessions/get", json!(t)),
    ]
}
fn set_reply(script: &mut [(&'static str, Value)], path: &str, reply: Value) {
    let index = script.iter().position(|(p, _)| *p == path).unwrap();
    script[index].1 = reply;
}
fn append_write(script: &mut Vec<(&'static str, Value)>, reply: Value) {
    script.extend([capability_read(), ("/api/artifacts/write", reply)]);
}
fn delivery_retry(script: &mut Vec<(&'static str, Value)>) {
    let write = script
        .iter()
        .position(|(path, _)| *path == "/api/artifacts/write")
        .unwrap();
    assert_eq!(script[write - 1].0, "/api/openapi.json");
    script.drain(write - 1..=write); // No write and no write-boundary GET on a no-op.
}
fn reorder_script(
    s: &SessionView,
    t: &SessionView,
    before: &QueueState,
    order: &[Change],
) -> Vec<(&'static str, Value)> {
    let mut script = vec![capability_read()];
    script.extend(context_script(s, t));
    script.push(("/api/artifacts/get", queue_artifact(t, before, 5)));
    for c in order {
        script.extend([
            ("/api/sessions/get", json!(s)),
            ("/api/channels/messages/list", json!([message(c)])),
        ]);
    }
    script.extend(context_script(s, t));
    for _ in order {
        script.push(("/api/sessions/get", json!(s)));
    }
    script
}

#[tokio::test]
async fn reorder_preserves_protected_active_and_authorization_registers_only_exact_new_outputs() {
    let (_repo, mut s, t, c, _) = fixture();
    let mut other = c.clone();
    other.id = "other".into();
    let mut active = c.clone();
    active.id = "active-review".into();
    s = registered(registered(s, &other), &active);
    let mut before = pending(&t, std::slice::from_ref(&c));
    before.pending[0] = entry(&c, Some(RequestedBy::User), "merge");
    before.active = Some(entry(&active, Some(RequestedBy::Autoland), "squash"));
    let order = vec![other.clone(), c.clone()];
    let mut after = before.clone();
    after.pending = vec![entry(&other, None, "squash"), before.pending[0].clone()];
    let mut script = reorder_script(&s, &t, &before, &order);
    append_write(&mut script, queue_artifact(&t, &after, 6));
    let (client, server) = mock(script).await;
    let view = queue::reorder_with_client(&client, &t.id, 5, &order)
        .await
        .unwrap();
    assert_eq!(view.queue, after);
    assert_eq!(view.revision, 6);
    let calls = server.await.unwrap();
    let write = &calls.last().unwrap().1;
    assert_eq!(write["base_rev"], 5);
    assert_eq!(write["branch"], t.branch.id);
    assert_eq!(write["repo"], false);
    assert_eq!(write["name"], "change-queue.json");
    assert_eq!(
        serde_json::from_str::<QueueState>(write["content"].as_str().unwrap()).unwrap(),
        after
    );
    assert!(calls
        .iter()
        .all(|(path, _)| !path.ends_with("/messages/create") && !path.ends_with("/tags/set")));
}

#[tokio::test]
async fn reorder_rejects_stale_omission_duplicate_active_altered_scope_and_foreign_repository() {
    let (_repo, s, t, c, _) = fixture();
    let mut active = c.clone();
    active.id = "protected".into();
    let mut before = pending(&t, std::slice::from_ref(&c));
    before.active = Some(entry(&active, Some(RequestedBy::User), "merge"));
    let mut altered = c.clone();
    altered.title = "altered same identity".into();
    let mut foreign = c.clone();
    foreign.id = "new".into();
    foreign.repo_root = "/foreign".into();
    for (revision, order) in [
        (4, vec![c.clone()]),
        (5, vec![]),
        (5, vec![c.clone(), c.clone()]),
        (5, vec![c.clone(), active.clone()]),
        (5, vec![altered]),
        (5, vec![c.clone(), foreign]),
    ] {
        let mut script = vec![capability_read()];
        script.extend(context_script(&s, &t));
        script.push(("/api/artifacts/get", queue_artifact(&t, &before, 5)));
        let (client, server) = mock(script).await;
        assert!(queue::reorder_with_client(&client, &t.id, revision, &order)
            .await
            .is_err());
        assert_eq!(server.await.unwrap().len(), 4);
    }
}

#[tokio::test]
async fn reorder_rejects_same_repo_wrong_track_accepted_receipt_and_changed_fresh_declaration() {
    let (_repo, s, t, c, _) = fixture();
    let before = pending(&t, std::slice::from_ref(&c));
    for failure in ["wrong-owner", "receipt", "declaration"] {
        let mut script = reorder_script(&s, &t, &before, std::slice::from_ref(&c));
        if failure == "wrong-owner" {
            let mut f = fleet(&s, &t);
            f[1]["parent_session_id"] = json!("other-track");
            let mut other = f[0].clone();
            other["id"] = json!("other-track");
            other["branch"]["id"] = json!("other-branch");
            other["branch"]["tags"][1]["value"] = json!("arachne/tracks/other");
            f.as_array_mut().unwrap().push(other);
            script[6].1 = f;
            script.truncate(8);
        } else {
            let mut changed = s.clone();
            if failure == "receipt" {
                tagged(
                    &mut changed,
                    "change_acceptance:native",
                    &json!(receipt(&c)),
                );
            } else {
                let mut altered = c.clone();
                altered.title = "changed".into();
                changed.branch.tags.clear();
                changed = registered(changed, &altered);
            }
            script.last_mut().unwrap().1 = json!(changed);
        }
        let (client, server) = mock(script).await;
        assert!(
            queue::reorder_with_client(&client, &t.id, 5, std::slice::from_ref(&c))
                .await
                .is_err(),
            "{failure}"
        );
        assert!(server
            .await
            .unwrap()
            .iter()
            .all(|(path, _)| !path.ends_with("/artifacts/write")));
    }
}

#[tokio::test]
async fn queue_read_missing_is_revision_zero_and_metadata_corruption_fails_closed() {
    use super::routing_regressions::{transition_mock, BackendTransition};
    let (_repo, s, t, c, _) = fixture();
    let before = pending(&t, std::slice::from_ref(&c));
    let mut missing = context_script(&s, &t);
    missing.push((
        "/api/artifacts/get",
        json!({"_http_status":404,"error":"not found"}),
    ));
    let state = std::sync::Arc::new(LoomState::default());
    let (url, server) =
        transition_mock(state.clone(), missing, 0, BackendTransition::HealthyRefresh).await;
    let client = selected_client(&state, &url).await.unwrap();
    let view = queue::change_queue_with_client(&client, &t.id)
        .await
        .unwrap();
    assert_eq!(view.revision, 0);
    assert_eq!(view.queue, QueueState::empty(&t).unwrap());
    server.await.unwrap();
    for failure in [
        "shared",
        "revision",
        "revision-huge",
        "qualified",
        "ask",
        "json",
        "target",
        "duplicate",
        "active-omitted",
        "authorization-omitted",
        "working-active",
        "unauthorized",
        "strategy",
    ] {
        let mut artifact = queue_artifact(&t, &before, 5);
        let mut q = json!(before);
        match failure {
            "shared" => artifact["meta"]["branch_id"] = Value::Null,
            "revision" => artifact["meta"]["rev"] = json!(0),
            "revision-huge" => artifact["meta"]["rev"] = json!(9_007_199_254_740_992i64),
            "qualified" => {
                let id = "loom:http%3A%2F%2Fother%3A7878:worker";
                q["pending"][0]["change"]["source_session"] = json!(id);
                q["pending"][0]["change"]["completion"]["channel_id"] = json!(id);
            }
            "ask" => q["pending"][0]["strategy"] = json!("ask"),
            "json" => artifact["content"] = json!("garbage"),
            "target" => q["target_branch"] = json!("changed"),
            "duplicate" => q["pending"]
                .as_array_mut()
                .unwrap()
                .push(json!(before.pending[0])),
            "active-omitted" => {
                q.as_object_mut().unwrap().remove("active");
            }
            "authorization-omitted" => {
                q["pending"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("requested_by");
            }
            "working-active" => {
                let mut working = c.clone();
                working.completion = None;
                working.scope = ChangeScope::Working {
                    base_oid: c.scope.base_oid().into(),
                    head_oid: c.scope.head_oid().into(),
                    paths: c.scope.paths().to_vec(),
                    worktree_version: "pinned".into(),
                };
                q["active"] = json!(entry(&working, Some(RequestedBy::User), "squash"));
                q["pending"] = json!([]);
            }
            "unauthorized" => q["pending"][0]["requested_by"] = json!("status"),
            "strategy" => q["pending"][0]["strategy"] = json!("push"),
            _ => unreachable!(),
        }
        if !matches!(failure, "shared" | "revision" | "revision-huge" | "json") {
            artifact["content"] = json!(q.to_string());
        }
        let mut script = context_script(&s, &t);
        script.push(("/api/artifacts/get", artifact));
        let (client, server) = mock(script).await;
        assert!(
            queue::change_queue_with_client(&client, &t.id)
                .await
                .is_err(),
            "{failure}"
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn land_promotes_behind_active_and_retry_delivers_without_rewriting_saved_intent() {
    let (_repo, s, t, c, _) = fixture();
    let mut other = c.clone();
    other.id = "other".into();
    let mut active = c.clone();
    active.id = "protected-review".into();
    let mut before = pending(&t, &[other.clone(), c.clone()]);
    before.active = Some(entry(&active, Some(RequestedBy::Autoland), "squash"));
    before.pending[0] = entry(&other, Some(RequestedBy::User), "rebase");
    let mut after = before.clone();
    after.pending = vec![
        entry(&c, Some(RequestedBy::User), "cherry-pick"),
        before.pending[0].clone(),
    ];
    let mut first = land_script(&s, &t, &c);
    set_reply(
        &mut first,
        "/api/artifacts/get",
        queue_artifact(&t, &before, 10),
    );
    set_reply(
        &mut first,
        "/api/artifacts/write",
        queue_artifact(&t, &after, 11),
    );
    first.last_mut().unwrap().1 = json!({"_http_error":"delivery down"});
    let (client, server) = mock(first).await;
    let error = land_change_with_client(&client, &s.id, &c.id, &c, Some("cherry-pick"))
        .await
        .unwrap_err();
    assert!(error.message.contains("remains queued at revision 11"));
    assert!(error.message.contains("Acceptance is not confirmed"));
    let first = server.await.unwrap();
    let persisted: QueueState = serde_json::from_str(
        first
            .iter()
            .find(|(path, _)| path.ends_with("/artifacts/write"))
            .unwrap()
            .1["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(persisted, after);
    let mut retry = land_script(&s, &t, &c);
    set_reply(
        &mut retry,
        "/api/artifacts/get",
        queue_artifact(&t, &after, 11),
    );
    delivery_retry(&mut retry);
    let (client, server) = mock(retry).await;
    land_change_with_client(&client, &s.id, &c.id, &c, Some("cherry-pick"))
        .await
        .unwrap();
    let retry = server.await.unwrap();
    assert!(retry
        .iter()
        .all(|(path, _)| !path.ends_with("/artifacts/write")));
    assert_eq!(
        first.last().unwrap().1["idempotency_key"],
        retry.last().unwrap().1["idempotency_key"]
    );
    assert_eq!(
        first.last().unwrap().1["payload"],
        retry.last().unwrap().1["payload"]
    );
}

#[tokio::test]
async fn cas_conflicts_never_overwrite_order_or_dispatch_land_and_leases_fence_queue_reads_writes()
{
    use super::routing_regressions::{transition_mock, BackendTransition};
    let (_repo, s, t, c, _) = fixture();
    let before = pending(&t, std::slice::from_ref(&c));
    for action in ["land", "reorder"] {
        let mut script = if action == "land" {
            land_script(&s, &t, &c)
        } else {
            reorder_script(&s, &t, &before, std::slice::from_ref(&c))
        };
        if action == "land" {
            let write = script
                .iter()
                .position(|(path, _)| *path == "/api/artifacts/write")
                .unwrap();
            script.truncate(write + 1);
            script.last_mut().unwrap().1 = json!({"_http_status":409,"error":"stale"});
        } else {
            append_write(&mut script, json!({"_http_status":409,"error":"stale"}));
        }
        let state = std::sync::Arc::new(LoomState::default());
        let (url, server) =
            transition_mock(state.clone(), script, 0, BackendTransition::HealthyRefresh).await;
        let client = selected_client(&state, &url).await.unwrap();
        let error = if action == "land" {
            land_change_with_client(&client, &s.id, &c.id, &c, None)
                .await
                .map(|_| ())
        } else {
            queue::reorder_with_client(&client, &t.id, 5, std::slice::from_ref(&c))
                .await
                .map(|_| ())
        }
        .unwrap_err();
        assert!(error.message.contains("revision conflict"));
        let calls = server.await.unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|(path, _)| path.ends_with("/artifacts/write"))
                .count(),
            1
        );
        assert!(calls
            .iter()
            .all(|(path, _)| !path.ends_with("/messages/create")));
    }
    for transition in [
        BackendTransition::Outage,
        BackendTransition::Replacement,
        BackendTransition::Recovery,
    ] {
        for write in [false, true] {
            let mut script = if write {
                reorder_script(&s, &t, &before, std::slice::from_ref(&c))
            } else {
                context_script(&s, &t)
            };
            if write {
                append_write(&mut script, queue_artifact(&t, &before, 6));
            } else {
                script.push(("/api/artifacts/get", queue_artifact(&t, &before, 5)));
            }
            let at = if write { script.len() - 2 } else { 0 };
            let state = std::sync::Arc::new(LoomState::default());
            let (url, server) = transition_mock(state.clone(), script, at, transition).await;
            let client = selected_client(&state, &url).await.unwrap();
            let error = if write {
                queue::reorder_with_client(&client, &t.id, 5, std::slice::from_ref(&c)).await
            } else {
                queue::change_queue_with_client(&client, &t.id).await
            }
            .unwrap_err();
            assert!(error.unreachable);
            assert!(server
                .await
                .unwrap()
                .iter()
                .all(|(path, _)| !path.ends_with("/artifacts/write")));
        }
    }
}

#[test]
fn drain_protocol_is_installed_in_selected_policy_and_special_coordinator_prompts() {
    let (_repo, s, t, c, _) = fixture();
    let request = IntegrationRequest {
        action: "land-change",
        source_session: s.id.clone(),
        source_branch: s.branch.branch.clone(),
        source_work_dir: s.work_dir.clone(),
        repo_root: s.branch.repo_root.clone(),
        target_session: t.id.clone(),
        target_branch: "arachne/tracks/accepted".into(),
        strategy: IntegrationStrategy::Squash,
        requested_by: "user",
    };
    let source_prompt = selected_wrap_up_prompt(&request, &c);
    for rule in [
        "GET /api/openapi.json",
        "x-loom-atomic-base-rev",
        "exact boolean true",
        "LIVE sessions.list(history:true)",
    ] {
        assert!(source_prompt.contains(rule), "{rule}");
    }
    for prompt in [
        selected_acceptance_prompt(&request, &c),
        crate::loom::autoland_policy_prompt(&t, &request.target_branch),
        crate::loom::topic_launch_guidance().root,
    ] {
        assert!(prompt.contains(queue::DRAIN_PROTOCOL));
        for rule in [
            "base_rev",
            "GET /api/openapi.json",
            "x-loom-atomic-base-rev",
            "exact boolean true",
            "LIVE sessions.list(history:true)",
            "ONE exact entry",
            "retains active",
            "complete+final",
            "Never manufacture receipts",
            "No publishing, scheduler or polling",
        ] {
            assert!(prompt.contains(rule), "{rule}");
        }
    }
}

#[tokio::test]
async fn queue_rejects_conflicting_canonical_owners_before_read_or_land_mutation() {
    let (_repo, s, t, c, _) = fixture();
    let mut conflict = fleet(&s, &t);
    let mut other = conflict[0].clone();
    other["id"] = json!("other-track");
    other["branch"]["id"] = json!("other-branch");
    conflict.as_array_mut().unwrap().push(other);
    let (client, server) = mock(vec![
        ("/api/sessions/summary/list", conflict.clone()),
        ("/api/sessions/get", json!(t)),
    ])
    .await;
    assert!(queue::change_queue_with_client(&client, &t.id)
        .await
        .unwrap_err()
        .message
        .contains("Conflicting live Tracks"));
    assert_eq!(server.await.unwrap().len(), 2);
    let mut script = land_script(&s, &t, &c);
    script[8].1 = conflict;
    script.truncate(10);
    let (client, server) = mock(script).await;
    assert!(land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .unwrap_err()
        .message
        .contains("Conflicting live Tracks"));
    assert!(server
        .await
        .unwrap()
        .iter()
        .all(|(path, _)| !path.ends_with("/artifacts/write")));
}

#[tokio::test]
async fn first_land_guards_artifact_creation_at_zero_and_active_retry_does_not_reclaim() {
    use super::routing_regressions::{transition_mock, BackendTransition};
    let (_repo, s, t, c, _) = fixture();
    let mut first = land_script(&s, &t, &c);
    set_reply(
        &mut first,
        "/api/artifacts/get",
        json!({"_http_status":404,"error":"not found"}),
    );
    let mut saved = pending(&t, std::slice::from_ref(&c));
    saved.pending[0].requested_by = Some(RequestedBy::User);
    set_reply(
        &mut first,
        "/api/artifacts/write",
        queue_artifact(&t, &saved, 1),
    );
    let state = std::sync::Arc::new(LoomState::default());
    let (url, server) =
        transition_mock(state.clone(), first, 0, BackendTransition::HealthyRefresh).await;
    let client = selected_client(&state, &url).await.unwrap();
    land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .unwrap();
    let calls = server.await.unwrap();
    assert_eq!(
        calls
            .iter()
            .find(|(path, _)| path.ends_with("/artifacts/write"))
            .unwrap()
            .1["base_rev"],
        0
    );
    let mut claimed = QueueState::empty(&t).unwrap();
    claimed.active = Some(entry(&c, Some(RequestedBy::Autoland), "rebase"));
    let mut retry = land_script(&s, &t, &c);
    set_reply(
        &mut retry,
        "/api/artifacts/get",
        queue_artifact(&t, &claimed, 8),
    );
    delivery_retry(&mut retry);
    let (client, server) = mock(retry).await;
    land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .unwrap();
    assert!(server
        .await
        .unwrap()
        .iter()
        .all(|(path, _)| !path.ends_with("/artifacts/write")));
}

#[tokio::test]
async fn acp_coordinator_wake_is_durable_and_queued_behind_protected_acceptance() {
    use super::routing_regressions::{transition_mock, BackendTransition};
    let (_repo, s, mut t, c, _) = fixture();
    t.protocol = "acp".into();
    let mut active = c.clone();
    active.id = "protected-review".into();
    let mut before = pending(&t, std::slice::from_ref(&c));
    before.active = Some(entry(&active, Some(RequestedBy::Autoland), "squash"));
    let mut after = before.clone();
    after.pending[0].requested_by = Some(RequestedBy::User);
    let mut script = land_script(&s, &t, &c);
    set_reply(
        &mut script,
        "/api/artifacts/get",
        queue_artifact(&t, &before, 6),
    );
    set_reply(
        &mut script,
        "/api/artifacts/write",
        queue_artifact(&t, &after, 7),
    );
    let second_route = script
        .iter()
        .enumerate()
        .filter(|(_, (path, _))| *path == "/api/channels/subscription/set")
        .nth(1)
        .unwrap()
        .0;
    script.remove(second_route); // ACP has no terminal ensure_request_route call.
    script.extend([
        capability_read(),
        (
            "/api/sessions/prompt/create",
            json!({"queued":true,"turn":null}),
        ),
    ]);
    let state = std::sync::Arc::new(LoomState::default());
    let (url, server) =
        transition_mock(state.clone(), script, 0, BackendTransition::HealthyRefresh).await;
    let client = selected_client(&state, &url).await.unwrap();
    land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .unwrap();
    let calls = server.await.unwrap();
    let record = &calls
        .iter()
        .find(|(path, _)| path.ends_with("/messages/create"))
        .unwrap()
        .1;
    assert_eq!(record["kind"], "status");
    assert_eq!(record["channel"], t.id);
    assert_eq!(record["payload"]["expected_change"], json!(c));
    assert!(record["idempotency_key"]
        .as_str()
        .unwrap()
        .starts_with("arachne-request-v1-"));
    let wake = &calls.last().unwrap().1;
    assert_eq!(wake["session"], t.id);
    assert_eq!(wake["send_now"], false);
    assert_eq!(wake["text"], record["body"]);
    assert!(calls.iter().all(|(path, body)| !path.ends_with("/send")
        && !path.ends_with("/interrupt")
        && !path.ends_with("/adopt")
        && !(path.ends_with("/subscription/set") && body["channel"] == s.id)));
    let saved: QueueState = serde_json::from_str(
        calls
            .iter()
            .find(|(path, _)| path.ends_with("/artifacts/write"))
            .unwrap()
            .1["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(saved.active, before.active);
}

#[path = "change_queue_capability_tests.rs"]
mod capability_tests;
