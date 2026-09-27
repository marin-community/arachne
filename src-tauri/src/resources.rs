//! Durable topic resource bindings. The manifest is a branch-scoped Loom
//! artifact, so it follows the topic's accepted branch rather than this Mac's
//! app installation or a particular session process.
//!
//! The user-facing Todo list follows the same pattern: a branch-scoped
//! artifact (`arachne-todos`) holding one durable cross-topic list. The
//! Topics inspector filters it to a single topic's slice (design.md: a
//! topic's plan and a worker's checklist are separate from user Todos).

use serde::{Deserialize, Serialize};

pub const MANIFEST_NAME: &str = "arachne-resources";
pub const TODOS_NAME: &str = "arachne-todos";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Repository,
    Worktree,
    PullRequest,
    File,
    DesignDocument,
    Artifact,
}

impl ResourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Repository => "repository",
            Self::Worktree => "worktree",
            Self::PullRequest => "pull_request",
            Self::File => "file",
            Self::DesignDocument => "design_document",
            Self::Artifact => "artifact",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDraft {
    pub kind: ResourceKind,
    pub title: String,
    pub repository: String,
    /// Git ref for repository files; normally the topic's accepted branch.
    #[serde(default)]
    pub reference: Option<String>,
    /// Relative repository path for file/design_document, or worktree path.
    #[serde(default)]
    pub path: Option<String>,
    /// PR or external resource URL.
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicResource {
    pub id: String,
    #[serde(flatten)]
    pub data: ResourceDraft,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TopicResourcesView {
    #[serde(default)]
    pub resources: Vec<TopicResource>,
    /// Latest Loom artifact revision. Clients supply this on mutation so
    /// concurrent Mac/agent edits never silently overwrite one another.
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopicResourceContent {
    pub resource: TopicResource,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMention {
    pub topic_id: String,
    pub resource_id: String,
}

impl ResourceDraft {
    pub fn validated(
        mut self,
        topic_repo: &str,
        topic_branch: &str,
    ) -> Result<TopicResource, String> {
        self.title = self.title.trim().to_owned();
        self.repository = self.repository.trim().to_owned();
        self.reference = self
            .reference
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        self.path = self
            .path
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        self.url = self
            .url
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        if self.title.is_empty() || self.title.len() > 256 {
            return Err("resource title must contain 1–256 characters".into());
        }
        if self.repository != topic_repo {
            return Err("resource repository must match the topic repository".into());
        }
        match self.kind {
            ResourceKind::File | ResourceKind::DesignDocument => {
                let path = self.path.as_deref().ok_or("file path is required")?;
                validate_relative_path(path)?;
                if self.reference.as_deref() != Some(topic_branch) {
                    return Err("file reference must be the topic's current branch".into());
                }
            }
            ResourceKind::PullRequest => {
                let url = self.url.as_deref().ok_or("pull request URL is required")?;
                if !(url.starts_with("https://github.com/")
                    || url.starts_with("https://www.github.com/"))
                {
                    return Err("pull request URL must be on github.com".into());
                }
            }
            ResourceKind::Repository => {}
            ResourceKind::Worktree => {
                if self.path.is_none() {
                    return Err("worktree path is required".into());
                }
            }
            ResourceKind::Artifact => {
                if self.path.is_none() {
                    return Err("artifact name is required in path".into());
                }
            }
        }
        // Resource identity is the backing object, not the session or a
        // mutable display title. This also makes repeated attachment idempotent.
        let locator = match self.kind {
            ResourceKind::Repository => self.repository.clone(),
            ResourceKind::PullRequest => self.url.clone().unwrap(),
            _ => format!(
                "{}:{}:{}",
                self.repository,
                self.reference.as_deref().unwrap_or(""),
                self.path.as_deref().unwrap_or("")
            ),
        };
        Ok(TopicResource {
            id: format!("{}:{locator}", self.kind.as_str()),
            data: self,
        })
    }
}

pub fn validate_relative_path(path: &str) -> Result<(), String> {
    use std::path::Path;
    if path.is_empty()
        || path.contains('\\')
        || Path::new(path).is_absolute()
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("path must be relative to the repository without . or ..".into());
    }
    Ok(())
}

// --- Todos -----------------------------------------------------------------

/// One durable user Todo. It belongs to a topic, but the list itself is
/// cross-topic; the inspector shows the slice for the open topic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub done: bool,
    /// Owning topic session id; kept on the item so the single cross-topic
    /// artifact can be filtered per topic (design.md "Todos" tab).
    pub topic_id: String,
    #[serde(default)]
    pub created_at: String,
}

/// The durable todo list, stored as the `arachne-todos` branch artifact.
/// Mirrors `TopicResourcesView` (revision-checked like the resource manifest)
/// so concurrent Mac/agent edits never silently overwrite one another.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TodoListView {
    #[serde(default)]
    pub todos: Vec<TodoItem>,
    pub revision: i64,
}

impl TodoListView {
    /// The slice belonging to one topic, with the list revision carried
    /// through for the same revision-checked mutations.
    pub fn topic_view(&self, topic_id: &str) -> TodoTopicView {
        TodoTopicView {
            todos: self
                .todos
                .iter()
                .filter(|todo| todo.topic_id == topic_id)
                .cloned()
                .collect(),
            revision: self.revision,
        }
    }
}

/// The per-topic view the inspector renders. The wire shape matches
/// `TopicResourcesView`: items plus the artifact revision.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TodoTopicView {
    #[serde(default)]
    pub todos: Vec<TodoItem>,
    pub revision: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn design(path: &str) -> ResourceDraft {
        ResourceDraft {
            kind: ResourceKind::DesignDocument,
            title: "Architecture".into(),
            repository: "/repo".into(),
            reference: Some("topic".into()),
            path: Some(path.into()),
            url: None,
        }
    }

    #[test]
    fn identity_is_stable_across_renames() {
        let a = design("docs/design.md")
            .validated("/repo", "topic")
            .unwrap();
        let mut b = design("docs/design.md");
        b.title = "New title".into();
        assert_eq!(a.id, b.validated("/repo", "topic").unwrap().id);
    }

    #[test]
    fn paths_and_other_branches_are_rejected() {
        assert!(design("../secret").validated("/repo", "topic").is_err());
        assert!(design("docs/./secret").validated("/repo", "topic").is_err());
        assert!(design("docs//secret").validated("/repo", "topic").is_err());
        assert!(design("/tmp/file").validated("/repo", "topic").is_err());
        assert!(design("docs/design.md")
            .validated("/repo", "other")
            .is_err());
    }

    #[test]
    fn resource_wire_shape_is_flat() {
        let resource = design("docs/design.md")
            .validated("/repo", "topic")
            .unwrap();
        let value = serde_json::to_value(&resource).unwrap();
        assert_eq!(value["kind"], "design_document");
        assert_eq!(value["path"], "docs/design.md");
        assert!(value.get("data").is_none());
        let decoded: TopicResource = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.id, resource.id);
    }

    #[test]
    fn mention_accepts_webview_field_names() {
        let mention: ResourceMention = serde_json::from_value(serde_json::json!({
            "topicId": "topic", "resourceId": "design_document:docs/design.md"
        }))
        .unwrap();
        assert_eq!(mention.topic_id, "topic");
        assert_eq!(mention.resource_id, "design_document:docs/design.md");
    }
}

#[cfg(test)]
mod todo_tests {
    use super::*;

    fn list() -> TodoListView {
        TodoListView {
            todos: vec![
                TodoItem {
                    id: "a".into(),
                    text: "Review attention UI worker".into(),
                    done: false,
                    topic_id: "topic-a".into(),
                    created_at: "2026-01-01T00:00:00Z".into(),
                },
                TodoItem {
                    id: "b".into(),
                    text: "Land Arachne topic".into(),
                    done: true,
                    topic_id: "topic-b".into(),
                    created_at: "2026-01-02T00:00:00Z".into(),
                },
            ],
            revision: 7,
        }
    }

    #[test]
    fn topic_view_filters_to_one_topic() {
        let view = list().topic_view("topic-a");
        assert_eq!(view.todos.len(), 1);
        assert_eq!(view.todos[0].id, "a");
        assert_eq!(view.revision, 7);
        assert!(list().topic_view("topic-z").todos.is_empty());
    }

    #[test]
    fn todo_wire_shape_is_flat() {
        let value = serde_json::to_value(list().todos[0].clone()).unwrap();
        assert_eq!(value["topic_id"], "topic-a");
        assert!(value.get("topicId").is_none());
    }
}
