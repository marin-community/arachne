export const oid = char => char.repeat(40);
export function source(id = "worker", tags = [], parent = "track") {
  return { id, status: "running", parent_session_id: parent, parent_id: null,
    work_dir: `/repo/.worktrees/${id}`, last_activity_at: "2026-10-10T00:00:00Z",
    branch: { id: `branch-${id}`, name: id, title: id, goal: "", description: "",
      repo_root: "/repo", branch: `weaver/${id}`, tags } };
}
export function declaration(id = "first", session = source(), scope = {}) {
  return { version: 1, id, title: `Output ${id}`, source_session: session.id,
    repo_root: session.branch.repo_root, source_branch: session.branch.branch,
    source_work_dir: session.work_dir,
    scope: { kind: "range", base_oid: oid("a"), head_oid: oid("b"), paths: [], ...scope },
    completion: { channel_id: session.id, message_id: "result-1" } };
}
export const changeTag = change => ({ key: `change:${change.id}`, value: JSON.stringify(change) });
export function receipt(change, overrides = {}) {
  return { version: 1, change_id: change.id, source_session: change.source_session,
    scope: change.scope, target_session: "track", target_branch: "accepted/topic", target_oid: oid("c"),
    outcome: "accepted", completion: change.completion, ...overrides };
}
export const acceptanceTag = value => ({ key: `change_acceptance:${value.change_id}`, value: JSON.stringify(value) });
