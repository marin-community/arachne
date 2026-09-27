// Verifies the LHS Projects grouping model (docs/design.md "Navigation"):
// a non-system Loom layout group is a Project; a topic's project is its
// coordinator's placement group; everything else is Ungrouped.

import assert from "node:assert/strict";
import test from "node:test";
import { buildProjectSections, layoutProjects, topicProjectId } from "../src/projects.ts";

const session = (id, groupId) => ({
  id,
  placement: groupId ? { group_id: groupId } : null,
});

const layout = {
  revision: 1,
  spaces: [
    { id: "u", name: "User", rank: 0, system_key: "user", groups: [
      { id: "inbox", name: "Inbox", rank: 0, system_key: "inbox" },
    ] },
    { id: "w", name: "Work", rank: 1, system_key: null, groups: [
      { id: "g2", name: "Beta", rank: 1, system_key: null },
      { id: "g1", name: "Alpha", rank: 0, system_key: null },
    ] },
    { id: "p", name: "Personal", rank: 2, system_key: null, groups: [
      { id: "g3", name: "Zed", rank: 0, system_key: null },
    ] },
  ],
  defaults: [],
};

test("projects are the non-system groups in space-then-group rank order", () => {
  assert.deepEqual(layoutProjects(layout).map((g) => g.id), ["g1", "g2", "g3"]);
  assert.deepEqual(layoutProjects(null), []);
});

test("a topic's project is its coordinator's non-system group", () => {
  const ids = new Set(layoutProjects(layout).map((g) => g.id));
  assert.equal(topicProjectId(session("t1", "g1"), ids), "g1");
  // System groups (the user Inbox), missing placements, and dangling group
  // ids are all unfiled.
  assert.equal(topicProjectId(session("t2", "inbox"), ids), null);
  assert.equal(topicProjectId(session("t3", null), ids), null);
  assert.equal(topicProjectId(session("t4", "deleted"), ids), null);
});

test("sections keep layout order, keep empty projects, and file ungrouped last", () => {
  const sections = buildProjectSections(
    [
      { session: session("t1", "g1"), note: "a" },
      { session: session("t2", "inbox"), note: "b" },
      { session: session("t3", "g2"), note: "c" },
      { session: session("t4", null), note: "d" },
    ],
    layout,
  );
  assert.deepEqual(
    sections.map((s) => [s.id, s.name, s.topics.length]),
    [["g1", "Alpha", 1], ["g2", "Beta", 1], ["g3", "Zed", 0], [null, "Ungrouped", 2]],
  );
  assert.deepEqual(
    sections[0].topics.map((t) => t.session.id),
    ["t1"],
  );
  assert.deepEqual(
    sections[3].topics.map((t) => t.session.id),
    ["t2", "t4"],
  );
});
