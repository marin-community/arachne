//! Chat-journal block rendering: narrowing `ChatBlockView.payload` (untyped
//! JSON on the wire) into the shapes the UI renders, keyed by `kind`.
//!
//! Mirrors loom's own SPA reading (`frontend/src/types.ts` in the loom repo):
//! loom stores whatever the ACP adapter journaled, and the browser — here,
//! Arachne — is the reader that knows the shapes.

use serde::Deserialize;

use crate::loom::ChatBlockView;

/// A block narrowed for display. Unknown kinds degrade to a debug JSON dump
/// rather than an error — a newer agent can journal new block kinds at any
/// time, and an old cockpit must keep rendering the rest of the conversation.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DisplayBlock {
    UserMessage {
        text: String,
        by: Option<String>,
    },
    AgentMessage {
        text: String,
    },
    Thought {
        text: String,
    },
    ToolCall {
        title: String,
        tool_kind: String,
        status: String,
        summary: String,
    },
    Plan {
        entries: Vec<(String, String)>, // (content, status)
    },
    Usage {
        used: Option<i64>,
        size: Option<i64>,
    },
    TurnEnd {
        stop_reason: String,
    },
    Other {
        unknown_kind: String,
        payload: String,
    },
}

impl DisplayBlock {
    pub fn from_view(block: &ChatBlockView) -> Self {
        #[derive(Deserialize)]
        struct Loose {
            #[serde(default)]
            text: String,
            #[serde(default)]
            by: Option<String>,
            #[serde(default)]
            tool_kind: String,
            #[serde(default)]
            title: String,
            #[serde(default)]
            status: String,
            #[serde(default)]
            content: Vec<serde_json::Value>,
            #[serde(default)]
            entries: Vec<serde_json::Value>,
            #[serde(default)]
            used: Option<i64>,
            #[serde(default)]
            size: Option<i64>,
            #[serde(default)]
            stop_reason: String,
        }
        // Everything deserializes leniently; a missing field is its default.
        let loose: Loose = match serde_json::from_value(block.payload.clone()) {
            Ok(l) => l,
            Err(_) => Loose {
                text: String::new(),
                by: None,
                tool_kind: String::new(),
                title: String::new(),
                status: String::new(),
                content: vec![],
                entries: vec![],
                used: None,
                size: None,
                stop_reason: String::new(),
            },
        };
        match block.kind.as_str() {
            "user_message" => DisplayBlock::UserMessage {
                text: loose.text,
                by: loose.by,
            },
            "agent_message" => DisplayBlock::AgentMessage { text: loose.text },
            "thought" => DisplayBlock::Thought { text: loose.text },
            "tool_call" => {
                // Condense the tool's content list into a one-line summary:
                // text runs join; diffs become `path (+n/-m)`.
                let mut summary = String::new();
                for item in &loose.content {
                    match item.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                                if !summary.is_empty() {
                                    summary.push(' ');
                                }
                                summary.push_str(t);
                            }
                        }
                        Some("diff") => {
                            let path = item.get("path").and_then(|p| p.as_str()).unwrap_or("?");
                            summary.push_str(&format!(" {path}"));
                        }
                        _ => {}
                    }
                }
                DisplayBlock::ToolCall {
                    title: loose.title,
                    tool_kind: loose.tool_kind,
                    status: loose.status,
                    summary: summary.trim().to_string(),
                }
            }
            "plan" => {
                let entries = loose
                    .entries
                    .iter()
                    .filter_map(|e| {
                        Some((
                            e.get("content").and_then(|c| c.as_str())?.to_string(),
                            e.get("status")
                                .and_then(|s| s.as_str())
                                .unwrap_or("")
                                .to_string(),
                        ))
                    })
                    .collect();
                DisplayBlock::Plan { entries }
            }
            "usage" => DisplayBlock::Usage {
                used: loose.used,
                size: loose.size,
            },
            "turn_end" => DisplayBlock::TurnEnd {
                stop_reason: loose.stop_reason,
            },
            _ => DisplayBlock::Other {
                unknown_kind: block.kind.clone(),
                payload: serde_json::to_string(&block.payload).unwrap_or_default(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_max_tokens_turn_boundary_for_the_ui() {
        let block = ChatBlockView {
            seq: 9,
            turn: 2,
            kind: "turn_end".into(),
            created_at: "2026-09-27T05:11:22Z".into(),
            payload: serde_json::json!({ "stop_reason": "max_tokens" }),
        };

        match DisplayBlock::from_view(&block) {
            DisplayBlock::TurnEnd { stop_reason } => assert_eq!(stop_reason, "max_tokens"),
            other => panic!("expected turn_end, got {other:?}"),
        }
    }
}
