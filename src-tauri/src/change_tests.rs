use super::*;
use crate::change::*;
use crate::commands::thread_landing_contract_tests::mock;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn source() -> SessionView {
    let mut source: SessionView =
        serde_json::from_str(include_str!("../tests/fixtures/launch-response.json")).unwrap();
    source.id = "worker".into();
    source.branch.id = "worker-branch".into();
    source.branch.branch = "weaver/worker".into();
    source.branch.repo_root = "/repo".into();
    source.work_dir = "/repo/.worktrees/worker".into();
    source.branch.tags.clear();
    source
}
fn change(source: &SessionView) -> Change {
    Change {
        version: 1,
        id: "native".into(),
        title: "Native output".into(),
        source_session: source.id.clone(),
        repo_root: source.branch.repo_root.clone(),
        source_branch: source.branch.branch.clone(),
        source_work_dir: source.work_dir.clone(),
        scope: ChangeScope::Range {
            base_oid: "a".repeat(40),
            head_oid: "b".repeat(40),
            paths: vec!["selected.txt".into()],
        },
        completion: Some(Completion {
            channel_id: source.id.clone(),
            message_id: "final-result".into(),
        }),
    }
}
fn tagged(source: &mut SessionView, key: &str, value: &Value) {
    source.branch.tags.push(crate::loom::TagView {
        key: key.into(),
        value: value.to_string(),
        note: "".into(),
        set_at: "".into(),
        set_by: "".into(),
    });
}
fn registered(mut source: SessionView, change: &Change) -> SessionView {
    tagged(
        &mut source,
        &format!("change:{}", change.id),
        &json!(change),
    );
    source
}
fn message(change: &Change) -> ResultMessage {
    let mut inline = change.clone();
    inline.completion = None;
    ResultMessage {
        id: "final-result".into(),
        channel_id: change.source_session.clone(),
        seq: 1,
        kind: "result".into(),
        author_kind: "session".into(),
        author_id: change.source_session.clone(),
        body: json!({"version":1,"outcome":"complete","intent":"final","changes":[inline]})
            .to_string(),
        payload: json!({}),
    }
}
fn receipt(change: &Change) -> ChangeAcceptance {
    ChangeAcceptance {
        version: 1,
        change_id: change.id.clone(),
        source_session: change.source_session.clone(),
        scope: change.scope.clone(),
        target_session: "track".into(),
        target_branch: "arachne/tracks/accepted".into(),
        target_oid: "c".repeat(40),
        outcome: "accepted".into(),
        completion: change.completion.clone().unwrap(),
    }
}

#[test]
fn declaration_strict_identity_scope_and_null_completion() {
    let s = source();
    let c = change(&s);
    c.verify_source(&s).unwrap();
    for id in ["", "a/b", "a b", "é", &"x".repeat(81)] {
        assert!(!valid_id(id));
    }
    assert!(valid_id("A-z_0.9"));
    for oid in ["abc", &"x".repeat(40), &"a".repeat(41)] {
        assert!(!valid_oid(oid));
    }
    assert!(valid_oid(&"A".repeat(64)));
    for path in [
        "",
        "/absolute",
        "../escape",
        "a/../b",
        "a//b",
        "./a",
        ".git/config",
        "a\\b",
        "a\0b",
    ] {
        let mut c = c.clone();
        c.scope = ChangeScope::Range {
            base_oid: "a".repeat(40),
            head_oid: "b".repeat(40),
            paths: vec![path.into()],
        };
        assert!(c.validate(false).is_err(), "{path}");
    }
    for field in ["id", "scope", "completion", "version"] {
        let mut v = json!(c);
        v.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<Change>(v).is_err(), "{field}");
    }
    let mut extra = json!(c);
    extra["guess"] = json!(true);
    assert!(serde_json::from_value::<Change>(extra).is_err());
    let mut mismatch = s.clone();
    mismatch.work_dir.push_str("-different");
    assert!(c.verify_source(&mismatch).is_err());
    let mut c = c;
    c.completion.as_mut().unwrap().channel_id = "parent".into();
    assert!(c.validate(false).is_err());
}

#[test]
fn registry_exact_duplicates_receipts_and_legacy_history_are_independent() {
    let s = source();
    let c = change(&s);
    let mut s = registered(s, &c);
    tagged(&mut s, "integration_result", &json!("old-accepted-sha"));
    tagged(&mut s, "change:native", &json!(c));
    let r = ChangeRegistry::parse(&s.branch.tags, &s).unwrap();
    r.select("native", &c).unwrap();
    let mut later = c.clone();
    later.id = "later".into();
    later.scope = ChangeScope::Range {
        base_oid: "b".repeat(40),
        head_oid: "d".repeat(40),
        paths: vec![],
    };
    tagged(&mut s, "change:later", &json!(later));
    tagged(&mut s, "change_acceptance:native", &json!(receipt(&c)));
    let r = ChangeRegistry::parse(&s.branch.tags, &s).unwrap();
    assert!(r.select("native", &c).unwrap_err().contains("accepted"));
    r.select("later", &later).unwrap();
    let mut conflicting = c.clone();
    conflicting.title = "Conflicting".into();
    tagged(&mut s, "change:native", &json!(conflicting));
    assert!(ChangeRegistry::parse(&s.branch.tags, &s)
        .unwrap_err()
        .contains("duplicate"));
    let mut s = registered(source(), &c);
    let mut bad_receipt = receipt(&c);
    bad_receipt.completion.message_id = "another".into();
    tagged(&mut s, "change_acceptance:native", &json!(bad_receipt));
    assert!(ChangeRegistry::parse(&s.branch.tags, &s).is_err());
    let mut s = source();
    tagged(&mut s, "integration_result", &json!("historical"));
    assert!(ChangeRegistry::parse(&s.branch.tags, &s)
        .unwrap()
        .changes
        .is_empty());
}

#[test]
fn completion_authenticates_original_and_exact_complete_final_output() {
    let s = source();
    let c = change(&s);
    let m = message(&c);
    verify_completion(&c, &m, &s).unwrap();
    for field in ["id", "channel_id", "kind", "author_kind", "author_id"] {
        let mut v = json!(m);
        v[field] = json!("other");
        let m: ResultMessage = serde_json::from_value(v).unwrap();
        assert!(verify_completion(&c, &m, &s).is_err(), "{field}");
    }
    for (outcome, intent) in [
        ("partial", "final"),
        ("complete", "checkpoint"),
        ("help", "final"),
        ("interrupted", "final"),
        ("failed", "final"),
    ] {
        let mut m = m.clone();
        let mut body: Value = serde_json::from_str(&m.body).unwrap();
        body["outcome"] = json!(outcome);
        body["intent"] = json!(intent);
        m.body = body.to_string();
        assert!(verify_completion(&c, &m, &s).is_err());
    }
    let mut m = m.clone();
    let body: Value = serde_json::from_str(&m.body).unwrap();
    m.body = "Successful final result".into();
    m.payload = body;
    verify_completion(&c, &m, &s).unwrap();
    m.payload["changes"][0]["scope"]["head_oid"] = json!("c".repeat(40));
    assert!(verify_completion(&c, &m, &s).is_err());
}

#[test]
fn inline_before_indexing_duplicates_zero_outputs_and_conflicts() {
    let s = source();
    let c = change(&s);
    let mut m = message(&c);
    let outputs = explicit_result_outputs(&m, &s).unwrap();
    assert_eq!(outputs, vec![c.clone()]);
    assert!(s.branch.tags.is_empty());
    let mut body: Value = serde_json::from_str(&m.body).unwrap();
    let inline = body["changes"][0].clone();
    body["changes"].as_array_mut().unwrap().push(inline);
    m.body = body.to_string();
    assert_eq!(explicit_result_outputs(&m, &s).unwrap().len(), 1);
    body["changes"][1]["title"] = json!("conflict");
    m.body = body.to_string();
    assert!(explicit_result_outputs(&m, &s).is_err());
    body["changes"] = json!([]);
    m.body = body.to_string();
    assert!(explicit_result_outputs(&m, &s).unwrap().is_empty());
    m.body = "Historical completion at an old branch".into();
    assert!(explicit_result_outputs(&m, &s).is_err());
}

struct Repository(PathBuf);
impl Drop for Repository {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().into()
}
fn commit(root: &Path, path: &str, contents: &str) {
    std::fs::write(root.join(path), contents).unwrap();
    git(root, &["add", "--", path]);
    git(root, &["commit", "-qm", path]);
}
fn fixture() -> (Repository, SessionView, SessionView, Change, Value) {
    let root = std::env::temp_dir().join(format!(
        "arachne-selected-native-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "weaver/worker"]);
    git(&root, &["config", "user.email", "test@example.test"]);
    git(&root, &["config", "user.name", "Test"]);
    commit(&root, "selected.txt", "base\n");
    let base = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["branch", "arachne/tracks/accepted"]);
    commit(&root, "selected.txt", "base\nselected\n");
    let head = git(&root, &["rev-parse", "HEAD"]);
    let mut s = source();
    s.protocol = "terminal".into();
    s.work_dir = root.to_str().unwrap().into();
    s.branch.repo_root = s.work_dir.clone();
    let mut c = change(&s);
    c.scope = ChangeScope::Range {
        base_oid: base,
        head_oid: head,
        paths: vec!["selected.txt".into()],
    };
    s = registered(s, &c);
    let mut track = s.clone();
    track.id = "track".into();
    track.branch.id = "track-branch".into();
    track.branch.branch = "weaver/track".into();
    track.branch.tags.clear();
    tagged(&mut track, "topic", &json!("true"));
    track.branch.tags.last_mut().unwrap().value = "true".into();
    tagged(
        &mut track,
        "track_topic_branch",
        &json!("arachne/tracks/accepted"),
    );
    track.branch.tags.last_mut().unwrap().value = "arachne/tracks/accepted".into();
    let fleet = fleet(&s, &track);
    (Repository(root), s, track, c, fleet)
}
fn fleet(s: &SessionView, track: &SessionView) -> Value {
    let mut worker = json!(s);
    worker["parent_session_id"] = json!("track");
    worker["parent_id"] = Value::Null;
    let mut track = json!(track);
    track["parent_session_id"] = Value::Null;
    track["parent_id"] = Value::Null;
    json!([track, worker])
}
fn sub(channel: &str) -> Value {
    json!({"channel_id":channel,"subject_kind":"session","subject_id":channel,"mode":"deliver"})
}
fn land_script(s: &SessionView, track: &SessionView, c: &Change) -> Vec<(&'static str, Value)> {
    vec![
        ("/api/sessions/get", json!(s)),
        ("/api/channels/messages/list", json!([message(c)])),
        ("/api/sessions/summary/list", fleet(s, track)),
        ("/api/sessions/get", json!(track)),
        ("/api/channels/subscription/set", sub("track")),
        ("/api/channels/subscription/set", sub("track")),
        ("/api/sessions/get", json!(s)),
        ("/api/sessions/summary/list", fleet(s, track)),
        ("/api/sessions/get", json!(track)),
        ("/api/channels/subscription/set", sub("track")),
        (
            "/api/channels/messages/create",
            json!({"_message_receipt":true}),
        ),
    ]
}

#[tokio::test]
async fn range_preview_is_immutable_and_excludes_siblings_and_dirty_checkout() {
    let (repo, s, _, c, _) = fixture();
    commit(&repo.0, "sibling.txt", "later sibling\n");
    std::fs::write(repo.0.join("selected.txt"), "foreign checkout\n").unwrap();
    std::fs::write(repo.0.join("foreign.txt"), "foreign\n").unwrap();
    git(&repo.0, &["add", "foreign.txt"]);
    let status = git(&repo.0, &["status", "--porcelain"]);
    let index = std::fs::read(repo.0.join(".git/index")).unwrap();
    let p = local_change_preview(&s, &c).await.unwrap();
    assert_eq!(p.changes.head_oid.as_deref(), Some(c.scope.head_oid()));
    assert_eq!(p.changes.files.len(), 1);
    let rendered = json!(p).to_string();
    assert!(rendered.contains("selected"));
    assert!(!rendered.contains("foreign checkout"));
    assert!(!rendered.contains("sibling.txt"));
    assert_eq!(git(&repo.0, &["status", "--porcelain"]), status);
    assert_eq!(std::fs::read(repo.0.join(".git/index")).unwrap(), index);
    let mut all = c;
    if let ChangeScope::Range { paths, .. } = &mut all.scope {
        paths.clear();
    }
    assert_eq!(
        local_change_preview(&s, &all)
            .await
            .unwrap()
            .changes
            .files
            .len(),
        1
    );
}

#[tokio::test]
async fn working_preview_bounds_paths_and_pins_head_version() {
    let (repo, s, _, mut c, _) = fixture();
    c.completion = None;
    c.scope = ChangeScope::Working {
        base_oid: c.scope.base_oid().into(),
        head_oid: c.scope.head_oid().into(),
        paths: vec!["selected.txt".into(), "new.txt".into()],
        worktree_version: "v1".into(),
    };
    std::fs::write(repo.0.join("selected.txt"), "working change\n").unwrap();
    std::fs::write(repo.0.join("new.txt"), "intended\n").unwrap();
    std::fs::write(repo.0.join("foreign.txt"), "foreign\n").unwrap();
    let p = local_change_preview(&s, &c).await.unwrap();
    assert_eq!(p.changes.files.len(), 2);
    assert!(!json!(p).to_string().contains("foreign.txt"));
    let mut snapshot = ChangeSetView {
        head_oid: Some(c.scope.head_oid().into()),
        version: Some("v1".into()),
        ..Default::default()
    };
    verify_working_snapshot(&c, &snapshot).unwrap();
    snapshot.version = Some("stale".into());
    assert!(verify_working_snapshot(&c, &snapshot).is_err());
    snapshot.version = Some("v1".into());
    snapshot.head_oid = Some("a".repeat(40));
    assert!(verify_working_snapshot(&c, &snapshot).is_err());
    commit(&repo.0, "later.txt", "later\n");
    assert!(local_change_preview(&s, &c).await.is_err());
}

#[tokio::test]
async fn completed_output_from_reviewer_labeled_source_previews_and_lands_exact_selection() {
    let (repo, mut s, track, c, _) = fixture();
    // The output was registered before the worker was relabeled reviewer.
    s.branch.tags.push(crate::loom::TagView {
        key: "role".into(),
        value: "reviewer".into(),
        note: "".into(),
        set_at: "".into(),
        set_by: "".into(),
    });
    let status = git(&repo.0, &["status", "--porcelain"]);
    let head = git(&repo.0, &["rev-parse", "HEAD"]);
    let (client, server) = mock(vec![
        ("/api/sessions/get", json!(s)),
        ("/api/channels/messages/list", json!([message(&c)])),
        ("/api/sessions/get", json!(s)),
    ])
    .await;
    let preview = change_changes_with_client(&client, &s.id, &c.id, &c)
        .await
        .unwrap();
    assert_eq!(
        preview.changes.head_oid.as_deref(),
        Some(c.scope.head_oid())
    );
    assert_eq!(preview.changes.files.len(), 1);
    assert_eq!(server.await.unwrap().len(), 3);

    let (client, server) = mock(land_script(&s, &track, &c)).await;
    land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .unwrap();
    let calls = server.await.unwrap();
    let deliveries: Vec<_> = calls
        .iter()
        .filter(|(path, _)| path.ends_with("/messages/create"))
        .collect();
    assert_eq!(deliveries.len(), 1);
    let delivery = &deliveries[0].1;
    assert_eq!(delivery["channel"], track.id);
    assert_eq!(delivery["payload"]["expected_change"], json!(c));
    assert_eq!(delivery["payload"]["request"]["strategy"], "squash");
    assert_eq!(git(&repo.0, &["status", "--porcelain"]), status);
    assert_eq!(git(&repo.0, &["rev-parse", "HEAD"]), head);
}

#[tokio::test]
async fn zero_output_reviewer_cannot_land_undeclared_or_uncompleted_output() {
    let mut s = source();
    s.branch.tags.push(crate::loom::TagView {
        key: "role".into(),
        value: "reviewer".into(),
        note: "".into(),
        set_at: "".into(),
        set_by: "".into(),
    });
    let c = change(&s);
    let mut verdict = message(&c);
    verdict.body =
        json!({"version":1,"outcome":"complete","intent":"final","changes":[],"verdict":"approve"})
            .to_string();
    assert!(explicit_result_outputs(&verdict, &s).unwrap().is_empty());
    let (client, server) = mock(vec![("/api/sessions/get", json!(s))]).await;
    assert!(land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .is_err());
    assert_eq!(server.await.unwrap().len(), 1);

    // Even an indexed declaration cannot use an ordinary verdict as its completion.
    let s = registered(s, &c);
    let (client, server) = mock(vec![
        ("/api/sessions/get", json!(s)),
        ("/api/channels/messages/list", json!([verdict])),
    ])
    .await;
    assert!(land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .is_err());
    assert_eq!(server.await.unwrap().len(), 2);
}

#[tokio::test]
async fn selected_land_retry_identity_survives_branch_advance_and_pins_whole_output() {
    let (repo, s, track, c, _) = fixture();
    let (client, server) = mock(land_script(&s, &track, &c)).await;
    land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .unwrap();
    let first = server.await.unwrap();
    commit(&repo.0, "sibling.txt", "later\n");
    let (client, server) = mock(land_script(&s, &track, &c)).await;
    land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .unwrap();
    let second = server.await.unwrap();
    let deliveries = |calls: Vec<(String, Value)>| {
        calls
            .into_iter()
            .filter(|(path, _)| path == "/api/channels/messages/create")
            .map(|(_, body)| body)
            .collect::<Vec<_>>()
    };
    let a = deliveries(first);
    let b = deliveries(second);
    assert_eq!(a.len(), 1);
    for (a, b) in a.iter().zip(&b) {
        assert_eq!(a["idempotency_key"], b["idempotency_key"]);
        assert_eq!(a["payload"]["expected_change"], json!(c));
        assert_eq!(a["payload"]["source_request_revision"], c.scope.head_oid());
        assert_eq!(a["payload"]["request"]["strategy"], "squash");
    }
    assert_eq!(a[0]["channel"], "track");
    assert_eq!(
        a[0]["payload"]["expected_change"]["completion"],
        json!(c.completion)
    );
    let prompt = a[0]["body"].as_str().unwrap();
    assert!(prompt.contains("change_acceptance:<id>"));
    assert!(prompt.contains("No siblings"));
    assert!(prompt.contains("non") || prompt.contains("complete"));
    assert!(!prompt.contains("Record integration_result on the ORIGINAL source"));
}

#[tokio::test]
async fn stale_registry_and_missing_completion_fail_before_delivery_or_reviewer_launch() {
    let s = source();
    let c = change(&s);
    let s = registered(s, &c);
    let mut expected = c.clone();
    expected.title = "stale".into();
    let (client, server) = mock(vec![("/api/sessions/get", json!(s))]).await;
    assert!(
        land_change_with_client(&client, &s.id, &c.id, &expected, None)
            .await
            .unwrap_err()
            .message
            .contains("changed")
    );
    assert_eq!(server.await.unwrap().len(), 1);
    let (client, server) = mock(vec![("/api/sessions/get", json!(s))]).await;
    assert!(crate::coordination::review_preflight(
        &client,
        &s,
        "Review",
        Some(&c.id),
        Some(&expected)
    )
    .await
    .unwrap_err()
    .message
    .contains("changed"));
    assert_eq!(server.await.unwrap().len(), 1);
    let (client, server) = mock(vec![
        ("/api/sessions/get", json!(s)),
        ("/api/channels/messages/list", json!([])),
    ])
    .await;
    assert!(selected_source(&client, &s.id, &c.id, &c)
        .await
        .unwrap_err()
        .message
        .contains("bounded"));
    let calls = server.await.unwrap();
    assert_eq!(calls[1].1["peek"], true);
    assert_eq!(calls[1].1["limit"], 500);
}

#[tokio::test]
async fn changed_registry_during_preflight_blocks_and_review_context_is_pinned() {
    let (repo, s, track, c, _) = fixture();
    let mut script = land_script(&s, &track, &c);
    let mut changed = s.clone();
    changed.branch.tags[0].value = "malformed".into();
    script[6].1 = json!(changed);
    script.truncate(7);
    let (client, server) = mock(script).await;
    assert!(land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .is_err());
    assert_eq!(server.await.unwrap().len(), 7);
    let comparison = crate::coordination::ReviewComparison {
        source_ref: s.branch.branch.clone(),
        target_ref: "arachne/tracks/accepted".into(),
        accepted_source: false,
    };
    let input = crate::coordination::selected_review_launch_input(
        &s,
        &comparison,
        "Check selected paths",
        &c,
    );
    assert_eq!(input.base.as_deref(), Some(c.scope.head_oid()));
    let goal = input.goal.unwrap();
    assert!(goal.contains(&serde_json::to_string(&c).unwrap()));
    assert!(!goal.contains("including inherited unaccepted changes"));
    assert!(goal.contains("ONLY"));
    drop(repo);
}

#[test]
fn autoland_requires_inline_explicit_outputs_and_per_output_receipts() {
    let prompt = crate::loom::autoland_policy_prompt(&source(), "arachne/tracks/accepted");
    for text in [
        "actual durable result wakes",
        "changes:[]",
        "zero-output review verdicts",
        "Source role/title/status never disqualifies",
        "complete+final",
        "BEFORE tag indexing",
        "within this same wake",
        "no second wake",
        "conflicting duplicate",
        "never retask historical sources",
        "change_acceptance:<id>",
        "authenticated actor provenance",
        "do not automatically resume",
        "before EVERY",
        "never blanket integration_result",
    ] {
        assert!(prompt.contains(text), "{text}");
    }
}

#[test]
fn working_finalization_is_the_only_permitted_declaration_transition() {
    let s = source();
    let final_change = change(&s);
    let mut working = final_change.clone();
    working.completion = None;
    working.scope = ChangeScope::Working {
        base_oid: final_change.scope.base_oid().into(),
        head_oid: "a".repeat(40),
        paths: final_change.scope.paths().to_vec(),
        worktree_version: "pinned-version".into(),
    };
    working.verify_working_finalization(&final_change).unwrap();
    for field in [
        "id",
        "title",
        "source_session",
        "repo_root",
        "source_branch",
        "source_work_dir",
    ] {
        let mut value = json!(final_change);
        value[field] = json!("changed");
        let changed: Change = serde_json::from_value(value).unwrap();
        assert!(working.verify_working_finalization(&changed).is_err());
    }
    let mut expanded = final_change;
    if let ChangeScope::Range { paths, .. } = &mut expanded.scope {
        paths.push("sibling.txt".into());
    }
    assert!(working.verify_working_finalization(&expanded).is_err());
    let mut invalid = working.clone();
    if let ChangeScope::Working { paths, .. } = &mut invalid.scope {
        paths.clear();
    }
    assert!(invalid.validate(false).is_err());
    let mut invalid = working;
    if let ChangeScope::Working {
        worktree_version, ..
    } = &mut invalid.scope
    {
        worktree_version.clear();
    }
    assert!(invalid.validate(false).is_err());
}

#[tokio::test]
async fn accepted_or_corrupt_receipt_and_changed_source_stop_before_completion_reads() {
    let s = source();
    let c = change(&s);
    let mut accepted = registered(s.clone(), &c);
    tagged(
        &mut accepted,
        "change_acceptance:native",
        &json!(receipt(&c)),
    );
    let (client, server) = mock(vec![("/api/sessions/get", json!(accepted))]).await;
    assert!(selected_source(&client, &s.id, &c.id, &c)
        .await
        .unwrap_err()
        .message
        .contains("accepted"));
    assert_eq!(server.await.unwrap().len(), 1);
    let mut corrupt = registered(s.clone(), &c);
    let mut r = receipt(&c);
    r.target_oid = "not-a-commit".into();
    tagged(&mut corrupt, "change_acceptance:native", &json!(r));
    let (client, server) = mock(vec![("/api/sessions/get", json!(corrupt))]).await;
    assert!(selected_source(&client, &s.id, &c.id, &c)
        .await
        .unwrap_err()
        .message
        .contains("receipt"));
    assert_eq!(server.await.unwrap().len(), 1);
    let mut changed = registered(s.clone(), &c);
    changed.work_dir = "/another/worktree".into();
    let (client, server) = mock(vec![("/api/sessions/get", json!(changed))]).await;
    assert!(selected_source(&client, &s.id, &c.id, &c)
        .await
        .unwrap_err()
        .message
        .contains("identity"));
    assert_eq!(server.await.unwrap().len(), 1);
}

#[tokio::test]
async fn source_reachability_or_branch_switch_invalidates_range_without_writing() {
    let (repo, s, _, c, _) = fixture();
    git(&repo.0, &["checkout", "-qb", "foreign"]);
    let status = git(&repo.0, &["status", "--porcelain"]);
    assert!(local_change_preview(&s, &c)
        .await
        .unwrap_err()
        .message
        .contains("branch changed"));
    assert_eq!(git(&repo.0, &["status", "--porcelain"]), status);
    git(&repo.0, &["checkout", "-q", "weaver/worker"]);
    git(&repo.0, &["reset", "--hard", c.scope.base_oid()]);
    assert!(local_change_preview(&s, &c).await.is_err());
}

#[tokio::test]
async fn changed_live_track_marker_or_canonical_scope_blocks_selected_dispatch() {
    let (_repo, s, track, c, _) = fixture();
    for (key, value) in [
        ("topic", "false"),
        ("track_topic_branch", "arachne/tracks/changed"),
    ] {
        let mut changed = track.clone();
        changed
            .branch
            .tags
            .iter_mut()
            .find(|t| t.key == key)
            .unwrap()
            .value = value.into();
        let mut script = land_script(&s, &track, &c);
        script[8].1 = json!(changed);
        script.truncate(9);
        let (client, server) = mock(script).await;
        assert!(land_change_with_client(&client, &s.id, &c.id, &c, None)
            .await
            .unwrap_err()
            .message
            .contains("ownership"));
        assert_eq!(server.await.unwrap().len(), 9);
    }
}

#[tokio::test]
async fn selected_review_success_pins_declaration_and_preflights_before_launch() {
    let (_repo, s, track, c, _) = fixture();
    let (client, server) = mock(vec![
        ("/api/sessions/get", json!(s)),
        ("/api/channels/messages/list", json!([message(&c)])),
        ("/api/sessions/get", json!(s)),
        ("/api/sessions/summary/list", fleet(&s, &track)),
        ("/api/sessions/get", json!(track)),
        ("/api/sessions/get", json!(s)),
        ("/api/channels/messages/list", json!([message(&c)])),
        ("/api/sessions/get", json!(s)),
    ])
    .await;
    let input = crate::coordination::review_preflight(
        &client,
        &s,
        "Review explicit output",
        Some(&c.id),
        Some(&c),
    )
    .await
    .unwrap();
    let calls = server.await.unwrap();
    assert!(calls.iter().all(|(path, _)| path != "/api/sessions/launch"));
    assert_eq!(input.base.as_deref(), Some(c.scope.head_oid()));
    assert!(input
        .goal
        .unwrap()
        .contains(&serde_json::to_string(&c).unwrap()));
    let (client, server) = mock(vec![]).await;
    assert!(
        crate::coordination::review_preflight(&client, &s, "Review", Some(&c.id), None)
            .await
            .is_err()
    );
    assert!(server.await.unwrap().is_empty());
}

#[tokio::test]
async fn working_land_pins_snapshot_in_both_noninterrupting_deliveries() {
    let (_repo, mut s, track, mut c, _) = fixture();
    c.completion = None;
    c.scope = ChangeScope::Working {
        base_oid: c.scope.base_oid().into(),
        head_oid: c.scope.head_oid().into(),
        paths: c.scope.paths().to_vec(),
        worktree_version: "version-1".into(),
    };
    s.branch.tags.clear();
    s = registered(s, &c);
    let snapshot = json!({"base":{},"head_oid":c.scope.head_oid(),"version":"version-1"});
    let (client, server) = mock(vec![
        ("/api/sessions/get", json!(s)),
        ("/api/sessions/summary/list", fleet(&s, &track)),
        ("/api/sessions/get", json!(track)),
        ("/api/sessions/changes", snapshot.clone()),
        ("/api/channels/subscription/set", sub("track")),
        ("/api/channels/subscription/set", sub("worker")),
        ("/api/channels/subscription/set", sub("track")),
        ("/api/sessions/get", json!(s)),
        ("/api/sessions/changes", snapshot),
        ("/api/sessions/summary/list", fleet(&s, &track)),
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
    ])
    .await;
    land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .unwrap();
    let calls = server.await.unwrap();
    for i in [12, 14] {
        assert_eq!(calls[i].1["payload"]["expected_change"], json!(c));
        assert_eq!(calls[i].1["payload"]["source_request_version"], "version-1");
        assert!(calls[i].1["body"]
            .as_str()
            .unwrap()
            .contains("same Land request"));
    }
    assert!(calls
        .iter()
        .all(|(path, _)| !path.contains("interrupt") && !path.ends_with("/send")));
}

#[tokio::test]
async fn independent_outputs_have_distinct_durable_retry_keys() {
    let (_repo, s, track, c, _) = fixture();
    let (client, server) = mock(land_script(&s, &track, &c)).await;
    land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .unwrap();
    let first = server.await.unwrap();
    let mut sibling = c;
    sibling.id = "another-output".into();
    let mut s = s;
    s.branch.tags.clear();
    s = registered(s, &sibling);
    let (client, server) = mock(land_script(&s, &track, &sibling)).await;
    land_change_with_client(&client, &s.id, &sibling.id, &sibling, None)
        .await
        .unwrap();
    let second = server.await.unwrap();
    for i in [10] {
        assert_ne!(
            first[i].1["idempotency_key"],
            second[i].1["idempotency_key"]
        );
    }
}

#[tokio::test]
async fn completed_ranges_never_adopt_prompt_or_notify_sources() {
    for protocol in ["acp", "terminal"] {
        for status in ["archived", "orphaned", "done", "running"] {
            let (_repo, mut s, track, c, _) = fixture();
            s.protocol = protocol.into();
            s.status = status.into();
            let (client, server) = mock(land_script(&s, &track, &c)).await;
            land_change_with_client(&client, &s.id, &c.id, &c, None)
                .await
                .unwrap();
            let calls = server.await.unwrap();
            assert!(
                calls.iter().all(|(path, body)| {
                    !path.ends_with("/adopt")
                        && !path.ends_with("/changes")
                        && !path.ends_with("/chat")
                        && !path.ends_with("/prompt/create")
                        && !path.ends_with("/send")
                        && !path.ends_with("/interrupt")
                        && (!matches!(
                            path.as_str(),
                            "/api/channels/subscription/set" | "/api/channels/messages/create"
                        ) || (body["channel"] != s.id && body["session"] != s.id))
                }),
                "{protocol}/{status}"
            );
            let messages: Vec<_> = calls
                .iter()
                .filter(|(path, _)| path.ends_with("/messages/create"))
                .collect();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].1["channel"], track.id);
            assert_eq!(
                messages[0].1["payload"]["expected_change"]["completion"],
                json!(c.completion)
            );
        }
    }
}

#[tokio::test]
async fn working_reviewer_preflights_snapshot_without_requiring_final_completion() {
    let (repo, mut s, track, mut c, _) = fixture();
    c.completion = None;
    c.scope = ChangeScope::Working {
        base_oid: c.scope.base_oid().into(),
        head_oid: c.scope.head_oid().into(),
        paths: c.scope.paths().to_vec(),
        worktree_version: "working-v1".into(),
    };
    s.branch.tags.clear();
    s = registered(s, &c);
    std::fs::write(repo.0.join("selected.txt"), "bounded working edits\n").unwrap();
    let status = git(&repo.0, &["status", "--porcelain"]);
    let snapshot = json!({"base":{},"head_oid":c.scope.head_oid(),"version":"working-v1"});
    let (client, server) = mock(vec![
        ("/api/sessions/get", json!(s)),
        ("/api/sessions/changes", snapshot.clone()),
        ("/api/sessions/get", json!(s)),
        ("/api/sessions/changes", snapshot.clone()),
        ("/api/sessions/summary/list", fleet(&s, &track)),
        ("/api/sessions/get", json!(track)),
        ("/api/sessions/get", json!(s)),
        ("/api/sessions/get", json!(s)),
        ("/api/sessions/changes", snapshot),
    ])
    .await;
    let input = crate::coordination::review_preflight(
        &client,
        &s,
        "Review working snapshot",
        Some(&c.id),
        Some(&c),
    )
    .await
    .unwrap();
    let calls = server.await.unwrap();
    assert!(calls
        .iter()
        .all(|(path, _)| path != "/api/channels/messages/list"));
    assert_eq!(input.base.as_deref(), Some(c.scope.head_oid()));
    let goal = input.goal.unwrap();
    assert!(goal.contains("completion is null, unavailable and unverified"));
    assert!(goal.contains("do not wait for or require a final result"));
    assert!(!goal.contains("Read and authenticate the original completion result"));
    assert!(goal.contains(&serde_json::to_string(&c).unwrap()));
    assert_eq!(git(&repo.0, &["status", "--porcelain"]), status);
}

#[tokio::test]
async fn working_acp_wrap_up_queues_behind_running_turn() {
    let (_repo, mut s, track, mut c, _) = fixture();
    c.completion = None;
    c.scope = ChangeScope::Working {
        base_oid: c.scope.base_oid().into(),
        head_oid: c.scope.head_oid().into(),
        paths: c.scope.paths().to_vec(),
        worktree_version: "working-v1".into(),
    };
    s.branch.tags.clear();
    s = registered(s, &c);
    s.protocol = "acp".into();
    s.status = "running".into();
    let snapshot = json!({"base":{},"head_oid":c.scope.head_oid(),"version":"working-v1"});
    let (client, server) = mock(vec![
        ("/api/sessions/get", json!(s)),
        ("/api/sessions/summary/list", fleet(&s, &track)),
        ("/api/sessions/get", json!(track)),
        ("/api/sessions/changes", snapshot.clone()),
        ("/api/channels/subscription/set", sub("track")),
        ("/api/channels/subscription/set", sub("track")),
        ("/api/sessions/get", json!(s)),
        ("/api/sessions/changes", snapshot),
        ("/api/sessions/summary/list", fleet(&s, &track)),
        ("/api/sessions/get", json!(track)),
        ("/api/sessions/get", json!(s)),
        (
            "/api/sessions/prompt/create",
            json!({"queued":true,"turn":null}),
        ),
        ("/api/channels/subscription/set", sub("track")),
        (
            "/api/channels/messages/create",
            json!({"_message_receipt":true}),
        ),
    ])
    .await;
    land_change_with_client(&client, &s.id, &c.id, &c, None)
        .await
        .unwrap();
    let calls = server.await.unwrap();
    let queued = calls
        .iter()
        .find(|(path, _)| path.ends_with("/prompt/create"))
        .unwrap();
    assert_eq!(queued.1["session"], s.id);
    assert_eq!(queued.1["send_now"], false);
    assert!(queued.1["text"]
        .as_str()
        .unwrap()
        .contains(&serde_json::to_string(&c).unwrap()));
    assert!(calls.iter().all(|(path, _)| !path.ends_with("/adopt")
        && !path.ends_with("/interrupt")
        && !path.ends_with("/send")));
}

#[test]
fn shared_frontend_native_text_contract_and_sibling_registry_behavior() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/change-text-contract.json"
    ))
    .unwrap();
    fn patch(base: &mut Value, updates: &Value) {
        for (key, value) in updates.as_object().unwrap() {
            if value.is_object() {
                patch(&mut base[key], value);
            } else {
                base[key] = value.clone();
            }
        }
    }
    let base_change: Change = serde_json::from_value(fixture["change"].clone()).unwrap();
    for item in fixture["cases"].as_array().unwrap() {
        let kind = item["kind"].as_str().unwrap();
        let mut value = fixture[kind].clone();
        patch(&mut value, &item["patch"]);
        let valid = if kind == "change" {
            let c: Change = serde_json::from_value(value.clone()).unwrap();
            let mut s = source();
            s.id = c.source_session.clone();
            s.branch.repo_root = c.repo_root.clone();
            s.branch.branch = c.source_branch.clone();
            s.work_dir = c.source_work_dir.clone();
            let mut sibling = change(&s);
            sibling.id = "sibling".into();
            s = registered(registered(s, &c), &sibling);
            let valid = c.validate(false).is_ok();
            assert_eq!(
                ChangeRegistry::parse(&s.branch.tags, &s).is_ok(),
                valid,
                "{} sibling registry",
                item["name"]
            );
            valid
        } else {
            let r: ChangeAcceptance = serde_json::from_value(value.clone()).unwrap();
            r.verify(&base_change).is_ok()
        };
        assert_eq!(valid, item["valid"].as_bool().unwrap(), "{}", item["name"]);
    }
}

#[path = "change_routing_tests.rs"]
mod routing_regressions;

fn preview_script(s: &SessionView, c: &Change) -> Vec<(&'static str, Value)> {
    vec![
        ("/api/sessions/get", json!(s)),
        ("/api/channels/messages/list", json!([message(c)])),
        ("/api/sessions/get", json!(s)),
    ]
}

fn review_script(s: &SessionView, track: &SessionView, c: &Change) -> Vec<(&'static str, Value)> {
    let mut reviewer = s.clone();
    reviewer.id = "reviewer".into();
    reviewer.branch.id = "reviewer-branch".into();
    reviewer.branch.branch = "weaver/reviewer".into();
    reviewer.branch.tags.clear();
    let mut script = vec![("/api/sessions/get", json!(s))];
    script.extend(preview_script(s, c));
    script.extend([
        ("/api/sessions/summary/list", fleet(s, track)),
        ("/api/sessions/get", json!(track)),
        ("/api/sessions/get", json!(s)),
        ("/api/channels/messages/list", json!([message(c)])),
        ("/api/sessions/get", json!(s)),
        ("/api/sessions/launch", json!(reviewer)),
        ("/api/branches/update", json!(reviewer.branch)),
        ("/api/sessions/tags/set", json!({})),
        ("/api/sessions/tags/set", json!({})),
    ]);
    script
}

#[tokio::test]
async fn colliding_backend_ids_route_preview_reviewer_and_land_with_local_wire_identity() {
    use crate::coordination::{review_session_on_server, ReviewSettings};
    use std::sync::Arc;
    let (_repo_a, source_a, track_a, change_a, _) = fixture();
    let (_repo_b, source_b, track_b, change_b, _) = fixture();
    assert_eq!(source_a.id, source_b.id);
    assert_eq!(change_a.id, change_b.id);
    assert_eq!(change_a.completion, change_b.completion);
    assert_ne!(change_a.repo_root, change_b.repo_root);
    let state = LoomState::default();
    let mut servers = Vec::new();
    for (source, track, change) in [
        (&source_a, &track_a, &change_a),
        (&source_b, &track_b, &change_b),
    ] {
        let mut script = preview_script(source, change);
        script.extend(review_script(source, track, change));
        script.extend(land_script(source, track, change));
        let (client, server) = mock(script).await;
        let url = client.server_url();
        state.servers.install_for_test(Arc::new(client)).await;
        servers.push((url, server));
    }
    // Both entries are installed before selecting either. Last-connected state
    // must not influence any evidence read or dispatch to the first backend.
    for ((url, server), (source, track, change)) in servers.into_iter().zip([
        (&source_a, &track_a, &change_a),
        (&source_b, &track_b, &change_b),
    ]) {
        let preview = change_changes_on_server(&state, &url, &source.id, &change.id, change)
            .await
            .unwrap();
        assert_eq!(
            preview.changes.head_oid.as_deref(),
            Some(change.scope.head_oid())
        );
        assert_eq!(preview.changes.files.len(), 1);
        assert_eq!(preview.changes.files[0]["path"]["display"], "selected.txt");
        let review = review_session_on_server(
            &state,
            &url,
            &source.id,
            "Review selected output",
            (Some(&change.id), Some(change)),
            ReviewSettings::default(),
        )
        .await
        .unwrap();
        let reviewed = serde_json::to_value(review).unwrap();
        assert_eq!(reviewed["session"]["id"], "reviewer");
        assert_eq!(
            reviewed["session"]["branch"]["repo_root"],
            source.branch.repo_root
        );
        let target = land_change_on_server(&state, &url, &source.id, &change.id, change, None)
            .await
            .unwrap();
        assert_eq!(target.coordinator_id, track.id);
        let calls = server.await.unwrap();
        let launch = &calls
            .iter()
            .find(|(path, _)| path == "/api/sessions/launch")
            .unwrap()
            .1;
        assert_eq!(launch["cwd"], source.branch.repo_root);
        assert_eq!(launch["base"], change.scope.head_oid());
        assert_eq!(launch["parent_branch"], source.branch.id);
        assert!(calls
            .iter()
            .filter(|(path, _)| path == "/api/sessions/tags/set")
            .all(|(_, body)| body["session"] == "reviewer"));
        assert!(launch["goal"]
            .as_str()
            .unwrap()
            .contains(&serde_json::to_string(change).unwrap()));
        let dispatches: Vec<_> = calls
            .iter()
            .filter(|(path, _)| path == "/api/channels/messages/create")
            .collect();
        assert_eq!(dispatches.len(), 1);
        let payload = &dispatches[0].1["payload"];
        assert_eq!(dispatches[0].1["channel"], track.id);
        assert_eq!(payload["expected_change"], json!(change));
        assert_eq!(payload["change_id"], change.id);
        assert_eq!(payload["request"]["source_session"], source.id);
        assert_eq!(
            payload["expected_change"]["completion"],
            json!(change.completion)
        );
        assert!(calls
            .iter()
            .filter(|(path, _)| path == "/api/channels/messages/list")
            .all(|(_, body)| body["channel"] == source.id));
        assert!(calls
            .iter()
            .filter(|(path, _)| path == "/api/channels/subscription/set")
            .all(|(_, body)| body["channel"] == track.id && body["session"] == track.id));
    }
}

#[tokio::test]
async fn selected_actions_fail_closed_for_unavailable_backend_without_healthy_fallback() {
    use crate::coordination::{review_session_on_server, ReviewSettings};
    use std::sync::Arc;
    let state = LoomState::default();
    let (_repo, source, track, change, _) = fixture();
    let (healthy, healthy_server) = mock(preview_script(&source, &change)).await;
    let healthy_url = healthy.server_url();
    state.servers.install_for_test(Arc::new(healthy)).await;
    // Keep a live HTTP listener for the unavailable entry to prove there is no
    // request even if the old transport could still answer.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unavailable_url = format!("http://{}", listener.local_addr().unwrap());
    let unavailable = state
        .servers
        .install_for_test(Arc::new(LoomClient::new(&unavailable_url, None).unwrap()))
        .await;
    unavailable.publish_fleet(false, || {});
    for url in [&unavailable_url, &"http://127.0.0.1:1".to_string()] {
        let errors = [
            change_changes_on_server(&state, url, &source.id, &change.id, &change)
                .await
                .err()
                .unwrap(),
            land_change_on_server(&state, url, &source.id, &change.id, &change, None)
                .await
                .err()
                .unwrap(),
            review_session_on_server(
                &state,
                url,
                &source.id,
                "Review",
                (Some(&change.id), Some(&change)),
                ReviewSettings::default(),
            )
            .await
            .err()
            .unwrap(),
            selected_preparation_on_server(
                &state,
                url,
                &source.id,
                &change.id,
                &change,
                "Bounded Prepare",
            )
            .await
            .err()
            .unwrap(),
        ];
        for error in errors {
            assert!(error.unreachable);
            assert_eq!(error.server_url.as_deref(), Some(url.as_str()));
        }
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
    // An outage on one backend leaves the healthy owning backend usable.
    change_changes_on_server(&state, &healthy_url, &source.id, &change.id, &change)
        .await
        .unwrap();
    assert_eq!(healthy_server.await.unwrap().len(), 3);
    assert_eq!(track.id, "track");
}

#[tokio::test]
async fn selected_backend_does_not_borrow_matching_completion_or_receipt_from_another_server() {
    use std::sync::Arc;
    let (_repo, source, _, change, _) = fixture();
    let state = LoomState::default();
    let (client, server) = mock(vec![
        ("/api/sessions/get", json!(source)),
        ("/api/channels/messages/list", json!([])),
    ])
    .await;
    let missing_url = client.server_url();
    state.servers.install_for_test(Arc::new(client)).await;
    // The second backend has the colliding output and an accepted receipt;
    // neither its evidence nor its receipt may affect the selected backend.
    let mut accepted = source.clone();
    tagged(
        &mut accepted,
        "change_acceptance:native",
        &json!(receipt(&change)),
    );
    let (client, server_accepted) = mock(vec![("/api/sessions/get", json!(accepted))]).await;
    let accepted_url = client.server_url();
    state.servers.install_for_test(Arc::new(client)).await;
    let missing = change_changes_on_server(&state, &missing_url, &source.id, &change.id, &change)
        .await
        .err()
        .unwrap();
    assert!(missing.message.contains("completion reference not found"));
    assert_eq!(missing.server_url.as_deref(), Some(missing_url.as_str()));
    let accepted =
        land_change_on_server(&state, &accepted_url, &source.id, &change.id, &change, None)
            .await
            .err()
            .unwrap();
    assert!(accepted.message.contains("accepted"));
    assert_eq!(accepted.server_url.as_deref(), Some(accepted_url.as_str()));
    assert_eq!(server.await.unwrap().len(), 2);
    assert_eq!(server_accepted.await.unwrap().len(), 1);
}

// R1: archival removes the linked checkout while durable range objects/refs
// remain in the original repository. This fixture removes a REAL worktree.
fn r1_removed_checkout() -> (Repository, SessionView, SessionView, Change) {
    let (repo, mut source, mut track, mut change, _) = fixture();
    git(&repo.0, &["checkout", "-qb", "primary"]);
    let checkout = repo.0.join(".worktrees/archived-source");
    git(
        &repo.0,
        &[
            "worktree",
            "add",
            "--quiet",
            checkout.to_str().unwrap(),
            "weaver/worker",
        ],
    );
    source.work_dir = checkout.to_str().unwrap().into();
    source.status = "archived".into();
    source.protocol = "acp".into();
    change.source_work_dir = source.work_dir.clone();
    source.branch.tags.clear();
    source = registered(source, &change);
    track.work_dir = repo.0.to_str().unwrap().into();
    git(&repo.0, &["worktree", "remove", checkout.to_str().unwrap()]);
    assert!(!checkout.exists());
    assert_eq!(
        git(&repo.0, &["rev-parse", "refs/heads/weaver/worker"]),
        change.scope.head_oid()
    );
    (repo, source, track, change)
}

#[tokio::test]
async fn r1_removed_linked_checkout_range_previews_reviews_and_lands_without_source_recovery() {
    use crate::coordination::{review_session_on_server, ReviewSettings};
    use std::sync::Arc;
    let (repo, source, track, change) = r1_removed_checkout();
    // Dirty primary checkout/index and sibling commits never enter range review.
    std::fs::write(repo.0.join("selected.txt"), "foreign primary edit\n").unwrap();
    std::fs::write(repo.0.join("foreign-staged.txt"), "foreign staged\n").unwrap();
    git(&repo.0, &["add", "foreign-staged.txt"]);
    std::fs::write(repo.0.join("foreign-staged.txt"), "foreign unstaged\n").unwrap();
    std::fs::write(repo.0.join("foreign-untracked.txt"), "foreign untracked\n").unwrap();
    std::fs::write(repo.0.join(".gitignore"), "foreign-ignored.txt\n").unwrap();
    std::fs::write(repo.0.join("foreign-ignored.txt"), "foreign ignored\n").unwrap();
    let before_status = git(&repo.0, &["status", "--porcelain", "--ignored"]);
    let before_head = git(&repo.0, &["rev-parse", "HEAD"]);
    let before_index = git(&repo.0, &["ls-files", "--stage"]);
    let mut script = preview_script(&source, &change);
    script.extend(review_script(&source, &track, &change));
    script.extend(land_script(&source, &track, &change));
    let (client, server) = mock(script).await;
    let state = LoomState::default();
    let url = client.server_url();
    state.servers.install_for_test(Arc::new(client)).await;
    let preview = change_changes_on_server(&state, &url, &source.id, &change.id, &change)
        .await
        .unwrap();
    assert_eq!(preview.changes.files.len(), 1);
    assert_eq!(preview.changes.files[0]["path"]["display"], "selected.txt");
    assert_eq!(
        preview.changes.head_oid.as_deref(),
        Some(change.scope.head_oid())
    );
    let preview_text = json!(preview).to_string();
    assert!(!preview_text.contains("foreign primary"));
    assert!(preview_text.contains("selected"));
    review_session_on_server(
        &state,
        &url,
        &source.id,
        "Review removed-checkout range",
        (Some(&change.id), Some(&change)),
        ReviewSettings::default(),
    )
    .await
    .unwrap();
    let target = land_change_on_server(&state, &url, &source.id, &change.id, &change, None)
        .await
        .unwrap();
    assert_eq!(target.target_branch, "arachne/tracks/accepted");
    let calls = server.await.unwrap();
    assert!(calls.iter().all(|(path, body)| {
        !path.ends_with("/adopt")
            && !path.ends_with("/prompt/create")
            && !path.ends_with("/send")
            && !path.ends_with("/interrupt")
            && !path.ends_with("/changes")
            && !path.ends_with("/chat")
            && (!matches!(
                path.as_str(),
                "/api/channels/subscription/set" | "/api/channels/messages/create"
            ) || (body["channel"] == track.id && body["session"] != source.id))
            && (path != "/api/sessions/tags/set" || body["session"] == "reviewer")
    }));
    let deliveries: Vec<_> = calls
        .iter()
        .filter(|(path, _)| path == "/api/channels/messages/create")
        .collect();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].1["payload"]["expected_change"], json!(change));
    assert_eq!(
        deliveries[0].1["payload"]["expected_change"]["source_work_dir"],
        source.work_dir
    );
    assert_eq!(
        deliveries[0].1["payload"]["expected_change"]["completion"],
        json!(change.completion)
    );
    let launch = &calls
        .iter()
        .find(|(path, _)| path == "/api/sessions/launch")
        .unwrap()
        .1;
    assert_eq!(launch["base"], change.scope.head_oid());
    assert!(launch["goal"]
        .as_str()
        .unwrap()
        .contains(&serde_json::to_string(&change).unwrap()));
    assert!(!Path::new(&source.work_dir).exists());
    assert_eq!(
        git(&repo.0, &["status", "--porcelain", "--ignored"]),
        before_status
    );
    assert_eq!(git(&repo.0, &["rev-parse", "HEAD"]), before_head);
    assert_eq!(git(&repo.0, &["ls-files", "--stage"]), before_index);
    assert_eq!(
        std::fs::read_to_string(repo.0.join("foreign-ignored.txt")).unwrap(),
        "foreign ignored\n"
    );
}

#[tokio::test]
async fn r1_removed_checkout_range_missing_repository_objects_or_refs_fails_before_delivery() {
    for failure in [
        "missing-repo",
        "malformed-repo",
        "enclosing-repo",
        "missing-base",
        "missing-head",
        "missing-source-ref",
        "unreachable-source-ref",
        "missing-target-ref",
    ] {
        let (repo, mut source, mut track, mut change) = r1_removed_checkout();
        match failure {
            "missing-repo" | "malformed-repo" | "enclosing-repo" => {
                let replacement = repo.0.join(failure);
                if failure != "missing-repo" {
                    std::fs::create_dir(&replacement).unwrap();
                }
                if failure == "malformed-repo" {
                    std::fs::write(replacement.join(".git"), "gitdir: missing-git-store\n")
                        .unwrap();
                }
                source.branch.repo_root = replacement.to_str().unwrap().into();
                change.repo_root = source.branch.repo_root.clone();
                track.branch.repo_root = source.branch.repo_root.clone();
            }
            "missing-base" => {
                if let ChangeScope::Range { base_oid, .. } = &mut change.scope {
                    *base_oid = "0".repeat(40);
                }
            }
            "missing-head" => {
                if let ChangeScope::Range { head_oid, .. } = &mut change.scope {
                    *head_oid = "0".repeat(40);
                }
            }
            "missing-source-ref" => {
                git(&repo.0, &["branch", "-D", "weaver/worker"]);
            }
            "unreachable-source-ref" => {
                git(
                    &repo.0,
                    &["branch", "-f", "weaver/worker", change.scope.base_oid()],
                );
            }
            "missing-target-ref" => {
                git(&repo.0, &["branch", "-D", "arachne/tracks/accepted"]);
            }
            _ => unreachable!(),
        }
        source.branch.tags.clear();
        source = registered(source, &change);
        let before = git(&repo.0, &["status", "--porcelain"]);
        let mut script = land_script(&source, &track, &change);
        script.truncate(4); // Original completion authenticated before local failure.
        let (client, server) = mock(script).await;
        assert!(
            land_change_with_client(&client, &source.id, &change.id, &change, None)
                .await
                .is_err(),
            "{failure}"
        );
        let calls = server.await.unwrap();
        assert_eq!(calls.len(), 4, "{failure}");
        assert!(!Path::new(&source.work_dir).exists());
        assert_eq!(git(&repo.0, &["status", "--porcelain"]), before);
    }
}

#[tokio::test]
async fn r1_removed_checkout_never_adopts_replacement_foreign_checkout_or_parent_directory() {
    for replacement in ["foreign-repository", "ordinary-directory"] {
        let (repo, source, _, change) = r1_removed_checkout();
        let path = Path::new(&source.work_dir);
        if replacement == "foreign-repository" {
            // Even a clone with the exact objects and same named source branch
            // cannot replace the declaration's original same-repository checkout.
            git(
                &repo.0,
                &[
                    "clone",
                    "--quiet",
                    "--local",
                    "--branch",
                    "weaver/worker",
                    repo.0.to_str().unwrap(),
                    &source.work_dir,
                ],
            );
        } else {
            std::fs::create_dir(path).unwrap();
            std::fs::write(path.join("foreign.txt"), "foreign replacement\n").unwrap();
        }
        let original_store = git(&repo.0, &["rev-parse", "--git-common-dir"]);
        let before = git(&repo.0, &["status", "--porcelain"]);
        let (client, server) = mock(vec![
            ("/api/sessions/get", json!(source)),
            ("/api/channels/messages/list", json!([message(&change)])),
        ])
        .await;
        assert!(
            change_changes_with_client(&client, &source.id, &change.id, &change)
                .await
                .is_err(),
            "{replacement}"
        );
        assert_eq!(server.await.unwrap().len(), 2);
        assert_eq!(git(&repo.0, &["status", "--porcelain"]), before);
        assert_eq!(
            git(&repo.0, &["rev-parse", "--git-common-dir"]),
            original_store
        );
        if replacement == "foreign-repository" {
            assert!(git(path, &["status", "--porcelain"]).is_empty());
        } else {
            assert_eq!(
                std::fs::read_to_string(path.join("foreign.txt")).unwrap(),
                "foreign replacement\n"
            );
        }
    }
}

#[tokio::test]
async fn r1_removed_checkout_working_scope_requires_original_checkout_even_with_matching_snapshot()
{
    let (_repo, mut source, track, mut change) = r1_removed_checkout();
    change.completion = None;
    change.scope = ChangeScope::Working {
        base_oid: change.scope.base_oid().into(),
        head_oid: change.scope.head_oid().into(),
        paths: change.scope.paths().to_vec(),
        worktree_version: "still-pinned".into(),
    };
    source.branch.tags.clear();
    source = registered(source, &change);
    let snapshot = json!({"base":{},"head_oid":change.scope.head_oid(),"version":"still-pinned"});
    let (client, server) = mock(vec![
        ("/api/sessions/get", json!(source)),
        ("/api/sessions/changes", snapshot.clone()),
    ])
    .await;
    assert!(
        change_changes_with_client(&client, &source.id, &change.id, &change)
            .await
            .is_err()
    );
    assert_eq!(server.await.unwrap().len(), 2);
    let (client, server) = mock(vec![
        ("/api/sessions/get", json!(source)),
        ("/api/sessions/summary/list", fleet(&source, &track)),
        ("/api/sessions/get", json!(track)),
        ("/api/sessions/changes", snapshot),
    ])
    .await;
    assert!(
        land_change_with_client(&client, &source.id, &change.id, &change, None)
            .await
            .is_err()
    );
    assert_eq!(server.await.unwrap().len(), 4);
    assert!(!Path::new(&source.work_dir).exists());
}
