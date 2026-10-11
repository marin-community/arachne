use super::*;

// Share the existing Change test module's real Git fixtures.
#[derive(Clone, Copy, Debug)]
enum BackendTransition {
    Outage,
    Recovery,
    Replacement,
    HealthyRefresh,
}

async fn transition_mock(
    state: std::sync::Arc<LoomState>,
    script: Vec<(&'static str, Value)>,
    transition_at: usize,
    transition: BackendTransition,
) -> (String, tokio::task::JoinHandle<Vec<(String, Value)>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let entry = state
        .servers
        .install_for_test(std::sync::Arc::new(LoomClient::new(&url, None).unwrap()))
        .await;
    let handle = tokio::spawn(async move {
        let mut calls = Vec::new();
        for (index, (path, reply)) in script.into_iter().enumerate() {
            let (mut socket, _) =
                tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
            let mut bytes = Vec::new();
            let (start, length) = loop {
                let mut chunk = [0; 8192];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(str::to_owned)
                        })
                        .unwrap()
                        .parse::<usize>()
                        .unwrap();
                    break (end + 4, length);
                }
            };
            while bytes.len() < start + length {
                let mut chunk = [0; 8192];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            assert!(String::from_utf8_lossy(&bytes[..start]).starts_with(&format!("POST {path} ")));
            let body: Value = serde_json::from_slice(&bytes[start..start + length]).unwrap();
            let reply = if reply.get("_message_receipt").is_some() {
                json!({"id":"durable-message", "channel_id":body["channel"], "kind":"message", "body":body["body"].as_str().unwrap().trim(), "payload":body["payload"]})
            } else {
                reply
            };
            calls.push((path.into(), body));
            if index == transition_at {
                match transition {
                    BackendTransition::Outage => entry.publish_fleet(false, || {}),
                    BackendTransition::Recovery => {
                        entry.publish_fleet(false, || {});
                        entry.publish_fleet(true, || {});
                    }
                    BackendTransition::Replacement => {
                        state
                            .servers
                            .install_for_test(std::sync::Arc::new(
                                LoomClient::new(&entry.client.server_url(), None).unwrap(),
                            ))
                            .await;
                    }
                    BackendTransition::HealthyRefresh => entry.publish_fleet(true, || {}),
                }
            }
            let status = reply
                .get("_http_status")
                .and_then(Value::as_u64)
                .unwrap_or(200);
            let response = reply.to_string();
            // Cancellation can close a read before its response. That is the
            // intended behavior; writes already dispatched require reconciliation.
            let _ = socket.write_all(format!("HTTP/1.1 {status} Reply\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await;
            if index == transition_at && !matches!(transition, BackendTransition::HealthyRefresh) {
                assert!(
                    tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
                        .await
                        .is_err(),
                    "stale selected action sent another request after {transition:?}"
                );
                return calls;
            }
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "selected action sent an unexpected request beyond its validated script"
        );
        calls
    });
    (url, handle)
}

fn prepare_script(s: &SessionView, c: &Change) -> Vec<(&'static str, Value)> {
    let mut script = preview_script(s, c);
    script.extend([
        ("/api/sessions/get", json!(s)), // explicit preparation runtime check
        ("/api/sessions/get", json!(s)), // exact selection immediately before input
        ("/api/sessions/send", json!({})),
    ]);
    script
}

#[tokio::test]
async fn selected_actions_cancel_outage_recovery_and_retirement_during_preflight() {
    use crate::coordination::{review_session_on_server, ReviewSettings};
    for transition in [
        BackendTransition::Outage,
        BackendTransition::Recovery,
        BackendTransition::Replacement,
    ] {
        for action in ["preview", "review", "prepare", "land"] {
            for late in [false, true] {
                let (_repo, source, track, change, _) = fixture();
                let state = std::sync::Arc::new(LoomState::default());
                let script = match action {
                    "preview" => preview_script(&source, &change),
                    "review" => review_script(&source, &track, &change),
                    "prepare" => prepare_script(&source, &change),
                    _ => land_script(&source, &track, &change),
                };
                let transition_at = if !late {
                    0
                } else {
                    match action {
                        "preview" => script.len() - 1,
                        "review" => {
                            script
                                .iter()
                                .position(|(path, _)| *path == "/api/sessions/launch")
                                .unwrap()
                                - 1
                        }
                        "prepare" => script.len() - 2,
                        _ => script.len() - 3,
                    }
                };
                let (url, server) =
                    transition_mock(state.clone(), script, transition_at, transition).await;
                let result = match action {
                    "preview" => {
                        change_changes_on_server(&state, &url, &source.id, &change.id, &change)
                            .await
                            .map(|_| ())
                    }
                    "review" => review_session_on_server(
                        &state,
                        &url,
                        &source.id,
                        "Selected review",
                        (Some(&change.id), Some(&change)),
                        ReviewSettings::default(),
                    )
                    .await
                    .map(|_| ()),
                    "prepare" => {
                        selected_preparation_on_server(
                            &state,
                            &url,
                            &source.id,
                            &change.id,
                            &change,
                            "Prepare selected output",
                        )
                        .await
                    }
                    _ => land_change_on_server(&state, &url, &source.id, &change.id, &change, None)
                        .await
                        .map(|_| ()),
                };
                let error = result.expect_err("obsolete selected preflight must fail closed");
                assert!(
                    error.unreachable,
                    "{action}/{transition:?}/{late}: {}",
                    error.message
                );
                assert_eq!(error.server_url.as_deref(), Some(url.as_str()));
                let calls = server.await.unwrap();
                assert_eq!(calls.len(), transition_at + 1);
                assert!(
                    calls
                        .iter()
                        .all(|(path, _)| !path.ends_with("/messages/create")
                            && !path.ends_with("/launch")
                            && !path.ends_with("/prompt/create")
                            && !path.ends_with("/adopt")
                            && !path.ends_with("/send")),
                    "no task or acceptance may dispatch after a stale preflight"
                );
            }
        }
    }
}

#[tokio::test]
async fn healthy_publication_preserves_selected_action_and_colliding_preparations_stay_owned() {
    let state = std::sync::Arc::new(LoomState::default());
    let (_repo_a, source_a, _, change_a, _) = fixture();
    let (_repo_b, source_b, _, change_b, _) = fixture();
    assert_eq!(source_a.id, source_b.id);
    assert_eq!(change_a.id, change_b.id);
    let mut backends = Vec::new();
    for (source, change) in [(&source_a, &change_a), (&source_b, &change_b)] {
        let (url, server) = transition_mock(
            state.clone(),
            prepare_script(source, change),
            0,
            BackendTransition::HealthyRefresh,
        )
        .await;
        backends.push((url, server));
    }
    for ((url, server), (source, change)) in backends
        .into_iter()
        .zip([(&source_a, &change_a), (&source_b, &change_b)])
    {
        selected_preparation_on_server(
            &state,
            &url,
            &source.id,
            &change.id,
            change,
            "Bounded Prepare",
        )
        .await
        .unwrap();
        let calls = server.await.unwrap();
        assert_eq!(calls.last().unwrap().0, "/api/sessions/send");
        assert_eq!(calls.last().unwrap().1["session"], source.id);
        assert!(calls.last().unwrap().1["text"]
            .as_str()
            .unwrap()
            .contains(&serde_json::to_string(change).unwrap()));
        assert!(calls
            .iter()
            .filter(|(path, _)| path.ends_with("/messages/list"))
            .all(|(_, body)| body["channel"] == source.id));
    }
}

#[tokio::test]
async fn source_delivery_outage_stops_working_land_before_coordinator_delivery() {
    for transition in [BackendTransition::Outage, BackendTransition::Replacement] {
        let (_repo, mut source, track, mut change, _) = fixture();
        change.completion = None;
        change.scope = ChangeScope::Working {
            base_oid: change.scope.base_oid().into(),
            head_oid: change.scope.head_oid().into(),
            paths: change.scope.paths().to_vec(),
            worktree_version: "pinned-version".into(),
        };
        source.branch.tags.clear();
        source = registered(source, &change);
        let snapshot =
            json!({"base":{}, "head_oid":change.scope.head_oid(), "version":"pinned-version"});
        let script = vec![
            ("/api/sessions/get", json!(source)),
            ("/api/sessions/summary/list", fleet(&source, &track)),
            ("/api/sessions/get", json!(track)),
            ("/api/sessions/changes", snapshot.clone()),
            ("/api/channels/subscription/set", sub("track")),
            ("/api/channels/subscription/set", sub("worker")),
            ("/api/channels/subscription/set", sub("track")),
            ("/api/sessions/get", json!(source)),
            ("/api/sessions/changes", snapshot),
            ("/api/sessions/summary/list", fleet(&source, &track)),
            ("/api/sessions/get", json!(track)),
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
        ];
        for during_source_delivery in [false, true] {
            let state = std::sync::Arc::new(LoomState::default());
            let transition_at = if during_source_delivery { 12 } else { 10 };
            let (url, server) =
                transition_mock(state.clone(), script.clone(), transition_at, transition).await;
            let error = land_change_on_server(&state, &url, &source.id, &change.id, &change, None)
                .await
                .err()
                .unwrap();
            let calls = server.await.unwrap();
            assert!(calls
                .iter()
                .filter(|(path, _)| path.ends_with("/messages/create"))
                .all(|(_, body)| body["channel"] == source.id));
            if during_source_delivery {
                assert!(
                    error.message.contains("uncertain"),
                    "a possibly delivered wrap-up is never retried as a fresh authorization"
                );
            } else {
                assert!(calls
                    .iter()
                    .all(|(path, _)| !path.ends_with("/messages/create")));
            }
        }
    }
}

#[tokio::test]
async fn cancellation_during_reviewer_launch_or_selected_input_reports_uncertain_delivery() {
    use crate::coordination::{review_session_on_server, ReviewSettings};
    for transition in [BackendTransition::Outage, BackendTransition::Replacement] {
        for review in [false, true] {
            let (_repo, source, track, change, _) = fixture();
            let state = std::sync::Arc::new(LoomState::default());
            let script = if review {
                review_script(&source, &track, &change)
            } else {
                prepare_script(&source, &change)
            };
            let transition_at = if review {
                script
                    .iter()
                    .position(|(path, _)| *path == "/api/sessions/launch")
                    .unwrap()
            } else {
                script.len() - 1
            };
            let (url, server) =
                transition_mock(state.clone(), script, transition_at, transition).await;
            let result = if review {
                review_session_on_server(
                    &state,
                    &url,
                    &source.id,
                    "Bounded review",
                    (Some(&change.id), Some(&change)),
                    ReviewSettings::default(),
                )
                .await
                .map(|_| ())
            } else {
                selected_preparation_on_server(
                    &state,
                    &url,
                    &source.id,
                    &change.id,
                    &change,
                    "Bounded Prepare",
                )
                .await
            };
            let error = result.err().unwrap();
            assert!(error.message.contains("uncertain"));
            assert!(error.message.contains("Reconcile"));
            assert_eq!(server.await.unwrap().len(), transition_at + 1);
        }
    }
}

// R3: a fresh runtime view must be authorized before selected adoption, and
// adoption's returned/contested view must remain authorized before input.
fn stale_preparation_view(original: &SessionView, change: &Change, mutation: &str) -> SessionView {
    let mut changed = original.clone();
    match mutation {
        "identity" => {
            changed.branch.branch = "weaver/different-objective".into();
            changed.work_dir = format!("{}/different-checkout", original.branch.repo_root);
            git(
                Path::new(&original.branch.repo_root),
                &["branch", &changed.branch.branch],
            );
            git(
                Path::new(&original.branch.repo_root),
                &[
                    "worktree",
                    "add",
                    "-q",
                    &changed.work_dir,
                    &changed.branch.branch,
                ],
            );
        }
        "scope" => {
            let mut other = change.clone();
            if let ChangeScope::Range { paths, .. } = &mut other.scope {
                *paths = vec!["sibling.txt".into()];
            }
            changed.branch.tags.clear();
            changed = registered(changed, &other);
        }
        "missing" => changed.branch.tags.clear(),
        "accepted" | "receipt" => {
            let mut accepted = receipt(change);
            if mutation == "receipt" {
                if let ChangeScope::Range { head_oid, .. } = &mut accepted.scope {
                    *head_oid = "d".repeat(40);
                }
            }
            tagged(
                &mut changed,
                &format!("change_acceptance:{}", change.id),
                &json!(accepted),
            );
        }
        _ => panic!("unknown stale view"),
    }
    changed
}

#[tokio::test]
async fn selected_prepare_rejects_stale_runtime_and_adoption_views_before_lifecycle_or_input() {
    for stage in ["runtime", "adopted", "pre-input", "contested"] {
        for mutation in ["identity", "scope", "missing", "accepted", "receipt"] {
            let (_repo, original, _, change, _) = fixture();
            let mut orphan = original.clone();
            orphan.status = "orphaned".into();
            let mut running = original.clone();
            running.status = "running".into();
            let mut stale = stale_preparation_view(&original, &change, mutation);
            stale.status = if stage == "runtime" {
                "orphaned"
            } else {
                "running"
            }
            .into();
            let mut script = preview_script(&original, &change);
            script.push((
                "/api/sessions/get",
                json!(if stage == "runtime" { &stale } else { &orphan }),
            ));
            if stage != "runtime" {
                script.push((
                    "/api/sessions/adopt",
                    if stage == "contested" {
                        json!({"_http_status":409,"error":"another client adopted"})
                    } else {
                        json!(if stage == "adopted" { &stale } else { &running })
                    },
                ));
                if stage == "contested" || stage == "pre-input" {
                    script.push(("/api/sessions/get", json!(stale)));
                }
            }
            let state = std::sync::Arc::new(LoomState::default());
            let (url, server) =
                transition_mock(state.clone(), script, 0, BackendTransition::HealthyRefresh).await;
            let error = selected_preparation_on_server(
                &state,
                &url,
                &original.id,
                &change.id,
                &change,
                "Prepare only original output",
            )
            .await
            .err()
            .unwrap();
            assert!(
                !error.unreachable,
                "{stage}/{mutation}: metadata must reject before another request: {}",
                error.message
            );
            assert_eq!(error.server_url.as_deref(), Some(url.as_str()));
            let calls = server.await.unwrap();
            assert_eq!(
                calls
                    .iter()
                    .filter(|(path, _)| path.ends_with("/adopt"))
                    .count(),
                usize::from(stage != "runtime")
            );
            assert!(calls.iter().all(|(path, _)| !path.ends_with("/send")
                && !path.ends_with("/prompt/create")
                && !path.ends_with("/messages/create")));
        }
    }
}

#[tokio::test]
async fn selected_prepare_validates_successful_and_contested_original_adoption_before_input() {
    for contested in [false, true] {
        let (_repo, original, _, change, _) = fixture();
        let mut orphan = original.clone();
        orphan.status = "orphaned".into();
        let mut running = original.clone();
        running.status = "running".into();
        let mut script = preview_script(&original, &change);
        script.push(("/api/sessions/get", json!(orphan)));
        script.push((
            "/api/sessions/adopt",
            if contested {
                json!({"_http_status":409,"error":"another client adopted"})
            } else {
                json!(running)
            },
        ));
        if contested {
            script.push(("/api/sessions/get", json!(running)));
        }
        script.extend([
            ("/api/sessions/get", json!(running)),
            ("/api/sessions/send", json!({})),
        ]);
        let state = std::sync::Arc::new(LoomState::default());
        let (url, server) =
            transition_mock(state.clone(), script, 0, BackendTransition::HealthyRefresh).await;
        selected_preparation_on_server(
            &state,
            &url,
            &original.id,
            &change.id,
            &change,
            "Prepare bounded original",
        )
        .await
        .unwrap();
        let calls = server.await.unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|(path, _)| path.ends_with("/adopt"))
                .count(),
            1
        );
        assert_eq!(calls.last().unwrap().0, "/api/sessions/send");
        assert_eq!(calls.last().unwrap().1["session"], original.id);
        assert!(calls.last().unwrap().1["text"]
            .as_str()
            .unwrap()
            .contains(&serde_json::to_string(&change).unwrap()));
    }
}

fn working_preparation_fixture() -> (Repository, SessionView, Change, Value) {
    let (repo, mut source, _, mut change, _) = fixture();
    change.completion = None;
    change.scope = ChangeScope::Working {
        base_oid: change.scope.base_oid().into(),
        head_oid: change.scope.head_oid().into(),
        paths: change.scope.paths().to_vec(),
        worktree_version: "original-snapshot".into(),
    };
    source.branch.tags.clear();
    source = registered(source, &change);
    let snapshot =
        json!({"base":{}, "head_oid":change.scope.head_oid(), "version":"original-snapshot"});
    (repo, source, change, snapshot)
}

fn working_preview_script(source: &SessionView, snapshot: &Value) -> Vec<(&'static str, Value)> {
    vec![
        ("/api/sessions/get", json!(source)),
        ("/api/sessions/changes", snapshot.clone()),
        ("/api/sessions/get", json!(source)),
        ("/api/sessions/changes", snapshot.clone()),
    ]
}

#[tokio::test]
async fn selected_prepare_rechecks_working_snapshot_before_and_after_adoption() {
    for stage in [0, 1, 2] {
        for field in ["version", "head_oid"] {
            let (_repo, original, change, snapshot) = working_preparation_fixture();
            let mut orphan = original.clone();
            orphan.status = "orphaned".into();
            let mut running = original.clone();
            running.status = "running".into();
            let mut stale = snapshot.clone();
            stale[field] = json!(if field == "version" {
                "changed-snapshot".into()
            } else {
                "d".repeat(40)
            });
            let mut script = working_preview_script(&original, &snapshot);
            script.extend([
                ("/api/sessions/get", json!(orphan)),
                (
                    "/api/sessions/changes",
                    if stage == 0 {
                        stale.clone()
                    } else {
                        snapshot.clone()
                    },
                ),
            ]);
            if stage > 0 {
                script.extend([
                    ("/api/sessions/adopt", json!(running)),
                    (
                        "/api/sessions/changes",
                        if stage == 1 {
                            stale.clone()
                        } else {
                            snapshot.clone()
                        },
                    ),
                ]);
                if stage == 2 {
                    script.extend([
                        ("/api/sessions/get", json!(running)),
                        ("/api/sessions/changes", stale),
                    ]);
                }
            }
            let state = std::sync::Arc::new(LoomState::default());
            let (url, server) =
                transition_mock(state.clone(), script, 0, BackendTransition::HealthyRefresh).await;
            let error = selected_preparation_on_server(
                &state,
                &url,
                &original.id,
                &change.id,
                &change,
                "Prepare original working output",
            )
            .await
            .err()
            .unwrap();
            assert!(
                error.message.contains("HEAD or worktree version changed"),
                "{stage}/{field}: {}",
                error.message
            );
            let calls = server.await.unwrap();
            assert_eq!(
                calls
                    .iter()
                    .filter(|(path, _)| path.ends_with("/adopt"))
                    .count(),
                usize::from(stage != 0)
            );
            assert!(calls
                .iter()
                .all(|(path, _)| !path.ends_with("/send") && !path.ends_with("/prompt/create")));
        }
    }
}

#[tokio::test]
async fn selected_prepare_adopts_only_exact_working_snapshot_and_preserves_local_wire_declaration()
{
    let (_repo, original, change, snapshot) = working_preparation_fixture();
    let mut orphan = original.clone();
    orphan.status = "orphaned".into();
    let mut running = original.clone();
    running.status = "running".into();
    let mut script = working_preview_script(&original, &snapshot);
    script.extend([
        ("/api/sessions/get", json!(orphan)),
        ("/api/sessions/changes", snapshot.clone()),
        ("/api/sessions/adopt", json!(running)),
        ("/api/sessions/changes", snapshot.clone()),
        ("/api/sessions/get", json!(running)),
        ("/api/sessions/changes", snapshot),
        ("/api/sessions/send", json!({})),
    ]);
    let state = std::sync::Arc::new(LoomState::default());
    let (url, server) =
        transition_mock(state.clone(), script, 0, BackendTransition::HealthyRefresh).await;
    selected_preparation_on_server(
        &state,
        &url,
        &original.id,
        &change.id,
        &change,
        "Prepare exact working selection",
    )
    .await
    .unwrap();
    let calls = server.await.unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|(path, _)| path.ends_with("/adopt"))
            .count(),
        1
    );
    assert!(calls.last().unwrap().1["text"]
        .as_str()
        .unwrap()
        .contains(&serde_json::to_string(&change).unwrap()));
    assert!(
        calls
            .iter()
            .all(|(path, _)| !path.ends_with("/messages/list")),
        "working identity is never treated as completion"
    );
}
