// Verifies the Topics list model (docs/design.md "Navigation"): top-level
// threads are topics with their delegated counts; archived topics disappear
// by default and the Archived toggle is a view over the same list; archived
// child threads keep a live topic's shape.

import assert from "node:assert/strict";
import test from "node:test";
import {
  archivedTopicCount,
  buildTopicList,
  childrenMap,
  parentOf,
  visibleTopics,
} from "../src/topicList.ts";
import { byTopicRecency, topicRecencyMap } from "../src/topicOrder.ts";

const session = (id, extra = {}) => ({
  id,
  status: "running",
  last_activity_at: `2026-01-01T00:00:00Z`,
  last_user_message_at: null,
  parent_session_id: null,
  parent_id: null,
  branch: { id: `b-${id}` },
  ...extra,
});

const compare = (fleet) => byTopicRecency(topicRecencyMap(fleet));

test("a session's parent resolves by either link; dangling and self-links yield null", () => {
  const leader = session("leader");
  const bySessionLink = session("kid", { parent_session_id: "leader" });
  // A dangling parent_session_id falls back to the branch link.
  const byBranchLink = session("branchkid", { parent_id: "b-leader", parent_session_id: "ghost" });
  const selfParent = session("self", { parent_session_id: "self" });
  const fleet = [leader, bySessionLink, byBranchLink, selfParent];
  assert.equal(parentOf(fleet, bySessionLink), leader);
  assert.equal(parentOf(fleet, byBranchLink), leader);
  assert.equal(parentOf(fleet, selfParent), null);
  assert.equal(parentOf(fleet, leader), null);
});

test("topics are the top-level threads with delegated child counts", () => {
  const fleet = [
    session("a"),
    session("a1", { parent_session_id: "a" }),
    session("a2", { parent_session_id: "a" }),
    session("b"),
    session("b1", { parent_session_id: "b", status: "archived" }),
  ];
  const topics = buildTopicList(fleet, compare(fleet));
  assert.deepEqual(
    topics.map((t) => [t.session.id, t.childCount]),
    [["a", 2], ["b", 1]],
  );
});

test("archived leaders disappear by default and return with the toggle", () => {
  const fleet = [
    session("live"),
    session("done", { status: "archived" }),
    session("done1", { parent_session_id: "done", status: "archived" }),
  ];
  const all = buildTopicList(fleet, compare(fleet));
  assert.deepEqual(visibleTopics(all, false).map((t) => t.session.id), ["live"]);
  assert.deepEqual(visibleTopics(all, true).map((t) => t.session.id).sort(), ["done", "live"]);
  assert.equal(archivedTopicCount(all), 1);
  // An all-archived fleet hides everything (the empty state says so).
  const archivedFleet = [session("gone", { status: "archived" })];
  const archivedAll = buildTopicList(archivedFleet, compare(archivedFleet));
  assert.deepEqual(visibleTopics(archivedAll, false), []);
  assert.equal(archivedTopicCount(archivedAll), 1);
});

test("childrenMap keeps archived children under their leader, newest first", () => {
  const fleet = [
    session("a"),
    session("old", { parent_session_id: "a", last_activity_at: "2026-01-01T00:00:01Z" }),
    session("new", { parent_session_id: "a", last_activity_at: "2026-01-02T00:00:00Z" }),
    session("gone", { parent_session_id: "a", status: "archived", last_activity_at: "2026-01-03T00:00:00Z" }),
  ];
  const children = childrenMap(fleet);
  // Archived children are kept — a topic's finished work is part of its
  // shape — and the sibling order is recency, archived or not.
  assert.deepEqual(
    children.get("a").map((s) => s.id),
    ["gone", "new", "old"],
  );
});

test("the Archived toggle never filters a live topic's child threads", () => {
  // childrenMap is the source for nested rows under a leader card; it is
  // not subject to showArchived at all (only leaders are filtered).
  const fleet = [
    session("a"),
    session("a1", { parent_session_id: "a" }),
    session("a2", { parent_session_id: "a", status: "archived" }),
  ];
  const children = childrenMap(fleet);
  const all = buildTopicList(fleet, compare(fleet));
  // Live leader stays visible either way; children are independent of the
  // leader filter decision.
  assert.equal(visibleTopics(all, false).length, 1);
  assert.equal(children.get("a").length, 2);
});
