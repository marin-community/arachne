//! Narrow Loom's untyped chat journal for the conversation view.
//!
//! The ACP adapter owns the payload schema. Keep the original message payload
//! available to the UI so image/resource parts survive even when an adapter
//! adds a new shape that this version of Arachne does not yet recognize.

use serde_json::Value;

use crate::loom::ChatBlockView;

/// A block narrowed for display. `(turn, seq)` is a stable journal coordinate
/// for disclosure state across pagination and refreshes.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DisplayBlock {
    UserMessage {
        turn: i64,
        seq: i64,
        text: String,
        by: Option<String>,
        payload: Value,
    },
    AgentMessage {
        turn: i64,
        seq: i64,
        text: String,
        payload: Value,
    },
    Thought {
        turn: i64,
        seq: i64,
        text: String,
        summary: String,
    },
    ToolCall {
        turn: i64,
        seq: i64,
        title: String,
        tool_kind: String,
        status: String,
        summary: String,
        /// ACP content, including `type: image` with base64 `data` and
        /// `mime_type`. The one-line summary intentionally skips binary data.
        content: Vec<Value>,
    },
    Plan {
        turn: i64,
        seq: i64,
        entries: Vec<(String, String)>,
    },
    Usage {
        turn: i64,
        seq: i64,
        used: Option<i64>,
        size: Option<i64>,
    },
    TurnEnd {
        turn: i64,
        seq: i64,
        stop_reason: String,
    },
    Other {
        turn: i64,
        seq: i64,
        unknown_kind: String,
        payload: String,
    },
}

fn string_field<'a>(payload: &'a Value, key: &str) -> &'a str {
    payload.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Codex currently journals thoughts like `\n\n**Inspecting files**`.
/// Extract that deliberate heading when present. For other adapters, use a
/// concise first nonempty line. This is a display label, not a model summary.
fn thought_summary(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let heading = first
        .strip_prefix("**")
        .and_then(|s| s.strip_suffix("**"))
        .or_else(|| first.strip_prefix("### "))
        .or_else(|| first.strip_prefix("## "))
        .or_else(|| first.strip_prefix("# "))
        .unwrap_or(first);
    heading.chars().take(160).collect()
}

impl DisplayBlock {
    pub fn from_view(block: &ChatBlockView) -> Self {
        let payload = &block.payload;
        let turn = block.turn;
        let seq = block.seq;
        match block.kind.as_str() {
            "user_message" => Self::UserMessage {
                turn,
                seq,
                text: string_field(payload, "text").to_string(),
                by: payload
                    .get("by")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                payload: payload.clone(),
            },
            "agent_message" => Self::AgentMessage {
                turn,
                seq,
                text: string_field(payload, "text").to_string(),
                payload: payload.clone(),
            },
            "thought" => {
                let text = string_field(payload, "text").to_string();
                Self::Thought {
                    turn,
                    seq,
                    summary: thought_summary(&text),
                    text,
                }
            }
            "tool_call" => {
                // Text runs join; diffs become `path`. Unknown content types
                // can still be inspected in the Loom journal itself.
                let mut parts = Vec::new();
                if let Some(content) = payload.get("content").and_then(Value::as_array) {
                    for item in content {
                        match item.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                if let Some(text) = item.get("text").and_then(Value::as_str) {
                                    parts.push(text.to_string());
                                }
                            }
                            Some("diff") => parts.push(
                                item.get("path")
                                    .and_then(Value::as_str)
                                    .unwrap_or("?")
                                    .to_string(),
                            ),
                            _ => {}
                        }
                    }
                }
                Self::ToolCall {
                    turn,
                    seq,
                    title: string_field(payload, "title").to_string(),
                    tool_kind: string_field(payload, "tool_kind").to_string(),
                    status: string_field(payload, "status").to_string(),
                    summary: parts.join(" "),
                    content: payload
                        .get("content")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                }
            }
            "plan" => {
                let entries = payload
                    .get("entries")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| {
                        Some((
                            entry.get("content")?.as_str()?.to_string(),
                            entry
                                .get("status")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                        ))
                    })
                    .collect();
                Self::Plan { turn, seq, entries }
            }
            "usage" => Self::Usage {
                turn,
                seq,
                used: payload.get("used").and_then(Value::as_i64),
                size: payload.get("size").and_then(Value::as_i64),
            },
            "turn_end" => Self::TurnEnd {
                turn,
                seq,
                stop_reason: string_field(payload, "stop_reason").to_string(),
            },
            _ => Self::Other {
                turn,
                seq,
                unknown_kind: block.kind.clone(),
                payload: serde_json::to_string(payload).unwrap_or_default(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn block(kind: &str, payload: Value) -> ChatBlockView {
        ChatBlockView {
            turn: 7,
            seq: 9,
            kind: kind.to_string(),
            created_at: "t".to_string(),
            payload,
        }
    }

    #[test]
    fn message_preserves_image_parts_and_coordinates() {
        let payload = json!({
            "text": "look",
            "resources": [{"type": "resource_link", "name": "screenshots/result.png", "uri": "file:///worktree/screenshots/result.png"}],
            "adapter_extension": {"image_ref": "abc"}
        });
        let display = serde_json::to_value(DisplayBlock::from_view(&block(
            "user_message",
            payload.clone(),
        )))
        .unwrap();
        assert_eq!(display["turn"], 7);
        assert_eq!(display["seq"], 9);
        assert_eq!(display["payload"], payload);
    }

    #[test]
    fn codex_thought_heading_is_display_summary() {
        let display = serde_json::to_value(DisplayBlock::from_view(&block(
            "thought",
            json!({"ms": null, "text": "\n\n**Inspecting files**"}),
        )))
        .unwrap();
        assert_eq!(display["summary"], "Inspecting files");
        assert_eq!(display["text"], "\n\n**Inspecting files**");
    }

    #[test]
    fn tool_image_content_survives_summary() {
        let content =
            json!([{"type": "image", "data": "aGVsbG8=", "mime_type": "image/png", "uri": null}]);
        let display = serde_json::to_value(DisplayBlock::from_view(&block(
            "tool_call",
            json!({"title": "screenshot", "content": content}),
        )))
        .unwrap();
        assert_eq!(display["content"], content);
        assert_eq!(display["summary"], "");
    }
}
