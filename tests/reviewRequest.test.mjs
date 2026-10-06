import assert from "node:assert/strict";
import test from "node:test";
import { preparationRequest, reviewIntegrationTarget } from "../src/reviewRequest.ts";

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
  assert.equal(reviewIntegrationTarget([track, parent, worker], "worker"), parent);
  assert.equal(reviewIntegrationTarget([track, worker], "worker"), null);
  assert.match(preparationRequest(false, "HEAD abc", null), /No integration target is resolved/);
  assert.match(preparationRequest(true, "HEAD abc", null), /landing strategy and destination have not been selected/);
});
