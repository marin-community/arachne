import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import ts from "typescript";
import { ref, computed, watch, reactive, nextTick } from "vue";
import { trackAutoland, threadAutoland, effectiveAutoland } from "../src/trackPolicy.ts";

const session = (id, value, key = "track_autoland", revision = "before") => ({
  id, status: "running", branch: { id: `branch-${id}`, branch: `weaver/${id}`, repo_root: "/repo",
    tags: [{ key, value, set_at: revision }] },
});

// Run the component's actual setup/change code with mocked invoke, avoiding a
// second implementation of its state transition and any live Tauri/provider.
function harness(props, invoke) {
  const source = readFileSync(new URL("../src/components/AutolandControl.vue", import.meta.url), "utf8")
    .match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1].replace(/^import .*;\n/gm, "");
  const events = [];
  const state = reactive(props);
  const context = { ref, computed, watch, trackAutoland, threadAutoland, effectiveAutoland,
    defineProps: () => state, defineEmits: () => (...event) => events.push(event), useBackendInvoke: () => invoke };
  vm.createContext(context);
  vm.runInContext(ts.transpileModule(`${source}\nglobalThis.control = { change, policy, effective, warning, busy };`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None },
  }).outputText, context);
  return { ...context.control, props: state, events };
}

for (const [before, after] of [["off", "on"], ["on", "off"]]) {
  test(`saved ${after} survives setup warning and a delayed ${before} fleet snapshot`, async () => {
    const track = session("track", before);
    const h = harness({ session: track, track, coordinator: true }, async (command, args) => {
      assert.equal(command, "set_autoland");
      assert.equal(args.policy, after);
      return { session: session("track", after, "track_autoland", ""),
        warnings: [`Autoland policy ${after} was saved, but setup is incomplete: route unavailable.`] };
    });
    const target = { value: after };
    await h.change({ target });
    assert.equal(target.value, after);
    assert.equal(h.policy.value, after);
    assert.match(h.warning.value, /was saved.*setup is incomplete/);
    assert.equal(h.events.filter(([kind]) => kind === "updated").length, 1);
    assert.equal(h.events.some(([kind]) => kind === "error"), false);
    h.props.session = session("track", before);
    await nextTick();
    assert.equal(h.policy.value, after);
    // A later live tag revision is authoritative even if another actor
    // changed it back. Setup warnings remain visible until another action.
    h.props.session = session("track", before, "track_autoland", "later");
    await nextTick();
    assert.equal(h.policy.value, before);
    assert.match(h.warning.value, /setup is incomplete/);
  });
}

test("a rejected pre-save action restores live policy and reports an error", async () => {
  const track = session("track", "off");
  const h = harness({ session: track, track, coordinator: true }, async () => { throw new Error("tag write rejected"); });
  const target = { value: "on" };
  await h.change({ target });
  assert.equal(target.value, "off");
  assert.equal(h.events.some(([kind]) => kind === "updated"), false);
  assert.equal(h.events.find(([kind]) => kind === "error")[1], "tag write rejected");
});

test("effective source policy follows live Track changes after a saved inherit", async () => {
  const worker = session("worker", "on", "thread_autoland");
  const h = harness({ session: worker, track: session("track", "on"), coordinator: false },
    async () => ({ session: session("worker", "inherit", "thread_autoland", "saved"), warnings: [] }));
  await h.change({ target: { value: "inherit" } });
  assert.equal(h.policy.value, "inherit");
  assert.equal(h.effective.value, "on");
  h.props.track = session("track", "off", "track_autoland", "later");
  await nextTick();
  assert.equal(h.effective.value, "off");
  h.props.session = session("worker", "off", "thread_autoland", "later");
  await nextTick();
  h.props.track = session("track", "on");
  await nextTick();
  assert.equal(h.policy.value, "off");
  assert.equal(h.effective.value, "off");
});

test("an in-flight save for another selected session cannot replace the new control", async () => {
  let resolve;
  const h = harness({ session: session("one", "off"), track: session("one", "off"), coordinator: true },
    () => new Promise(done => { resolve = done; }));
  const change = h.change({ target: { value: "on" } });
  h.props.session = session("two", "off");
  h.props.track = h.props.session;
  await nextTick();
  resolve({ session: session("one", "on"), warnings: ["setup warning"] });
  await change;
  assert.equal(h.policy.value, "off");
  assert.equal(h.warning.value, "");
  assert.equal(h.events.find(([kind]) => kind === "updated")[1].id, "one");
});

test("a newer live policy arriving during a save supersedes an unavailable-refresh fallback", async () => {
  let resolve;
  const track = session("track", "off");
  const h = harness({ session: track, track, coordinator: true },
    () => new Promise(done => { resolve = done; }));
  const change = h.change({ target: { value: "on" } });
  h.props.session = session("track", "off", "track_autoland", "newer-live-revision");
  await nextTick();
  resolve({ session: session("track", "on", "track_autoland", ""), warnings: ["Policy on was saved; refresh unavailable."] });
  await change;
  assert.equal(h.policy.value, "off");
  assert.match(h.warning.value, /saved.*refresh unavailable/);
  assert.equal(h.events.some(([kind]) => kind === "updated"), false);
});
