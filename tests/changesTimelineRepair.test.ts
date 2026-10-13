import assert from "node:assert/strict";
import test from "node:test";
import { nextTick } from "vue";
import { scopeSession } from "../src/backendIdentity.ts";
import { compareQueueSnapshots, validateQueueView } from "../src/changeQueue.ts";
import { declaration, source, changeTag } from "./fixtures/durable-change.mjs";
import { deferred, mountRoutingComponent, settle } from "./componentRoutingHarness.ts";

const url = "https://a.example";
const changes = ["first", "second"].map(id => declaration(id));
const entry = (change: any) => ({ change, strategy: "squash", requested_by: null });
const queue = (revision = 4, pending = changes.map(entry), active: any = null) => ({ revision,
  queue: { version: 1, track_session: "track", repo_root: "/repo", target_branch: "accepted/topic", active, pending } });
const claimed = (revision = 6) => queue(revision, [entry(changes[0])], { ...entry(changes[1]), requested_by: "autoland" });
function props(baseUrl = url) {
  const topic = scopeSession(baseUrl, source("track", [{ key: "topic", value: "true" }, { key: "track_topic_branch", value: "accepted/topic" }], null));
  return { topic, fleet: [topic, scopeSession(baseUrl, source("worker", changes.map(changeTag)))], selectedId: null, backendUrl: baseUrl, landingRequest: 0 };
}
const flush = async () => { await nextTick(); await settle(); await nextTick(); };
const activeCandidate = (panel: any) => panel.state.candidates.find((row: any) => row.change.id === "second");
const landButtons = (panel: any) => panel.elements((el: any) => el.type === "button" && el.text === "Land");
const transport = (getQueue: () => any, write?: () => any) => async (command: string) => {
  if (command === "topic_todos") return { todos: [], revision: 0 };
  if (command === "change_queue") return getQueue();
  if (command === "reorder_change_queue" && write) return write();
  throw new Error(`Unexpected fixture command ${command}`);
};

for (const readInFlight of [false, true]) {
  test(`F1 delayed reorder preserves newer active claim with fleet read ${readInFlight ? "still in flight" : "already applied"}`, async t => {
    const write = deferred<any>(), read = deferred<any>();
    let view: any = queue();
    const panel = mountRoutingComponent("TopicInspector", props(), transport(() => view, () => write.promise), true);
    t.after(panel.stop); await flush();
    const moving = panel.state.movePending(1, 0); await flush();
    view = readInFlight ? read.promise : claimed();
    panel.props.fleet[1].last_activity_at = "2026-10-12T01:00:00Z";
    await flush();
    if (!readInFlight) assert.equal(panel.state.queueView.revision, 6);
    write.resolve(queue(5, [entry(changes[1]), entry(changes[0])]));
    await moving; await flush();
    if (readInFlight) {
      assert.equal(panel.state.queueLoading, true, "write reply must not retire the newer read");
      assert.equal(panel.state.canLand(activeCandidate(panel)), false, "actions await the newer authoritative read");
      assert.ok(landButtons(panel).every((button: any) => button.props.disabled));
      read.resolve(claimed()); await flush();
    }
    assert.equal(panel.state.queueView.revision, 6);
    assert.equal(panel.state.timeline.active.change.id, "second");
    assert.equal(panel.state.canLand(activeCandidate(panel)), false);
    assert.equal(landButtons(panel).length, 1, "active claim has no Land control");
    assert.equal(Boolean(landButtons(panel)[0].props.disabled), false, "other pending Change remains eligible");
  });
}

for (const higherArrivesFirst of [false, true]) {
  test(`F1 overlapping reads apply revisions monotonically when higher revision arrives ${higherArrivesFirst ? "first" : "last"}`, async t => {
    const higher = deferred<any>(), lower = deferred<any>();
    let reads = 0;
    const panel = mountRoutingComponent("TopicInspector", props(), transport(() => ++reads === 1 ? queue() : reads === 2 ? higher.promise : lower.promise), true);
    t.after(panel.stop); await flush();
    const first = panel.state.refreshQueue(), second = panel.state.refreshQueue();
    if (higherArrivesFirst) {
      higher.resolve(claimed()); await first;
      assert.equal(panel.state.queueLoading, true, "latest read still owns loading");
      lower.resolve(queue(5)); await second;
    } else {
      lower.resolve(queue(5)); await second;
      assert.equal(panel.state.queueView.revision, 5);
      higher.resolve(claimed()); await first;
    }
    await flush();
    assert.equal(panel.state.queueView.revision, 6);
    assert.equal(panel.state.timeline.active.change.id, "second");
    assert.equal(panel.state.canLand(activeCandidate(panel)), false);
    assert.equal(panel.state.queueLoading, false);
  });
}

test("F1 lower read after write cannot roll back committed order", async t => {
  const write = deferred<any>(), read = deferred<any>();
  let view: any = queue();
  const panel = mountRoutingComponent("TopicInspector", props(), transport(() => view, () => write.promise), true);
  t.after(panel.stop); await flush();
  const moving = panel.state.movePending(1, 0);
  view = read.promise;
  const reading = panel.state.refreshQueue();
  write.resolve(queue(5, [entry(changes[1]), entry(changes[0])])); await moving;
  assert.equal(panel.state.queueLoading, true);
  read.resolve(queue(4)); await reading; await flush();
  assert.equal(panel.state.queueView.revision, 5);
  assert.deepEqual(panel.state.timeline.pending.map((row: any) => row.change.id), ["second", "first"]);
});

test("F1 lower revisions cannot revive actions after an invalid reply; high water survives errors", async t => {
  let view: any = claimed();
  const panel = mountRoutingComponent("TopicInspector", props(), transport(() => view), true);
  t.after(panel.stop); await flush();
  view = { ...queue(7), queue: { ...queue(7).queue, track_session: "foreign" } };
  await panel.state.refreshQueue();
  assert.equal(panel.state.queueView, null);
  view = queue(5); await panel.state.refreshQueue(); await flush();
  assert.equal(panel.state.queueView, null);
  assert.ok(landButtons(panel).every((button: any) => button.props.disabled));
  view = claimed(); await panel.state.refreshQueue();
  assert.equal(panel.state.queueView.revision, 6);
  assert.equal(panel.state.timeline.active.change.id, "second");
});

test("F1 equal-revision conflicting read data fails closed until a greater validated revision", async t => {
  let view: any = claimed();
  const panel = mountRoutingComponent("TopicInspector", props(), transport(() => view), true);
  t.after(panel.stop); await flush();
  view = queue(6); await panel.state.refreshQueue(); await flush();
  assert.equal(panel.state.queueView, null);
  assert.match(panel.state.queueError, /Conflicting Changes queue data/);
  assert.equal(panel.state.canReorder, false);
  assert.equal(panel.state.canLand(activeCandidate(panel)), false);
  view = claimed(); await panel.state.refreshQueue();
  assert.equal(panel.state.queueView, null, "same revision cannot silently reconcile conflicting evidence");
  view = claimed(7); await panel.state.refreshQueue();
  assert.equal(panel.state.queueView.revision, 7);
  assert.equal(panel.state.queueError, "");
  assert.equal(panel.state.canLand(activeCandidate(panel)), false);
});

test("F1 equal-revision conflicting write reply fails closed and does not report a successful move", async t => {
  const write = deferred<any>();
  let view = queue();
  const panel = mountRoutingComponent("TopicInspector", props(), transport(() => view, () => write.promise), true);
  t.after(panel.stop); await flush();
  const moving = panel.state.movePending(1, 0);
  view = claimed(); await panel.state.refreshQueue();
  write.resolve(queue(6, [entry(changes[1]), entry(changes[0])]));
  await moving; await flush();
  assert.equal(panel.state.queueView, null);
  assert.match(panel.state.queueError, /Conflicting/);
  assert.match(panel.state.orderNote, /Could not verify/);
  assert.doesNotMatch(panel.state.orderNote, /Moved Output/);
  assert.ok(landButtons(panel).every((button: any) => button.props.disabled));
});

test("F1 monotonic state resets only on owner epoch change and fences the retired conflict reply", async t => {
  const old = deferred<any>();
  let reads = 0;
  const panel = mountRoutingComponent("TopicInspector", props(), transport(() => ++reads === 1 ? claimed(20) : reads === 2 ? old.promise : queue(1)), true);
  t.after(panel.stop); await flush();
  const reading = panel.state.refreshQueue();
  panel.props.topic = props("https://b.example").topic;
  panel.props.fleet = props("https://b.example").fleet;
  await flush();
  assert.equal(panel.state.queueView.revision, 1);
  old.resolve(queue(20)); await reading;
  assert.equal(panel.state.queueView.revision, 1);
  assert.equal(panel.state.queueError, "");
});

test("F1 validated equal revisions compare exact queue data while tolerating wire property order", () => {
  const topic = props().topic;
  const first = validateQueueView(queue(), topic);
  const same = validateQueueView({ queue: { ...queue().queue, pending: queue().queue.pending.map(item => ({ requested_by: item.requested_by, strategy: item.strategy, change: item.change })) }, revision: 4 }, topic);
  assert.equal(compareQueueSnapshots(first, same), "same");
  assert.equal(compareQueueSnapshots(first, validateQueueView(queue(3), topic)), "older");
  assert.equal(compareQueueSnapshots(first, validateQueueView(queue(5), topic)), "newer");
  assert.throws(() => compareQueueSnapshots(first, validateQueueView(claimed(4), topic)), /Conflicting/);
});

test("F2 later explicit Changes navigation wins over a still-selected artifact", async t => {
  const panel = mountRoutingComponent("TopicInspector", props(), transport(() => queue()), true);
  t.after(panel.stop); await flush();
  panel.props.artifactRequest = { resourceId: "first-artifact" }; await flush();
  assert.equal(panel.state.tab, "resources");
  panel.props.landingRequest++; await flush();
  assert.equal(panel.state.tab, "changes");
  assert.ok(panel.props.artifactRequest, "App keeps the selected artifact when it requests Changes");
  panel.props.artifactRequest = { resourceId: "second-artifact" }; await flush();
  assert.equal(panel.state.tab, "resources", "a newly opened artifact still navigates to Resources");
});

test("F2 initial artifact navigation wins over an initial Changes request", async t => {
  const panel = mountRoutingComponent("TopicInspector", { ...props(), artifactRequest: { resourceId: "artifact" }, landingRequest: 1 }, transport(() => queue()), true);
  t.after(panel.stop); await flush();
  assert.equal(panel.state.tab, "resources");
  panel.props.landingRequest++; await flush();
  assert.equal(panel.state.tab, "changes");
});

test("F2 a newly opened artifact wins a batched Changes request, then later Changes works", async t => {
  const panel = mountRoutingComponent("TopicInspector", props(), transport(() => queue()), true);
  t.after(panel.stop); await flush();
  panel.props.landingRequest++;
  panel.props.artifactRequest = { resourceId: "new-artifact" }; await flush();
  assert.equal(panel.state.tab, "resources");
  panel.props.landingRequest++; await flush();
  assert.equal(panel.state.tab, "changes");
});


test("F1 replacing a same-owner Track summary preserves its revision high water", async t => {
  const write = deferred<any>();
  let view = queue();
  const panel = mountRoutingComponent("TopicInspector", props(), transport(() => view, () => write.promise), true);
  t.after(panel.stop); await flush();
  const moving = panel.state.movePending(1, 0);
  view = claimed();
  panel.props.topic = { ...props().topic, last_activity_at: "2026-10-12T02:00:00Z" };
  panel.props.fleet = [panel.props.topic, props().fleet[1]];
  await flush();
  assert.equal(panel.state.queueView.revision, 6);
  assert.equal(panel.state.queueBusy, true, "same owner retains the pending mutation fence");
  write.resolve(queue(5, [entry(changes[1]), entry(changes[0])])); await moving;
  assert.equal(panel.state.queueView.revision, 6);
  assert.equal(panel.state.timeline.active.change.id, "second");
  assert.equal(panel.state.canLand(activeCandidate(panel)), false);
});
