import assert from "node:assert/strict";
import test from "node:test";
import { nextTick } from "vue";
import { scopedId } from "../src/backendIdentity.ts";
import { emptyDraft } from "../src/newTopicDraft.ts";
import { deferred, mountRoutingComponent, settle } from "./componentRoutingHarness.ts";

const server = "https://a.example";
const key = "design_document:/repo:docs/design.md";
const project = { id: scopedId(server, "project"), name: "Cached project" };
const layout = { revision: 1, spaces: [{ id: scopedId(server, "space"), rank: 0, groups: [{ ...project, rank: 0, system_key: null }] }] };
const bindings = (revision = 9) => ({ revision, bindings: [{ id: key, title: "Design", kind: "design_document", repository: "/repo", path: "docs/design.md" }] });
function savedChat(available: boolean, transport: (command: string, args: any) => Promise<any>, renderTemplate = false) {
  return mountRoutingComponent("NewTrackChat", { backendUrl: server, available, backends: [], fleet: [], project: null, layout, launchOptions: null }, transport, renderTemplate, {
    [`arachne.newTopicDraft@${server}`]: JSON.stringify({ ...emptyDraft(), project, repo: "/repo", body: "Saved offline task" }),
  });
}

test("a project's first track enables Send after an empty resource checklist loads", async t => {
  const response = deferred<any>();
  const chat = savedChat(true, async command => command === "project_bindings" ? response.promise : [], true);
  t.after(chat.stop); await settle(); await nextTick();
  const send = () => chat.elements(element => element.props["aria-label"] === "Send and launch track")[0];
  const status = () => chat.elements(element => element.props.id === "new-track-send-status")[0];
  assert.equal(send().props.disabled, true);
  assert.match(status().text!, /Loading project resources/);
  response.resolve({ bindings: [], revision: 0 }); await settle(); await nextTick();
  assert.equal(send().props.disabled, false);
  send().props.onClick();
  const launch = chat.events.find(event => event.name === "launch")!;
  assert.equal(launch.args[0], "Saved offline task");
  assert.deepEqual(launch.args[2].project, project);
  assert.equal(launch.args[2].hiddenBindingKeys, undefined);
});

test("Send explains a failed project resource read and enables after retry", async t => {
  let fail = true;
  const chat = savedChat(true, async command => {
    if (command === "project_bindings" && fail) throw new Error("Resource read failed");
    return { bindings: [], revision: 0 };
  }, true);
  t.after(chat.stop); await settle(); await nextTick();
  const send = () => chat.elements(element => element.props["aria-label"] === "Send and launch track")[0];
  const status = () => chat.elements(element => element.props.id === "new-track-send-status")[0];
  assert.equal(send().props.disabled, true);
  assert.match(send().props.title, /Resource read failed/);
  assert.match(status().text!, /Retry project resources above/);
  assert.equal(send().props["aria-describedby"], status().props.id);
  fail = false;
  await chat.state.loadProjectBindings(); await nextTick();
  assert.equal(send().props.disabled, false);
});

test("a saved offline project draft reloads inherited resources on recovery before launch", async t => {
  const response = deferred<any>();
  const calls: any[] = [];
  const chat = savedChat(false, async (command, args) => {
    if (command === "project_bindings") { calls.push(args); return response.promise; }
    return [];
  });
  t.after(chat.stop); await settle();
  assert.equal(chat.state.liveProject.id, project.id);
  assert.equal(chat.state.repo, "/repo");
  assert.equal(calls.length, 0);
  chat.state.send(); assert.equal(chat.events.length, 0);
  chat.props.available = true; await nextTick();
  assert.deepEqual(calls, [{ baseUrl: server, projectId: "project", repo: "/repo" }]);
  assert.equal(chat.state.projectBindingsLoading, true);
  chat.state.send(); assert.equal(chat.events.length, 0, "wait for the inherited checklist");
  response.resolve(bindings()); await settle();
  assert.equal(chat.state.projectBindingsLoading, false);
  assert.equal(chat.state.projectBindings[0].id, scopedId(server, key));
  chat.state.toggleBinding(scopedId(server, key), false); chat.state.send();
  assert.deepEqual(chat.events.find(event => event.name === "launch")?.args[2].hiddenBindingKeys, [key]);
});

for (const retired of ["success", "failure"] as const) {
  test(`outage retires binding ${retired} while a recovery read is pending`, async t => {
    const old = deferred<any>(), fresh = deferred<any>();
    let reads = 0;
    const chat = savedChat(true, async command => command === "project_bindings" ? (++reads === 1 ? old.promise : fresh.promise) : []);
    t.after(chat.stop); await settle();
    assert.equal(reads, 1);
    chat.props.available = false; await nextTick();
    assert.equal(chat.state.projectBindingsLoading, false);
    assert.deepEqual(chat.state.projectBindings, []);
    chat.props.available = true; await nextTick();
    assert.equal(reads, 2);
    if (retired === "success") old.resolve(bindings(1)); else old.reject(new Error("retired backend outage"));
    await settle();
    assert.equal(chat.state.projectBindingsLoading, true, "retired completion cannot finish the recovery read");
    assert.deepEqual(chat.state.projectBindings, []);
    assert.equal(chat.state.projectBindingsError, "");
    fresh.resolve(bindings()); await settle();
    assert.equal(chat.state.projectRevision, 9);
    assert.equal(chat.state.projectBindingsLoading, false);
    chat.state.send(); assert.equal(chat.events.filter(event => event.name === "launch").length, 1);
  });
}

test("a failed recovery checklist blocks launch and can be retried without changing the draft", async t => {
  let reads = 0;
  const chat = savedChat(false, async command => {
    if (command !== "project_bindings") return [];
    if (++reads === 1) throw new Error("Resource read failed");
    return bindings();
  });
  t.after(chat.stop);
  chat.props.available = true; await nextTick(); await settle();
  assert.match(chat.state.projectBindingsError, /Resource read failed/);
  chat.state.send(); assert.equal(chat.events.length, 0);
  await chat.state.loadProjectBindings();
  assert.equal(chat.state.projectBindingsError, "");
  assert.equal(chat.state.body, "Saved offline task");
  chat.state.send(); assert.equal(chat.events.filter(event => event.name === "launch").length, 1);
});

test("same-project recovery preserves chosen exclusions and intersects removed/new bindings", async t => {
  const removed = "file:/repo:old.txt", added = "file:/repo:new.txt";
  const row = (id: string) => ({ ...bindings().bindings[0], id });
  const pending = deferred<any>();
  let reads = 0;
  const chat = savedChat(true, async command => command === "project_bindings"
    ? (++reads === 1 ? { revision: 1, bindings: [row(key), row(removed)] } : pending.promise) : []);
  t.after(chat.stop); await settle();
  chat.state.toggleBinding(scopedId(server, key), false);
  chat.state.toggleBinding(scopedId(server, removed), false);
  chat.props.available = false; await nextTick();
  assert.deepEqual([...chat.state.uncheckedBindings], [scopedId(server, key), scopedId(server, removed)]);
  chat.props.available = true; await nextTick();
  pending.resolve({ revision: 2, bindings: [row(key), row(added)] }); await settle();
  assert.deepEqual([...chat.state.uncheckedBindings], [scopedId(server, key)], "removed keys disappear; new keys inherit");
  chat.state.send();
  assert.deepEqual(chat.events.find(event => event.name === "launch")?.args[2].hiddenBindingKeys, [key]);
});

test("recovery failure and retry retain exclusions, while repository/project changes reset them", async t => {
  let reads = 0;
  const chat = savedChat(true, async command => {
    if (command !== "project_bindings") return [];
    if (++reads === 2) throw new Error("Recovery failed");
    return bindings();
  });
  t.after(chat.stop); await settle();
  chat.state.toggleBinding(scopedId(server, key), false);
  chat.props.available = false; await nextTick();
  chat.props.available = true; await nextTick(); await settle();
  assert.match(chat.state.projectBindingsError, /Recovery failed/);
  chat.state.send(); assert.equal(chat.events.length, 0);
  await chat.state.loadProjectBindings();
  assert.deepEqual([...chat.state.uncheckedBindings], [scopedId(server, key)]);
  chat.state.repo = "/another"; await nextTick(); await settle();
  assert.equal(chat.state.uncheckedBindings.size, 0);
  chat.state.toggleBinding(scopedId(server, key), false);
  chat.state.project = { id: scopedId(server, "another-project"), name: "Another" };
  await nextTick(); await settle();
  assert.equal(chat.state.uncheckedBindings.size, 0);
});

for (const retired of ["success", "failure"] as const) {
  test(`retired ${retired} cannot discard an exclusion during recovery`, async t => {
    const old = deferred<any>(), fresh = deferred<any>(); let reads = 0;
    const chat = savedChat(true, async command => command === "project_bindings"
      ? (++reads === 1 ? bindings() : reads === 2 ? old.promise : fresh.promise) : []);
    t.after(chat.stop); await settle();
    chat.state.toggleBinding(scopedId(server, key), false);
    const refresh = chat.state.loadProjectBindings();
    chat.props.available = false; await nextTick();
    chat.props.available = true; await nextTick();
    if (retired === "success") old.resolve({ revision: 1, bindings: [] }); else old.reject(new Error("Retired failure"));
    await refresh;
    assert.equal(chat.state.projectBindingsLoading, true);
    assert.deepEqual([...chat.state.uncheckedBindings], [scopedId(server, key)]);
    fresh.resolve(bindings()); await settle();
    chat.state.send();
    assert.deepEqual(chat.events.find(event => event.name === "launch")?.args[2].hiddenBindingKeys, [key]);
  });
}
