//! Durable topic resource bindings. The manifest is a branch-scoped Loom
//! artifact, so it follows the topic's accepted branch rather than this Mac's
//! app installation or a particular session process.

use serde::{Deserialize, Serialize};

pub const MANIFEST_NAME: &str = "arachne-resources";

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
        || path.split('/').any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("path must be relative to the repository without . or ..".into());
    }
    Ok(())
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
}
