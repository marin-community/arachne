import assert from "node:assert/strict";
import test from "node:test";
import { preparationRequest, reviewIntegrationTarget, independentReviewRequest, reviewPreviewEvidence } from "../src/reviewRequest.ts";
import { declaration } from "./fixtures/durable-change.mjs";

const session = (id, parent = null, repo = "/repo", tags = []) => ({
  id, status: "running", parent_session_id: parent, parent_id: null,
  branch: { id: `branch-${id}`, name: id, branch: `work/${id}`, repo_root: repo, tags },
});
test("preparation routes nested workers to same-repository track rather than main", () => {
  const track = session("track", null, "/repo", [{ key: "topic", value: "true" }]);
  const parent = session("parent", "track");
  const worker = session("worker", "parent");
  assert.equal(reviewIntegrationTarget([track, parent, worker], "worker"), track);
  const prompt = preparationRequest(false, "HEAD abc", track, ["src/app.ts"], declaration());
  assert.match(prompt, /branch work\/track, repository \/repo/);
  assert.match(prompt, /src\/app\.ts/);
  assert.match(prompt, /If any intended source edits remain uncommitted/);
  assert.match(prompt, /do not set integration_ready/);
});
test("unresolved or cross-repository parents never fabricate an integration destination", () => {
  const track = session("track", null, "/other", [{ key: "topic", value: "true" }]);
  const parent = session("parent", "track");
  const worker = session("worker", "parent");
  assert.equal(reviewIntegrationTarget([track, parent, worker], "worker"), null);
  assert.equal(reviewIntegrationTarget([track, worker], "worker"), null);
  assert.match(preparationRequest(false, "HEAD abc", null, [], declaration()), /No landing target is resolved/);
  assert.match(preparationRequest(true, "HEAD abc", null), /publishing strategy and destination have not been selected/);
});

test("manual Review Prepare remains optional preparation without landing authorization", () => {
  for (const isTrack of [true, false]) {
    const prompt = preparationRequest(isTrack, "HEAD abc", session("track"), ["src/app.ts"], isTrack ? null : declaration());
    assert.match(prompt, /optional agent preparation/);
    assert.match(prompt, /visible Loom child sessions, including reviewers/);
    assert.match(prompt, /preserving unrelated manual edits/);
    assert.match(prompt, /Re-verify the exact source branch and revision and the exact target/);
    assert.match(prompt, /Do not land, publish, push, or open a PR until I choose that action/);
  }
});


test("both preparation and independent review name the canonical accepted ref", () => {
  const track = session("track", null, "/repo", [
    { key: "topic", value: "true" }, { key: "track_topic_branch", value: "arachne/tracks/accepted" },
  ]);
  const worker = session("worker", "track");
  const target = reviewIntegrationTarget([track, worker], worker.id);
  for (const prompt of [preparationRequest(false, "HEAD source", target, [], declaration()), independentReviewRequest("HEAD source", target, false, declaration())]) {
    assert.match(prompt, /arachne\/tracks\/accepted/);
    assert.doesNotMatch(prompt, /work\/track/);
    assert.match(prompt, /Do not land, publish, push/);
  }
  assert.match(independentReviewRequest("HEAD source", null), /No same-repository Track is resolved/);
  assert.match(preparationRequest(true, "HEAD source", track), /Canonical publish source: arachne\/tracks\/accepted/);
});


test("root review and optional preparation exclude coordinator scratch edits", () => {
  const track = session("track", null, "/repo", [
    { key: "topic", value: "true" }, { key: "track_topic_branch", value: "accepted/topic" },
  ]);
  const review = independentReviewRequest("Snapshot evidence", track, true);
  assert.match(review, /Fork the committed canonical accepted ref/);
  assert.match(review, /recorded publishing base/);
  assert.match(review, /Coordinator scratch commits and checkout edits are outside this review/);
  const prepare = preparationRequest(true, "Snapshot evidence", track, ["scratch-edit.ts"]);
  assert.match(prepare, /validate committed canonical accepted state in an isolated checkout/);
  assert.doesNotMatch(prepare, /scratch-edit.ts|Stabilize and commit the intended worker/);
  assert.match(prepare, /Do not land, publish, push, or open a PR until I choose that action/);
});

test("preview evidence records actual accepted or worker source and target snapshots", () => {
  const snapshot = { source_ref: "accepted/topic", accepted_source: true, head_oid: "abc",
    base: { state: "available", reference: "release", oid: "def" } };
  assert.match(reviewPreviewEvidence(snapshot), /accepted\/topic at abc compared with release at def/);
  assert.match(reviewPreviewEvidence(snapshot), /checkout edits are excluded/);
  assert.match(reviewPreviewEvidence({ ...snapshot, source_ref: "work/worker", accepted_source: false }), /included worker checkout edits/);
  assert.match(reviewPreviewEvidence({ ...snapshot, head_oid: null, base: { state: "unavailable", reference: "release" } }), /comparison release was unavailable/);
  assert.doesNotMatch(reviewPreviewEvidence({ ...snapshot, head_oid: null, base: { state: "unavailable", reference: "release" } }), /at null|at undefined|It covered|It included/);
});

test("selected review carries exact scope and excludes cumulative branch and live edits", () => {
  const change = declaration();
  const prompt = independentReviewRequest("", session("track"), false, change);
  assert.ok(prompt.includes(JSON.stringify(change)));
  assert.match(prompt, /immutable base_oid→head_oid range and literal paths; exclude live checkout edits/);
  assert.match(prompt, /Do not replace the selected range/);
  assert.match(prompt, /Do not expand to sibling outputs/);
  assert.doesNotMatch(prompt, /Fork the original worker branch|including inherited unaccepted changes/);
});

test("unselected source cannot receive broad preparation", () => {
  assert.match(preparationRequest(false, "", session("track")), /No Change is selected. Do not prepare this source/);
  assert.match(independentReviewRequest("", session("track")), /unselected inspection only/);
});

test("working preparation pins version and declares fresh review on finalization", () => {
  const change = { ...declaration("working", undefined, { kind: "working", paths: ["src/app.ts"], worktree_version: "snapshot-1" }), completion: null };
  const prompt = preparationRequest(false, "", session("track"), [], change);
  assert.ok(prompt.includes(JSON.stringify(change)));
  assert.match(prompt, /commit only its declared paths/);
  assert.match(prompt, /retain its ID and original identity/);
  assert.match(prompt, /require fresh scope review/);
  assert.match(prompt, /Do not prepare unselected source changes/);
});
