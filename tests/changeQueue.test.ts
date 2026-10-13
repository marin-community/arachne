import assert from "node:assert/strict";
import test from "node:test";
import { scopeSession, scopedId } from "../src/backendIdentity.ts";
import { projectChangeQueue, reorderedDeclarations, validateQueueView } from "../src/changeQueue.ts";
import { landingCandidates, topicThreadRows } from "../src/topicInspector.ts";
import { source, declaration, changeTag, acceptanceTag, receipt } from "./fixtures/durable-change.mjs";

const url = "https://a.example";
const root = () => scopeSession(url, source("track", [{ key: "topic", value: "true" }, { key: "track_topic_branch", value: "accepted/topic" }], null));
const changes = ["first", "second", "third"].map(id => declaration(id));
const entry = (change = changes[0], requested_by: null | "user" | "autoland" = null) => ({ change, strategy: "squash", requested_by });
const wire = (pending = [entry()], active: any = null) => ({ revision: 3, queue: { version: 1, track_session: "track", repo_root: "/repo", target_branch: "accepted/topic", active, pending } });
const candidates = (tags = changes.map(changeTag)) => {
  const track = root(), worker = scopeSession(url, source("worker", tags));
  const fleet = [track, worker];
  return landingCandidates(topicThreadRows(fleet, track.id, new Set()), track.id, fleet);
};

test("queue validation pins exact raw evidence and rejects foreign, corrupt and duplicate metadata", () => {
  const view = validateQueueView(wire(), root());
  assert.deepEqual(view.queue.pending[0].change, changes[0]);
  assert.ok(Object.isFrozen(view.queue.pending[0].change.scope.paths));
  for (const bad of [
    { ...wire(), revision: -1 }, { ...wire(), revision: 1.1 },
    { ...wire(), queue: { ...wire().queue, track_session: "foreign" } },
    { ...wire(), queue: { ...wire().queue, repo_root: "/other" } },
    { ...wire(), queue: { ...wire().queue, target_branch: "other/topic" } },
    wire([entry(), entry()]), wire([entry()], entry()),
    wire([{ ...entry(), change: { ...changes[0], source_session: scopedId(url, "worker") } }]),
    wire([{ ...entry(), requested_by: "yes" } as any]),
    wire([{ ...entry(), change: { ...changes[0], completion: null } }]),
    { ...wire(), queue: { ...wire().queue, active: undefined } },
  ]) assert.throws(() => validateQueueView(bad, root()), /invalid|another Track/);
});

test("persisted order wins over lifecycle/activity and new declarations append in discovery order", () => {
  const rows = candidates().reverse();
  const view = validateQueueView(wire([entry(changes[1])]), root());
  const projected = projectChangeQueue(view, rows, root().id, [rows.find(row => row.change.id === "third")!.key]);
  assert.deepEqual(projected.pending.map(row => row.change.id), ["second", "third", "first"]);
  assert.equal(projected.pending[0].status, "Declared output", "durable membership alone conveys no authorization");
  assert.deepEqual(projectChangeQueue(view, rows.reverse(), root().id, [rows.find(row => row.change.id === "third")!.key]).pending.map(row => row.change.id), ["second", "third", "first"]);
});

test("active claim stays visible without its source and never becomes acceptance history", () => {
  const view = validateQueueView(wire([entry(changes[1], "user")], entry()), root());
  const projected = projectChangeQueue(view, candidates().filter(row => row.change.id !== "first"), root().id);
  assert.equal(projected.active!.status, "Acceptance in progress");
  assert.match(projected.active!.problem, /unavailable/);
  assert.equal(projected.history.length, 0);
  assert.equal(projected.pending[0].status, "Queued");
  assert.equal(projected.canReorder, true, "unavailable active does not preempt pending ordering");
});

test("receipt history is immovable and order contains the complete pending set only", () => {
  const rows = candidates([...changes.map(changeTag), acceptanceTag(receipt(changes[0]))]);
  const view = validateQueueView(wire([entry(changes[2])]), root());
  const projected = projectChangeQueue(view, rows, root().id);
  assert.deepEqual(projected.history.map(row => row.change.id), ["first"]);
  assert.deepEqual(reorderedDeclarations(projected.pending, 1, 0), [changes[1], changes[2]]);
  assert.throws(() => reorderedDeclarations(projected.history, 0, 0), /pending/);
});

test("unavailable or altered pending evidence blocks complete-set reorder without silently dropping it", () => {
  const view = validateQueueView(wire([entry(changes[0]), entry(changes[1])]), root());
  const rows = candidates().filter(row => row.change.id !== "first");
  const projected = projectChangeQueue(view, rows, root().id);
  assert.equal(projected.pending[0].change.id, "first");
  assert.equal(projected.canReorder, false);
  assert.throws(() => reorderedDeclarations(projected.pending, 0, 1), /available pending/);
  const changed = rows.map(row => row.change.id === "second" ? { ...row, change: { ...row.change, title: "Altered" } } : row);
  assert.match(projectChangeQueue(view, changed, root().id).pending[1].problem, /declaration changed/);
});
