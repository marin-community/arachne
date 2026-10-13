// Run by the native HTTP adapter test with its actual serialized fleet.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { scopeSession, scopedId } from "../../src/backendIdentity.ts";
import { landingCandidates, topicThreadRows } from "../../src/topicInspector.ts";
import { pinSelection, selectionProblem, selectedChangePayload } from "../../src/durableChange.ts";

const { sessions, expected, server } = JSON.parse(readFileSync(0, "utf8"));
const raw = structuredClone(sessions);
const fleet = sessions.map(s => scopeSession(server, s));
const track = scopedId(server, "track");
const queue = landingCandidates(topicThreadRows(fleet, track, new Set()), track, fleet);
assert.equal(queue.length, Object.keys(expected).length);
for (const item of queue) {
  const rawId = item.change.source_session;
  assert.equal(item.state, expected[rawId], rawId);
  assert.equal(item.session.id, scopedId(server, rawId));
  assert.equal(item.key, `${scopedId(server, rawId)}/output`);
  const selected = pinSelection(item.session.id, item.change);
  const payload = selectedChangePayload(selected);
  assert.equal(payload.expectedChange.source_session, rawId);
  assert.equal(payload.expectedChange.completion.channel_id, rawId);
  assert.equal(payload.expectedChange.source_work_dir, `/repo/.worktrees/${rawId}`);
  const problem = selectionProblem(selected, item.session);
  if (item.state === "waiting") assert.equal(problem, "");
  else assert.ok(problem, `${rawId} must remain non-actionable`);
  if (item.state === "unavailable") {
    assert.match(item.note, /evidence is unavailable/);
    assert.doesNotMatch(item.note, /reconcil|does not match/);
  }
}
assert.deepEqual(sessions, raw, "UI scoping must not mutate raw wire identity or tags");
