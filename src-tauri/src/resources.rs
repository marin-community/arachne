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
/// The repo-shared store holding each Project's resource bindings, keyed
/// by the project's layout group id (design.md "Project defaults and
/// resource inheritance"). Repo-shared scope falls out per repository, so
/// a topic can only inherit bindings from its own repo — the same rule the
/// topic manifest enforces ("resource repository must match the topic").
pub const PROJECTS_NAME: &str = "arachne-projects";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Repository,
    Worktree,
    PullRequest,
    Issue,
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
            Self::Issue => "issue",
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
    /// Project binding keys this topic opted out of (design.md: a Topic can
    /// hide an inherited resource without deleting it from the Project).
    /// Reference-free keys, matched by [`binding_key`] — never resource ids,
    /// which are reference-scoped for files.
    #[serde(default)]
    pub hidden: Vec<String>,
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

/// Reference-free identity for matching project↔topic bindings: the
/// backing object (repository + path, or the GitHub URL), never the mutable
/// reference. A project binding retargets its file reference per topic
/// (inheritance grants context, not a pinned foreign branch), so overrides
/// and hides must match on something the retargeting cannot change.
pub fn binding_key(draft: &ResourceDraft) -> String {
    match draft.kind {
        ResourceKind::PullRequest | ResourceKind::Issue => format!(
            "{}:{}",
            draft.kind.as_str(),
            draft.url.as_deref().unwrap_or("")
        ),
        ResourceKind::Repository => {
            format!("{}:{}", draft.kind.as_str(), draft.repository)
        }
        _ => format!(
            "{}:{}:{}",
            draft.kind.as_str(),
            draft.repository,
            draft.path.as_deref().unwrap_or("")
        ),
    }
}

impl TopicResource {
    /// The reference-free binding key of this resource.
    pub fn binding_key(&self) -> String {
        binding_key(&self.data)
    }
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
        // A repository binding points at its OWN repo — often another one
        // (design.md: a project can bind multiple repositories), so only
        // the file-backed kinds are pinned to the topic's repo.
        let repo_must_match = !matches!(self.kind, ResourceKind::Repository);
        if repo_must_match && self.repository != topic_repo {
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
            ResourceKind::PullRequest | ResourceKind::Issue => {
                let url = self.url.as_deref().ok_or("a github.com URL is required")?;
                if !(url.starts_with("https://github.com/")
                    || url.starts_with("https://www.github.com/"))
                {
                    return Err("the URL must be on github.com".into());
                }
            }
            ResourceKind::Repository => {
                if self.repository.is_empty() {
                    return Err("repository binding needs a repository".into());
                }
            }
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
        Ok(TopicResource {
            id: self.locator_id(),
            data: self,
        })
    }

    /// The resource's id: the backing object's locator, prefixed by kind.
    /// Inherited rows compute the id a topic's own attach of the same
    /// object would produce (mentions survive overrides).
    pub fn locator_id(&self) -> String {
        let locator = match self.kind {
            ResourceKind::Repository => self.repository.clone(),
            // PRs and issues are their GitHub URL — stable identity across
            // checkouts, sessions, and who attached them.
            ResourceKind::PullRequest | ResourceKind::Issue => self.url.clone().unwrap(),
            _ => format!(
                "{}:{}:{}",
                self.repository,
                self.reference.as_deref().unwrap_or(""),
                self.path.as_deref().unwrap_or("")
            ),
        };
        format!("{}:{locator}", self.kind.as_str())
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

// --- Projects ---------------------------------------------------------------

/// Where a row of the effective view came from (design.md: show each
/// binding's origin so the user can tell what came from the Project, the
/// Topic, or a Thread).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceOrigin {
    Project,
    Topic,
}

/// One row of a topic's effective resources: the topic's own binding or an
/// inherited project binding (reference retargeted to the topic's branch).
/// `hidden` is true only for project bindings this topic opted out of —
/// they stay in the view for the restore affordance but never resolve for
/// reads or mentions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveResource {
    #[serde(flatten)]
    pub resource: TopicResource,
    pub origin: ResourceOrigin,
    #[serde(default)]
    pub hidden: bool,
}

/// One Project resource binding (design.md "Project defaults and resource
/// inheritance"). Projects are layout groups; their bindings live in the
/// repo-shared `arachne-projects` artifact of the repository they point
/// at, keyed by group id — so a topic only ever inherits bindings from its
/// own repository, the same rule the topic manifest enforces. `id` is the
/// reference-free [`binding_key`]: inheritance matches on the backing
/// object, and a project binding's file reference is retargeted per topic
/// (inheritance grants context, not a pinned foreign branch).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectBinding {
    pub id: String,
    #[serde(flatten)]
    pub data: ResourceDraft,
    #[serde(default)]
    pub created_at: String,
}

impl ProjectBinding {
    /// Validate a draft as a project binding for `repo_root`'s store. The
    /// same rules as a topic resource, except the file reference: a
    /// project binding may record a default ref, but it is retargeted to
    /// each topic's branch at merge time, so no branch constraint applies.
    pub fn validated_for_project(
        mut draft: ResourceDraft,
        repo_root: &str,
    ) -> Result<Self, String> {
        draft.title = draft.title.trim().to_owned();
        draft.repository = draft.repository.trim().to_owned();
        draft.reference = draft
            .reference
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        draft.path = draft
            .path
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        draft.url = draft
            .url
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        if draft.title.is_empty() || draft.title.len() > 256 {
            return Err("resource title must contain 1–256 characters".into());
        }
        // A repository binding points at its own repo — the point of the
        // kind is binding ANOTHER repo (design.md: multiple repositories
        // per project) — so it is exempt from the host-repo pin; the
        // command layer stamps its canonical root so identity is stable.
        let repo_must_match = !matches!(draft.kind, ResourceKind::Repository);
        if repo_must_match && draft.repository != repo_root {
            return Err("resource repository must match the project repository".into());
        }
        match draft.kind {
            ResourceKind::File | ResourceKind::DesignDocument => {
                let path = draft.path.as_deref().ok_or("file path is required")?;
                validate_relative_path(path)?;
            }
            ResourceKind::PullRequest | ResourceKind::Issue => {
                let url = draft.url.as_deref().ok_or("a github.com URL is required")?;
                if !(url.starts_with("https://github.com/")
                    || url.starts_with("https://www.github.com/"))
                {
                    return Err("the URL must be on github.com".into());
                }
            }
            ResourceKind::Repository => {
                if draft.repository.is_empty() {
                    return Err("repository binding needs a repository".into());
                }
            }
            ResourceKind::Worktree => {
                if draft.path.is_none() {
                    return Err("worktree path is required".into());
                }
            }
            ResourceKind::Artifact => {
                if draft.path.is_none() {
                    return Err("artifact name is required in path".into());
                }
            }
        }
        Ok(Self {
            id: binding_key(&draft),
            data: draft,
            created_at: String::new(),
        })
    }

    /// The binding as one topic's row: file references retarget to that
    /// topic's own branch — inheritance grants context, not a pinned foreign
    /// branch. The row's id is exactly what attaching the same file would
    /// produce, so a mention of the inherited row keeps resolving after the
    /// topic attaches its own copy (the override).
    pub fn as_topic_resource(&self, topic_branch: &str) -> TopicResource {
        let mut data = self.data.clone();
        if matches!(data.kind, ResourceKind::File | ResourceKind::DesignDocument) {
            data.reference = Some(topic_branch.to_owned());
        }
        TopicResource {
            id: data.locator_id(),
            data,
        }
    }
}

/// The repo-shared `arachne-projects` store: every project's bindings for
/// one repository, keyed by layout group id. One artifact per repo → one
/// revision (the `arachne-todos` pattern), so concurrent Mac/agent edits to
/// any project's bindings in a repo serialize through `base_rev`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectsStore {
    #[serde(default)]
    pub projects: std::collections::BTreeMap<String, Vec<ProjectBinding>>,
    #[serde(default)]
    pub revision: i64,
}

/// The topic's effective view plus the project it inherits from (design.md
/// "Project defaults and resource inheritance"): a live merge, never a
/// copy — project bindings keep supplying shared references to existing
/// topics while the topic's own additions, overrides, and hides apply.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TopicEffectiveView {
    /// The effective rows (topic's own + un-hidden project bindings, topic
    /// overrides winning) — what reads resolve and mentions offer.
    #[serde(default)]
    pub resources: Vec<EffectiveResource>,
    /// Project bindings this topic hid, kept for the restore affordance.
    #[serde(default)]
    pub hidden_resources: Vec<EffectiveResource>,
    /// The topic manifest's revision (revision-checked mutations).
    #[serde(default)]
    pub revision: i64,
    /// The topic's home project, when it has one: even a project with no
    /// bindings is reported so the panel can offer attaching to it.
    #[serde(default)]
    pub project: Option<ProjectInfo>,
}

/// The slice of the project's store the panel needs: identity for labels
/// and the store revision for revision-checked binding edits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub revision: i64,
}

/// Merge a project's bindings into a topic's effective resources
/// (design.md: **Project resources → Topic additions and overrides →
/// hides**). Topic rows always win over a project binding with the same
/// backing object (an override); hidden project bindings stay in the list
/// flagged `hidden` so the UI can offer restore, but they are not effective.
/// File references retarget to the topic's branch.
pub fn merge_effective(
    bindings: &[ProjectBinding],
    topic: &TopicResourcesView,
    topic_branch: &str,
) -> Vec<EffectiveResource> {
    let mut claimed = std::collections::HashSet::new();
    let mut rows: Vec<EffectiveResource> = topic
        .resources
        .iter()
        .map(|resource| EffectiveResource {
            resource: resource.clone(),
            origin: ResourceOrigin::Topic,
            hidden: false,
        })
        .collect();
    for resource in &topic.resources {
        claimed.insert(resource.binding_key());
    }
    let hidden: std::collections::HashSet<&str> = topic.hidden.iter().map(String::as_str).collect();
    for binding in bindings {
        // A topic row with the same backing object overrides the project's.
        if !claimed.insert(binding.id.clone()) {
            continue;
        }
        rows.push(EffectiveResource {
            resource: binding.as_topic_resource(topic_branch),
            origin: ResourceOrigin::Project,
            hidden: hidden.contains(binding.id.as_str()),
        });
    }
    rows
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
    fn issue_and_pr_identity_is_their_github_url() {
        let issue = ResourceDraft {
            kind: ResourceKind::Issue,
            title: "Panel misses issues".into(),
            repository: "/repo".into(),
            reference: None,
            path: None,
            url: Some("https://github.com/acme/app/issues/12".into()),
        }
        .validated("/repo", "topic")
        .unwrap();
        assert_eq!(issue.id, "issue:https://github.com/acme/app/issues/12");
        let pr = ResourceDraft {
            kind: ResourceKind::PullRequest,
            title: "Panel misses issues".into(),
            repository: "/repo".into(),
            reference: None,
            path: None,
            url: Some("https://github.com/acme/app/pull/13".into()),
        }
        .validated("/repo", "topic")
        .unwrap();
        assert_eq!(pr.id, "pull_request:https://github.com/acme/app/pull/13");
        // Same URL, same identity regardless of kind label noise.
        let mut renamed = issue.data.clone();
        renamed.title = "Renamed".into();
        assert_eq!(renamed.validated("/repo", "topic").unwrap().id, issue.id);
    }

    #[test]
    fn issue_urls_must_be_github() {
        let bad = ResourceDraft {
            kind: ResourceKind::Issue,
            title: "Offsite".into(),
            repository: "/repo".into(),
            reference: None,
            path: None,
            url: Some("https://example.com/12".into()),
        };
        assert!(bad.validated("/repo", "topic").is_err());
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
mod project_tests {
    use super::*;

    fn binding(kind: ResourceKind, path: &str, reference: Option<&str>) -> ProjectBinding {
        ProjectBinding::validated_for_project(
            ResourceDraft {
                kind,
                title: format!("{kind:?}"),
                repository: "/repo".into(),
                reference: reference.map(str::to_owned),
                path: Some(path.into()),
                url: None,
            },
            "/repo",
        )
        .unwrap()
    }

    fn issue_binding(url: &str) -> ProjectBinding {
        ProjectBinding::validated_for_project(
            ResourceDraft {
                kind: ResourceKind::Issue,
                title: "Panel misses issues".into(),
                repository: "/repo".into(),
                reference: None,
                path: None,
                url: Some(url.into()),
            },
            "/repo",
        )
        .unwrap()
    }

    fn topic_with(resources: Vec<ResourceDraft>, hidden: Vec<&str>) -> TopicResourcesView {
        TopicResourcesView {
            resources: resources
                .into_iter()
                .map(|draft| draft.validated("/repo", "topic").unwrap())
                .collect(),
            hidden: hidden.into_iter().map(str::to_owned).collect(),
            revision: 3,
        }
    }

    #[test]
    fn project_binding_id_is_the_reference_free_key() {
        let a = binding(
            ResourceKind::DesignDocument,
            "docs/design.md",
            Some("weaver/old"),
        );
        let b = binding(ResourceKind::DesignDocument, "docs/design.md", None);
        // Same backing object (kind + repo + path): the same identity,
        // regardless of the recorded default reference.
        assert_eq!(a.id, b.id);
        let other = binding(ResourceKind::File, "docs/design.md", None);
        assert_ne!(a.id, other.id);
        assert!(a.id.starts_with("design_document:"));
    }

    #[test]
    fn merge_inherits_and_retargets_file_references() {
        let bindings = [binding(
            ResourceKind::DesignDocument,
            "docs/design.md",
            Some("weaver/stale"),
        )];
        let rows = merge_effective(&bindings, &topic_with(vec![], vec![]), "topic");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].origin, ResourceOrigin::Project);
        // Inheritance grants context, not a pinned foreign branch: the file
        // reference is the topic's own branch.
        assert_eq!(rows[0].resource.data.reference.as_deref(), Some("topic"));
    }

    #[test]
    fn merge_topic_row_overrides_project_binding() {
        let bindings = [binding(
            ResourceKind::DesignDocument,
            "docs/design.md",
            None,
        )];
        let topic = topic_with(
            vec![ResourceDraft {
                kind: ResourceKind::DesignDocument,
                title: "Topic's own copy".into(),
                repository: "/repo".into(),
                reference: Some("topic".into()),
                path: Some("docs/design.md".into()),
                url: None,
            }],
            vec![],
        );
        let rows = merge_effective(&bindings, &topic, "topic");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].origin, ResourceOrigin::Topic);
        assert_eq!(rows[0].resource.data.title, "Topic's own copy");
    }

    #[test]
    fn merge_hidden_project_binding_stays_but_flags_hidden() {
        let bindings = [binding(
            ResourceKind::DesignDocument,
            "docs/design.md",
            None,
        )];
        let topic = topic_with(vec![], vec![bindings[0].id.as_str()]);
        let rows = merge_effective(&bindings, &topic, "topic");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].origin, ResourceOrigin::Project);
        assert!(rows[0].hidden);
    }

    #[test]
    fn hidden_binding_not_removed_after_topic_attaches_same_object() {
        // A hide is not an override: once the topic attaches its own copy of
        // the same backing object, the binding no longer appears (topic wins),
        // and the stale hide entry is simply irrelevant.
        let bindings = [binding(
            ResourceKind::DesignDocument,
            "docs/design.md",
            None,
        )];
        let key = bindings[0].id.clone();
        let topic = topic_with(
            vec![ResourceDraft {
                kind: ResourceKind::DesignDocument,
                title: "Attached".into(),
                repository: "/repo".into(),
                reference: Some("topic".into()),
                path: Some("docs/design.md".into()),
                url: None,
            }],
            vec![key.as_str()],
        );
        let rows = merge_effective(&bindings, &topic, "topic");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].origin, ResourceOrigin::Topic);
        assert!(!rows[0].hidden);
    }

    #[test]
    fn inherited_row_has_the_id_a_topic_attach_would_produce() {
        // A mention of the inherited row must keep resolving even after the
        // topic attaches its own copy — the override carries the same id.
        let bindings = [binding(
            ResourceKind::DesignDocument,
            "docs/design.md",
            None,
        )];
        let rows = merge_effective(&bindings, &topic_with(vec![], vec![]), "topic");
        let inherited_id = rows[0].resource.id.clone();
        let attached = ResourceDraft {
            kind: ResourceKind::DesignDocument,
            title: "Attached".into(),
            repository: "/repo".into(),
            reference: Some("topic".into()),
            path: Some("docs/design.md".into()),
            url: None,
        }
        .validated("/repo", "topic")
        .unwrap();
        assert_eq!(inherited_id, attached.id);
    }

    #[test]
    fn issue_url_bindings_keep_github_identity() {
        let bindings = [issue_binding("https://github.com/acme/app/issues/12")];
        let rows = merge_effective(&bindings, &topic_with(vec![], vec![]), "topic");
        assert_eq!(
            rows[0].resource.id,
            "issue:https://github.com/acme/app/issues/12"
        );
        assert!(ProjectBinding::validated_for_project(
            ResourceDraft {
                kind: ResourceKind::Issue,
                title: "Offsite".into(),
                repository: "/repo".into(),
                reference: None,
                path: None,
                url: Some("https://example.com/12".into()),
            },
            "/repo",
        )
        .is_err());
    }

    #[test]
    fn store_round_trips_through_json() {
        let mut store = ProjectsStore::default();
        store.projects.insert(
            "grp".into(),
            vec![binding(ResourceKind::File, "a.md", None)],
        );
        store.revision = 9;
        let value = serde_json::to_value(&store).unwrap();
        assert_eq!(value["projects"]["grp"][0]["title"], "File");
        let decoded: ProjectsStore = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.revision, 9);
        assert_eq!(decoded.projects.len(), 1);
        assert!(decoded.projects.contains_key("grp"));
    }

    // --- Repository bindings (design.md: a project can bind multiple
    //     repositories) --------------------------------------------------

    fn repo_binding(root: &str) -> ProjectBinding {
        ProjectBinding::validated_for_project(
            ResourceDraft {
                kind: ResourceKind::Repository,
                title: root.rsplit('/').next().unwrap().into(),
                repository: root.into(),
                reference: None,
                path: None,
                url: None,
            },
            "/host", // the store's host repo — different from the binding's
        )
        .unwrap()
    }

    #[test]
    fn repository_bindings_may_point_at_other_repos() {
        // The point of the kind: a project in one repo binds other repos.
        let loom = repo_binding("/repos/marin-community/loom");
        let arachne = repo_binding("/repos/marin-community/arachne");
        assert_eq!(loom.id, "repository:/repos/marin-community/loom");
        assert_ne!(loom.id, arachne.id);
        // Multiple repositories coexist in one project's store.
        let rows = merge_effective(
            &[loom.clone(), arachne],
            &topic_with(vec![], vec![]),
            "topic",
        );
        assert_eq!(rows.len(), 2);
        // And the merge does not retarget anything (no file reference).
        assert_eq!(rows[0].resource.data.repository, "/repos/marin-community/loom");
    }

    #[test]
    fn repository_binding_overrides_by_root_and_unhides_on_attach() {
        let loom = repo_binding("/repos/marin-community/loom");
        // A topic attach of the same repo overrides the inherited row.
        let attached = ResourceDraft {
            kind: ResourceKind::Repository,
            title: "Loom".into(),
            repository: "/repos/marin-community/loom".into(),
            reference: None,
            path: None,
            url: None,
        }
        .validated("/repos/marin-community/arachne", "topic")
        .unwrap();
        assert_eq!(attached.id, loom.as_topic_resource("topic").id);
        // A hide keyed by the binding key covers the inherited row.
        let rows = merge_effective(&[loom], &topic_with(vec![], vec![&attached.binding_key()]), "topic");
        assert_eq!(rows[0].hidden, true);
    }

    #[test]
    fn repository_binding_requires_a_repo() {
        assert!(ProjectBinding::validated_for_project(
            ResourceDraft {
                kind: ResourceKind::Repository,
                title: "Nowhere".into(),
                repository: "  ".into(),
                reference: None,
                path: None,
                url: None,
            },
            "/host",
        )
        .is_err());
        // And topic-side: a repository attach still validates with a repo
        // that differs from the topic's own.
        assert!(ResourceDraft {
            kind: ResourceKind::Repository,
            title: "Loom".into(),
            repository: "/repos/marin-community/loom".into(),
            reference: None,
            path: None,
            url: None,
        }
        .validated("/repos/marin-community/arachne", "topic")
        .is_ok());
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
