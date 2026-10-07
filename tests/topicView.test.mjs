import assert from "node:assert/strict";
import test from "node:test";
import {
  resolveTopicThread,
  threadAncestors,
  threadBelongsToTopic,
  topicRootOf,
} from "../src/topic-view.ts";

// A session fixture: id, parent session id, parent branch id, status.
const session = (id, parentId = null, status = "sleeping") => ({
  id,
  status,
  parent_session_id: parentId,
  parent_id: parentId ? `${parentId}-branch` : null,
  branch: { id: `${id}-branch` },
});

// coordinator delegates to workerA and workerB; workerA delegates to
// grandchild. otherRoot is an unrelated topic.
const fleet = [
  session("coordinator"),
  session("workerA", "coordinator"),
  session("workerB", "coordinator", "archived"),
  session("grandchild", "workerA"),
  session("otherRoot"),
  session("otherChild", "otherRoot"),
];

test("topicRootOf walks the delegation chain to the root", () => {
  assert.equal(topicRootOf(fleet, "grandchild").id, "coordinator");
  assert.equal(topicRootOf(fleet, "workerA").id, "coordinator");
  assert.equal(topicRootOf(fleet, "coordinator").id, "coordinator");
  assert.equal(topicRootOf(fleet, "otherChild").id, "otherRoot");
  assert.equal(topicRootOf(fleet, "missing"), null);
  assert.equal(topicRootOf(fleet, null), null);
});

test("topicRootOf follows branch-id parent links too", () => {
  const byBranchOnly = [
    { ...session("root"), branch: { id: "root-branch" } },
    { id: "child", status: "running", parent_session_id: null, parent_id: "root-branch", branch: { id: "child-branch" } },
  ];
  assert.equal(topicRootOf(byBranchOnly, "child").id, "root");
});

test("topicRootOf stops at cycles instead of hanging", () => {
  const a = session("a");
  const b = session("b");
  a.parent_session_id = "b";
  b.parent_session_id = "a";
  assert.equal(topicRootOf([a, b], "a").id, "a");
});

test("threadAncestors lists the chain nearest-first, excluding the session itself", () => {
  assert.deepEqual(
    threadAncestors(fleet, "grandchild").map((s) => s.id),
    ["workerA", "coordinator"],
  );
  assert.deepEqual(
    threadAncestors(fleet, "workerA").map((s) => s.id),
    ["coordinator"],
  );
  // A root's own chain is empty, and unknown ids yield nothing.
  assert.deepEqual(threadAncestors(fleet, "coordinator"), []);
  assert.deepEqual(threadAncestors(fleet, "missing"), []);
  assert.deepEqual(threadAncestors(fleet, null), []);
});

test("threadAncestors stops at cycles instead of hanging", () => {
  const a = session("a");
  const b = session("b");
  a.parent_session_id = "b";
  b.parent_session_id = "a";
  const ids = threadAncestors([a, b], "a").map((s) => s.id);
  assert.deepEqual([...ids].sort(), ["b"]);
});

test("threadBelongsToTopic scopes membership to the topic subtree", () => {
  assert.equal(threadBelongsToTopic(fleet, "coordinator", "coordinator"), true);
  assert.equal(threadBelongsToTopic(fleet, "coordinator", "grandchild"), true);
  assert.equal(threadBelongsToTopic(fleet, "coordinator", "otherChild"), false);
  assert.equal(threadBelongsToTopic(fleet, "coordinator", "missing"), false);
  assert.equal(threadBelongsToTopic(fleet, "coordinator", null), false);
});

test("a track click opens the coordinator, first visit or not", () => {
  // Entering from anywhere lands on the coordinator — never on whatever
  // worker thread was open here before (no restore).
  assert.deepEqual(
    resolveTopicThread({ topicId: "coordinator", currentThreadId: null }),
    { threadId: "coordinator", source: "coordinator" },
  );
});

test("clicking an already-selected track keeps the coordinator open", () => {
  assert.deepEqual(
    resolveTopicThread({ topicId: "coordinator", currentThreadId: "coordinator" }),
    { threadId: "coordinator", source: "current" },
  );
});

test("clicking a track while one of its threads is open goes to the coordinator", () => {
  // The worker is already on screen; resolving to it would make the click
  // a no-op with no way back to the track's own chat.
  assert.deepEqual(
    resolveTopicThread({ topicId: "coordinator", currentThreadId: "grandchild" }),
    { threadId: "coordinator", source: "coordinator" },
  );
});

test("clicking a track while a thread of another topic is open goes to the coordinator", () => {
  assert.deepEqual(
    resolveTopicThread({ topicId: "coordinator", currentThreadId: "otherChild" }),
    { threadId: "coordinator", source: "coordinator" },
  );
});

test("the coordinator itself always qualifies as the current thread", () => {
  // The fleet snapshot can lag a freshly launched coordinator.
  assert.deepEqual(
    resolveTopicThread({ topicId: "fresh", currentThreadId: "fresh" }),
    { threadId: "fresh", source: "current" },
  );
});
