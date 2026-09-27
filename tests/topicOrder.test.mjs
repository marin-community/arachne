// Verifies topic ordering (docs/design.md "Navigation"): topics are ordered
// by when a person last steered them — the newest user message anywhere in
// the topic's subtree — so agent activity (loom restamps
// `last_activity_at` on every streamed frame) never reshuffles the list.

import assert from "node:assert/strict";
import test from "node:test";
import {
  byTopicRecency,
  sessionRecency,
  topicRecency,
  topicRecencyMap,
} from "../src/topicOrder.ts";

const session = (id, lastActivity, lastUserMessage, parent = null) => ({
  id,
  last_activity_at: lastActivity,
  last_user_message_at: lastUserMessage,
  status: "running",
  parent_session_id: parent,
  parent_id: null,
  branch: { id: `b-${id}` },
});

test("recency prefers the last user message, falling back to activity", () => {
  assert.equal(
    sessionRecency(session("a", "2026-01-01T00:00:01Z", "2026-01-01T00:00:05Z")),
    "2026-01-01T00:00:05Z",
  );
  // A journal with no user input yet (null) falls back.
  assert.equal(
    sessionRecency(session("b", "2026-01-01T00:00:02Z", null)),
    "2026-01-01T00:00:02Z",
  );
  // An older loom that predates the field entirely still orders.
  assert.equal(
    sessionRecency({ id: "c", last_activity_at: "2026-01-01T00:00:03Z" }),
    "2026-01-01T00:00:03Z",
  );
});

test("a busy agent never outranks a later user message", () => {
  const fleet = [
    // The busy topic streams constantly (activity keeps advancing); the
    // quiet one was steered later. The list must not move.
    session("busy", "2026-03-01T00:00:00Z", "2026-01-01T00:00:00Z"),
    session("quiet", "2026-01-02T00:00:00Z", "2026-02-01T00:00:00Z"),
  ];
  const recency = topicRecencyMap(fleet);
  const compare = byTopicRecency(recency);
  assert.ok(compare(fleet[1], fleet[0]) < 0, "quiet (later user message) sorts first");
  // And a fresh burst of agent output changes nothing.
  const burst = [session("busy", "2026-04-01T00:00:00Z", "2026-01-01T00:00:00Z"), ...fleet.slice(1)];
  const burstRecency = topicRecencyMap(burst);
  assert.ok(
    byTopicRecency(burstRecency)(burst[1], burst[0]) < 0,
    "still quiet first",
  );
});

test("a worker's user message floats its topic, not just the coordinator's", () => {
  const fleet = [
    session("topicA", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
    session("workerA", "2026-01-02T00:00:00Z", "2026-01-02T00:00:00Z", "topicA"),
    session("topicB", "2026-01-03T00:00:00Z", "2026-01-03T00:00:00Z"),
  ];
  const recency = topicRecencyMap(fleet);
  assert.equal(recency["topicA"], "2026-01-02T00:00:00Z");
  assert.equal(recency["topicB"], "2026-01-03T00:00:00Z");
  const compare = byTopicRecency(recency);
  assert.ok(compare(fleet[2], fleet[0]) < 0, "topicB (steered later) sorts first");
  // Steering topicA's worker again floats topicA above topicB.
  const after = [
    session("topicA", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
    session("workerA", "2026-01-02T00:00:00Z", "2026-01-05T00:00:00Z", "topicA"),
    session("topicB", "2026-01-03T00:00:00Z", "2026-01-03T00:00:00Z"),
  ];
  const afterRecency = topicRecencyMap(after);
  assert.ok(
    byTopicRecency(afterRecency)(after[0], after[2]) < 0,
    "topicA floats to the top",
  );
});

test("a worker's busy output stays quiet while its topic rests", () => {
  const fleet = [
    session("topicA", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
    // The worker streams (activity) but nobody steered it after launch.
    session("workerA", "2026-03-01T00:00:00Z", "2026-01-02T00:00:00Z", "topicA"),
    session("topicB", "2026-01-03T00:00:00Z", "2026-01-03T00:00:00Z"),
  ];
  const compare = byTopicRecency(topicRecencyMap(fleet));
  assert.ok(compare(fleet[2], fleet[0]) < 0, "topicB first: agent output doesn't float topicA");
});

test("nested workers fold through to the topic root", () => {
  const fleet = [
    session("topic", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
    session("mid", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z", "topic"),
    session("leaf", "2026-01-01T00:00:00Z", "2026-01-09T00:00:00Z", "mid"),
    session("other", "2026-01-05T00:00:00Z", "2026-01-05T00:00:00Z"),
  ];
  const recency = topicRecencyMap(fleet);
  assert.equal(recency["topic"], "2026-01-09T00:00:00Z");
  assert.ok(byTopicRecency(recency)(fleet[0], fleet[3]) < 0, "topic first");
});

test("parent cycles never loop", () => {
  // A corrupted parent loop must terminate. topicRootOf stops each walk at
  // the first node it revisits, so each cycle member roots at itself.
  const fleet = [
    { ...session("x", "2026-01-01T00:00:00Z", null), parent_session_id: "y" },
    { ...session("y", "2026-01-02T00:00:00Z", null), parent_session_id: "x" },
  ];
  const recency = topicRecencyMap(fleet);
  assert.equal(recency["x"], "2026-01-01T00:00:00Z");
  assert.equal(recency["y"], "2026-01-02T00:00:00Z");
});

test("a lone session without a map entry falls back to its own recency", () => {
  const s = session("solo", "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z");
  assert.equal(topicRecency(s, {}), "2026-01-02T00:00:00Z");
});
test("equal recency breaks ties by id for a total, stable order", () => {
  const fleet = [
    session("a", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
    session("b", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
  ];
  const compare = byTopicRecency(topicRecencyMap(fleet));
  assert.ok(compare(fleet[0], fleet[1]) < 0);
  assert.ok(compare(fleet[1], fleet[0]) > 0);
  assert.equal(compare(fleet[0], fleet[0]), 0);
});

test("older looms without the field keep the activity order", () => {
  const fleet = [
    { id: "x", last_activity_at: "2026-01-01T00:00:00Z", parent_session_id: null, parent_id: null, branch: { id: "bx" } },
    { id: "y", last_activity_at: "2026-02-01T00:00:00Z", parent_session_id: null, parent_id: null, branch: { id: "by" } },
  ];
  const compare = byTopicRecency(topicRecencyMap(fleet));
  assert.ok(compare(fleet[1], fleet[0]) < 0);
});
