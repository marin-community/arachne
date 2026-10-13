import assert from "node:assert/strict";
import test from "node:test";
import { nextTick } from "vue";
import { scopeSession, scopedId } from "../src/backendIdentity.ts";
import { createBackendInvoke, scopeBackendResult } from "../src/backendInvoke.ts";
import { acceptSnapshot, combinedFleet, markUnavailable } from "../src/backendFleet.ts";
import { changeRegistry, pinSelection, selectedChangePayload, selectionKey, selectionProblem } from "../src/durableChange.ts";
import { landingCandidates, topicThreadRows } from "../src/topicInspector.ts";
import { effectiveAutoland, resolveTrack } from "../src/trackPolicy.ts";
import { source, declaration, changeTag, acceptanceTag, receipt } from "./fixtures/durable-change.mjs";
import { deferred, mountRoutingComponent, settle } from "./componentRoutingHarness.ts";

const a = "https://a.example", b = "https://b.example";
const change = declaration("same-output");
const root = () => source("track", [{ key: "topic", value: "true" }, { key: "track_topic_branch", value: "accepted/topic" }, { key: "track_autoland", value: "on" }], null);
const worker = (accepted = false) => source("worker", [changeTag(change), ...(accepted ? [acceptanceTag(receipt(change))] : []), { key: "role", value: "reviewer" }]);
const scoped = (url: string, session: any) => scopeSession(url, session);
const snapshot = (selected = change) => ({ source_ref: selected.source_branch, accepted_source: false,
  head_oid: selected.scope.head_oid, version: selected.scope.kind === "working" ? selected.scope.worktree_version : null,
  base: { state: "available", reference: selected.scope.base_oid, oid: selected.scope.base_oid },
  files: [], totals: { files: 0, additions: 0, deletions: 0 }, truncated: false });
const queue = (fleet: any[], url: string) => landingCandidates(topicThreadRows(fleet, scopedId(url, "track"), new Set()), scopedId(url, "track"), fleet);

test("colliding local Change IDs retain independent queues, selections, completion references and receipts", () => {
  const aWorker = scoped(a, worker(true)), bWorker = scoped(b, worker());
  const fleet = [scoped(a, root()), aWorker, scoped(b, root()), bWorker];
  assert.equal(queue(fleet, a)[0].state, "landed");
  assert.equal(queue(fleet, b)[0].state, "waiting", "A acceptance cannot accept B's identical output");
  const aEntry = changeRegistry(aWorker).entries[0], bEntry = changeRegistry(bWorker).entries[0];
  assert.notEqual(aEntry.key, bEntry.key);
  assert.equal(aEntry.acceptance!.target_session, scopedId(a, "track"));
  assert.equal(aEntry.acceptance!.source_session, scopedId(a, "worker"));
  assert.equal(aEntry.acceptance!.completion.message_id, scopedId(a, "result-1"));
  assert.equal(bEntry.completion!.channel_id, scopedId(b, "worker"));
  assert.equal(bEntry.completion!.message_id, scopedId(b, "result-1"));
  assert.deepEqual(aEntry.change, change);
  assert.deepEqual(bEntry.change, change);
  assert.equal(aWorker.branch.tags[0].value, JSON.stringify(change), "stored declarations stay local");
  const selectedA = pinSelection(aWorker.id, change), selectedB = pinSelection(bWorker.id, change);
  assert.notEqual(selectionKey(selectedA), selectionKey(selectedB));
  assert.match(selectionProblem(selectedA, bWorker), /another.*backend/);
  assert.equal(selectionProblem(selectedB, bWorker), "");
  assert.match(selectionProblem(selectedA, aWorker), /already.*accepted/);
  assert.deepEqual(selectedChangePayload(selectedB).expectedChange, change);
  // Even a malformed cross-server ancestry cannot borrow a same-path Track.
  const crossParent = { ...bWorker, parent_session_id: scopedId(a, "track") };
  assert.equal(resolveTrack([fleet[0], crossParent], crossParent.id), null);
  assert.equal(effectiveAutoland(bWorker, fleet[0]), "off");
});

test("Change commands route only to the owning backend and never rewrite result evidence", async () => {
  const calls: any[] = [];
  const invoke = createBackendInvoke(async <T>(command: string, args: any) => {
    calls.push({ command, args });
    return (command === "review_session" ? { session: source("reviewer"), warnings: [] } : { coordinator_id: "track" }) as T;
  });
  for (const url of [a, b]) {
    const selected = pinSelection(scopedId(url, "worker"), change);
    const payload = selectedChangePayload(selected);
    await invoke("change_changes", payload);
    const land = await invoke<any>("land_change", { ...payload, strategy: "squash" });
    assert.equal(land.coordinator_id, scopedId(url, "track"));
    const reviewer = await invoke<any>("review_session", { sourceId: selected.sourceId, changeId: change.id, expectedChange: change, brief: "Review selected output" });
    assert.equal(reviewer.session.id, scopedId(url, "reviewer"));
    await invoke("send_input", { id: selected.sourceId, changeId: change.id, expectedChange: change, text: "Prepare only selected output" });
    assert.ok(calls.slice(-4).every(call => call.args.baseUrl === url && (call.args.sessionId ?? call.args.sourceId ?? call.args.id) === "worker"));
    assert.ok(calls.slice(-4).every(call => JSON.stringify(call.args.expectedChange) === JSON.stringify(change)));
  }
  const count = calls.length;
  await assert.rejects(invoke("land_change", { ...selectedChangePayload(pinSelection(scopedId(b, "worker"), change)), strategy: "squash" }, a), /different/);
  await assert.rejects(invoke("review_session", { sourceId: scopedId(a, "other"), changeId: change.id, expectedChange: change }), /owning source/);
  await assert.rejects(invoke("change_changes", { sessionId: scopedId(a, "worker"), changeId: "other", expectedChange: change }), /exact wire declaration/);
  await assert.rejects(invoke("review_session", { sourceId: scopedId(a, "worker"), expectedChange: change }), /exact wire declaration/);
  await assert.rejects(invoke("send_input", { id: scopedId(a, "other"), changeId: change.id, expectedChange: change, text: "Prepare" }), /owning source/);
  await assert.rejects(invoke("send_input", { id: scopedId(a, "worker"), changeId: change.id, text: "Prepare" }), /exact wire declaration/);
  assert.equal(calls.length, count, "invalid selection fails before transport");
  const inline = { ...change, completion: null }, accepted = receipt(change);
  const reply = scopeBackendResult("track_activity", b, { messages: [{ id: "result-1", channel_id: "worker", payload: { changes: [inline], indexed: change, receipt: accepted } }] });
  assert.equal(reply.messages[0].id, scopedId(b, "result-1"));
  assert.deepEqual(reply.messages[0].payload, { changes: [inline], indexed: change, receipt: accepted });
});

test("rendered inspectors select colliding outputs independently and offline source cannot borrow a healthy backend", async t => {
  const backends = [a, b].map(url => ({ url, name: url, available: false, sessions: [], layout: null } as any));
  for (const backend of backends) acceptSnapshot(backend, [root(), { ...worker(), status: "archived" }], { spaces: [], defaults: [], revision: 0 });
  const calls: any[] = [];
  const transport = async (command: string, args: any) => {
    if (command === "topic_todos") return { todos: [], revision: 0 };
    if (command === "change_queue") return { revision: 0, queue: { version: 1, track_session: "track", repo_root: "/repo", target_branch: "accepted/topic", active: null, pending: [] } };
    calls.push({ command, args });
    return { coordinator_id: "track", coordinator_name: "Track", target_branch: "accepted/topic" };
  };
  const panels = [a, b].map(url => mountRoutingComponent("TopicInspector", { topic: scoped(url, root()), fleet: combinedFleet(backends), selectedId: scopedId(url, "worker"), backendUrl: a, landingRequest: 1 }, transport, true));
  t.after(() => [...panels].reverse().forEach(panel => panel.stop()));
  await settle();
  for (const panel of panels) {
    const select = panel.elements(el => el.type === "button" && el.props.class === "link name")[0];
    select.props.onClick(); await nextTick();
    assert.equal(panel.elements(el => el.type === "button" && el.props.class === "link name")[0].props["aria-pressed"], true);
    assert.equal(panel.state.selectedProblem, "");
    await panel.state.onLand(panel.state.candidates[0], "squash");
  }
  assert.deepEqual(calls.map(call => [call.command, call.args.baseUrl, call.args.sessionId]), [["land_change", a, "worker"], ["land_change", b, "worker"]]);
  assert.deepEqual(calls.map(call => call.args.expectedChange), [change, change]);
  // Source disappearance leaves the old pinned selection explicitly invalid.
  markUnavailable(backends[1], "offline");
  for (const panel of panels) panel.props.fleet = combinedFleet(backends);
  await nextTick();
  assert.equal(panels[1].state.candidates.length, 0);
  assert.match(panels[1].state.selectedProblem, /no longer belongs/);
  const count = calls.length;
  await panels[1].state.onLand(queue([scoped(b, root()), scoped(b, worker())], b)[0], "squash");
  assert.equal(calls.length, count);
  assert.equal(panels[0].state.candidates.length, 1, "healthy A keeps its actionable queue");
  assert.equal(panels[0].state.selectedProblem, "");
});

test("rendered Changes distinguishes unavailable identity and blocks stale review and Land", async t => {
  const track = scoped(a, root()), healthy = scoped(a, worker());
  const calls: string[] = [];
  const panel = mountRoutingComponent("TopicInspector", { topic: track, fleet: [track, healthy], selectedId: healthy.id, backendUrl: a, landingRequest: 1 }, async command => {
    if (command === "topic_todos") return { todos: [], revision: 0 };
    if (command === "change_queue") return { revision: 0, queue: { version: 1, track_session: "track", repo_root: "/repo", target_branch: "accepted/topic", active: null, pending: [] } };
    calls.push(command); return {};
  }, true);
  t.after(panel.stop); await settle();
  panel.elements(el => el.type === "button" && el.props.class === "link name")[0].props.onClick();
  await nextTick();
  assert.equal(panel.state.selectedProblem, "");
  const unavailable = { ...healthy, work_dir: "", change_identity_problem: "unavailable" };
  panel.props.fleet = [track, unavailable]; await nextTick();
  assert.equal(panel.state.candidates[0].state, "unavailable");
  assert.ok(panel.elements(el => el.type === "span" && el.text === "Identity unavailable").length);
  assert.equal(panel.elements(el => el.type === "span" && el.text === "Needs reconciliation").length, 0);
  assert.equal(Boolean(panel.elements(el => el.type === "button" && el.text === "Select / review")[0].props.disabled), true);
  assert.match(panel.state.selectedProblem, /evidence is unavailable/);
  await panel.state.onLand(panel.state.candidates[0], "squash");
  assert.deepEqual(calls, []);
  panel.props.fleet = [track, { ...unavailable, change_identity_problem: "conflict" }]; await nextTick();
  assert.equal(panel.state.candidates[0].state, "blocked");
  assert.ok(panel.elements(el => el.type === "span" && el.text === "Needs reconciliation").length);
  await panel.state.onLand(panel.state.candidates[0], "squash");
  assert.deepEqual(calls, []);
  panel.props.fleet = [track, healthy]; await nextTick();
  assert.equal(panel.state.candidates[0].state, "waiting");
  assert.equal(panel.state.selectedProblem, "");
});

test("actual selected preview, review and Prepare use each backend's local declaration and source", async t => {
  const calls: any[] = [];
  for (const url of [a, b]) {
    const selected = pinSelection(scopedId(url, "worker"), change);
    const panel = mountRoutingComponent("ChangeReview", { backendUrl: url === a ? b : a, sessionId: selected.sourceId, selectedChange: selected, integrationTarget: scoped(url, root()) }, async (command, args) => {
      calls.push({ command, args });
      if (command === "change_changes") return snapshot(args.expectedChange);
      if (command === "review_session") return { session: source("reviewer"), warnings: [] };
      if (command === "send_input") return;
      throw new Error(`Unexpected broad operation: ${command}`);
    }, true);
    t.after(panel.stop); await settle();
    panel.state.prepareRequest("review");
    panel.state.requestText = "Check regression risks.";
    await panel.state.sendRequest();
    panel.state.prepareRequest("prepare");
    await panel.state.sendRequest();
    assert.deepEqual(calls.slice(-4).map(call => call.command), ["change_changes", "review_session", "change_changes", "send_input"]);
    assert.ok(calls.slice(-4).every(call => call.args.baseUrl === url));
    for (const call of calls.slice(-4).filter(call => call.command !== "send_input")) {
      assert.equal(call.args.sessionId ?? call.args.sourceId, "worker");
      assert.deepEqual(call.args.expectedChange, change);
    }
    assert.equal(calls.at(-1).args.id, "worker");
    assert.equal(calls.at(-1).args.changeId, change.id);
    assert.deepEqual(calls.at(-1).args.expectedChange, change, "native preparation preflight receives the same local declaration");
    assert.ok(calls.at(-1).args.text.includes(JSON.stringify(change)));
    assert.ok(!calls.at(-1).args.text.includes(scopedId(url, "track")), "agent target identity is local");
  }
});

test("cross-backend selections and targets fail before dispatch; unavailable selected backend never retries elsewhere", async t => {
  let calls = 0;
  const selected = pinSelection(scopedId(b, "worker"), change);
  const cross = mountRoutingComponent("ChangeReview", { backendUrl: a, sessionId: scopedId(a, "worker"), selectedChange: selected, integrationTarget: scoped(a, root()) }, async () => { calls++; return snapshot(); }, true);
  t.after(cross.stop); await settle();
  assert.match(cross.state.error, /another.*backend/);
  cross.state.prepareRequest("review"); await cross.state.sendRequest();
  assert.equal(calls, 0);
  cross.props.sessionId = selected.sourceId; await nextTick(); await settle();
  assert.match(cross.state.error, /target.*another.*backend/);
  cross.state.prepareRequest("prepare"); await cross.state.sendRequest();
  assert.equal(calls, 0);
  cross.props.integrationTarget = scoped(b, root()); await nextTick(); await settle();
  assert.ok(calls > 0, "correcting the target enables the selected backend preview");
  const urls: string[] = [];
  const offline = mountRoutingComponent("ChangeReview", { backendUrl: a, sessionId: selected.sourceId, selectedChange: selected, integrationTarget: scoped(b, root()) }, async (_command, args) => { urls.push(args.baseUrl); throw new Error("B offline"); }, true);
  t.after(offline.stop); await settle();
  assert.match(offline.state.error, /offline/);
  offline.state.prepareRequest("prepare"); await offline.state.sendRequest();
  assert.deepEqual(urls, [b]);
});

test("switching backend invalidates identical working snapshot requests before preparation delivery", async t => {
  const working = { ...change, scope: { ...change.scope, kind: "working", paths: ["src/a.ts"], worktree_version: "local-version" }, completion: null };
  const preflight = deferred<any>(), calls: any[] = [];
  const panel = mountRoutingComponent("ChangeReview", { backendUrl: a, sessionId: scopedId(a, "worker"), selectedChange: pinSelection(scopedId(a, "worker"), working), integrationTarget: scoped(a, root()) }, async (command, args) => {
    calls.push({ command, args });
    if (command === "change_changes") return calls.length === 2 ? preflight.promise : snapshot(working);
    throw new Error("Retired preparation must not be dispatched");
  });
  t.after(panel.stop); await settle();
  panel.state.prepareRequest("prepare"); const sending = panel.state.sendRequest();
  panel.props.sessionId = scopedId(b, "worker");
  panel.props.selectedChange = pinSelection(scopedId(b, "worker"), working);
  panel.props.integrationTarget = scoped(b, root());
  await nextTick(); await settle();
  preflight.resolve(snapshot(working)); await sending;
  assert.equal(panel.state.requestMode, null);
  assert.equal(calls.some(call => call.command === "send_input"), false);
  assert.ok(calls.some(call => call.args.baseUrl === b));
  assert.ok(calls.every(call => call.args.expectedChange.source_session === "worker" && call.args.expectedChange.completion === null && call.args.expectedChange.scope.worktree_version === "local-version"));
});
