//! Same-endpoint capability rollback at queue lifecycle and dispatch boundaries.
//! All requests use actual TCP, owning auth/lease and disposable Git fixtures.
use super::*;

fn acp_source_script(
    s: &SessionView,
    t: &SessionView,
    c: &Change,
    contested: bool,
) -> Vec<(&'static str, Value)> {
    let mut script = working_script(s, t, c);
    let initial_source_route = script
        .iter()
        .position(|(path, reply)| {
            *path == "/api/channels/subscription/set" && reply["channel_id"] == s.id
        })
        .unwrap();
    script.remove(initial_source_route); // ACP has no terminal route setup.
    let runtime = script
        .iter()
        .position(|(path, reply)| {
            *path == "/api/channels/subscription/set" && reply["channel_id"] == s.id
        })
        .unwrap();
    script[runtime] = ("/api/sessions/get", json!(s));
    let prompt = script
        .iter()
        .position(|(path, _)| *path == "/api/channels/messages/create")
        .unwrap();
    script[prompt] = (
        "/api/sessions/prompt/create",
        json!({"queued":true,"turn":null}),
    );
    if s.status == "orphaned" {
        let mut running = s.clone();
        running.status = "running".into();
        let mut adoption = vec![capability_read()];
        if contested {
            adoption.extend([
                (
                    "/api/sessions/adopt",
                    json!({"_http_status":409,"error":"already adopted"}),
                ),
                ("/api/sessions/get", json!(running)),
            ]);
        } else {
            adoption.push(("/api/sessions/adopt", json!(running)));
        }
        script.splice(runtime + 1..runtime + 1, adoption);
    }
    script
}

/// A response changes the mock's LIVE marker while the selected lease stays
/// healthy. The following GET must read false, not its scripted true/prior GET.
fn lose_support_after(script: &mut Vec<(&'static str, Value)>, read: usize) {
    script[read].1["_atomic_support"] = json!(false);
    assert_eq!(script[read + 1].0, "/api/openapi.json");
    script.truncate(read + 2);
}

async fn assert_saved_but_undelivered(
    repo: &Repository,
    s: &SessionView,
    c: &Change,
    mut script: Vec<(&'static str, Value)>,
    allowed_adoptions: usize,
) {
    let status = git(&repo.0, &["status", "--porcelain"]);
    let head = git(&repo.0, &["rev-parse", "HEAD"]);
    let index = std::fs::read(repo.0.join(".git/index")).unwrap();
    let expected_saved: Value = serde_json::from_str(
        script
            .iter()
            .find(|(path, _)| *path == "/api/artifacts/write")
            .unwrap()
            .1["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    for (_, reply) in &mut script {
        if reply.is_object() {
            reply["_expected_auth"] = json!("owning-dispatch-token");
        }
    }
    let expected_len = script.len();
    let state = std::sync::Arc::new(LoomState::default());
    let (url, server) = transition_mock_with_token(
        state.clone(),
        script,
        0,
        BackendTransition::HealthyRefresh,
        Some("owning-dispatch-token".into()),
    )
    .await;
    let client = selected_client(&state, &url).await.unwrap();
    let error = land_change_with_client(&client, &s.id, &c.id, c, None)
        .await
        .unwrap_err();
    assert!(
        error.message.contains("remains queued at revision 2"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("backend update"),
        "{}",
        error.message
    );
    assert!(error.message.contains("Acceptance is not confirmed"));
    assert!(
        !error.unreachable,
        "same healthy endpoint, no lease cancellation"
    );
    let calls = server.await.unwrap();
    assert_eq!(calls.len(), expected_len);
    let writes: Vec<_> = calls
        .iter()
        .filter(|(path, _)| path == "/api/artifacts/write")
        .collect();
    assert_eq!(
        writes.len(),
        1,
        "saved intent must not be rewritten/cleared"
    );
    assert_eq!(writes[0].1["base_rev"], 1);
    let saved: Value = serde_json::from_str(writes[0].1["content"].as_str().unwrap()).unwrap();
    assert_eq!(saved, expected_saved);
    assert_eq!(saved["pending"][0]["change"], json!(c));
    assert_eq!(saved["pending"][0]["requested_by"], "user");
    assert_eq!(saved["pending"][0]["strategy"], "squash");
    assert_eq!(
        calls
            .iter()
            .filter(|(p, _)| p == "/api/sessions/adopt")
            .count(),
        allowed_adoptions
    );
    assert!(calls.iter().all(|(path, _)| !matches!(
        path.as_str(),
        "/api/sessions/prompt/create" | "/api/sessions/send" | "/api/channels/messages/create"
    )));
    assert_eq!(calls.last().unwrap().0, "/api/openapi.json");
    assert_eq!(git(&repo.0, &["status", "--porcelain"]), status);
    assert_eq!(git(&repo.0, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read(repo.0.join(".git/index")).unwrap(), index);
}

#[tokio::test]
async fn runtime_lookup_capability_loss_blocks_source_adoption_or_prompt() {
    for status in ["orphaned", "running"] {
        let (repo, s, t, c, _) = fixture();
        let (mut s, c) = working(s, c);
        s.protocol = "acp".into();
        s.status = status.into();
        let mut script = acp_source_script(&s, &t, &c, false);
        let runtime = script
            .iter()
            .rposition(|(p, _)| *p == "/api/sessions/get")
            .unwrap();
        lose_support_after(&mut script, runtime);
        assert_saved_but_undelivered(&repo, &s, &c, script, 0).await;
    }
}

#[tokio::test]
async fn successful_adoption_capability_loss_blocks_source_prompt() {
    let (repo, s, t, c, _) = fixture();
    let (mut s, c) = working(s, c);
    s.protocol = "acp".into();
    s.status = "orphaned".into();
    let mut script = acp_source_script(&s, &t, &c, false);
    let adopt = script
        .iter()
        .position(|(p, _)| *p == "/api/sessions/adopt")
        .unwrap();
    lose_support_after(&mut script, adopt);
    assert_saved_but_undelivered(&repo, &s, &c, script, 1).await;
}

#[tokio::test]
async fn contested_adoption_read_capability_loss_blocks_source_prompt() {
    let (repo, s, t, c, _) = fixture();
    let (mut s, c) = working(s, c);
    s.protocol = "acp".into();
    s.status = "orphaned".into();
    let mut script = acp_source_script(&s, &t, &c, true);
    let contested = script
        .iter()
        .rposition(|(p, _)| *p == "/api/sessions/get")
        .unwrap();
    lose_support_after(&mut script, contested);
    assert_saved_but_undelivered(&repo, &s, &c, script, 1).await;
}

#[tokio::test]
async fn final_route_capability_loss_blocks_source_message_and_coordinator_message_or_status() {
    for recipient in ["source-terminal", "coordinator-terminal", "coordinator-acp"] {
        let (repo, mut s, mut t, mut c, _) = fixture();
        let mut script = if recipient == "source-terminal" {
            (s, c) = working(s, c);
            working_script(&s, &t, &c)
        } else {
            if recipient == "coordinator-acp" {
                t.protocol = "acp".into();
            }
            let mut script = land_script(&s, &t, &c);
            if t.protocol == "acp" {
                let route = script
                    .iter()
                    .enumerate()
                    .filter(|(_, (p, _))| *p == "/api/channels/subscription/set")
                    .nth(1)
                    .unwrap()
                    .0;
                script.remove(route);
            }
            script
        };
        let channel = if recipient == "source-terminal" {
            &s.id
        } else {
            &t.id
        };
        let route = script
            .iter()
            .rposition(|(p, reply)| {
                *p == "/api/channels/subscription/set" && reply["channel_id"] == *channel
            })
            .unwrap();
        lose_support_after(&mut script, route);
        assert_saved_but_undelivered(&repo, &s, &c, script, 0).await;
    }
}

#[tokio::test]
async fn supported_successful_and_contested_adoption_queue_only_noninterrupting_wrap_up() {
    for contested in [false, true] {
        let (_repo, s, t, c, _) = fixture();
        let (mut s, c) = working(s, c);
        s.protocol = "acp".into();
        s.status = "orphaned".into();
        let script = acp_source_script(&s, &t, &c, contested);
        let state = std::sync::Arc::new(LoomState::default());
        let (url, server) =
            transition_mock(state.clone(), script, 0, BackendTransition::HealthyRefresh).await;
        let client = selected_client(&state, &url).await.unwrap();
        land_change_with_client(&client, &s.id, &c.id, &c, None)
            .await
            .unwrap();
        let calls = server.await.unwrap();
        for path in [
            "/api/sessions/adopt",
            "/api/sessions/prompt/create",
            "/api/channels/messages/create",
        ] {
            let dispatch = calls.iter().position(|(p, _)| p == path).unwrap();
            assert_eq!(calls[dispatch - 1].0, "/api/openapi.json");
        }
        let prompt = calls
            .iter()
            .find(|(p, _)| p == "/api/sessions/prompt/create")
            .unwrap();
        assert_eq!(prompt.1["session"], s.id);
        assert_eq!(prompt.1["send_now"], false);
        assert!(calls.iter().all(|(p, _)| p != "/api/sessions/send"));
    }
}

#[tokio::test]
async fn ordinary_nongated_delivery_keeps_terminal_and_acp_adoption_semantics() {
    for protocol in ["terminal", "acp"] {
        let mut s = source();
        s.protocol = protocol.into();
        s.status = "orphaned".into();
        let script = if protocol == "terminal" {
            let mut route = sub(&s.id);
            route["_atomic_support"] = json!(false);
            vec![
                ("/api/channels/subscription/set", route),
                (
                    "/api/channels/messages/create",
                    json!({"_message_receipt":true}),
                ),
            ]
        } else {
            let mut runtime = json!(s);
            runtime["_atomic_support"] = json!(false);
            let mut adopted = s.clone();
            adopted.status = "running".into();
            vec![
                ("/api/sessions/get", runtime),
                ("/api/sessions/adopt", json!(adopted)),
                (
                    "/api/sessions/prompt/create",
                    json!({"queued":true,"turn":null}),
                ),
            ]
        };
        let state = std::sync::Arc::new(LoomState::default());
        let (url, server) =
            transition_mock(state.clone(), script, 0, BackendTransition::HealthyRefresh).await;
        let client = selected_client(&state, &url).await.unwrap();
        client
            .deliver_request(&s, "ordinary request", json!({"ordinary":true}))
            .await
            .unwrap();
        let calls = server.await.unwrap();
        assert!(calls.iter().all(|(p, _)| p != "/api/openapi.json"));
    }
}

#[tokio::test]
async fn active_noop_retry_rechecks_after_final_route_without_changing_claim_or_authorization() {
    let (_repo, s, t, c, _) = fixture();
    let mut active = queue::QueueState::empty(&t).unwrap();
    active.active = Some(entry(&c, Some(queue::RequestedBy::Autoland), "merge"));
    let mut script = land_script(&s, &t, &c);
    set_reply(
        &mut script,
        "/api/artifacts/get",
        queue_artifact(&t, &active, 19),
    );
    delivery_retry(&mut script);
    let route = script
        .iter()
        .rposition(|(p, _)| *p == "/api/channels/subscription/set")
        .unwrap();
    lose_support_after(&mut script, route);
    let state = std::sync::Arc::new(LoomState::default());
    let (url, server) =
        transition_mock(state.clone(), script, 0, BackendTransition::HealthyRefresh).await;
    let client = selected_client(&state, &url).await.unwrap();
    let error = land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .unwrap_err();
    assert!(error.message.contains("remains queued at revision 19"));
    assert!(error.message.contains("Coordinator dispatch capability"));
    assert!(error.message.contains("backend update"));
    assert!(server.await.unwrap().iter().all(|(p, _)| !matches!(
        p.as_str(),
        "/api/artifacts/write" | "/api/channels/messages/create" | "/api/sessions/prompt/create"
    )));
}

#[tokio::test]
async fn final_dispatch_capability_reads_retain_original_availability_and_retirement_lease() {
    for boundary in ["source-adopt", "source-prompt", "coordinator-dispatch"] {
        for transition in [
            BackendTransition::Outage,
            BackendTransition::Recovery,
            BackendTransition::Replacement,
        ] {
            let (_repo, mut s, t, mut c, _) = fixture();
            let mut script = if boundary.starts_with("source") {
                (s, c) = working(s, c);
                s.protocol = "acp".into();
                s.status = "orphaned".into();
                acp_source_script(&s, &t, &c, false)
            } else {
                land_script(&s, &t, &c)
            };
            let dispatch = script
                .iter()
                .position(|(p, _)| {
                    *p == match boundary {
                        "source-adopt" => "/api/sessions/adopt",
                        "source-prompt" => "/api/sessions/prompt/create",
                        _ => "/api/channels/messages/create",
                    }
                })
                .unwrap();
            let gate = dispatch - 1;
            assert_eq!(script[gate].0, "/api/openapi.json");
            script.truncate(gate + 1);
            let state = std::sync::Arc::new(LoomState::default());
            let (url, server) = transition_mock(state.clone(), script, gate, transition).await;
            let client = selected_client(&state, &url).await.unwrap();
            let error = land_change_with_client(&client, &s.id, &c.id, &c, None)
                .await
                .unwrap_err();
            assert!(error.unreachable);
            assert!(error.message.contains("remains queued at revision 2"));
            let calls = server.await.unwrap();
            assert_eq!(
                calls
                    .iter()
                    .filter(|(p, _)| p == "/api/artifacts/write")
                    .count(),
                1
            );
            assert_eq!(
                calls
                    .iter()
                    .filter(|(p, _)| p == "/api/sessions/adopt")
                    .count(),
                usize::from(boundary == "source-prompt")
            );
            assert!(calls.iter().all(|(p, _)| !matches!(
                p.as_str(),
                "/api/channels/messages/create"
                    | "/api/sessions/prompt/create"
                    | "/api/sessions/send"
            )));
        }
    }
}
