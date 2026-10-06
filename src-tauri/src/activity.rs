//! Read-only activity projection over Loom's existing channel/watch APIs.
//! No scheduler, read-marker mutation, or delivery is created by opening it.
use std::collections::HashSet;

use futures_util::{stream, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use tauri::State;

use crate::commands::{LoomState, UiError};

#[derive(Serialize)]
pub struct TrackActivity {
    channels: Vec<Value>,
    messages: Vec<Value>,
    watches: Vec<Value>,
    warnings: Vec<String>,
    channels_truncated: bool,
}

fn relevant_channel(channel: &Value, sessions: &HashSet<String>) -> bool {
    channel["session_id"]
        .as_str()
        .is_some_and(|id| sessions.contains(id))
        || channel["bindings"].as_array().is_some_and(|bindings| {
            bindings.iter().any(|binding| {
                binding["target_session_id"]
                    .as_str()
                    .is_some_and(|id| sessions.contains(id))
            })
        })
}

fn relevant_watch(watch: &Value, repo: &str) -> bool {
    ["trigger", "scope"].iter().all(|key| {
        watch[key]["repo"]
            .as_str()
            .is_none_or(|filter| filter == repo)
    })
}

fn message_query(channel: &Value) -> Value {
    // The API pages forward; use the authoritative last sequence to read the
    // newest 30 entries, not the first page of a long-lived conversation.
    let latest = channel["last_message"]["seq"].as_i64().unwrap_or(0);
    json!({ "channel": channel["id"], "after": (latest - 30).max(0),
        "limit": 30, "kinds": [], "peek": true })
}

#[tauri::command]
pub async fn track_activity(
    state: State<'_, LoomState>,
    topic_id: String,
) -> Result<TrackActivity, UiError> {
    let client = state.client.read().await.clone().ok_or_else(|| UiError {
        message: "Connect to Loom to inspect Track activity.".into(),
        unreachable: true,
    })?;
    let topic = client.get_session(&topic_id).await?;
    let mut warnings = Vec::new();
    let mut ids = HashSet::from([topic.id.clone()]);
    let mut branches = HashSet::from([topic.branch.id.clone()]);
    match client.list_sessions().await {
        Ok(fleet) => loop {
            let before = ids.len();
            for session in &fleet {
                if session
                    .parent_session_id
                    .as_ref()
                    .is_some_and(|id| ids.contains(id))
                    || session
                        .parent_id
                        .as_ref()
                        .is_some_and(|id| branches.contains(id))
                {
                    ids.insert(session.id.clone());
                    branches.insert(session.branch.id.clone());
                }
            }
            if ids.len() == before {
                break;
            }
        },
        Err(error) => warnings.push(format!("Worker activity unavailable: {error}")),
    }
    let channel_args = json!({ "archived": true });
    let watch_args = json!({});
    let (channels_result, watches_result) = tokio::join!(
        client.op::<Vec<Value>>("/api/channels/list", &channel_args),
        client.op::<Vec<Value>>("/api/watches/list", &watch_args),
    );
    let mut channels = match channels_result {
        Ok(channels) => channels
            .into_iter()
            .filter(|c| relevant_channel(c, &ids))
            .collect::<Vec<_>>(),
        Err(error) => {
            warnings.push(format!("Mailbox unavailable: {error}"));
            vec![]
        }
    };
    channels.sort_by(|a, b| {
        b["last_message"]["created_at"]
            .as_str()
            .cmp(&a["last_message"]["created_at"].as_str())
    });
    let channels_truncated = channels.len() > 20;
    channels.truncate(20);
    let mut watches = match watches_result {
        Ok(watches) => watches
            .into_iter()
            .filter(|w| relevant_watch(w, &topic.branch.repo_root))
            .collect::<Vec<_>>(),
        Err(error) => {
            warnings.push(format!("Watch configuration unavailable: {error}"));
            vec![]
        }
    };
    let pages = stream::iter(channels.clone().into_iter().map(|channel| {
        let client = client.clone();
        async move {
            (
                channel["name"].as_str().unwrap_or("channel").to_owned(),
                client
                    .op::<Vec<Value>>("/api/channels/messages/list", &message_query(&channel))
                    .await,
            )
        }
    }))
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    let mut messages = Vec::new();
    for (name, result) in pages {
        match result {
            Ok(page) => messages.extend(page),
            Err(error) => warnings.push(format!("{name}: {error}")),
        }
    }
    messages.sort_by(|a, b| b["created_at"].as_str().cmp(&a["created_at"].as_str()));
    messages.truncate(60);
    // Fetch diagnostics only for failed watches, with a bounded fan-out.
    for watch in watches
        .iter_mut()
        .filter(|w| w["last_outcome"] == "error")
        .take(4)
    {
        match client
            .op::<Vec<Value>>(
                "/api/watches/runs",
                &json!({ "key": watch["id"], "limit": 1 }),
            )
            .await
        {
            Ok(runs) => watch["latest_run"] = runs.into_iter().next().unwrap_or(Value::Null),
            Err(error) => warnings.push(format!("Watch diagnostics unavailable: {error}")),
        }
    }
    Ok(TrackActivity {
        channels,
        messages,
        watches,
        warnings,
        channels_truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_include_descendant_owners_and_bound_custom_channels_only() {
        let ids = HashSet::from(["worker".to_string()]);
        assert!(relevant_channel(&json!({"session_id":"worker"}), &ids));
        assert!(relevant_channel(
            &json!({"bindings":[{"target_session_id":"worker"}]}),
            &ids
        ));
        assert!(!relevant_channel(
            &json!({"session_id":"other", "repo_root":"same-repo"}),
            &ids
        ));
    }

    #[test]
    fn watches_require_both_repository_filters_to_match() {
        assert!(relevant_watch(&json!({"trigger":{},"scope":{}}), "/repo"));
        assert!(relevant_watch(
            &json!({"trigger":{"repo":"/repo"},"scope":{}}),
            "/repo"
        ));
        assert!(!relevant_watch(
            &json!({"trigger":{"repo":"/other"},"scope":{"repo":"/repo"}}),
            "/repo"
        ));
    }

    #[test]
    fn history_reads_tail_without_advancing_read_marker() {
        let query = message_query(&json!({"id":"track", "last_message":{"seq":120}}));
        assert_eq!(query["after"], 90);
        assert_eq!(query["peek"], true);
        assert_eq!(message_query(&json!({"id":"new"}))["after"], 0);
    }
}
