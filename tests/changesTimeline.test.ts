import assert from "node:assert/strict";
import test from "node:test";
import { nextTick } from "vue";
import { scopedId, scopeSession } from "../src/backendIdentity.ts";
import { declaration, source, changeTag, acceptanceTag, receipt } from "./fixtures/durable-change.mjs";
import { deferred, mountRoutingComponent, settle } from "./componentRoutingHarness.ts";

const a = "https://a.example", b = "https://b.example";
const changes = ["first", "second", "third"].map(id => declaration(id));
const entry = (change = changes[0], requested_by: null | "user" | "autoland" = null) => ({ change, strategy: "squash", requested_by });
const queue = (pending: any[] = [], active: any = null, revision = 4) => ({ revision, queue: { version: 1, track_session: "track", repo_root: "/repo", target_branch: "accepted/topic", active, pending } });
function props(url = a, tags = changes.map(changeTag), status = "running") {
  const topic = scopeSession(url, source("track", [{ key: "topic", value: "true" }, { key: "track_topic_branch", value: "accepted/topic" }], null));
  const worker = scopeSession(url, { ...source("worker", tags), status });
  return { topic, fleet: [topic, worker], selectedId: null, backendUrl: url === a ? b : a };
}
const flush = async () => { await nextTick(); await settle(); await nextTick(); };
const text = (panel: any) => panel.elements((el: any) => el.type !== "comment").map((el: any) => el.text ?? "").join(" ");
const button = (panel: any, label: string) => panel.elements((el: any) => el.type === "button" && (el.props["aria-label"] === label || el.text === label))[0];
const queueTransport = (view: any, calls: any[] = []) => async (command: string, args: any) => {
  calls.push({ command, args });
  if (command === "topic_todos") return { todos: [], revision: 0 };
  if (command === "change_queue") return view;
  throw new Error(`Unexpected ${command}`);
};

test("Changes opens first by default, separates receipts and active claims, and protects history from reorder", async t => {
  const tags = [...changes.map(changeTag), acceptanceTag(receipt(changes[2]))];
  const panel = mountRoutingComponent("TopicInspector", props(a, tags, "archived"), queueTransport(queue([entry(changes[1])], entry())), true);
  t.after(panel.stop); await flush();
  const tabs = panel.elements(el => el.props.role === "tab");
  assert.equal(tabs[0].text, "Changes");
  assert.equal(tabs[0].props["aria-selected"], true);
  assert.equal(panel.state.tab, "changes");
  assert.match(text(panel), /In progress.*Acceptance in progress.*Pending.*Landed/);
  assert.equal(panel.elements(el => el.props["aria-label"] === "Active acceptance").length, 1);
  assert.equal(panel.elements(el => el.props["aria-label"] === "Landed history").length, 1);
  assert.ok(!button(panel, "Move Output first up"));
  assert.ok(!button(panel, "Move Output third up"));
  assert.equal(button(panel, "Move Output second up").props.disabled, true);
  assert.equal(button(panel, "Move Output second down").props.disabled, true);
  assert.ok(!text(panel).includes(scopedId(a, "worker")), "protocol IDs are not product copy");
  const primary = button(panel, "Land");
  assert.equal(Boolean(primary.props.disabled), false, "active acceptance does not disable pending Land");
});

test("explicit resource requests stay honored and Track switches reset to Changes", async t => {
  const panel = mountRoutingComponent("TopicInspector", { ...props(), artifactRequest: { resourceId: "artifact" }, landingRequest: 1 }, queueTransport(queue()), true);
  t.after(panel.stop); await flush();
  assert.equal(panel.state.tab, "resources");
  panel.props.artifactRequest = null;
  button(panel, "Threads").props.onClick();
  panel.props.topic = props(b).topic; panel.props.fleet = props(b).fleet;
  await flush();
  assert.equal(panel.state.tab, "changes");
});

test("keyboard move buttons persist exact complete pending order and preserve active authorization", async t => {
  const calls: any[] = [];
  let view = queue([entry(changes[1])], entry());
  const panel = mountRoutingComponent("TopicInspector", props(), async (command, args) => {
    calls.push({ command, args });
    if (command === "topic_todos") return { todos: [], revision: 0 };
    if (command === "change_queue") return view;
    if (command === "reorder_change_queue") {
      view = queue(args.order.map((change: any) => entry(change)), view.queue.active, args.expectedRevision + 1);
      return view;
    }
  }, true);
  t.after(panel.stop); await flush();
  assert.deepEqual(panel.state.timeline.pending.map((row: any) => row.change.id), ["second", "third"]);
  button(panel, "Move Output third up").props.onClick(); await flush();
  const write = calls.find(call => call.command === "reorder_change_queue");
  assert.deepEqual(write.args, { topicId: "track", baseUrl: a, expectedRevision: 4, order: [changes[2], changes[1]] });
  assert.deepEqual(view.queue.active, entry());
  assert.ok(view.queue.pending.every((item: any) => item.requested_by === null));
  assert.deepEqual(panel.state.timeline.pending.map((row: any) => row.change.id), ["third", "second"]);
  assert.match(text(panel), /Moved Output third to position 1/);
});

test("drag/drop uses the same CAS mutation and does not accept external drags", async t => {
  const calls: any[] = [];
  const panel = mountRoutingComponent("TopicInspector", props(), async (command, args) => {
    if (command === "topic_todos") return { todos: [], revision: 0 };
    if (command === "change_queue") return queue(changes.map(change => entry(change)));
    calls.push({ command, args }); return queue(args.order.map((change: any) => entry(change)), null, 5);
  }, true);
  t.after(panel.stop); await flush();
  const rows = panel.elements(el => el.type === "li" && el.props.draggable);
  rows[0].props.onDrop({ preventDefault() {} }); await flush(); assert.equal(calls.length, 0);
  rows[2].props.onDragstart({ dataTransfer: { setData() {} }, preventDefault() {} });
  rows[0].props.onDrop({ preventDefault() {} }); await flush();
  assert.deepEqual(calls[0].args.order, [changes[2], changes[0], changes[1]]);
});

test("direct rendered Land pins the clicked output and reports queued while active remains", async t => {
  const calls: any[] = [];
  let view = queue([entry(changes[1]), entry(changes[2])], entry());
  const panel = mountRoutingComponent("TopicInspector", props(), async (command, args) => {
    if (command === "topic_todos") return { todos: [], revision: 0 };
    if (command === "change_queue") return view;
    calls.push({ command, args });
    view = queue([entry(args.expectedChange, "user"), entry(changes[1])], view.queue.active, 5);
    return { coordinator_id: "track", coordinator_name: "Track", target_branch: "accepted/topic" };
  }, true);
  t.after(panel.stop); await flush();
  const land = panel.elements(el => el.type === "button" && el.text === "Land");
  assert.equal(Boolean(land[1].props.disabled), false);
  land[1].props.onClick(); await flush();
  assert.deepEqual(calls, [{ command: "land_change", args: { sessionId: "worker", changeId: "third", expectedChange: changes[2], strategy: "squash", baseUrl: a } }]);
  assert.match(text(panel), /Queued for accepted\/topic.*Waiting for successful completion/);
  assert.equal(panel.state.timeline.active.change.id, "first");
  assert.deepEqual(panel.state.timeline.pending.map((row: any) => row.change.id), ["third", "second"]);
  assert.equal(panel.state.timeline.history.length, 0);
});

test("CAS failure refreshes and discloses the concurrent order without retrying the write", async t => {
  const calls: any[] = [];
  let view = queue(changes.map(change => entry(change)));
  const panel = mountRoutingComponent("TopicInspector", props(), async (command, args) => {
    calls.push({ command, args });
    if (command === "topic_todos") return { todos: [], revision: 0 };
    if (command === "change_queue") return view;
    view = queue([entry(changes[2]), entry(changes[1]), entry(changes[0])], null, 5);
    throw new Error("Queue revision conflict");
  }, true);
  t.after(panel.stop); await flush();
  button(panel, "Move Output second up").props.onClick(); await flush();
  assert.deepEqual(panel.state.timeline.pending.map((row: any) => row.change.id), ["third", "second", "first"]);
  assert.match(text(panel), /Order was not saved.*revision conflict.*Refreshed/);
  assert.equal(calls.filter(call => call.command === "reorder_change_queue").length, 1);
});

test("queued-but-undelivered Land stays truthful and repeated click retries the same pinned intent", async t => {
  const calls: any[] = [];
  let view = queue();
  const panel = mountRoutingComponent("TopicInspector", props(), async (command, args) => {
    if (command === "topic_todos") return { todos: [], revision: 0 };
    if (command === "change_queue") return view;
    calls.push({ command, args });
    view = queue([entry(args.expectedChange, "user")], null, 5);
    throw new Error("Change queued, but coordinator delivery failed. Retry Land to deliver.");
  }, true);
  t.after(panel.stop); await flush();
  button(panel, "Land").props.onClick(); await flush();
  assert.match(text(panel), /queued, but coordinator delivery failed/);
  assert.equal(panel.state.timeline.pending[0].status, "Queued");
  assert.equal(panel.state.timeline.history.length, 0);
  button(panel, "Land").props.onClick(); await flush();
  assert.equal(calls.length, 2); assert.deepEqual(calls[1], calls[0]);
});

test("old backend reads and writes cannot replace a new Track view, including A to B to A", async t => {
  const read = deferred<any>(), write = deferred<any>(), calls: any[] = [];
  let reads = 0;
  const panel = mountRoutingComponent("TopicInspector", props(), async (command, args) => {
    calls.push({ command, args });
    if (command === "topic_todos") return { todos: [], revision: 0 };
    if (command === "change_queue") return ++reads === 1 ? read.promise : queue([entry(changes[1])]);
    return write.promise;
  }, true);
  t.after(panel.stop); await flush();
  panel.props.topic = props(b).topic; panel.props.fleet = props(b).fleet;
  await flush();
  assert.equal(panel.state.queueView.queue.pending[0].change.id, "second");
  read.resolve(queue([entry(changes[0])], null, 99)); await flush();
  assert.equal(panel.state.queueView.revision, 4);
  const moving = panel.state.movePending(1, 0); await flush();
  assert.equal(panel.state.queueBusy, true);
  panel.props.topic = props(a).topic; panel.props.fleet = props(a).fleet; await flush();
  assert.equal(panel.state.queueBusy, false);
  write.resolve(queue([entry(changes[2])], null, 88)); await moving; await flush();
  assert.equal(panel.state.queueView.revision, 4);
  assert.equal(panel.state.orderNote, "");
  assert.ok(calls.some(call => call.args.baseUrl === b));
});

test("retired Land completion cannot clear a current operation or write feedback across identical local Track IDs", async t => {
  const old = deferred<any>(), current = deferred<any>();
  const panel = mountRoutingComponent("TopicInspector", props(), async (command, args) => {
    if (command === "topic_todos") return { todos: [], revision: 0 };
    if (command === "change_queue") return queue();
    return args.baseUrl === a ? old.promise : current.promise;
  }, true);
  t.after(panel.stop); await flush();
  const first = panel.state.onLand(panel.state.candidates[0], "squash");
  panel.props.topic = props(b).topic; panel.props.fleet = props(b).fleet; await flush();
  const second = panel.state.onLand(panel.state.candidates[0], "merge");
  const busy = panel.state.integratingId;
  old.resolve({ target_branch: "old/topic", coordinator_name: "old" }); await first; await flush();
  assert.equal(panel.state.integratingId, busy); assert.equal(panel.state.integrationNote, "");
  current.resolve({ target_branch: "accepted/topic", coordinator_name: "new" }); await second; await flush();
  assert.equal(panel.state.integratingId, null); assert.match(panel.state.integrationNote, /Queued for accepted/);
});

test("unavailable and altered queue sources remain visible and disable complete-set reorder and Land", async t => {
  const panel = mountRoutingComponent("TopicInspector", props(), queueTransport(queue([entry()], entry(changes[1]))), true);
  t.after(panel.stop); await flush();
  panel.props.fleet = [panel.props.topic]; await flush();
  assert.equal(panel.state.timeline.active.change.id, "second");
  assert.match(text(panel), /Acceptance in progress.*Source unavailable/);
  assert.equal(panel.state.timeline.pending[0].change.id, "first");
  assert.equal(button(panel, "Move Output first up").props.disabled, true);
  assert.ok(!button(panel, "Land"));
});

test("corrupt and offline queue replies fail closed on the owning backend without fallback", async t => {
  const calls: any[] = [];
  const panel = mountRoutingComponent("TopicInspector", props(b), async (command, args) => {
    calls.push({ command, args });
    if (command === "topic_todos") return { todos: [], revision: 0 };
    return { ...queue(), queue: { ...queue().queue, track_session: "foreign" } };
  }, true);
  t.after(panel.stop); await flush();
  assert.match(text(panel), /invalid or belongs to another Track/);
  assert.equal(Boolean(button(panel, "Land").props.disabled), true);
  await panel.state.onLand(panel.state.candidates[0], "squash");
  assert.ok(calls.every(call => call.args.baseUrl === b));
  assert.ok(!calls.some(call => call.command === "land_change"));
});
