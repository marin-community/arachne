// Verifies home row naming (docs/design.md: "With no Project or Topic
// selected … Every row names its Project and parent Topic"). The aggregate
// home prefixes Project · Topic; a Project home keeps Topic only (the
// heading already names the project); a Topic home shows the row alone.

import assert from "node:assert/strict";
import test from "node:test";
import { homeRowTitle } from "../src/homeRows.ts";

// A structural slice of SessionSummary: only the fields homeRowTitle reads.
const session = (id, extra = {}) => ({
  id,
  branch: { title: `Title ${id}`, name: `name-${id}`, tags: [] },
  ...extra,
});

test("the aggregate home names Project, Topic, and the row", () => {
  const worker = session("w1");
  assert.equal(
    homeRowTitle(worker, session("root"), "Alpha", { topic: false, project: false }),
    "Alpha · Title root · Title w1",
  );
});

test("the coordinator row names the topic it coordinates", () => {
  const root = session("root");
  assert.equal(
    homeRowTitle(root, root, "Beta", { topic: false, project: false }),
    "Beta · Title root · Coordinator thread",
  );
});

test("a project home names Topic and row but not the shared project", () => {
  assert.equal(
    homeRowTitle(session("w1"), session("root"), "Alpha", { topic: false, project: true }),
    "Title root · Title w1",
  );
});

test("a topic home names the row alone", () => {
  assert.equal(
    homeRowTitle(session("w1"), session("root"), "Alpha", { topic: true, project: false }),
    "Title w1",
  );
});

test("titles fall back to branch names and join with the row separator", () => {
  const worker = session("w1", { branch: { title: "", name: "name-w1", tags: [] } });
  const root = session("root", { branch: { title: "", name: "name-root", tags: [] } });
  assert.equal(
    homeRowTitle(worker, root, "—", { topic: false, project: false }),
    "— · name-root · name-w1",
  );
});


test("standalone roots show their own title instead of coordinator", () => {
  const root = session("one-off", { branch: { title: "Inspect a log", name: "one-off", tags: [{ key: "topic", value: "false" }] } });
  assert.equal(homeRowTitle(root, root, "Alpha", { topic: false, project: false }), "Alpha · Inspect a log");
  assert.equal(homeRowTitle(root, root, "Alpha", { topic: false, project: true }), "Inspect a log");
});
