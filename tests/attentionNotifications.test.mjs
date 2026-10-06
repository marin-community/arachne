import assert from "node:assert/strict";
import test from "node:test";
import { attentionNotices, retainActiveDismissals } from "../src/attentionNotifications.ts";

const session = (id, tags = [], extra = {}) => ({
  id, status: "running", parent_session_id: null, parent_id: null,
  last_activity_at: "2026-10-01T12:00:00Z",
  branch: { id: `branch-${id}`, title: id, name: id, description: "", tags },
  ...extra,
});
const tag = (value, note = "Choose a release target") => ({ key: "attention", value, note, set_at: "2026-10-01" });

test("idle, completed and PR metadata alone do not notify", () => {
  const pr = session("pr");
  pr.branch.github = { pr_state: "open", is_draft: false, pr_number: 1 };
  assert.deepEqual(attentionNotices([
    session("idle", [{ key: "idle", value: "true" }]), session("done", [], { status: "stopped" }),
    session("archived", [tag("blocked")], { status: "archived" }), pr,
  ]), []);
});

test("notices retain direct worker target and coordinator context, prioritize approvals", () => {
  const worker = session("worker", [tag("blocked")], { parent_session_id: "topic" });
  const approval = session("approval", [], { pending_permissions: [{ request_id: "a", title: "Write config?" }] });
  const notices = attentionNotices([session("topic"), worker, approval]);
  assert.deepEqual(notices.map((n) => n.sessionId), ["approval", "worker"]);
  assert.equal(notices[1].topicId, "topic");
  assert.equal(notices[1].title, "topic · worker");
  assert.equal(notices[1].reason, "Choose a release target");
  assert.equal(notices[1].action, "Resolve blocker");
});

test("snapshot activity preserves dismissal but changed request resurfaces", () => {
  const before = session("topic", [tag("attention")]);
  const [notice] = attentionNotices([before]);
  const hidden = new Set([notice.key]);
  const update = { ...before, last_activity_at: "2026-10-02T10:00:00Z" };
  assert.equal(attentionNotices([update])[0].key, notice.key);
  assert.equal(retainActiveDismissals(hidden, attentionNotices([update])).size, 1);
  const changed = session("topic", [tag("attention", "Review release PR")]);
  assert.equal(retainActiveDismissals(hidden, attentionNotices([changed])).size, 0);
  assert.equal(retainActiveDismissals(hidden, []).size, 0);
});

test("a fresh permission request with the same title resurfaces", () => {
  const a = session("topic", [], { pending_permissions: [{ request_id: "a", title: "Write config?" }] });
  const b = session("topic", [], { pending_permissions: [{ request_id: "b", title: "Write config?" }] });
  assert.notEqual(attentionNotices([a])[0].key, attentionNotices([b])[0].key);
});
