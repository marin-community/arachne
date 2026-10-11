import assert from "node:assert/strict";
import test from "node:test";
import { ensureLaunchConfig, launchConfigFor, launchSelectionFor, saveLaunchSelection } from "../src/launch.ts";

test("launch catalogs stay per URL when requests finish out of order", async () => {
  const a = "https://catalog-a.example", b = "https://catalog-b.example";
  const pending = new Map<string, (value: unknown) => void>();
  const calls: string[] = [];
  const fetch = async <T>(command: string, _args: Record<string, unknown> = {}, url?: string | null): Promise<T> => {
    assert.equal(command, "launch_options"); calls.push(url!);
    return new Promise(resolve => pending.set(url!, value => resolve(value as T)));
  };
  const requestA = ensureLaunchConfig(`${a}/`, fetch);
  const requestB = ensureLaunchConfig(b, fetch);
  const duplicateA = ensureLaunchConfig(a, fetch);
  assert.deepEqual(calls, [a, b]);
  assert.equal(launchConfigFor(a), undefined);
  pending.get(b)!({ agents: [{ kind: "b", models: [{ id: "b-model" }] }], profiles: [], default_agent: "b" });
  await requestB;
  assert.equal(launchConfigFor(a), undefined);
  assert.equal(launchConfigFor(b)?.default_agent, "b");
  pending.get(a)!({ agents: [{ kind: "a", models: [{ id: "a-model" }] }], profiles: [], default_agent: "a" });
  await Promise.all([requestA, duplicateA]);
  assert.equal(launchConfigFor(a)?.default_agent, "a");
  assert.equal(launchConfigFor(b)?.default_agent, "b");
  assert.equal(launchConfigFor(a)?.agents[0].models[0].id, "a-model");
  assert.equal(launchConfigFor("https://unseen.example"), undefined);
  await assert.rejects(ensureLaunchConfig(undefined, fetch), /explicit/);
});


test("remembered launch selections stay per backend and default changes cannot migrate them", () => {
  const a = "https://selection-a.example", b = "https://selection-b.example";
  const saved = { agent: "a-agent", model: "a-model", effort: "high" };
  saveLaunchSelection(a, saved);
  assert.deepEqual(launchSelectionFor(a), saved);
  assert.deepEqual(launchSelectionFor(b), { agent: "", model: "", effort: "" });
  saveLaunchSelection(b, { agent: "b-agent", model: "b-model", effort: "low" });
  assert.deepEqual(launchSelectionFor(a), saved);
  assert.equal(launchSelectionFor(b).model, "b-model");
});
