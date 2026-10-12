import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { compileScript, parse } from "@vue/compiler-sfc";
import { createRenderer, ref } from "vue";
import ts from "typescript";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { mountRoutingComponent } from "./componentRoutingHarness.ts";
import { scopedId } from "../src/backendIdentity.ts";
import { draftStorageKey } from "../src/newTopicDraft.ts";

const require = createRequire(import.meta.url);
const a = "https://a.example";
const b = "https://b.example";
const wireLayout = { revision: 1, defaults: [], spaces: [{ id: "space", name: "User", rank: 0, system_key: "user", groups: [
  { id: "project", space_id: "space", name: "Same project name", rank: 0, system_key: null, collapsed: false, session_ids: ["track"] },
] }] };
const wireSession = { id: "track", status: "idle", profile: "default", class: "interactive", origin: "user", parent_id: null, parent_session_id: null,
  branch: { id: "branch", name: "Same track name", tags: [] }, placement: { space_id: "space", group_id: "project", session_id: "track" } };

function compile(source: string, resolve: (name: string) => any) {
  const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2021, esModuleInterop: true } }).outputText;
  const module = { exports: {} as any };
  new Function("require", "module", "exports", code)(resolve, module, module.exports);
  return module.exports;
}
async function mountApp(startupCredential?: Promise<string | null>, nativeTransport?: (command: string, args: any) => Promise<any>, servers = [a, b]) {
  const [a, b] = servers;
  const previous = new Map(["localStorage", "window"].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const values = new Map<string, string>([["loomUrl", a], ["arachne.loomServers", JSON.stringify([{ name: "A", url: a }, { name: "B", url: b }])]]);
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value), removeItem: (key: string) => values.delete(key) } });
  Object.defineProperty(globalThis, "window", { configurable: true, value: new EventTarget() });
  const events = new Map<string, (event: { payload: any }) => void>();
  const calls: { command: string; args: any }[] = [];
  const installedTokens = new Map<string, string | null>();
  const savedTokens = new Map<string, string>();
  let bUnavailable = false;
  let deferLaunch = false;
  let releaseLaunch: (() => void) | undefined;
  let deferOpen = false;
  let releaseOpen: (() => void) | undefined;
  const transport = async (command: string, args: any) => {
    calls.push({ command, args });
    if (command === "load_token") return args.baseUrl === a && startupCredential ? startupCredential : null;
    if (command === "save_token") { savedTokens.set(args.baseUrl, args.token); return; }
    if (bUnavailable && args.baseUrl === b) throw { message: "B is down", unreachable: true };
    if (command === "connect") { installedTokens.set(args.baseUrl, args.token); if (nativeTransport) return nativeTransport(command, args); return; }
    if (nativeTransport && ["connect", "refresh_fleet", "open_session", "fetch_chat"].includes(command)) return nativeTransport(command, args);
    if (command === "refresh_fleet") return { server_url: args.baseUrl, sessions: [wireSession], layout: wireLayout };
    if (command === "launch_options") return { profiles: [], agents: [], default_agent: "" };
    if (command === "open_session") {
      if (deferOpen) await new Promise<void>(resolve => { releaseOpen = resolve; });
      return { ...wireSession, id: args.id };
    }
    if (command === "launch_session") {
      if (deferLaunch) await new Promise<void>(resolve => { releaseLaunch = resolve; });
      return { ...wireSession, id: "new-track" };
    }
  };
  const bridgeSource = readFileSync(new URL("../src/backendInvoke.ts", import.meta.url), "utf8");
  const bridge = compile(bridgeSource, name => name === "@tauri-apps/api/core" ? { invoke: transport }
    : name.startsWith("./") ? require(new URL(`../src/${name.slice(2).replace(/\.ts$/, "")}.ts`, import.meta.url).pathname) : require(name));
  const source = readFileSync(process.env.NAVIGATION_APP_SOURCE ?? new URL("../src/App.vue", import.meta.url), "utf8");
  const appModule = compile(compileScript(parse(source).descriptor, { id: "multi-server-app-test", isProd: true }).content, name => {
    if (name === "@tauri-apps/api/core") return { invoke: transport };
    if (name === "@tauri-apps/api/event") return { listen: async (event: string, receive: any) => { events.set(event, receive); return () => events.delete(event); } };
    if (name === "./backendInvoke") return bridge;
    if (name === "./launch") return { launchSelectionFor: () => ({ agent: "", model: "", effort: "" }) };
    if (name.endsWith(".vue")) return {};
    return name.startsWith("./") ? require(new URL(`../src/${name.slice(2)}.ts`, import.meta.url).pathname) : require(name);
  });
  const renderer = createRenderer<any, any>({ createElement: () => ({}), createText: () => ({}), createComment: () => ({}),
    insert() {}, remove() {}, setText() {}, setElementText() {}, patchProp() {}, parentNode: () => null, nextSibling: () => null });
  const app = renderer.createApp({ ...appModule.default, render: () => null });
  const instance = app.mount({}) as any;
  await new Promise(setImmediate);
  return { state: instance.$.setupState, calls, values, installedTokens, savedTokens, emit: (name: string, payload: any) => events.get(name)?.({ payload }),
    setDown: (value: boolean) => { bUnavailable = value; },
    deferLaunch: () => { deferLaunch = true; }, releaseLaunch: () => releaseLaunch?.(),
    deferOpen: () => { deferOpen = true; }, releaseOpen: () => releaseOpen?.(),
    stop: () => { app.unmount(); for (const [key, descriptor] of previous) { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else Reflect.deleteProperty(globalThis, key); } } };
}

test("App boots both fleets, routes colliding IDs, and reconnects only the failed server", async t => {
  const app = await mountApp(); t.after(app.stop);
  assert.equal(app.state.fleet.length, 2);
  assert.equal(app.state.layout.spaces.length, 2);
  await app.state.selectSession(scopedId(a, "track"));
  assert.equal(app.state.selectedView.id, scopedId(a, "track"));
  // Sidebar actions target their owner even while a different backend is open.
  await app.state.updateTopic(scopedId(b, "track"), { title: "Updated B" });
  const update = app.calls.find(call => call.command === "update_session")!;
  assert.equal(update.args.baseUrl, b); assert.equal(update.args.session, "track");
  app.setDown(true);
  app.emit("loom://error", { server_url: b, message: "B is down", unreachable: true });
  assert.deepEqual(app.state.fleet.map((row: any) => row.id), [scopedId(a, "track")]);
  assert.equal(app.state.layout.spaces.length, 2, "offline headings remain");
  assert.equal(app.state.selectedView.id, scopedId(a, "track"), "healthy thread remains selected");
  app.state.openNewTopicInProject({ id: scopedId(b, "project"), name: "Unavailable project" });
  assert.equal(app.state.showNewTopic, false, "unavailable project cannot launch");
  const before = app.calls.filter(call => call.command === "connect" && call.args.baseUrl === a).length;
  app.setDown(false); await app.state.retryBackend(b);
  assert.equal(app.state.fleet.length, 2);
  assert.equal(app.state.selectedId, scopedId(a, "track"));
  assert.equal(app.calls.filter(call => call.command === "connect" && call.args.baseUrl === a).length, before);
  app.state.openNewTopicInProject({ id: scopedId(b, "project"), name: "Project B" });
  assert.equal(app.state.composerUrl, b); assert.equal(app.state.operationUrl, b);
  assert.equal(app.state.composerFleet[0].id, scopedId(b, "track"));
  await app.state.launchTopic("Work on B", "owner/repo", { project: { id: scopedId(b, "project"), name: "Project B" } });
  const launch = app.calls.find(call => call.command === "launch_session")!;
  assert.equal(launch.args.baseUrl, b); assert.equal(launch.args.project.id, "project");
  assert.equal(app.state.selectedId, scopedId(b, "new-track"));
});

for (const lookup of ["token", "null", "error"] as const) {
  test(`retired startup credential ${lookup} cannot connect after a newer settings credential`, async t => {
    let release!: (value: string | null) => void;
    let reject!: (reason: Error) => void;
    const credential = new Promise<string | null>((resolve, fail) => { release = resolve; reject = fail; });
    const app = await mountApp(credential); t.after(app.stop);
    assert.equal(app.calls.filter(call => call.command === "connect" && call.args.baseUrl === a).length, 0);
    assert.equal(app.state.backends.find((backend: any) => backend.url === b).available, true, "B boots while A's Keychain lookup stalls");
    await app.state.selectSession(scopedId(b, "track"));
    const bConnects = app.calls.filter(call => call.command === "connect" && call.args.baseUrl === b).length;
    await app.state.saveSettings(a, "new-token");
    assert.equal(app.installedTokens.get(a), "new-token");
    assert.equal(app.savedTokens.get(a), "new-token");
    if (lookup === "error") reject(new Error("Old Keychain lookup failed"));
    else release(lookup === "token" ? "old-token" : null);
    await new Promise(setImmediate); await new Promise(setImmediate);
    assert.deepEqual(app.calls.filter(call => call.command === "connect" && call.args.baseUrl === a).map(call => call.args.token), ["new-token"], "fence native side effects, including null/error fallback");
    assert.equal(app.installedTokens.get(a), "new-token");
    assert.equal(app.savedTokens.get(a), "new-token");
    const backend = app.state.backends.find((backend: any) => backend.url === a);
    assert.equal(backend.available, true); assert.equal(backend.connecting, false);
    assert.equal(app.state.selectedId, scopedId(b, "track"));
    assert.equal(app.calls.filter(call => call.command === "connect" && call.args.baseUrl === b).length, bConnects);
    assert.equal(JSON.stringify([...app.values.values()]).includes("new-token"), false, "credential storage remains native-only");
  });
}


test("a delayed launch keeps its backend and cannot steal a newly selected healthy thread", async t => {
  const app = await mountApp(); t.after(app.stop);
  app.state.openNewTopicInProject({ id: scopedId(b, "project"), name: "B project" });
  app.deferLaunch();
  const pending = app.state.launchTopic("Delayed B", "owner/repo", { project: { id: scopedId(b, "project"), name: "B" } });
  await new Promise(setImmediate);
  assert.equal(app.state.launching, true);
  await app.state.selectSession(scopedId(a, "track"));
  assert.equal(app.state.launching, false, "B's pending launch does not lock healthy A");
  app.releaseLaunch();
  await pending;
  assert.equal(app.state.selectedId, scopedId(a, "track"));
  assert.equal(app.state.selectedView.id, scopedId(a, "track"));
});

test("a delayed launch cannot close or clear a newer project composer on the same backend", async t => {
  const app = await mountApp(); t.after(app.stop);
  app.state.openNewTopicInProject({ id: scopedId(b, "project"), name: "Old B project" });
  const oldGeneration = app.state.composerGeneration;
  app.deferLaunch();
  const pending = app.state.launchTopic("Old B request", "owner/repo");
  await new Promise(setImmediate);
  app.state.openNewTopicInProject({ id: scopedId(b, "new-project"), name: "New B project" });
  assert.notEqual(app.state.composerGeneration, oldGeneration, "same backend gets a new composer lifetime");
  const key = draftStorageKey({ getItem: name => name === "loomUrl" ? b : null, setItem() {}, removeItem() {} });
  const newerDraft = JSON.stringify({ title: "New work", body: "Keep this draft", project: { id: scopedId(b, "new-project"), name: "New B project" } });
  app.values.set(key, newerDraft);
  app.releaseLaunch(); await pending;
  await new Promise(setImmediate);
  assert.equal(app.state.showNewTopic, true);
  assert.equal(app.state.newTopicProject.id, scopedId(b, "new-project"));
  assert.equal(app.state.selectedId, null, "old result does not select its track");
  assert.equal(app.values.get(key), newerDraft, "old result cannot remove the new backend-scoped draft");
  assert.equal(app.calls.filter(call => call.command === "open_session").length, 0);
});

test("closing and reopening a same-backend thread composer isolates its pending launch", async t => {
  const app = await mountApp(); t.after(app.stop);
  app.state.openNewThread(); app.deferLaunch();
  const pending = app.state.launchThread("Old A thread", "owner/repo");
  await new Promise(setImmediate);
  app.state.closeNewThreadSheet(); app.state.openNewThread();
  app.releaseLaunch(); await pending;
  assert.equal(app.state.showNewThread, true);
  assert.equal(app.state.selectedId, null);
});

test("launch completion awaiting its thread cannot clear a composer opened during the fetch", async t => {
  const app = await mountApp(); t.after(app.stop);
  app.state.openNewTopicInProject({ id: scopedId(b, "project"), name: "Old B project" });
  app.deferOpen();
  const pending = app.state.launchTopic("Old B", "owner/repo");
  await new Promise(setImmediate);
  assert.equal(app.state.showNewTopic, true, "main keeps the submitted composer painted until the created thread is ready");
  app.state.openNewTopicInProject({ id: scopedId(b, "new-project"), name: "New B" });
  const key = draftStorageKey({ getItem: name => name === "loomUrl" ? b : null, setItem() {}, removeItem() {} });
  app.values.set(key, "newer draft snapshot");
  app.releaseOpen(); await pending;
  await new Promise(setImmediate);
  assert.equal(app.state.showNewTopic, true);
  assert.equal(app.state.newTopicProject.id, scopedId(b, "new-project"));
  assert.equal(app.values.get(key), "newer draft snapshot");
  assert.equal(app.state.selectedView, null, "late thread fetch cannot publish into newer composer navigation");
});

test("successful background launch removes its unchanged submitted draft without leaving healthy navigation", async t => {
  const app = await mountApp(); t.after(app.stop);
  app.state.openNewTopicInProject({ id: scopedId(b, "project"), name: "B" });
  const key = `arachne.newTopicDraft@${b}`;
  const snapshot = JSON.stringify({ title: "Submitted B", body: "Already launched", repo: "owner/repo" });
  app.values.set(key, snapshot);
  app.deferLaunch(); const pending = app.state.launchTopic("Already launched", "owner/repo");
  await new Promise(setImmediate);
  await app.state.selectSession(scopedId(a, "track"));
  app.values.set(key, snapshot); // The actual child snapshots the same draft on unmount.
  app.releaseLaunch(); await pending; await new Promise(setImmediate);
  assert.equal(app.state.selectedId, scopedId(a, "track"));
  assert.equal(app.values.has(key), false, "submitted B task cannot be resurrected after its background success");
});

test("background launch preserves changed snapshots and newly reopened identical drafts", async () => {
  for (const reopen of [false, true]) {
    const app = await mountApp();
    try {
      app.state.openNewTopicInProject({ id: scopedId(b, "project"), name: "B" });
      const key = `arachne.newTopicDraft@${b}`;
      const submitted = JSON.stringify({ title: "Original B", body: "Same content", repo: "owner/repo" });
      app.values.set(key, submitted);
      app.deferLaunch(); const pending = app.state.launchTopic("Same content", "owner/repo");
      await new Promise(setImmediate);
      await app.state.selectSession(scopedId(a, "track"));
      const newer = reopen ? submitted : JSON.stringify({ title: "Edited B", body: "New work" });
      if (reopen) app.state.openNewTopicInProject({ id: scopedId(b, "project"), name: "B reopened" });
      app.values.set(key, newer);
      app.releaseLaunch(); await pending; await new Promise(setImmediate);
      assert.equal(app.values.get(key), newer, reopen ? "a newer composer owns even an identical draft" : "changed content is preserved");
      assert.equal(app.state.showNewTopic, reopen);
    } finally { app.stop(); }
  }
});


test("renaming a server persists its live alias without changing connection, credentials or default", async t => {
  const app = await mountApp(); t.after(app.stop);
  await app.state.selectSession(scopedId(b, "track"));
  const backend = app.state.backends.find((row: any) => row.url === b);
  const original = { ...backend };
  const calls = app.calls.length;
  const tokens = [...app.installedTokens];
  app.state.renameServer(b, "  Research server  ");
  assert.equal(backend.name, "Research server");
  assert.deepEqual({ ...backend, name: original.name }, original, "URL, fleet, subscriptions and availability keep their identity");
  assert.equal(app.calls.length, calls, "rename has no native or credential calls");
  assert.deepEqual([...app.installedTokens], tokens);
  assert.equal(app.savedTokens.size, 0);
  assert.equal(app.state.loomUrl, a); assert.equal(app.values.get("loomUrl"), a);
  assert.equal(app.state.selectedId, scopedId(b, "track"));
  assert.equal(app.state.operationUrl, b);
  const { readServerProfiles } = await import("../src/serverProfiles.ts");
  assert.equal(readServerProfiles({ getItem: key => app.values.get(key) ?? null, setItem() {}, removeItem() {} }).find(row => row.url === b)?.name, "Research server");
  const { registeredBackends } = await import("../src/backendFleet.ts");
  assert.equal(registeredBackends({ getItem: key => app.values.get(key) ?? null, setItem() {}, removeItem() {} }, a).find(row => row.url === b)?.name, "Research server", "alias survives restart");
});

test("actual App selections render owning chat content over strict two-server HTTP transport", { timeout: 60000 }, async t => {
  const wire: { server: string; method: string; path: string; body: any; status: number }[] = [];
  const sessionFixture = JSON.parse(readFileSync(new URL("../src-tauri/tests/fixtures/launch-response.json", import.meta.url), "utf8"));
  const sessions = new Map<string, any>();
  const hosts = await Promise.all(["A", "B"].map(async server => {
    const session = { ...sessionFixture, id: "track", status: "idle", parent_id: null, parent_session_id: null,
      branch: { ...sessionFixture.branch, id: "distinct-branch", name: `${server} track`, title: `${server} track` }, placement: wireSession.placement };
    sessions.set(server, session);
    const http = createServer(async (request, response) => {
      let text = ""; for await (const chunk of request) text += chunk;
      const body = text ? JSON.parse(text) : {};
      const path = request.url!;
      let result: any = { error: "session not found" }, status = 404;
      if (request.method === "GET" && path === "/api/health") { status = 200; result = {}; }
      if (request.method === "POST") {
        if (path === "/api/sessions/summary/list") { status = 200; result = [session]; }
        if (path === "/api/session_layout/get") { status = 200; result = wireLayout; }
        if (["/api/sessions/get", "/api/sessions/chat"].includes(path) && ["track", "distinct-branch"].includes(body.session)) {
          status = 200;
          result = path.endsWith("/get") ? session : { blocks: [{ kind: "agent_message", turn: 1, seq: 1, created_at: "2026-10-11T00:00:00Z", payload: { text: `Conversation from ${server}` } }], metadata: { config_options: [], commands: [] }, older_cursor: null, pending_prompt: null, live_turn: null };
        }
      }
      wire.push({ server, method: request.method!, path, body, status });
      response.writeHead(status, { "Content-Type": "application/json" }); response.end(JSON.stringify(result));
    });
    await new Promise<void>(resolve => http.listen(0, "127.0.0.1", resolve));
    t.after(() => new Promise<void>(resolve => { http.closeAllConnections(); http.close(() => resolve()); }));
    return `http://127.0.0.1:${(http.address() as any).port}`;
  }));
  let transport: (command: string, args: any) => Promise<any>;
  if (process.env.NATIVE_NAVIGATION_BRIDGE) {
    const child = spawn(process.env.NATIVE_NAVIGATION_BRIDGE, [], { stdio: ["pipe", "pipe", "inherit"] });
    const replies: { resolve: (value: any) => void; reject: (error: Error) => void }[] = [];
    createInterface({ input: child.stdout }).on("line", line => {
      const reply = replies.shift()!; const value = JSON.parse(line);
      if (value.error) reply.reject(new Error(value.error)); else reply.resolve(value.result);
    });
    child.on("error", error => { for (const reply of replies.splice(0)) reply.reject(error); });
    child.on("exit", code => { for (const reply of replies.splice(0)) reply.reject(new Error(`native bridge exited ${code}`)); });
    t.after(() => child.kill());
    transport = (command, args) => new Promise((resolve, reject) => { replies.push({ resolve, reject }); child.stdin.write(JSON.stringify({ command, args }) + "\n"); });
    t.diagnostic("Actual Rust ServerRegistry, LoomClient and display-block conversion in use.");
  } else {
    // Plain frontend runs use the same operation contract; the documented
    // native bridge run exercises the actual Rust HTTP implementation.
    const op = async (url: string, path: string, body?: any) => {
      const response = await fetch(url + path, body ? { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) } : {});
      if (!response.ok) throw new Error(`Loom${response.status} ${path}`);
      return response.json();
    };
    transport = async (command, args) => {
      if (command === "connect") return op(args.baseUrl, "/api/health");
      if (command === "refresh_fleet") return { server_url: args.baseUrl, sessions: await op(args.baseUrl, "/api/sessions/summary/list", { archived: true }), layout: await op(args.baseUrl, "/api/session_layout/get", {}) };
      const chat = await op(args.baseUrl, command === "open_session" ? "/api/sessions/get" : "/api/sessions/chat", { session: args.id });
      return command === "open_session" ? chat : { ...chat, blocks: chat.blocks.map((block: any) => ({ ...block, text: block.payload.text })) };
    };
  }
  const app = await mountApp(undefined, transport, hosts); t.after(app.stop);
  // Initial fleet requests are real HTTP/native work, not resolved fixtures.
  for (let attempt = 0; attempt < 1000 && app.state.fleet.length !== 2; attempt++) await new Promise(resolve => setTimeout(resolve, 10));
  assert.equal(app.state.fleet.length, 2, JSON.stringify(app.state.backends.map((backend: any) => backend.error)));
  const [first, second] = hosts;
  assert.equal(app.state.loomUrl, first);
  // A deliberately leaked UI identity gets the real server's 404. The App
  // path below must avoid it by stripping exactly one qualification layer.
  await assert.rejects(transport("open_session", { baseUrl: second, id: scopedId(second, "track") }), /404/);
  assert.equal(wire.at(-1)?.status, 404);
  wire.length = 0;
  app.emit("loom://fleet", { server_url: second + "/", sessions: [sessions.get("B")], layout: wireLayout });
  assert.deepEqual(app.state.fleet.map((row: any) => row.id), hosts.map(url => scopedId(url, "track")), "raw pushed fleets scope exactly once");
  const cache = JSON.parse(app.values.get("arachne.backendProjects.v1")!);
  assert.equal(cache[second].spaces[0].groups[0].id, "project");
  assert.deepEqual(cache[second].spaces[0].groups[0].session_ids, []);
  const { registeredBackends } = await import("../src/backendFleet.ts");
  const restored = registeredBackends({ getItem: key => app.values.get(key) ?? null, setItem() {}, removeItem() {} }, first);
  assert.deepEqual(restored.map(row => row.layout!.spaces[0].groups[0].id), hosts.map(url => scopedId(url, "project")), "cached raw headings scope exactly once on restart");
  assert.ok(restored.every(row => row.sessions.length === 0), "chat/selection is not restored from project cache");
  for (const [url, server, select] of [[second, "B", "selectTopic"], [first, "A", "selectSession"], [second, "B", "selectSession"]]) {
    await app.state[select](scopedId(url, "track"));
    // selectTopic starts its async open; wait for the actual selected reply.
    for (let attempt = 0; attempt < 1000 && !app.state.selectedView; attempt++) await new Promise(resolve => setTimeout(resolve, 10));
    assert.equal(app.state.connError, null);
    assert.equal(app.state.selectedView.id, scopedId(url, "track"));
    assert.equal(app.state.selectedView.branch.id, scopedId(url, "distinct-branch"));
    assert.equal(app.state.selectedView.branch.title, `${server} track`);
    assert.equal(app.state.operationUrl, url);
    app.state.onProjectLayout(app.state.backends.find((row: any) => row.url === url).layout);
    assert.equal(JSON.stringify(JSON.parse(app.values.get("arachne.backendProjects.v1")!)[url]).includes("loom:"), false, "scoped layout replies are cached as wire identities");
    const chat = mountRoutingComponent("ThreadView", { session: app.state.selectedView, fleet: app.state.fleet, loomUrl: url, launchOptions: null, topic: null, unarchiving: false }, (command, args) => command === "fetch_chat" ? transport(command, args) : Promise.resolve(null), true);
    try {
      for (let attempt = 0; attempt < 1000 && !chat.state.historyReady; attempt++) await new Promise(resolve => setTimeout(resolve, 10));
      assert.equal(chat.state.historyReady, true, JSON.stringify(chat.events));
      assert.equal(chat.state.blocks[0].text, `Conversation from ${server}`);
      assert.ok(chat.elements(el => el.text === `Conversation from ${server}`).length, "selected conversation is rendered");
    } finally { chat.stop(); }
    assert.equal(app.state.loomUrl, first, "selecting B does not change A default");
  }
  const reads = wire.filter(call => ["/api/sessions/get", "/api/sessions/chat"].includes(call.path));
  assert.deepEqual(reads.map(call => [call.server, call.path, call.body, call.status]), ["B", "A", "B"].flatMap(server => [
    [server, "/api/sessions/get", { session: "track" }, 200], [server, "/api/sessions/chat", { session: "track" }, 200],
  ]));
  assert.ok(wire.every(call => call.status === 200), "strict endpoint/raw-ID server never returned 404");
});
