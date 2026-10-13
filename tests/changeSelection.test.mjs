import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import ts from "typescript";
import { ref, computed, watch, reactive, nextTick } from "vue";
import * as identities from "../src/backendIdentity.ts";
import * as changes from "../src/durableChange.ts";
import * as review from "../src/reviewRequest.ts";
import * as inspector from "../src/topicInspector.ts";
import * as policy from "../src/trackPolicy.ts";
import * as strategies from "../src/trackStrategies.ts";
import * as queues from "../src/changeQueue.ts";
import { declaration, source, oid, changeTag } from "./fixtures/durable-change.mjs";

const component = name => readFileSync(new URL(`../src/components/${name}.vue`, import.meta.url), "utf8");
const clean = value => JSON.parse(JSON.stringify(value));
const flush = async () => { await nextTick(); await new Promise(resolve => setImmediate(resolve)); };
function harness(name, input, invoke, exposed) {
  const script = component(name).match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1].replace(/^import[\s\S]*?;\n/gm, "");
  const props = reactive(input), events = [];
  const context = { ref, computed, watch, onBeforeUnmount() {}, ...queues, ...identities, ...changes, ...review, ...inspector, ...policy, ...strategies,
    useBackendInvoke: () => invoke, useLaunchSelection: () => ref({ agent: "codex", model: "test", effort: "high" }),
    localStorage: { setItem() {} },
    defineProps: () => props, withDefaults: value => value,
    defineEmits: () => (...event) => events.push(event), invoke };
  vm.createContext(context);
  vm.runInContext(ts.transpileModule(`${script}\nglobalThis.exposed = { ${exposed.join(",")} };`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None },
  }).outputText, context);
  return { ...context.exposed, props, events };
}
const snapshot = change => ({ source_ref: change.source_branch, accepted_source: false, head_oid: change.scope.head_oid,
  version: change.scope.kind === "working" ? change.scope.worktree_version : null,
  base: { state: "available", reference: change.scope.base_oid, oid: change.scope.base_oid },
  totals: { files: 1, additions: 1, deletions: 0, truncated: false }, files: [], truncated: false });
const track = () => source("track", [{ key: "topic", value: "true" }, { key: "track_topic_branch", value: "accepted/topic" }], null);
const reviewProps = change => ({ sessionId: change.source_session, selectedChange: changes.pinSelection(change.source_session, change), selectionError: "",
  isTopic: false, integrationTarget: track(), canEdit: true, embedded: true });
const reviewExpose = ["changes", "refresh", "prepareRequest", "sendRequest", "requestText", "requestNote", "requestMode", "checkoutEditable"];

test("actual review preview, reviewer launch and preparation carry only the selected declaration", async () => {
  const change = declaration("second", undefined, { base_oid: oid("b"), head_oid: oid("d"), paths: ["src/second.ts"] });
  const calls = [];
  const h = harness("ChangeReview", reviewProps(change), async (command, args) => {
    calls.push([command, clean(args)]);
    if (command === "change_changes") return snapshot(args.expectedChange);
    if (command === "review_session") return { session: { id: "reviewer" }, warnings: [] };
    if (command === "send_input") return;
    throw new Error(`Unexpected broad command ${command}`);
  }, reviewExpose);
  await flush();
  assert.deepEqual(calls[0], ["change_changes", changes.selectedChangePayload(changes.pinSelection(change.source_session, change))]);
  assert.equal(h.checkoutEditable.value, false); // Immutable review cannot silently save outside its scope.
  h.prepareRequest("review");
  assert.ok(h.requestText.value.includes(JSON.stringify(change)));
  // Editing the brief cannot remove the mandatory selected scope from dispatch.
  h.requestText.value = "Check regression risks.";
  await h.sendRequest();
  const reviewer = calls.find(([command]) => command === "review_session")[1];
  assert.equal(reviewer.sourceId, "worker");
  assert.equal(reviewer.changeId, "second");
  assert.deepEqual(reviewer.expectedChange, change);
  assert.ok(reviewer.brief.includes(JSON.stringify(change)));
  assert.match(reviewer.brief, /Do not replace|Do not expand/);
  h.prepareRequest("prepare");
  await h.sendRequest();
  assert.deepEqual(calls.slice(-2).map(([command]) => command), ["change_changes", "send_input"]);
  assert.ok(calls.at(-1)[1].text.includes(JSON.stringify(change)));
  assert.match(calls.at(-1)[1].text, /Do not prepare unselected source changes/);
  assert.equal(calls.some(([command]) => command === "work_changes"), false);
});

test("switching output in the same Thread drops old preview responses and review drafts", async () => {
  const first = declaration("first"), second = declaration("second", undefined, { head_oid: oid("d") });
  let finishFirst;
  const h = harness("ChangeReview", reviewProps(first), async (command, args) => {
    if (args.expectedChange.id === "first") return new Promise(resolve => { finishFirst = resolve; });
    return snapshot(second);
  }, reviewExpose);
  h.props.selectedChange = changes.pinSelection(second.source_session, second);
  await flush();
  assert.equal(h.changes.value.head_oid, second.scope.head_oid);
  finishFirst(snapshot(first));
  await flush();
  assert.equal(h.changes.value.head_oid, second.scope.head_oid);
  h.prepareRequest("review");
  h.props.selectedChange = changes.pinSelection(first.source_session, first);
  await flush();
  assert.equal(h.requestMode.value, null);
});

test("metadata becoming stale during preparation preflight prevents source delivery", async () => {
  const change = declaration(), calls = [];
  let finishPreflight;
  const h = harness("ChangeReview", reviewProps(change), async (command, args) => {
    calls.push([command, args]);
    if (calls.length === 1) return snapshot(change);
    if (command === "change_changes") return new Promise(resolve => { finishPreflight = resolve; });
    throw new Error("Preparation must not be delivered after scope changed");
  }, reviewExpose);
  await flush();
  h.prepareRequest("prepare");
  const request = h.sendRequest();
  h.props.selectionError = "The declaration changed. Reselect it for fresh review.";
  await flush();
  finishPreflight(snapshot(change));
  await request;
  assert.equal(calls.some(([command]) => command === "send_input"), false);
  assert.equal(h.requestMode.value, null);
  assert.equal(h.changes.value, null);
});

test("canonical target changes invalidate drafts and cancel selected preparation in flight", async () => {
  const change = declaration(), calls = [];
  let finish;
  const h = harness("ChangeReview", reviewProps(change), async (command, args) => {
    calls.push(command);
    if (calls.length === 2) return new Promise(resolve => { finish = resolve; });
    return snapshot(change);
  }, reviewExpose);
  await flush();
  h.prepareRequest("prepare");
  const request = h.sendRequest();
  h.props.integrationTarget.branch.tags.find(tag => tag.key === "track_topic_branch").value = "accepted/new";
  await flush();
  finish(snapshot(change));
  await request;
  assert.equal(calls.includes("send_input"), false);
  assert.equal(h.requestMode.value, null);
});

test("actual unselected review refuses preparation and permits read-only inspection", async () => {
  const calls = [], change = declaration();
  const h = harness("ChangeReview", { ...reviewProps(change), selectedChange: null }, async command => {
    calls.push(command); return snapshot(change);
  }, reviewExpose);
  await flush();
  h.prepareRequest("prepare");
  assert.equal(h.requestMode.value, null);
  await h.sendRequest();
  assert.deepEqual(calls, ["work_changes"]);
  h.prepareRequest("review");
  assert.match(h.requestText.value, /unselected inspection only/);
});

test("inspector selection and Land never substitute sibling, later scope or completion", async () => {
  const worker = source(), first = declaration("first"), second = declaration("second");
  worker.branch.tags = [changeTag(first), changeTag(second)];
  const root = track(), calls = [];
  const h = harness("TopicInspector", { topic: root, fleet: [root, worker], selectedId: worker.id, loomUrl: "http://localhost", landingRequest: 1 }, async (command, args) => {
    if (command === "topic_todos") return { todos: [], revision: 0 };
    if (command === "change_queue") return { revision: 0, queue: { version: 1, track_session: "track", repo_root: "/repo", target_branch: "accepted/topic", active: null, pending: [] } };
    calls.push([command, clean(args)]);
    return { coordinator_id: "track", coordinator_name: "track", target_branch: "accepted/topic" };
  }, ["selectCandidate", "reviewCandidate", "onLand", "selectedChange", "selectedProblem", "candidates", "tab"]);
  await flush();
  const one = h.candidates.value.find(candidate => candidate.key === "worker/first");
  const two = h.candidates.value.find(candidate => candidate.key === "worker/second");
  h.selectCandidate(one);
  await h.onLand(two, "squash");
  assert.deepEqual(calls[0], ["land_change", { ...changes.selectedChangePayload(changes.pinSelection(second.source_session, second)), strategy: "squash" }]);
  calls.length = 0;
  await h.onLand(one, "squash");
  assert.deepEqual(calls[0], ["land_change", { ...changes.selectedChangePayload(changes.pinSelection(first.source_session, first)), strategy: "squash" }]);
  const updated = { ...first, scope: { ...first.scope, head_oid: oid("d") }, completion: { channel_id: "worker", message_id: "new-result" } };
  h.props.fleet[1].branch.tags[0] = changeTag(updated);
  await flush();
  assert.deepEqual(clean(h.selectedChange.value.change), first);
  assert.match(h.selectedProblem.value, /changed.*Reselect/);
  await h.onLand(one, "squash"); // Old rendered row cannot silently substitute the fresh declaration.
  assert.equal(calls.length, 1);
  h.reviewCandidate(h.candidates.value.find(candidate => candidate.key === "worker/first"));
  assert.equal(h.selectedProblem.value, "");
  assert.equal(h.tab.value, "review");
  assert.deepEqual(clean(h.selectedChange.value.change), updated);
  await h.onLand(h.candidates.value.find(candidate => candidate.key === "worker/first"), "merge");
  assert.deepEqual(calls[1], ["land_change", { ...changes.selectedChangePayload(changes.pinSelection(updated.source_session, updated)), strategy: "merge" }]);
});

test("chat has no thread-only Land or autoland control; explicit outputs open the inspector", () => {
  const chat = component("ThreadView"), panel = component("TopicInspector");
  assert.doesNotMatch(chat, /land_thread|integrate_session|land_change|AutolandControl|kind="land"/);
  assert.match(chat, /v-if="isWorker && hasExplicitChanges && integrationTarget" @click="emit\('open-changes'\)"/);
  assert.match(chat, /kind="publish"[^\n]+@run="onPublish"/);
  assert.match(panel, /No declared Changes/);
  assert.match(panel, /:key="row.key"/);
  assert.match(panel, /AutolandControl :session="row.session"/); // Policies remain useful even for zero-output sources.
});
