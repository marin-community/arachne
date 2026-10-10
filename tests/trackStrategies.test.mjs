import assert from "node:assert/strict";
import test from "node:test";
import { landStrategies, publishStrategies, selectedStrategy, strategyMemoryKey } from "../src/trackStrategies.ts";

test("worker Land offers acceptance strategies, and cherry-pick requires known commits", () => {
  assert.deepEqual(landStrategies(true).map(o => o.value), ["squash", "merge", "rebase", "cherry-pick"]);
  assert.deepEqual(landStrategies(false).map(o => o.value), ["squash", "merge", "rebase"]);
  assert.equal(selectedStrategy("land", landStrategies(), null), "squash");
  assert.equal(selectedStrategy("land", landStrategies(), "open-pr"), "squash");
  assert.equal(selectedStrategy("land", landStrategies(), "merge"), "merge");
  assert.equal(selectedStrategy("land", landStrategies(false), "cherry-pick"), "squash");
});

test("Publish has explicit Single PR/PR stack, local primary requires local server", () => {
  const remote = publishStrategies(false);
  const local = publishStrategies(true);
  assert.equal(remote.find(o => o.value === "open-pr").label, "Single PR");
  assert.equal(remote.find(o => o.value === "pr-stack").label, "PR stack");
  assert.equal(remote.some(o => o.value === "land-locally"), false);
  assert.match(local.find(o => o.value === "land-locally").label, /local primary current branch/);
  assert.equal(selectedStrategy("publish", local, null), "open-pr");
  assert.equal(selectedStrategy("publish", remote, "land-locally"), "open-pr");
  assert.equal(selectedStrategy("publish", remote, "pr-stack"), "pr-stack");
});

test("strategy memory separates Land/Publish, repositories, and old Land publishing preferences", () => {
  assert.notEqual(strategyMemoryKey("land", "/a"), strategyMemoryKey("publish", "/a"));
  assert.notEqual(strategyMemoryKey("land", "/a"), strategyMemoryKey("land", "/b"));
  assert.notEqual(strategyMemoryKey("land", "/a"), "arachne:strategy:land:/a");
  assert.equal(selectedStrategy("land", [{ value: "merge", label: "Merge" }], null), "merge");
  assert.equal(selectedStrategy("land", [], null), "");
});
