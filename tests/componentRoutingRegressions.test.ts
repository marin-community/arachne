import assert from "node:assert/strict";
import test from "node:test";
import { nextTick } from "vue";
import { scopeSession, scopedId } from "../src/backendIdentity.ts";
import { pinSelection } from "../src/durableChange.ts";
import { declaration } from "./fixtures/durable-change.mjs";
import { deferred, mountRoutingComponent, settle } from "./componentRoutingHarness.ts";

const a = "https://a.example", b = "https://b.example";
const bindingKey = "design_document:/repo:docs/design.md";
const topic = (server: string, id = "track") => scopeSession(server, {
  id, status: "idle", parent_id: null, parent_session_id: null, placement: null,
  worktree_present: false, work_dir: "", github_repo: "org/own", last_activity_at: "2026-10-10T10:00:00Z",
  branch: { id: `branch-${id}`, name: id, title: id, repo_root: "/repo", branch: `weaver/${id}`, tags: [{ key: "topic", value: "true" }] },
} as any);
const resources = { resources: [], hidden_resources: [], revision: 0, issues: [], prs: [] };

test("Home completion controls disable only the pending Todo's owning row", async t => {
  const pendingB = deferred<void>();
  let aToggles = 0;
  const home = mountRoutingComponent("HomeView", { fleet: [topic(a), topic(b)], topic: null, project: null, layout: null }, async (command, args) => {
    if (command === "toggle_todo") {
      if (args.baseUrl === b) return pendingB.promise;
      aToggles++; return;
    }
    return { todos: [{ id: "same-todo", topic_id: "track", text: "Work", revision: 1 }], warnings: [] };
  }, true);
  t.after(home.stop); await settle();
  const rowB = home.state.visibleTodos.find((row: any) => row.topic_id === scopedId(b, "track"));
  const completionB = home.state.completeTodo(rowB); await nextTick();
  const controls = home.elements(el => el.props.class === "home-todo-complete");
  assert.equal(controls.length, 2);
  assert.equal(Boolean(controls[0].props.disabled), false, "healthy A remains actionable");
  assert.equal(Boolean(controls[1].props.disabled), true, "pending B is locked");
  await controls[0].props.onClick(); assert.equal(aToggles, 1);
  pendingB.resolve(); await completionB;
});

test("ProjectSheet can switch away from an unavailable backend and dismiss while mutations remain disabled", async t => {
  const calls: string[] = [];
  const sheet = mountRoutingComponent("ProjectSheet", { backendUrl: a, backends: [{ url: a, name: "A", available: false }, { url: b, name: "B", available: true }], available: false }, async command => { calls.push(command); return []; }, true);
  t.after(sheet.stop); await settle();
  sheet.state.name = "Offline draft"; sheet.state.repo = "/repo"; await nextTick();
  const picker = sheet.elements(el => el.type === "select")[0];
  assert.equal(Boolean(picker.props.disabled), false, "offline A must not disable the backend picker");
  picker.props.onChange({ target: { value: b } });
  assert.deepEqual(sheet.events.find(event => event.name === "backend")?.args, [b]);
  const close = sheet.elements(el => el.props["aria-label"] === "Close project settings")[0];
  const cancel = sheet.elements(el => el.type === "button" && el.text === "Cancel")[0];
  for (const control of [close, cancel]) { assert.equal(Boolean(control.props.disabled), false); control.props.onClick(); }
  assert.equal(sheet.events.filter(event => event.name === "close").length, 2);
  const create = sheet.elements(el => el.type === "button" && el.text?.trim() === "Create project")[0];
  assert.equal(Boolean(create.props.disabled), true);
  await sheet.state.create(); await sheet.state.attach();
  await sheet.state.remove({ id: scopedId(a, bindingKey) });
  assert.equal(calls.filter(command => ["create_group", "add_project_binding", "remove_project_binding"].includes(command)).length, 0);
  sheet.props.backendUrl = b; sheet.props.available = true; await nextTick();
  assert.equal(Boolean(sheet.elements(el => el.type === "select")[0].props.disabled), false);
  assert.equal(Boolean(sheet.elements(el => el.type === "button" && el.text?.trim() === "Create project")[0].props.disabled), false);
});

test("unchecked inherited bindings launch as raw keys through the actual NewTrackChat and bridge", async t => {
  const calls: { command: string; args: any }[] = [];
  const chat = mountRoutingComponent("NewTrackChat", { backendUrl: a, backends: [], available: true, fleet: [], launchOptions: null, project: { id: scopedId(a, "project"), name: "Project" } }, async (command, args) => {
    calls.push({ command, args });
    if (command === "project_bindings") return { bindings: [{ id: bindingKey, title: "Design", kind: "design_document", repository: "/repo", path: "docs/design.md" }], revision: 1 };
    if (command === "launch_session") return { ...topic(a), id: "new-track", branch: { id: "new-branch", tags: [] } };
    return [];
  });
  t.after(chat.stop);
  chat.state.repo = "/repo"; await nextTick(); await settle();
  assert.equal(chat.state.projectBindings[0].id, scopedId(a, bindingKey));
  chat.state.toggleBinding(chat.state.projectBindings[0].id, false);
  chat.state.body = "Launch without the inherited design"; chat.state.send();
  const submittedDraft = localStorage.getItem(`arachne.newTopicDraft@${a}`);
  assert.equal(JSON.parse(submittedDraft!).body, "Launch without the inherited design", "actual composer saves the submitted snapshot before launch delivery");
  const launch = chat.events.find(event => event.name === "launch")!;
  assert.deepEqual(launch.args[2].hiddenBindingKeys, [bindingKey]);
  // The parent uses the same bridge for launch and persists these raw keys.
  const { createBackendInvoke } = await import("../src/backendInvoke.ts");
  const invoke = createBackendInvoke(async <T>(command: string, args: any) => { calls.push({ command, args }); return { id: "new", branch: { id: "branch", tags: [] }, placement: null } as T; });
  await invoke("launch_session", { task: launch.args[0], project: launch.args[2].project, hiddenBindingKeys: launch.args[2].hiddenBindingKeys }, a);
  assert.deepEqual(calls.at(-1)?.args.hiddenBindingKeys, [bindingKey]);
  chat.state.toggleBinding(scopedId(b, bindingKey), false); chat.state.send();
  assert.equal(chat.events.filter(event => event.name === "launch").length, 1, "foreign hidden binding cannot bypass owner validation");
  assert.match(chat.state.projectBindingsError, /different/);
  assert.equal(localStorage.getItem(`arachne.newTopicDraft@${a}`), submittedDraft, "rejected launch cannot replace the submitted snapshot");
});

test("a pending backend catalog remains usable after switching tracks on that backend", async t => {
  const catalog = deferred<{ slug: string }[]>(); let requests = 0;
  const panel = mountRoutingComponent("ResourcePanel", { topic: topic(a, "one") }, async command => {
    if (command === "topic_resources") return resources;
    if (command === "managed_repos") { requests++; return catalog.promise; }
    return null;
  });
  t.after(panel.stop); await settle();
  panel.props.topic = topic(a, "two"); await nextTick();
  catalog.resolve([{ slug: "org/catalog" }]); await settle();
  assert.equal(requests, 1, "the catalog is owned by the backend, so the pending request stays valid");
  assert.deepEqual(panel.state.managedRepos, ["org/catalog"]);
  assert.ok(panel.state.repoOptions.includes("org/catalog"));
});

test("an old A catalog failure cannot clear the successfully loaded B catalog", async t => {
  const old = deferred<unknown>();
  const panel = mountRoutingComponent("ResourcePanel", { topic: topic(a) }, async (command, args) => {
    if (command === "topic_resources") return resources;
    if (command === "managed_repos") return args.baseUrl === a ? old.promise : [{ slug: "org/b-only" }];
    return null;
  });
  t.after(panel.stop); await settle();
  panel.props.topic = topic(b); await nextTick(); await settle();
  assert.deepEqual(panel.state.managedRepos, ["org/b-only"]);
  old.reject(new Error("A unavailable")); await settle();
  assert.deepEqual(panel.state.managedRepos, ["org/b-only"]);
});

test("actual ChangeReview preparation sends a raw coordinator identity while preserving user text", async t => {
  const calls: { command: string; args: any }[] = [];
  const selected = declaration();
  const review = mountRoutingComponent("ChangeReview", { backendUrl: a, sessionId: scopedId(a, "worker"), selectedChange: pinSelection(scopedId(a, "worker"), selected), integrationTarget: topic(a, "coordinator"), isTopic: false }, async (command, args) => {
    calls.push({ command, args });
    if (command === "change_changes") return { source_ref: "weaver/worker", accepted_source: false, head_oid: selected.scope.head_oid, base: { state: "available", reference: selected.scope.base_oid, oid: selected.scope.base_oid }, files: [], totals: {} };
    return null;
  });
  t.after(review.stop); await settle();
  review.state.prepareRequest("prepare");
  assert.match(review.state.requestText, /session coordinator\)/);
  assert.ok(!review.state.requestText.includes(scopedId(a, "coordinator")));
  const userText = `User note: keep ${scopedId(b, "literal-user-text")} unchanged.`;
  review.state.requestText += ` ${userText}`;
  await review.state.sendRequest();
  const sent = calls.find(call => call.command === "send_input")!;
  assert.equal(sent.args.baseUrl, a); assert.equal(sent.args.id, "worker");
  assert.match(sent.args.text, /session coordinator\)/);
  assert.ok(sent.args.text.includes(userText));
  assert.ok(sent.args.text.includes(JSON.stringify(selected)), "selected declaration remains mandatory after user edits");
});

test("an older catalog request cannot erase a later generation after returning to A", async t => {
  const oldA = deferred<unknown>(); let aRequests = 0;
  const panel = mountRoutingComponent("ResourcePanel", { topic: topic(a) }, async (command, args) => {
    if (command === "topic_resources") return resources;
    if (command === "managed_repos") {
      if (args.baseUrl === a) return ++aRequests === 1 ? oldA.promise : [{ slug: "org/new-a" }];
      return [{ slug: "org/b" }];
    }
    return null;
  });
  t.after(panel.stop); await settle();
  panel.props.topic = topic(b); await nextTick(); await settle();
  panel.props.topic = topic(a); await nextTick(); await settle();
  assert.deepEqual(panel.state.managedRepos, ["org/new-a"]);
  oldA.reject(new Error("retired A request")); await settle();
  assert.deepEqual(panel.state.managedRepos, ["org/new-a"]);
});
