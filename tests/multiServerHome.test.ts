import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { compileScript, parse } from "@vue/compiler-sfc";
import { createRenderer, nextTick } from "vue";
import ts from "typescript";
import { scopeSession, scopedId } from "../src/backendIdentity.ts";

const require = createRequire(import.meta.url);
function compile(source: string, resolve: (name: string) => any) {
  const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2021, esModuleInterop: true } }).outputText;
  const module = { exports: {} as any };
  new Function("require", "module", "exports", code)(resolve, module, module.exports);
  return module.exports;
}

const a = "https://todos-a.example", b = "https://todos-b.example";
async function mountHome(transport: (command: string, args: Record<string, any>) => Promise<any>) {
  const bridge = compile(readFileSync(new URL("../src/backendInvoke.ts", import.meta.url), "utf8"), name => {
    if (name === "@tauri-apps/api/core") return { invoke: transport };
    return name.startsWith("./") ? require(new URL(`../src/${name.slice(2).replace(/\.ts$/, "")}.ts`, import.meta.url).pathname) : require(name);
  });
  const source = readFileSync(new URL("../src/components/HomeView.vue", import.meta.url), "utf8");
  const component = compile(compileScript(parse(source).descriptor, { id: "aggregate-todos-test" }).content, name => {
    if (name === "@tauri-apps/api/event") return { listen: async () => () => {} };
    if (name === "../backendInvoke") return bridge;
    if (name.endsWith(".vue")) return {};
    return name.startsWith("../") ? require(new URL(`../src/${name.slice(3)}.ts`, import.meta.url).pathname) : require(name);
  });
  const wire = { id: "same-track", status: "idle", parent_id: null, parent_session_id: null, placement: null,
    branch: { id: "branch", name: "Track", title: "Track", repo_root: "/repo", tags: [{ key: "topic", value: "true" }] } } as any;
  const renderer = createRenderer<any, any>({ createElement: () => ({}), createText: () => ({}), createComment: () => ({}),
    insert() {}, remove() {}, setText() {}, setElementText() {}, patchProp() {}, parentNode: () => null, nextSibling: () => null });
  const app = renderer.createApp({ ...component.default, render: () => null }, { fleet: [scopeSession(a, wire), scopeSession(b, wire)], topic: null, project: null, layout: null });
  const instance = app.mount({}) as any;
  await new Promise(setImmediate);
  return { state: instance.$.setupState, stop: () => app.unmount(),
    setBackends: async (urls: string[]) => { instance.$.props.fleet = urls.map(url => scopeSession(url, wire)); await nextTick(); } };
}

test("a stalled backend cannot withhold healthy aggregate Todos or erase them on failure", async t => {
  let rejectB: (error: unknown) => void = () => {};
  const home = await mountHome(async (_command, args) => {
    assert.deepEqual(args.topicIds, ["same-track"]);
    if (args.baseUrl === b) return new Promise((_resolve, reject) => { rejectB = reject; });
    assert.equal(args.baseUrl, a);
    return { todos: [{ id: "same-todo", topic_id: "same-track", title: "Healthy task", revision: 1 }], warnings: [] };
  }); t.after(home.stop);
  const state = home.state;
  assert.equal(state.todosLoading, true, "B's query is still pending");
  assert.deepEqual(state.visibleTodos.map((row: any) => row.id), [scopedId(a, "same-todo")]);
  rejectB({ message: "B unavailable", unreachable: true });
  await new Promise(setImmediate);
  assert.equal(state.todosLoading, false);
  assert.equal(state.visibleTodos[0].topic_id, scopedId(a, "same-track"));
  assert.match(state.todoWarnings[0], /todos-b.example.*B unavailable/);
});

test("healthy Todo refresh and completion continue while a sibling request remains stalled", async t => {
  let resolveB: (reply: any) => void = () => {};
  let aReads = 0, bReads = 0;
  const home = await mountHome(async (command, args) => {
    if (command === "toggle_todo") { assert.equal(args.baseUrl, a); return; }
    if (args.baseUrl === b) { bReads++; return new Promise(resolve => { resolveB = resolve; }); }
    aReads++;
    return { todos: [{ id: "same-todo", topic_id: "same-track", text: `A revision ${aReads}`, revision: aReads }], warnings: [] };
  }); t.after(home.stop);
  void home.state.refreshUserTodos();
  await new Promise(setImmediate);
  assert.equal(aReads, 2, "A refreshes without awaiting B's first response");
  assert.equal(bReads, 1, "B alone coalesces its stalled request");
  assert.equal(home.state.visibleTodos[0].revision, 2);
  await home.state.completeTodo(home.state.visibleTodos[0]);
  assert.equal(home.state.todoBusy.size, 0, "B cannot hold A's completion busy");
  assert.equal(home.state.visibleTodos[0].revision, 3);
  await home.setBackends([a]);
  assert.equal(home.state.visibleTodos[0].revision, 3, "outage preserves healthy rows before refresh");
  await new Promise(resolve => setTimeout(resolve, 700));
  assert.equal(home.state.visibleTodos[0].revision, 4, "A refreshes after B's outage removes its topics");
  assert.equal(home.state.todosLoading, false, "removed B cannot keep the active loading state busy");
  resolveB({ todos: [{ id: "stale-todo", topic_id: "same-track", text: "Old B", revision: 1 }], warnings: ["Stale B warning"] });
  await new Promise(setImmediate);
  assert.deepEqual(home.state.visibleTodos.map((row: any) => row.id), [scopedId(a, "same-todo")]);
  assert.deepEqual(home.state.todoWarnings, []);
  assert.equal(bReads, 1, "retired B response cannot restart its dirty request");
});

test("stalled Todo mutations are isolated and retired results cannot unlock a newer same-Todo action", async () => {
  for (const failOld of [false, true]) {
    const bActions: { resolve: () => void; reject: (error: unknown) => void }[] = [];
    let aToggles = 0, bReads = 0;
    const home = await mountHome(async (command, args) => {
      if (command === "toggle_todo") {
        if (args.baseUrl === a) { aToggles++; return; }
        return new Promise<void>((resolve, reject) => bActions.push({ resolve, reject }));
      }
      if (args.baseUrl === b) bReads++;
      return { todos: [{ id: "same-todo", topic_id: "same-track", text: "Work", revision: 1 }], warnings: [] };
    });
    try {
      const todoA = () => home.state.visibleTodos.find((row: any) => row.topic_id === scopedId(a, "same-track"));
      const todoB = () => home.state.visibleTodos.find((row: any) => row.topic_id === scopedId(b, "same-track"));
      const oldB = home.state.completeTodo(todoB()); await new Promise(setImmediate);
      await home.state.completeTodo(todoA());
      assert.equal(aToggles, 1, "pending B does not block A's mutation");
      assert.equal(home.state.todoBusy.has(scopedId(a, "same-todo")), false);
      await home.state.completeTodo(todoB());
      assert.equal(bActions.length, 1, "the same active Todo stays locked");
      await home.setBackends([a]);
      assert.equal(home.state.todoBusy.has(scopedId(b, "same-todo")), false, "outage retires B's lock");
      await home.state.completeTodo(todoA()); assert.equal(aToggles, 2);
      await home.setBackends([a, b]); await home.state.refreshUserTodos();
      const newB = home.state.completeTodo(todoB()); await new Promise(setImmediate);
      const readsBeforeOldResult = bReads;
      if (failOld) bActions[0].reject(new Error("retired B error")); else bActions[0].resolve();
      await oldB;
      assert.equal(home.state.todoBusy.has(scopedId(b, "same-todo")), true, "old B cannot unlock the new request");
      assert.equal(bReads, readsBeforeOldResult, "old mutation does not start another refresh");
      assert.deepEqual(home.state.todoWarnings, []);
      assert.ok(todoB(), "retired success does not remove a newer row");
      bActions[1].resolve(); await newB;
      assert.equal(home.state.todoBusy.size, 0);
    } finally { home.stop(); }
  }
});
