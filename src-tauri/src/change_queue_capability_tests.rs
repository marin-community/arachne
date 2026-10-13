//! Transactional-CAS prerequisite over real HTTP, never production Loom/Git.
use super::super::routing_regressions::{
    transition_mock, transition_mock_with_token, BackendTransition,
};
use super::*;

fn not_supported() -> Value {
    json!({"paths":{"/api/artifacts/write":{"post":{"x-loom-atomic-base-rev":false}}}})
}
fn readonly_land_prefix(
    s: &SessionView,
    t: &SessionView,
    c: &Change,
) -> Vec<(&'static str, Value)> {
    if c.scope.is_range() {
        land_script(s, t, c).into_iter().take(4).collect()
    } else {
        vec![
            ("/api/sessions/get", json!(s)),
            ("/api/sessions/summary/list", fleet(s, t)),
            ("/api/sessions/get", json!(t)),
            ("/api/sessions/changes", snapshot(c)),
        ]
    }
}
fn snapshot(c: &Change) -> Value {
    json!({"base":{},"head_oid":c.scope.head_oid(),"version":c.scope.working_version()})
}
fn working(mut s: SessionView, mut c: Change) -> (SessionView, Change) {
    c.completion = None;
    c.scope = ChangeScope::Working {
        base_oid: c.scope.base_oid().into(),
        head_oid: c.scope.head_oid().into(),
        paths: c.scope.paths().to_vec(),
        worktree_version: "atomic-capability-pin".into(),
    };
    s.branch.tags.clear();
    s = registered(s, &c);
    (s, c)
}
fn working_script(s: &SessionView, t: &SessionView, c: &Change) -> Vec<(&'static str, Value)> {
    let mut script = readonly_land_prefix(s, t, c);
    script.extend([
        ("/api/channels/subscription/set", sub("track")),
        ("/api/channels/subscription/set", sub("worker")),
        ("/api/channels/subscription/set", sub("track")),
        ("/api/sessions/get", json!(s)),
        ("/api/sessions/changes", snapshot(c)),
        ("/api/sessions/summary/list", fleet(s, t)),
        ("/api/sessions/get", json!(t)),
    ]);
    script.extend(queue_save_script(t, c));
    script.extend([
        ("/api/channels/subscription/set", sub("worker")),
        (
            "/api/channels/messages/create",
            json!({"_message_receipt":true}),
        ),
        ("/api/channels/subscription/set", sub("track")),
        (
            "/api/channels/messages/create",
            json!({"_message_receipt":true}),
        ),
    ]);
    advertise_land_capability(&mut script, true);
    script
}
fn is_mutation(path: &str) -> bool {
    matches!(
        path,
        "/api/artifacts/write"
            | "/api/sessions/adopt"
            | "/api/sessions/send"
            | "/api/sessions/prompt/create"
            | "/api/channels/messages/create"
            | "/api/channels/subscription/set"
            | "/api/sessions/tags/set"
    )
}

#[tokio::test]
async fn capability_requires_authenticated_get_and_exact_boolean_true_on_exact_post_path() {
    for (name, mut response, supported) in [
        ("true", atomic_support(), true),
        ("absent", json!({"info":{"version":"999"}}), false),
        ("false", not_supported(), false),
        (
            "string",
            json!({"paths":{"/api/artifacts/write":{"post":{"x-loom-atomic-base-rev":"true"}}}}),
            false,
        ),
        (
            "wrong-path",
            json!({"paths":{"/api/artifacts/get":{"post":{"x-loom-atomic-base-rev":true}}}}),
            false,
        ),
        (
            "wrong-method",
            json!({"paths":{"/api/artifacts/write":{"get":{"x-loom-atomic-base-rev":true}}}}),
            false,
        ),
        (
            "wrong-case",
            json!({"paths":{"/api/artifacts/write":{"post":{"X-Loom-Atomic-Base-Rev":true}}}}),
            false,
        ),
        ("root-marker", json!({"x-loom-atomic-base-rev":true}), false),
        ("malformed-paths", json!({"paths":[true]}), false),
        ("malformed-json", json!({"_raw_ack":"{not-json"}), false),
        (
            "unauthenticated",
            json!({"_http_status":401,"error":"credential rejected"}),
            false,
        ),
        (
            "forbidden",
            json!({"_http_status":403,"error":"forbidden"}),
            false,
        ),
    ] {
        response["_expected_auth"] = json!("owning-capability-token");
        let state = std::sync::Arc::new(LoomState::default());
        let (url, server) = transition_mock_with_token(
            state.clone(),
            vec![("/api/openapi.json", response)],
            0,
            BackendTransition::HealthyRefresh,
            Some("owning-capability-token".into()),
        )
        .await;
        let client = selected_client(&state, &url).await.unwrap();
        let result = queue::require_atomic_cas(&client).await;
        assert_eq!(result.is_ok(), supported, "{name}");
        if let Err(error) = result {
            assert!(
                error.message.contains("backend update")
                    || error.message.contains("updated Loom backend"),
                "{name}: {}",
                error.message
            );
        }
        let calls = server.await.unwrap();
        assert_eq!(calls, vec![("/api/openapi.json".into(), json!({}))]);
    }
}

#[tokio::test]
async fn unsupported_range_working_and_reorder_have_zero_route_source_artifact_or_wake_mutation() {
    for action in ["range", "working", "reorder"] {
        for response in [
            json!({}),
            not_supported(),
            json!({"_raw_ack":"not json"}),
            json!({"paths":{"/api/artifacts/get":{"post":{"x-loom-atomic-base-rev":true}}}}),
        ] {
            let (repo, mut s, t, mut c, _) = fixture();
            if action == "working" {
                (s, c) = working(s, c);
            }
            let status = git(&repo.0, &["status", "--porcelain"]);
            let head = git(&repo.0, &["rev-parse", "HEAD"]);
            let mut script = if action == "reorder" {
                vec![]
            } else {
                readonly_land_prefix(&s, &t, &c)
            };
            script.push(("/api/openapi.json", response));
            let expected = script.len();
            let state = std::sync::Arc::new(LoomState::default());
            let (url, server) =
                transition_mock(state.clone(), script, 0, BackendTransition::HealthyRefresh).await;
            let client = selected_client(&state, &url).await.unwrap();
            let error = if action == "reorder" {
                queue::reorder_with_client(&client, &t.id, 0, std::slice::from_ref(&c))
                    .await
                    .map(|_| ())
            } else {
                land_change_with_client(&client, &s.id, &c.id, &c, None)
                    .await
                    .map(|_| ())
            }
            .unwrap_err();
            assert!(error.message.contains("Loom backend"), "{}", error.message);
            let calls = server.await.unwrap();
            assert_eq!(calls.len(), expected);
            assert!(calls.iter().all(|(path, _)| !is_mutation(path)));
            assert_eq!(git(&repo.0, &["status", "--porcelain"]), status);
            assert_eq!(git(&repo.0, &["rev-parse", "HEAD"]), head);
        }
    }
}

#[tokio::test]
async fn capability_get_is_fenced_by_outage_recovery_replacement_and_explicit_cancellation() {
    for transition in [
        BackendTransition::Outage,
        BackendTransition::Recovery,
        BackendTransition::Replacement,
    ] {
        for action in ["range", "working", "reorder"] {
            let (_repo, mut s, t, mut c, _) = fixture();
            if action == "working" {
                (s, c) = working(s, c);
            }
            let mut script = if action == "reorder" {
                vec![]
            } else {
                readonly_land_prefix(&s, &t, &c)
            };
            script.push(capability_read());
            let at = script.len() - 1;
            let state = std::sync::Arc::new(LoomState::default());
            let (url, server) = transition_mock(state.clone(), script, at, transition).await;
            let client = selected_client(&state, &url).await.unwrap();
            let error = if action == "reorder" {
                queue::reorder_with_client(&client, &t.id, 0, std::slice::from_ref(&c))
                    .await
                    .map(|_| ())
            } else {
                land_change_with_client(&client, &s.id, &c.id, &c, None)
                    .await
                    .map(|_| ())
            }
            .unwrap_err();
            assert!(
                error.unreachable,
                "{transition:?}/{action}: {}",
                error.message
            );
            assert!(server
                .await
                .unwrap()
                .iter()
                .all(|(path, _)| !is_mutation(path)));
        }
    }
    let (client, server) = mock(vec![]).await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let selected = client.for_selected_action(cancel.clone());
    cancel.cancel();
    assert!(
        queue::require_atomic_cas(&selected)
            .await
            .unwrap_err()
            .unreachable
    );
    assert!(server.await.unwrap().is_empty());
}

#[tokio::test]
async fn fresh_capability_is_required_again_at_artifact_write_and_after_saved_promotion() {
    for action in ["range", "working", "reorder"] {
        let (_repo, mut s, t, mut c, _) = fixture();
        if action == "working" {
            (s, c) = working(s, c);
        }
        let mut script = match action {
            "working" => working_script(&s, &t, &c),
            "reorder" => {
                let before = pending(&t, std::slice::from_ref(&c));
                let mut script = reorder_script(&s, &t, &before, std::slice::from_ref(&c));
                append_write(&mut script, queue_artifact(&t, &before, 6));
                script
            }
            _ => land_script(&s, &t, &c),
        };
        let write = script
            .iter()
            .position(|(path, _)| *path == "/api/artifacts/write")
            .unwrap();
        assert_eq!(script[write - 1].0, "/api/openapi.json");
        script[write - 1].1 = not_supported();
        script.truncate(write);
        let (client, server) = mock(script).await;
        let error = if action == "reorder" {
            queue::reorder_with_client(&client, &t.id, 5, std::slice::from_ref(&c))
                .await
                .map(|_| ())
        } else {
            land_change_with_client(&client, &s.id, &c.id, &c, None)
                .await
                .map(|_| ())
        }
        .unwrap_err();
        assert!(error.message.contains("backend update"));
        assert!(server.await.unwrap().iter().all(|(path, _)| !matches!(
            path.as_str(),
            "/api/artifacts/write"
                | "/api/sessions/adopt"
                | "/api/sessions/prompt/create"
                | "/api/channels/messages/create"
        )));
    }
    for is_working in [false, true] {
        let (_repo, mut s, t, mut c, _) = fixture();
        let mut script = if is_working {
            (s, c) = working(s, c);
            working_script(&s, &t, &c)
        } else {
            land_script(&s, &t, &c)
        };
        let write = script
            .iter()
            .position(|(path, _)| *path == "/api/artifacts/write")
            .unwrap();
        assert_eq!(script[write + 1].0, "/api/openapi.json");
        script[write + 1].1 = not_supported();
        script.truncate(write + 2);
        let (client, server) = mock(script).await;
        let error = land_change_with_client(&client, &s.id, &c.id, &c, None)
            .await
            .unwrap_err();
        assert!(error.message.contains("remains queued at revision 2"));
        assert!(error.message.contains("backend update"));
        assert!(error.message.contains("Acceptance is not confirmed"));
        let calls = server.await.unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|(path, _)| *path == "/api/artifacts/write")
                .count(),
            1
        );
        assert!(calls.iter().all(|(path, _)| !matches!(
            path.as_str(),
            "/api/sessions/adopt"
                | "/api/sessions/send"
                | "/api/sessions/prompt/create"
                | "/api/channels/messages/create"
        )));
    }
}

#[tokio::test]
async fn saved_intent_retry_after_reconnect_or_replacement_never_reuses_capability_or_old_auth() {
    for replacement in [false, true] {
        let (_repo, s, t, c, _) = fixture();
        let mut script = land_script(&s, &t, &c);
        let first_len = script.len();
        script.extend(readonly_land_prefix(&s, &t, &c));
        let mut unsupported = not_supported();
        unsupported["_expected_auth"] = json!(if replacement {
            "replacement-token"
        } else {
            "owning-token"
        });
        script.push(("/api/openapi.json", unsupported));
        let state = std::sync::Arc::new(LoomState::default());
        let (url, server) = transition_mock_with_token(
            state.clone(),
            script,
            0,
            BackendTransition::HealthyRefresh,
            Some("owning-token".into()),
        )
        .await;
        let first = selected_client(&state, &url).await.unwrap();
        land_change_with_client(&first, &s.id, &c.id, &c, None)
            .await
            .unwrap();
        if replacement {
            state
                .servers
                .install_for_test(std::sync::Arc::new(
                    LoomClient::new(&url, Some("replacement-token".into())).unwrap(),
                ))
                .await;
        } else {
            let entry = state.servers.resolve(&url).await.unwrap();
            entry.publish_fleet(false, || {});
            entry.publish_fleet(true, || {});
        }
        assert!(
            queue::require_atomic_cas(&first)
                .await
                .unwrap_err()
                .unreachable
        );
        let current = selected_client(&state, &url).await.unwrap();
        let error = land_change_with_client(&current, &s.id, &c.id, &c, None)
            .await
            .unwrap_err();
        assert!(error.message.contains("backend update"));
        let calls = server.await.unwrap();
        assert!(calls[first_len..]
            .iter()
            .all(|(path, _)| !is_mutation(path)));
        assert_eq!(calls[first_len..].last().unwrap().0, "/api/openapi.json");
    }
}

#[tokio::test]
async fn unreachable_capability_is_an_update_prerequisite_not_source_completion() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let client = LoomClient::new(&url, None).unwrap();
    let error = queue::reorder_with_client(&client, "no-read-or-write", 0, &[])
        .await
        .unwrap_err();
    assert!(error.unreachable);
    assert!(error.message.contains("atomic artifact base_rev support"));
    assert!(!error.message.contains("completion"));
}

#[tokio::test]
async fn capability_loss_after_durable_acp_instruction_blocks_the_noninterrupting_wake() {
    let (_repo, s, mut t, c, _) = fixture();
    t.protocol = "acp".into();
    let mut script = land_script(&s, &t, &c);
    let second_route = script
        .iter()
        .enumerate()
        .filter(|(_, (path, _))| *path == "/api/channels/subscription/set")
        .nth(1)
        .unwrap()
        .0;
    script.remove(second_route);
    script.push(("/api/openapi.json", not_supported()));
    let state = std::sync::Arc::new(LoomState::default());
    let (url, server) =
        transition_mock(state.clone(), script, 0, BackendTransition::HealthyRefresh).await;
    let client = selected_client(&state, &url).await.unwrap();
    let error = land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .unwrap_err();
    assert!(error.message.contains("remains queued at revision 2"));
    assert!(error.message.contains("Coordinator ACP wake capability"));
    assert!(error.message.contains("backend update"));
    let calls = server.await.unwrap();
    let records: Vec<_> = calls
        .iter()
        .filter(|(path, _)| *path == "/api/channels/messages/create")
        .collect();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].1["kind"], "status");
    assert_eq!(records[0].1["channel"], t.id);
    assert!(calls.iter().all(|(path, _)| !matches!(
        path.as_str(),
        "/api/sessions/prompt/create" | "/api/sessions/adopt" | "/api/sessions/send"
    )));
}

#[path = "change_queue_dispatch_tests.rs"]
mod dispatch_boundaries;
