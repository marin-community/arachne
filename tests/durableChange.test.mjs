import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { parseChange, parseAcceptance, changeRegistry, pinChange, pinSelection, selectionProblem, selectedChangePayload } from "../src/durableChange.ts";
import { landingCandidates, topicThreadRows } from "../src/topicInspector.ts";
import { declaration, source, oid, changeTag, receipt, acceptanceTag } from "./fixtures/durable-change.mjs";

const track = () => source("track", [{ key: "topic", value: "true" }], null);
const queue = (...workers) => landingCandidates(topicThreadRows([track(), ...workers], "track", new Set()), "track");

test("strict v1 decoding rejects malformed scope, identities, paths and completion", () => {
  const change = declaration();
  assert.deepEqual(parseChange(JSON.stringify(change)), change);
  assert.deepEqual(parseChange({ ...change, completion: null }, true), { ...change, completion: null }); // Inline typed result.
  assert.equal(parseChange({ ...change, completion: null }), null); // Indexed range needs its evidence pointer.
  assert.equal(parseChange(change, true), null); // Inline results cannot self-reference.
  const invalid = [null, "{", [], { ...change, version: 2 }, { ...change, extra: true },
    { ...change, id: "" }, { ...change, id: "a/b" }, { ...change, id: "é" }, { ...change, id: "a".repeat(81) },
    { ...change, title: " " }, { ...change, repo_root: "relative" }, { ...change, source_work_dir: "" },
    { ...change, source_branch: "HEAD" }, { ...change, completion: undefined },
    { ...change, completion: { channel_id: "someone-else", message_id: "r" } },
    { ...change, completion: { channel_id: "worker", message_id: "" } },
    ...[{ kind: "unknown" }, { base_oid: "abc" }, { head_oid: "z".repeat(40) }, { head_oid: "b".repeat(64) },
      { paths: ["../secret"] }, { paths: ["src/../app"] }, { paths: ["/absolute"] }, { paths: ["."] },
      { paths: ["src//app"] }, { paths: [".git/config"] }, { paths: ["src/.GIT/index"] }, { paths: ["src/app", "src/app"] }, { paths: "src/app" },
      { kind: "working", worktree_version: "v1", paths: [] }, { kind: "working", paths: ["src/app"] },
      { worktree_version: "unexpected" }].map(scope => ({ ...change, scope: { ...change.scope, ...scope } }))];
  for (const value of invalid) assert.equal(parseChange(value), null, JSON.stringify(value));
  const working = { ...change, scope: { ...change.scope, kind: "working", paths: ["src/app"], worktree_version: "v1" }, completion: null };
  assert.deepEqual(parseChange(working), working);
  assert.equal(parseChange({ ...working, completion: change.completion }), null);
  assert.ok(parseChange({ ...change, scope: { ...change.scope, base_oid: "a".repeat(64), head_oid: "b".repeat(64) } }));
});

test("zero outputs never become obligations across status, role, title or legacy signals", () => {
  for (const status of ["running", "stopped", "archived", "error"]) {
    for (const role of ["reviewer", "researcher", "coordinator", null]) {
      const tags = [{ key: "integration_result", value: "266e0b4" }, { key: "integration_ready", value: "true" },
        { key: "integration_state", value: "ready" }, { key: "landing_request", value: "pending" },
        ...(role ? [{ key: "role", value: role }] : [])];
      const worker = { ...source("worker", tags), status };
      worker.branch.title = "Implementation complete: land now";
      assert.deepEqual(queue(worker), [], `${status}/${role}`);
    }
  }
});

test("relabeling a source reviewer preserves its declared output eligibility", () => {
  const worker = source(), change = declaration();
  worker.branch.tags = [changeTag(change)];
  const pinned = pinChange(change);
  worker.branch.tags.push({ key: "role", value: "reviewer" });
  assert.equal(queue(worker)[0].state, "waiting");
  assert.deepEqual(queue(worker)[0].change, change);
  assert.equal(selectionProblem(pinSelection(worker.id, pinned), worker), "");
  assert.deepEqual(selectedChangePayload(pinSelection(worker.id, pinned)), { sessionId: worker.id, changeId: change.id, expectedChange: change });
  worker.branch.tags = [{ key: "role", value: "reviewer" }];
  assert.deepEqual(queue(worker), []);
});

test("multiple outputs remain separate and one acceptance never hides a sibling or later output", () => {
  const worker = source();
  const old = declaration("old", worker), later = declaration("later", worker, { base_oid: oid("b"), head_oid: oid("d"), paths: ["src/new.ts"] });
  worker.branch.tags = [changeTag(old), acceptanceTag(receipt(old)), changeTag(later),
    { key: "integration_result", value: "266e0b4" }];
  const result = queue(worker);
  assert.deepEqual(result.map(item => [item.key, item.state]), [["worker/later", "waiting"], ["worker/old", "landed"]]);
  assert.deepEqual(result[0].change.scope, later.scope);
  assert.equal(changeRegistry(worker).entries.find(entry => entry.key === "worker/old").acceptance.target_oid, oid("c"));
  const other = source("other", [changeTag(declaration("later", source("other")))]);
  assert.equal(queue(worker, other).filter(item => item.change.id === "later").length, 2);
});

test("working scope is bounded request identity without inferred readiness", () => {
  const worker = source();
  const working = { ...declaration("working", worker, { kind: "working", paths: ["src/a"], worktree_version: "v1" }), completion: null };
  worker.branch.tags = [changeTag(working), { key: "integration_ready", value: "true" }];
  assert.equal(queue(worker)[0].state, "working");
  const pinned = pinChange(working);
  worker.branch.tags = [changeTag(declaration("working", worker, { paths: ["src/a"] }))];
  assert.match(selectionProblem(pinSelection(worker.id, pinned), worker), /changed.*Reselect/);
});

test("identical duplicate declarations and receipts dedupe; conflicts fail closed", () => {
  const worker = source(), change = declaration();
  const reordered = Object.fromEntries(Object.entries(change).reverse());
  worker.branch.tags = [changeTag(change), changeTag(reordered), acceptanceTag(receipt(change)), acceptanceTag(receipt(change))];
  assert.equal(queue(worker).length, 1);
  assert.equal(queue(worker)[0].state, "landed");
  worker.branch.tags = [changeTag(change), changeTag({ ...change, title: "different" })];
  assert.equal(queue(worker)[0].state, "blocked");
  assert.match(selectionProblem(pinSelection(worker.id, change), worker), /Conflicting duplicate/);
  worker.branch.tags = [changeTag(change), { key: "change:first", value: "malformed" }];
  assert.equal(queue(worker)[0].state, "blocked");
  const rows = [track(), source("worker", [changeTag(change)]), source("worker", [changeTag({ ...change, scope: { ...change.scope, head_oid: oid("d") } })])]
    .map(session => ({ session, depth: 0, childCount: 0, isCollapsed: false }));
  assert.equal(landingCandidates(rows, "track")[0].state, "blocked");
});

test("malformed or mismatched receipt blocks reacceptance rather than becoming fresh", () => {
  const worker = source(), change = declaration();
  const receipts = ["{", receipt(change, { version: 2 }), receipt(change, { source_session: "other" }),
    receipt(change, { completion: { channel_id: "worker", message_id: "another" } }),
    receipt(change, { scope: { ...change.scope, paths: ["different"] } }), receipt(change, { target_oid: "short" }),
    receipt(change, { outcome: "pending" }), receipt(change, { target_oid: "a".repeat(64) })];
  for (const value of receipts) {
    worker.branch.tags = [changeTag(change), { key: "change_acceptance:first", value: typeof value === "string" ? value : JSON.stringify(value) }];
    assert.equal(queue(worker)[0].state, "blocked");
    assert.match(selectionProblem(pinSelection(worker.id, change), worker), /receipt/);
  }
  assert.deepEqual(parseAcceptance(receipt(change)), receipt(change));
  worker.branch.tags = [changeTag(change), acceptanceTag(receipt(change)), acceptanceTag(receipt(change, { target_oid: oid("d") }))];
  assert.equal(queue(worker)[0].state, "blocked");
});

test("pinning preserves exact range and completion across sibling and branch progress", () => {
  const worker = source(), change = declaration("first", worker, { paths: ["src/one.ts"] });
  worker.branch.tags = [changeTag(change)];
  const selected = pinChange(change);
  change.scope.paths.push("src/accidental.ts");
  assert.deepEqual(selected.scope.paths, ["src/one.ts"]);
  const sibling = declaration("next", worker, { base_oid: oid("b"), head_oid: oid("d") });
  worker.branch.tags.push(changeTag(sibling));
  assert.equal(selectionProblem(pinSelection(worker.id, selected), worker), "");
  assert.deepEqual(selectedChangePayload(pinSelection(worker.id, selected)), { sessionId: "worker", changeId: "first", expectedChange: selected });
  assert.throws(() => selected.scope.paths.push("src/other.ts"));
  worker.branch.tags[0] = changeTag({ ...selected, completion: { channel_id: "worker", message_id: "later-result" } });
  assert.match(selectionProblem(pinSelection(worker.id, selected), worker), /changed.*Reselect/);
  worker.branch.tags = [];
  assert.match(selectionProblem(pinSelection(worker.id, selected), worker), /removed/);
});

test("original identity and same-repository Track ownership remain required", () => {
  const worker = source(), change = declaration();
  for (const mismatch of [{ source_session: "other", completion: { channel_id: "other", message_id: "result" } }, { repo_root: "/other" }, { source_branch: "weaver/other" }, { source_work_dir: "/another" }]) {
    worker.branch.tags = [changeTag({ ...change, ...mismatch })];
    assert.equal(queue(worker)[0].state, "blocked");
  }
  const foreign = source("foreign"); foreign.branch.repo_root = "/other";
  foreign.branch.tags = [changeTag(declaration("foreign", foreign))];
  assert.deepEqual(queue(foreign), []);
  const nestedTrack = source("nested-track", [{ key: "topic", value: "true" }]);
  const nestedWorker = source("nested-worker", [], nestedTrack.id);
  nestedWorker.branch.tags = [changeTag(declaration("nested", nestedWorker))];
  assert.deepEqual(queue(nestedTrack, nestedWorker), []);
  const invalid = source("invalid", [{ key: "change:bad", value: "{broken" }]);
  assert.deepEqual(queue(invalid), []);
  assert.match(changeRegistry(invalid).errors[0], /Invalid declaration/);
});

test("missing source evidence is unavailable and never authorizes selection or acceptance", () => {
  const change = declaration();
  for (const work_dir of [undefined, ""]) {
    for (const accepted of [false, true]) {
      const worker = { ...source(), work_dir };
      worker.branch.tags = [changeTag(change), ...(accepted ? [acceptanceTag(receipt(change))] : [])];
      const candidate = queue(worker)[0];
      assert.equal(candidate.state, "unavailable");
      assert.match(candidate.note, /evidence is unavailable/);
      assert.doesNotMatch(candidate.note, /reconcil|does not match/);
      assert.match(selectionProblem(pinSelection(worker.id, change), worker), /unavailable/);
      assert.equal(changeRegistry(worker).entries[0].acceptance, null);
      assert.deepEqual(changeRegistry(worker).errors, []);
    }
  }
  const failed = source();
  failed.branch.tags = [changeTag(change)];
  failed.change_identity_problem = "unavailable";
  assert.equal(queue(failed)[0].state, "unavailable");
  assert.match(selectionProblem(pinSelection(failed.id, change), failed), /unavailable/);
});

test("verified conflicts and invalid receipts still block when checkout evidence is missing", () => {
  const worker = source(), change = declaration();
  worker.work_dir = "";
  worker.branch.tags = [changeTag(change)];
  worker.change_identity_problem = "conflict";
  assert.equal(queue(worker)[0].state, "blocked");
  assert.match(selectionProblem(pinSelection(worker.id, change), worker), /does not match/);
  delete worker.change_identity_problem;
  worker.branch.tags.push({ key: "change_acceptance:first", value: "{" });
  assert.equal(queue(worker)[0].state, "blocked");
  assert.match(selectionProblem(pinSelection(worker.id, change), worker), /receipt/);
  worker.branch.tags = [changeTag({ ...change, repo_root: "/other" })];
  assert.equal(queue(worker)[0].state, "blocked");
});

test("duplicate snapshots with missing evidence remain unavailable in either order", () => {
  const worker = source(), change = declaration();
  worker.branch.tags = [changeTag(change)];
  const missing = { ...worker, work_dir: "" };
  for (const snapshots of [[worker, missing], [missing, worker]]) {
    const fleet = [track(), ...snapshots];
    const candidate = landingCandidates(topicThreadRows(fleet, "track", new Set()), "track", fleet)[0];
    assert.equal(candidate.state, "unavailable");
    assert.match(selectionProblem(pinSelection(worker.id, change), candidate.session), /unavailable/);
  }
});

test("known path conflicts dominate missing evidence in both orders and every three-row permutation", () => {
  const permutations = items => items.length === 1 ? [items] : items.flatMap((item, index) =>
    permutations(items.filter((_, i) => i !== index)).map(rest => [item, ...rest]));
  for (const accepted of [false, true]) {
    const worker = source(), change = declaration();
    worker.branch.tags = [changeTag(change), ...(accepted ? [acceptanceTag(receipt(change))] : [])];
    const missing = { ...worker, work_dir: "", change_identity_problem: "unavailable" };
    // Exact raw path comparison: even a trailing slash proves a mismatch.
    for (const work_dir of ["/repo/.worktrees/replacement", `${worker.work_dir}/`]) {
      const replacement = { ...worker, work_dir };
      const cases = [...permutations([missing, replacement]), ...permutations([missing, worker, replacement])];
      for (const snapshots of cases) {
        const fleet = [track(), ...snapshots];
        const candidate = landingCandidates(topicThreadRows(fleet, "track", new Set()), "track", fleet)[0];
        assert.equal(candidate.state, "blocked", JSON.stringify(snapshots.map(s => s.work_dir)));
        assert.match(candidate.note, /identity/);
        assert.doesNotMatch(candidate.note, /unavailable/);
        assert.match(selectionProblem(pinSelection(worker.id, change), candidate.session), /identity/);
        assert.equal(changeRegistry(candidate.session).entries[0].acceptance, null);
        assert.deepEqual(candidate.change, change, "conflicts cannot rewrite pinned declarations");
      }
    }
    for (const snapshots of permutations([missing, worker, { ...worker }])) {
      const fleet = [track(), ...snapshots];
      const candidate = landingCandidates(topicThreadRows(fleet, "track", new Set()), "track", fleet)[0];
      assert.equal(candidate.state, "unavailable");
      assert.match(selectionProblem(pinSelection(worker.id, change), candidate.session), /unavailable/);
    }
  }
});

test("malformed sibling indexing and orphan receipts cannot bypass native registry validation", () => {
  const worker = source(), change = declaration();
  for (const invalid of [{ key: "change:missing", value: "{" },
    { key: "change_acceptance:missing", value: JSON.stringify(receipt(change, { change_id: "missing" })) },
    { key: "change:wrong-id", value: JSON.stringify(change) }]) {
    worker.branch.tags = [changeTag(change), invalid];
    assert.equal(queue(worker).find(item => item.key === "worker/first").state, "blocked");
    assert.match(selectionProblem(pinSelection(worker.id, change), worker), /registry/);
  }
});


test("shared native/frontend text policy preserves literal paths and rejects unsupported metadata", () => {
  const fixture = JSON.parse(readFileSync(new URL("./fixtures/change-text-contract.json", import.meta.url), "utf8"));
  const patch = (base, updates) => {
    for (const [key, value] of Object.entries(updates)) {
      if (value && typeof value === "object" && !Array.isArray(value)) patch(base[key], value);
      else base[key] = value;
    }
    return base;
  };
  for (const item of fixture.cases) {
    const value = patch(structuredClone(fixture[item.kind]), item.patch);
    const parsed = item.kind === "change" ? parseChange(value) : parseAcceptance(value);
    assert.equal(!!parsed, item.valid, item.name);
    if (item.valid) assert.deepEqual(parsed, value, `${item.name}: preserve exact strings`);
    if (item.kind === "change") {
      const worker = source(value.source_session);
      worker.branch.repo_root = value.repo_root;
      worker.branch.branch = value.source_branch;
      worker.work_dir = value.source_work_dir;
      const sibling = declaration("sibling", worker);
      worker.branch.tags = [changeTag(value), changeTag(sibling)];
      const registry = changeRegistry(worker);
      const siblingEntry = registry.entries.find(entry => entry.change.id === "sibling");
      assert.equal(!!siblingEntry && !siblingEntry.blocked, item.valid, item.name);
    }
  }
});
