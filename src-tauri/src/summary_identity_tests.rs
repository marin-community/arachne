use super::*;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn fixture() -> Value {
    serde_json::from_str(include_str!("../../tests/fixtures/summary-identity.json")).unwrap()
}

#[derive(Default)]
struct Requests {
    ids: Mutex<Vec<String>>,
    active: AtomicUsize,
    max_active: AtomicUsize,
    get_started: tokio::sync::Notify,
}

// Uses a real HTTP transport and checks credentials, operation bodies, and
// unqualified IDs. A held GET exercises cancellation and timeout boundaries.
async fn mock(
    summaries: Value,
    full: Value,
    hold: bool,
) -> (LoomClient, Arc<Requests>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let client = LoomClient::new(&url, Some("identity-test-token".into())).unwrap();
    let requests = Arc::new(Requests::default());
    let observed = requests.clone();
    let task = tokio::spawn(async move {
        let mut handlers = tokio::task::JoinSet::new();
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let summaries = summaries.clone();
            let full = full.clone();
            let requests = observed.clone();
            handlers.spawn(async move {
                let mut bytes = Vec::new();
                let (start, length) = loop {
                    let mut chunk = [0; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                        assert!(header.contains("authorization: bearer identity-test-token"));
                        let length: usize = header.lines().find_map(|l| l.strip_prefix("content-length: ")).unwrap().parse().unwrap();
                        break (end + 4, length);
                    }
                };
                while bytes.len() < start + length {
                    let mut chunk = [0; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                }
                let path = String::from_utf8_lossy(&bytes[..start]).split_whitespace().nth(1).unwrap().to_owned();
                let body: Value = serde_json::from_slice(&bytes[start..start + length]).unwrap();
                let (status, reply) = match path.as_str() {
                    "/api/sessions/summary/list" => {
                        assert_eq!(body, json!({"archived": true}));
                        ("200 OK", summaries)
                    }
                    "/api/sessions/get" => {
                        let id = body["session"].as_str().unwrap().to_owned();
                        assert_eq!(body, json!({"session": id}));
                        requests.ids.lock().unwrap().push(id.clone());
                        let active = requests.active.fetch_add(1, Ordering::SeqCst) + 1;
                        requests.max_active.fetch_max(active, Ordering::SeqCst);
                        requests.get_started.notify_one();
                        if hold { std::future::pending::<()>().await; }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        requests.active.fetch_sub(1, Ordering::SeqCst);
                        if id == "failed" { ("403 Forbidden", json!({"error": "denied"})) }
                        else { ("200 OK", full[&id].clone()) }
                    }
                    _ => panic!("unexpected operation {path}"),
                };
                let body = reply.to_string();
                let reply = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                socket.write_all(reply.as_bytes()).await.unwrap();
            });
            // Surface assertions from completed handlers rather than hiding
            // task failures in an otherwise healthy mock response.
            while let Some(result) = handlers.try_join_next() {
                result.unwrap();
            }
        }
    });
    (client, requests, task)
}

fn assert_queue(
    client: &LoomClient,
    sessions: &[crate::loom::SessionSummaryView],
    expected: &Value,
) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("node")
        .current_dir(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap(),
        )
        .arg("tests/fixtures/summary-identity-queue.mjs")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Node 22+ for native-to-frontend queue regression");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            json!({"sessions": sessions, "expected": expected, "server": client.server_url()})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn live_summary_resolves_actual_identity_and_drives_the_scoped_queue() {
    let data = fixture();
    let (client, requests, server) =
        mock(data["summaries"].clone(), data["full"].clone(), false).await;
    let sessions = client.list_sessions().await.unwrap();
    assert_queue(&client, &sessions, &data["expected"]);
    let ids = requests.ids.lock().unwrap().clone();
    assert_eq!(ids.len(), data["full"].as_object().unwrap().len());
    assert!(!ids
        .iter()
        .any(|id| ["track", "zero", "modern"].contains(&id.as_str())));
    assert_eq!(
        sessions
            .iter()
            .find(|s| s.id == "waiting")
            .unwrap()
            .work_dir,
        "/repo/.worktrees/waiting"
    );
    server.abort();
}

#[tokio::test]
async fn failed_decode_and_missing_path_remain_unavailable() {
    let data = fixture();
    for reply in [Value::Null, json!({"id": "waiting"}), {
        let mut full = data["full"]["waiting"].clone();
        full.as_object_mut().unwrap().remove("work_dir");
        full
    }] {
        let summaries = json!([data["summaries"][0], data["summaries"][1]]);
        let (client, _, server) = mock(summaries, json!({"waiting": reply}), false).await;
        let sessions = client.list_sessions().await.unwrap();
        assert_queue(&client, &sessions, &json!({"waiting": "unavailable"}));
        server.abort();
    }
}

#[tokio::test]
async fn output_lookups_deduplicate_and_bound_concurrency() {
    let data = fixture();
    let mut summaries = Vec::new();
    let mut full = serde_json::Map::new();
    for i in 0..24 {
        let id = format!("output-{i}");
        let mut summary = data["summaries"][1].clone();
        summary["id"] = id.clone().into();
        let mut view = data["full"]["waiting"].clone();
        view["id"] = id.clone().into();
        full.insert(id, view);
        summaries.extend([summary.clone(), summary]);
    }
    let (client, requests, server) = mock(json!(summaries), full.into(), false).await;
    assert_eq!(client.list_sessions().await.unwrap().len(), 48);
    assert_eq!(requests.ids.lock().unwrap().len(), 24);
    assert!(requests.max_active.load(Ordering::SeqCst) <= 8);
    assert!(requests.max_active.load(Ordering::SeqCst) > 1);
    server.abort();
}

#[tokio::test]
async fn missing_path_without_declared_outputs_never_fetches_full_sessions() {
    let data = fixture();
    let summaries = json!([
        data["summaries"][0],
        data["summaries"].as_array().unwrap().last().unwrap()
    ]);
    let (client, requests, server) = mock(summaries, json!({}), false).await;
    assert_eq!(client.list_sessions().await.unwrap().len(), 2);
    assert!(requests.ids.lock().unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn identity_get_respects_selected_action_cancellation_and_timeout() {
    let data = fixture();
    for cancel in [true, false] {
        let (client, requests, server) =
            mock(json!([data["summaries"][1]]), data["full"].clone(), true).await;
        let token = tokio_util::sync::CancellationToken::new();
        let client = client.for_selected_action(token.clone());
        let lookup = tokio::spawn(async move { client.list_sessions().await });
        requests.get_started.notified().await;
        if cancel {
            token.cancel();
        }
        let result = tokio::time::timeout(Duration::from_secs(if cancel { 1 } else { 6 }), lookup)
            .await
            .unwrap()
            .unwrap();
        if cancel {
            assert!(result.is_err());
        } else {
            let sessions = result.unwrap();
            assert!(matches!(
                sessions[0].change_identity_problem,
                Some(crate::loom::ChangeIdentityProblem::Unavailable)
            ));
            assert!(sessions[0].work_dir.is_empty());
        }
        server.abort();
    }
}

#[tokio::test]
async fn identity_get_cannot_survive_owning_backend_outage_or_client_replacement() {
    let data = fixture();
    for replace in [false, true] {
        let (client, requests, server) =
            mock(json!([data["summaries"][1]]), data["full"].clone(), true).await;
        let registry = crate::routing::ServerRegistry::default();
        let entry = registry.install_for_test(Arc::new(client)).await;
        let ticket = entry.fleet_ticket().unwrap();
        let selected = entry
            .client
            .for_selected_action(entry.selected_action_token().unwrap());
        let lookup = tokio::spawn(async move { selected.list_sessions().await });
        requests.get_started.notified().await;
        if replace {
            let replacement =
                LoomClient::new(&entry.client.server_url(), Some("replacement-token".into()))
                    .unwrap();
            registry.install_for_test(Arc::new(replacement)).await;
        } else {
            entry.publish_fleet(false, || {});
            entry.publish_fleet(true, || {});
        }
        assert!(tokio::time::timeout(Duration::from_secs(1), lookup)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert!(!entry.publish_read(ticket, true, || panic!("stale hydration must not publish")));
        assert_eq!(requests.ids.lock().unwrap().as_slice(), ["waiting"]);
        server.abort();
    }
}

#[tokio::test]
async fn unavailable_authenticated_lookup_cannot_hide_known_duplicate_path_conflicts() {
    let data = fixture();
    let missing = data["summaries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "failed")
        .unwrap()
        .clone();
    let mut correct = missing.clone();
    correct["work_dir"] = "/repo/.worktrees/failed".into();
    let mut replacement = missing.clone();
    replacement["work_dir"] = "/repo/.worktrees/replacement".into();
    let rows = [missing, correct, replacement];
    for order in [
        vec![0, 2],
        vec![2, 0],
        vec![0, 1, 2],
        vec![0, 2, 1],
        vec![1, 0, 2],
        vec![1, 2, 0],
        vec![2, 0, 1],
        vec![2, 1, 0],
    ] {
        let mut summaries = vec![data["summaries"][0].clone()];
        summaries.extend(order.into_iter().map(|i| rows[i].clone()));
        let (client, requests, server) = mock(json!(summaries), data["full"].clone(), false).await;
        let sessions = client.list_sessions().await.unwrap();
        assert_queue(&client, &sessions, &json!({"failed": "blocked"}));
        assert_eq!(requests.ids.lock().unwrap().as_slice(), ["failed"]);
        server.abort();
    }
}
