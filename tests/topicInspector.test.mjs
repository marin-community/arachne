import assert from "node:assert/strict";
import test from "node:test";
import {
  badgeLabel,
  dismissibleAttention,
  integrationCandidates,
  isIdle,
  isReadyCandidate,
  loudTag,
  pendingPermissionSummary,
  statusClass,
  topicThreadRows,
} from "../src/topicInspector.ts";

const session = (id, overrides = {}) => ({
  id,
  status: "running",
  parent_session_id: null,
  parent_id: null,
  last_activity_at: "2026-01-01T00:00:00Z",
  branch: {
    id: `branch-${id}`,
    branch: `weaver/${id}`,
    name: id,
    title: `Title ${id}`,
    description: "",
    goal: "",
    repo_root: "/repo",
    tags: [],
  },
  ...overrides,
});

const withTag = (s, key, value, note = "") => ({
  ...s,
  branch: { ...s.branch, tags: [...s.branch.tags, { key, value, note, set_at: "", set_by: "arachne" }] },
});

test("topic thread rows root at the topic and nest workers at depth", () => {
  const fleet = [
    session("topic"),
    session("worker-a", { parent_session_id: "topic" }),
    session("worker-b", { parent_session_id: "worker-a" }),
    session("other-topic"),
    session("other-worker", { parent_session_id: "other-topic" }),
  ];
  const rows = topicThreadRows(fleet, "topic", new Set());
  assert.deepEqual(rows.map((r) => r.session.id), ["topic", "worker-a", "worker-b"]);
  assert.deepEqual(rows.map((r) => r.depth), [0, 1, 2]);
  assert.deepEqual(rows.map((r) => r.childCount), [1, 1, 0]);
});

test("collapse hides a subtree; archived workers stay in the tree", () => {
  const fleet = [
    session("topic"),
    session("worker-a", { parent_session_id: "topic" }),
    session("worker-b", { parent_session_id: "worker-a", status: "archived" }),
  ];
  const collapsed = topicThreadRows(fleet, "topic", new Set(["worker-a"]));
  assert.deepEqual(collapsed.map((r) => r.session.id), ["topic", "worker-a"]);
  assert.equal(collapsed[1].childCount, 1); // count survives collapsing
  const open = topicThreadRows(fleet, "topic", new Set());
  assert.deepEqual(open.map((r) => r.session.id), ["topic", "worker-a", "worker-b"]);
});

test("children sort by recency and cycles cannot loop the walk", () => {
  const fleet = [
    session("topic"),
    session("old", { parent_session_id: "topic", last_activity_at: "2026-01-01T00:00:00Z" }),
    session("new", { parent_session_id: "topic", last_activity_at: "2026-02-01T00:00:00Z" }),
  ];
  const rows = topicThreadRows(fleet, "topic", new Set());
  assert.deepEqual(rows.slice(1).map((r) => r.session.id), ["new", "old"]);
  // A self-parent and a two-node cycle must not hang or duplicate.
  const selfParent = [session("topic", { parent_session_id: "topic" })];
  assert.deepEqual(topicThreadRows(selfParent, "topic", new Set()).map((r) => r.session.id), ["topic"]);
  const cyc = [
    session("topic"),
    session("a", { parent_session_id: "b" }),
    session("b", { parent_session_id: "a" }),
  ];
  assert.deepEqual(topicThreadRows(cyc, "topic", new Set()).map((r) => r.session.id), ["topic"]);
});

test("row states: working spins, resting is quiet, loud tags keep badges", () => {
  const working = session("w");
  assert.equal(badgeLabel(working), null); // running → spinner
  assert.equal(isIdle(working), false);
  const resting = withTag(session("r"), "idle", "true");
  assert.equal(isIdle(resting), true);
  assert.equal(badgeLabel(resting), null); // still quiet
  const needsAttention = withTag(session("a"), "attention", "attention");
  assert.deepEqual(loudTag(needsAttention), { level: "attention" });
  assert.equal(badgeLabel(needsAttention), "attention");
  assert.equal(statusClass(needsAttention), "attention");
  const approval = session("approval", { pending_permissions: [
    { request_id: "req-1", title: "Allow Computer Use?" },
    { request_id: "req-2", title: "Allow file access?" },
  ] });
  assert.deepEqual(loudTag(approval), { level: "attention" });
  assert.equal(badgeLabel(approval), "attention");
  assert.equal(pendingPermissionSummary(approval), "2 tool approvals: Allow Computer Use? · Allow file access?");
  const blocked = withTag(session("b"), "triage", "blocked");
  assert.equal(statusClass(blocked), "error");
  const triage = withTag(session("t"), "triage", "ok");
  assert.equal(loudTag(triage), null); // calm value on a loud key is quiet
  const stopped = session("stopped", { status: "stopped" });
  assert.equal(badgeLabel(stopped), "stopped");
  const archived = session("arch", { status: "archived" });
  assert.equal(badgeLabel(archived), "done");
});

test("dismissal: tag-raised attention only, never permissions or archived", () => {
  // Both loud keys and both loud values are dismissable.
  assert.equal(dismissibleAttention(withTag(session("a"), "attention", "attention")), true);
  assert.equal(dismissibleAttention(withTag(session("b"), "attention", "blocked")), true);
  assert.equal(dismissibleAttention(withTag(session("t"), "triage", "blocked")), true);
  // A calm value on a loud key is quiet — nothing to dismiss.
  assert.equal(dismissibleAttention(withTag(session("c"), "attention", "ok")), false);
  // Unanswered tool approvals raise attention but are NOT dismissable.
  const approval = session("approval", { pending_permissions: [
    { request_id: "req-1", title: "Allow Computer Use?" },
  ] });
  assert.equal(loudTag(approval)?.level, "attention");
  assert.equal(dismissibleAttention(approval), false);
  // Quiet rows and archived rows never offer the dismiss.
  assert.equal(dismissibleAttention(session("plain")), false);
  assert.equal(dismissibleAttention(session("idle", { status: "archived" })), false);
});

test("readiness requires explicit verified candidate state, not lifecycle", () => {
  const sleeping = withTag(session("s"), "idle", "true");
  const stopped = session("stopped", { status: "stopped" });
  const archived = session("arch", { status: "archived" });
  for (const s of [sleeping, stopped, archived]) {
    assert.equal(isReadyCandidate(s), false, `${s.status} must not imply readiness`);
  }
  const readyTag = withTag(session("r1"), "integration_ready", "true");
  const readyState = withTag(session("r2"), "integration_state", "ready");
  assert.equal(isReadyCandidate(readyTag), true);
  assert.equal(isReadyCandidate(readyState), true);
  // An explicit not-ready state is not readiness.
  const notReady = withTag(session("r3"), "integration_state", "conflicted");
  assert.equal(isReadyCandidate(notReady), false);
});

test("candidate queue: verified ready first, integrated outcomes after, topic excluded", () => {
  const fleet = [
    session("topic"),
    session("ready", { parent_session_id: "topic", last_activity_at: "2026-01-01T00:00:00Z" }),
    session("integrated", { parent_session_id: "topic", last_activity_at: "2026-02-01T00:00:00Z" }),
    session("stopped", { parent_session_id: "topic", status: "stopped" }),
    session("sleeping", { parent_session_id: "topic" }),
  ];
  const taggedFleet = fleet.map((s) => {
    if (s.id === "ready") return withTag(s, "integration_ready", "true", "clean against topic@789abc");
    if (s.id === "integrated") return withTag(s, "integration_result", "7a29ef", "squash into topic");
    if (s.id === "sleeping") return withTag(s, "idle", "true");
    return s;
  });
  const rows = topicThreadRows(taggedFleet, "topic", new Set());
  const candidates = integrationCandidates(rows, "topic");
  assert.deepEqual(candidates.map((c) => c.session.id), ["ready", "integrated"]);
  assert.deepEqual(candidates.map((c) => c.state), ["ready", "integrated"]);
  assert.equal(candidates[0].note, "clean against topic@789abc");
  assert.equal(candidates[1].note, "squash into topic");
  // A worker already integrated is not offered Integrate again.
  assert.equal(candidates.find((c) => c.session.id === "integrated").state, "integrated");
  // Stopped and sleeping workers never appear.
  assert.ok(!candidates.some((c) => c.session.id === "stopped"));
  assert.ok(!candidates.some((c) => c.session.id === "sleeping"));
});
