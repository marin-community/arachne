import assert from "node:assert/strict";
import test from "node:test";
import { preparationRequest, reviewIntegrationTarget, independentReviewRequest } from "../src/reviewRequest.ts";

const session = (id, parent = null, repo = "/repo", tags = []) => ({
  id, status: "running", parent_session_id: parent, parent_id: null,
  branch: { id: `branch-${id}`, name: id, branch: `work/${id}`, repo_root: repo, tags },
});
test("preparation routes nested workers to same-repository track rather than main", () => {
  const track = session("track", null, "/repo", [{ key: "topic", value: "true" }]);
  const parent = session("parent", "track");
  const worker = session("worker", "parent");
  assert.equal(reviewIntegrationTarget([track, parent, worker], "worker"), track);
  const prompt = preparationRequest(false, "HEAD abc", track, ["src/app.ts"]);
  assert.match(prompt, /branch work\/track, repository \/repo/);
  assert.match(prompt, /src\/app\.ts/);
  assert.match(prompt, /If any uncommitted edits remain/);
  assert.match(prompt, /do not set integration_ready/);
});
test("unresolved or cross-repository parents never fabricate an integration destination", () => {
  const track = session("track", null, "/other", [{ key: "topic", value: "true" }]);
  const parent = session("parent", "track");
  const worker = session("worker", "parent");
  assert.equal(reviewIntegrationTarget([track, parent, worker], "worker"), null);
  assert.equal(reviewIntegrationTarget([track, worker], "worker"), null);
  assert.match(preparationRequest(false, "HEAD abc", null), /No landing target is resolved/);
  assert.match(preparationRequest(true, "HEAD abc", null), /publishing strategy and destination have not been selected/);
});

test("manual Review Prepare remains optional preparation without landing authorization", () => {
  for (const isTrack of [true, false]) {
    const prompt = preparationRequest(isTrack, "HEAD abc", session("track"), ["src/app.ts"]);
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
  for (const prompt of [preparationRequest(false, "HEAD source", target), independentReviewRequest("HEAD source", target)]) {
    assert.match(prompt, /arachne\/tracks\/accepted/);
    assert.doesNotMatch(prompt, /work\/track/);
    assert.match(prompt, /Do not land, publish, push/);
  }
  assert.match(independentReviewRequest("HEAD source", null), /No same-repository Track is resolved/);
  assert.match(preparationRequest(true, "HEAD source", track), /Canonical publish source: arachne\/tracks\/accepted/);
});
