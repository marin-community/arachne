//! Durable Change v1: explicit bounded outputs, never inferred Thread obligations.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::loom::{SessionView, TagView};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub channel_id: String,
    pub message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChangeScope {
    Range {
        base_oid: String,
        head_oid: String,
        paths: Vec<String>,
    },
    Working {
        base_oid: String,
        head_oid: String,
        paths: Vec<String>,
        worktree_version: String,
    },
}

impl ChangeScope {
    pub fn base_oid(&self) -> &str {
        match self {
            Self::Range { base_oid, .. } | Self::Working { base_oid, .. } => base_oid,
        }
    }
    pub fn head_oid(&self) -> &str {
        match self {
            Self::Range { head_oid, .. } | Self::Working { head_oid, .. } => head_oid,
        }
    }
    pub fn paths(&self) -> &[String] {
        match self {
            Self::Range { paths, .. } | Self::Working { paths, .. } => paths,
        }
    }
    pub fn working_version(&self) -> Option<&str> {
        match self {
            Self::Working {
                worktree_version, ..
            } => Some(worktree_version),
            _ => None,
        }
    }
    pub fn is_range(&self) -> bool {
        matches!(self, Self::Range { .. })
    }
    pub fn validate(&self) -> Result<(), String> {
        if !valid_oid(self.base_oid())
            || !valid_oid(self.head_oid())
            || self.base_oid().len() != self.head_oid().len()
        {
            return Err("Change scope requires full Git object IDs of the same format.".into());
        }
        let mut seen = std::collections::HashSet::new();
        if self
            .paths()
            .iter()
            .any(|p| !valid_path(p) || !seen.insert(p))
        {
            return Err("Change paths must be unique repository-relative literal paths.".into());
        }
        if let Some(version) = self.working_version() {
            if !valid_text(version) || self.paths().is_empty() {
                return Err(
                    "Working Change requires nonempty paths and a pinned worktree version.".into(),
                );
            }
        }
        Ok(())
    }
}

pub fn valid_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

// Match frontend String.trim whitespace for validation only; never rewrite text
// or literal paths. ASCII C0 controls and DEL are unsupported in v1 metadata.
fn valid_text(value: &str) -> bool {
    !value
        .trim_matches(|c| {
            matches!(c,
                '\u{9}'..='\u{d}' | ' ' | '\u{a0}' | '\u{1680}' |
                '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' |
                '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
            )
        })
        .is_empty()
        && !value.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}

fn valid_path(path: &str) -> bool {
    valid_text(path)
        && !path.contains('\\')
        && !path.starts_with('/')
        && !path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == ".." || p.eq_ignore_ascii_case(".git"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub version: u32,
    pub id: String,
    pub title: String,
    pub source_session: String,
    pub repo_root: String,
    pub source_branch: String,
    pub source_work_dir: String,
    pub scope: ChangeScope,
    #[serde(deserialize_with = "required_completion")]
    pub completion: Option<Completion>,
}

fn required_completion<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Completion>, D::Error> {
    Option::<Completion>::deserialize(deserializer)
}

impl Change {
    /// Inline final declarations have null completion because results cannot self-reference.
    pub fn validate(&self, inline: bool) -> Result<(), String> {
        if self.version != 1
            || !valid_id(&self.id)
            || !valid_text(&self.title)
            || !valid_text(&self.source_session)
            || !self.repo_root.starts_with('/')
            || !self.source_work_dir.starts_with('/')
            || !valid_text(&self.repo_root)
            || !valid_text(&self.source_work_dir)
            || !crate::track_policy::valid_topic_branch(&self.source_branch)
        {
            return Err("Invalid Change declaration identity or version.".into());
        }
        self.scope.validate()?;
        match (&self.completion, self.scope.is_range(), inline) {
            (None, _, true) | (None, false, false) => Ok(()),
            (Some(c), true, false) if c.channel_id == self.source_session && valid_text(&c.message_id) => Ok(()),
            _ => Err("Change completion must reference the source own-channel result; working/inline completion must be null.".into()),
        }
    }
    /// The sole declaration transition authorized by a selected working Land.
    /// Git reachability and fresh exact scope review remain required on the host.
    pub fn verify_working_finalization(&self, finalized: &Change) -> Result<(), String> {
        self.validate(false)?;
        finalized.validate(false)?;
        if self.scope.working_version().is_none()
            || !finalized.scope.is_range()
            || self.id != finalized.id
            || self.title != finalized.title
            || self.source_session != finalized.source_session
            || self.repo_root != finalized.repo_root
            || self.source_branch != finalized.source_branch
            || self.source_work_dir != finalized.source_work_dir
            || self.scope.base_oid() != finalized.scope.base_oid()
            || self.scope.paths() != finalized.scope.paths()
        {
            return Err("Working finalization changed original identity or bounded scope.".into());
        }
        Ok(())
    }

    pub fn verify_source(&self, source: &SessionView) -> Result<(), String> {
        self.validate(false)?;
        if self.source_session != source.id
            || self.repo_root != source.branch.repo_root
            || self.source_branch != source.branch.branch
            || self.source_work_dir != source.work_dir
        {
            return Err(
                "Original Change session/repository/branch/worktree identity changed; reselect."
                    .into(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeAcceptance {
    pub version: u32,
    pub change_id: String,
    pub source_session: String,
    pub scope: ChangeScope,
    pub target_session: String,
    pub target_branch: String,
    pub target_oid: String,
    pub outcome: String,
    pub completion: Completion,
}

impl ChangeAcceptance {
    pub fn verify(&self, change: &Change) -> Result<(), String> {
        if self.version != 1
            || self.outcome != "accepted"
            || self.change_id != change.id
            || self.source_session != change.source_session
            || self.scope != change.scope
            || Some(&self.completion) != change.completion.as_ref()
            || !change.scope.is_range()
            || !valid_text(&self.target_session)
            || !crate::track_policy::valid_topic_branch(&self.target_branch)
            || !valid_oid(&self.target_oid)
            || self.target_oid.len() != change.scope.head_oid().len()
        {
            return Err(
                "Invalid or mismatched Change acceptance receipt; reconcile before reacceptance."
                    .into(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct ChangeRegistry {
    pub changes: BTreeMap<String, Change>,
    pub acceptances: BTreeMap<String, ChangeAcceptance>,
}

fn insert_exact<T: PartialEq>(
    map: &mut BTreeMap<String, T>,
    id: &str,
    value: T,
) -> Result<(), String> {
    if let Some(previous) = map.get(id) {
        if previous != &value {
            return Err("Conflicting duplicate Change metadata.".into());
        }
    } else {
        map.insert(id.into(), value);
    }
    Ok(())
}

impl ChangeRegistry {
    pub fn parse(tags: &[TagView], source: &SessionView) -> Result<Self, String> {
        let mut registry = Self::default();
        for tag in tags {
            if let Some(id) = tag.key.strip_prefix("change:") {
                let change: Change = serde_json::from_str(&tag.value)
                    .map_err(|e| format!("Malformed Change tag: {e}"))?;
                if !valid_id(id) || change.id != id {
                    return Err("Change tag ID differs from declaration.".into());
                }
                change.verify_source(source)?;
                insert_exact(&mut registry.changes, id, change)?;
            } else if let Some(id) = tag.key.strip_prefix("change_acceptance:") {
                let receipt: ChangeAcceptance = serde_json::from_str(&tag.value)
                    .map_err(|e| format!("Malformed Change receipt: {e}"))?;
                if !valid_id(id) || receipt.change_id != id {
                    return Err("Receipt tag ID differs from Change.".into());
                }
                insert_exact(&mut registry.acceptances, id, receipt)?;
            }
        }
        for (id, receipt) in &registry.acceptances {
            receipt.verify(
                registry
                    .changes
                    .get(id)
                    .ok_or("Receipt has no original declaration; reconcile history.")?,
            )?;
        }
        Ok(registry)
    }
    pub fn select(&self, id: &str, expected: &Change) -> Result<&Change, String> {
        let change = self
            .changes
            .get(id)
            .ok_or("Selected Change declaration is unavailable.")?;
        if change != expected || expected.id != id {
            return Err(
                "Selected declaration changed; reselect and review the fresh scope.".into(),
            );
        }
        if self.acceptances.contains_key(id) {
            return Err("Selected Change is already accepted.".into());
        }
        Ok(change)
    }
}

/// Exact real Loom channels.messages.list DTO fields needed for authenticated evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultMessage {
    pub id: String,
    pub channel_id: String,
    pub seq: i64,
    pub kind: String,
    pub author_kind: String,
    pub author_id: String,
    pub body: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct ChangeResult {
    pub version: u32,
    pub outcome: String,
    pub intent: String,
    pub changes: Vec<Change>,
}

/// Validates all inline outputs, deduplicating identical IDs and rejecting conflicts.
pub fn explicit_result_outputs(
    message: &ResultMessage,
    source: &SessionView,
) -> Result<Vec<Change>, String> {
    if message.kind != "result"
        || message.author_kind != "session"
        || message.author_id != source.id
        || message.channel_id != source.id
        || message.id.trim().is_empty()
    {
        return Err("Completion is not an authenticated source own-channel result.".into());
    }
    let body = serde_json::from_str::<ChangeResult>(&message.body);
    let payload = serde_json::from_value::<ChangeResult>(message.payload.clone());
    let result = match (body, payload) {
        (Ok(body), Ok(payload)) => {
            if body.version != payload.version
                || body.outcome != payload.outcome
                || body.intent != payload.intent
                || body.changes != payload.changes
            {
                return Err("Conflicting result body and structured payload.".into());
            }
            body
        }
        (Ok(result), Err(_)) | (Err(_), Ok(result)) => result,
        _ => return Err("Completion has no Durable Change result body or payload.".into()),
    };
    if result.version != 1 || result.outcome != "complete" || result.intent != "final" {
        return Err("Only complete final intended results qualify.".into());
    }
    let mut outputs = BTreeMap::new();
    for inline in result.changes {
        inline.validate(true)?;
        if !inline.scope.is_range() {
            return Err("Final result must declare immutable ranges.".into());
        }
        let mut registered = inline.clone();
        registered.completion = Some(Completion {
            channel_id: message.channel_id.clone(),
            message_id: message.id.clone(),
        });
        registered.verify_source(source)?;
        insert_exact(&mut outputs, &inline.id, registered)?;
    }
    Ok(outputs.into_values().collect())
}

pub fn verify_completion(
    change: &Change,
    message: &ResultMessage,
    source: &SessionView,
) -> Result<(), String> {
    let completion = change
        .completion
        .as_ref()
        .ok_or("Working scope is request identity, not completion.")?;
    if completion.message_id != message.id || completion.channel_id != message.channel_id {
        return Err("Original completion reference not found.".into());
    }
    if !explicit_result_outputs(message, source)?
        .iter()
        .any(|output| output == change)
    {
        return Err("Completion does not contain the exact selected declaration and scope.".into());
    }
    Ok(())
}

/// Shared selected-output instructions for manual Land and result-driven autoland.
pub const SELECTED_ACCEPTANCE_GUARD: &str = "Accept ONLY this explicit Change's pinned base_oid to head_oid delta and literal paths; empty paths mean that exact range, never the cumulative branch. No siblings, ancestors, already accepted history or unrelated checkout edits may leak into any strategy. Squash is default; merge/rebase/cherry-pick require proof they include only the selected delta; otherwise stop for a scope/strategy decision. Preserve original source history. Read the ORIGINAL authenticated own-channel kind=result, author_kind=session, author_id=source_session, exact completion.channel_id/message_id. Require JSON body or structured payload {version:1,outcome:\"complete\",intent:\"final\",changes:[inline range declarations with completion:null]} containing the exact selected ID, title, original source identity and scope; complete+final ONLY. Linked notices are pointers, not evidence; missing original reference in bounded history fails closed. Working HEAD/version/paths are request identity, not readiness: finish only that bounded output, retain its ID and original identity, finalize to a range and fresh completion, and require fresh exact scope review within the same Land request before accepting. Never silently substitute a changed declaration. Immediately before EVERY acceptance write reread live owning same-repository Track, canonical ref/revision and initialization, original session/repository/branch/worktree, exact selected declaration/scope/completion and change_acceptance:<id>. Refresh review on changed target; changed selected declaration requires reselection. Malformed/conflicting/mismatched receipt blocks until reconciled; exact accepted receipt is an idempotent no-op, never replay. Repeated historical completions must never manufacture declarations, new pending deltas or retask historical sources. integration_result is historical compatibility evidence only, never accepts all outputs or blocks a later new ID/range. Verify exact already-contained/no-op delta with containment evidence and validation. ONLY after actual canonical acceptance and validation stamp change_acceptance:<id> on ORIGINAL source with JSON {version:1,change_id,source_session,scope,target_session,target_branch,target_oid,outcome:\"accepted\",completion}; match exact scope/completion, keep historical records, never update siblings or stamp integration_result for selected acceptance.";
