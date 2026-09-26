// Decode harness: wire-format regression tests against captured loom responses.
use arachne_lib::loom::{SessionChatView, SessionSummaryView, SessionView};

#[test]
fn decode_launch_response() {
    let data = std::fs::read_to_string("/tmp/launch-resp.json").unwrap();
    let v: SessionView = serde_json::from_str(&data).expect("SessionView decode");
    assert_eq!(v.protocol, "acp");
    assert_eq!(v.work_dir, "/Users/dlwh/.weaver/repos/marin-community/arachne/.worktrees/decode-probe");
}

#[test]
fn decode_summary_list() {
    // A captured summary-list entry (fleet sidebar source).
    let sample = r#"[{
        "branch": {"branch":"weaver/test-3","description":"","github":null,"github_pr":null,
          "id":"84a81h3s","name":"test-3","repo_root":"/r","tags":[],"title":"test"},
        "class":"interactive","created_at":"2026-09-26T00:54:09.730Z","created_by":"dlwh",
        "github_issue":null,"github_repo":null,"id":"sgtallgq",
        "last_activity_at":"2026-09-26T00:54:10.481Z","origin":"user","parent_id":null,
        "parent_session_id":null,
        "placement": {"group_id":"group-user-inbox","group_name":"Inbox","group_system_key":"inbox",
          "rank":7,"session_id":"sgtallgq","space_id":"space-user","space_name":"User"},
        "profile":"default","status":"running","tracking_issue":null,"transition":null,
        "usage": {"cost":null,"size":131072,"used":0}
    }]"#;
    let v: Vec<SessionSummaryView> = serde_json::from_str(sample).expect("summary list decode");
    assert_eq!(v[0].placement.as_ref().unwrap().group_name.as_deref(), Some("Inbox"));
}

#[test]
fn decode_chat() {
    let sample = r#"{"blocks":[{"created_at":"t","kind":"user_message","payload":{"by":null,"resources":[],"text":"hi"},"seq":0,"turn":0}],"effective_mode":"auto","live_turn":null,"metadata":{},"older_cursor":null,"pending_prompt":null}"#;
    let v: SessionChatView = serde_json::from_str(sample).expect("chat decode");
    assert_eq!(v.blocks[0].kind, "user_message");
}

#[test]
fn chat_snapshot_live_timing_from_journal() {
    // A live turn's elapsed clock is rebuilt from the journal: the turn's
    // opening block is the start, the newest block of that turn is the last
    // observed progress. Older turns (paging) and other kinds are ignored.
    use arachne_lib::commands::ChatSnapshot;
    let sample = r#"{"blocks":[
        {"created_at":"2026-09-26T05:00:00.000Z","kind":"usage","payload":{},"seq":0,"turn":0},
        {"created_at":"2026-09-26T05:00:01.000Z","kind":"user_message","payload":{},"seq":1,"turn":3},
        {"created_at":"2026-09-26T05:00:02.000Z","kind":"thought","payload":{},"seq":2,"turn":3},
        {"created_at":"2026-09-26T05:00:03.000Z","kind":"tool_call","payload":{},"seq":3,"turn":3}
    ],"effective_mode":null,"live_turn":3,"metadata":{},"older_cursor":null,"pending_prompt":"queued behind"}"#;
    let chat: SessionChatView = serde_json::from_str(sample).expect("chat decode");
    let (started, progress) = ChatSnapshot::live_timing(&chat);
    assert_eq!(started.as_deref(), Some("2026-09-26T05:00:01.000Z"));
    assert_eq!(progress.as_deref(), Some("2026-09-26T05:00:03.000Z"));

    // No live turn: no timing.
    let idle = serde_json::json!({
        "blocks": [],
        "effective_mode": null,
        "live_turn": null,
        "metadata": {},
        "older_cursor": null,
        "pending_prompt": null
    });
    let chat: SessionChatView = serde_json::from_value(idle).expect("idle decode");
    let (started, progress) = ChatSnapshot::live_timing(&chat);
    assert_eq!(started, None);
    assert_eq!(progress, None);
}
