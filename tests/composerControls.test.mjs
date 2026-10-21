import assert from "node:assert/strict";
import { scopeSession, scopedId, splitScopedId } from "../src/backendIdentity.ts";
const SERVER = "http://127.0.0.1:7878";
import { after, before, test } from "node:test";
import { readFile, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";
import { build } from "esbuild";
import { parse, compileScript } from "@vue/compiler-sfc";
import { createRenderer, h, nextTick, reactive } from "vue";

const root = fileURLToPath(new URL("../", import.meta.url));
const fixture = JSON.parse(await readFile(new URL("../src-tauri/tests/fixtures/launch-response.json", import.meta.url), "utf8"));
let directory;
let ThreadView;
let FleetSidebar;

// Compile the real client templates, then replace only the native boundary.
// Vue's custom renderer lets these tests exercise updates and click handlers
// without adding a browser, DOM emulator, or a separate test runner.
before(async () => {
  directory = await mkdtemp(path.join(tmpdir(), "arachne-controls-"));
  const result = await build({
    stdin: {
      contents: 'export { default as ThreadView } from "./src/components/ThreadView.vue"; export { default as FleetSidebar } from "./src/components/FleetSidebar.vue";',
      resolveDir: root,
    },
    bundle: true,
    format: "esm",
    platform: "node",
    write: false,
    plugins: [{
      name: "component-test",
      setup(builder) {
        builder.onResolve({ filter: /^vue$/ }, () => ({ path: import.meta.resolve("vue"), external: true }));
        builder.onResolve({ filter: /^@tauri-apps\// }, ({ path }) => ({ path, namespace: "native-test" }));
        builder.onResolve({ filter: /ChatMarkdown\.vue$/ }, ({ path }) => ({ path, namespace: "stub-test" }));
        builder.onLoad({ filter: /.*/, namespace: "native-test" }, () => ({ contents: `
          export const invoke = (...args) => globalThis.__composerControls.invoke(...args);
          export const listen = (...args) => globalThis.__composerControls.listen(...args);
          export const open = () => { throw new Error("Unexpected shell open"); };
        ` }));
        // ChatMarkdown needs a real DOM (DOMPurify, template parsing); this
        // harness exercises the message rows around it, not its sanitized
        // HTML. A span child keeps the body slot structure intact.
        builder.onLoad({ filter: /.*/, namespace: "stub-test" }, () => ({ contents: `
          import { h, defineComponent } from "vue";
          const ChatMarkdown = defineComponent({
            props: { text: { type: String, default: "" } },
            emits: ["open-artifact"],
            setup(props) { return () => h("span", { class: "chat-markdown" }, props.text); },
          });
          export default ChatMarkdown;
        ` }));
        builder.onLoad({ filter: /\.vue$/ }, async ({ path: filename }) => {
          const { descriptor } = parse(await readFile(filename, "utf8"), { filename });
          return {
            contents: compileScript(descriptor, {
              id: path.relative(root, filename),
              inlineTemplate: true,
              templateOptions: { compilerOptions: { hoistStatic: false } },
            }).content,
            loader: "ts",
            resolveDir: path.dirname(filename),
          };
        });
      },
    }],
  });
  const filename = path.join(directory, "components.mjs");
  await writeFile(filename, result.outputFiles[0].text);
  ({ ThreadView, FleetSidebar } = await import(pathToFileURL(filename).href));
  globalThis.Document = class {};
  globalThis.ShadowRoot = class {};
  globalThis.document = { activeElement: null, addEventListener() {}, removeEventListener() {} };
  globalThis.localStorage = { getItem: () => null };
  globalThis.requestAnimationFrame = (callback) => setImmediate(callback);
  globalThis.cancelAnimationFrame = clearImmediate;
});

after(async () => {
  delete globalThis.__composerControls;
  delete globalThis.document;
  delete globalThis.Document;
  delete globalThis.ShadowRoot;
  delete globalThis.localStorage;
  delete globalThis.requestAnimationFrame;
  delete globalThis.cancelAnimationFrame;
  if (directory) await rm(directory, { recursive: true, force: true });
});

function element(type, text = "") {
  return {
    type, text, props: {}, children: [], parent: null, listeners: {}, style: {},
    value: "", scrollHeight: 0, scrollTop: 0, clientHeight: 0,
    addEventListener(name, handler) { (this.listeners[name] ??= []).push(handler); },
    removeEventListener() {},
    getBoundingClientRect: () => ({ top: 0 }),
    getRootNode: () => globalThis.document,
    querySelectorAll: () => [],
    setAttribute(name, value) { this.props[name] = value; },
    removeAttribute(name) { delete this.props[name]; },
  };
}

const renderer = createRenderer({
  createElement: element,
  createText: (text) => element("#text", text),
  createComment: (text) => element("#comment", text),
  setText: (node, text) => { node.text = text; },
  setElementText: (node, text) => { node.text = text; node.children = []; },
  patchProp: (node, key, _previous, value) => {
    node.props[key] = value;
    if (key === "class" && value === "conversation" && globalThis.__composerControls.geometry) {
      // Model real scroll clamping and height changes as message rows and
      // the fallback control render. This also catches double anchoring.
      let top = 0;
      let delayedHeight = 0;
      node.clientHeight = 400;
      Object.defineProperty(node, "scrollHeight", { get: () => 1000 + delayedHeight + node.children.reduce((height, child) =>
        height + (child.props.class?.split(" ").includes("block") ? 300 : child.props.class === "load-older" ? 40 : 0), 0) });
      Object.defineProperty(node, "scrollTop", {
        get: () => top,
        set: (value) => { top = Math.max(0, Math.min(value, node.scrollHeight - node.clientHeight)); },
      });
      node.growAboveViewport = (height) => {
        delayedHeight += height;
        if (node.style.overflowAnchor !== "none") node.scrollTop += height;
      };
    }
  },
  parentNode: (node) => node.parent,
  nextSibling: (node) => node.parent?.children[node.parent.children.indexOf(node) + 1] ?? null,
  insert(node, parent, anchor = null) {
    if (node.parent) node.parent.children.splice(node.parent.children.indexOf(node), 1);
    const index = anchor ? parent.children.indexOf(anchor) : -1;
    parent.children.splice(index < 0 ? parent.children.length : index, 0, node);
    node.parent = parent;
  },
  remove(node) {
    node.parent.children.splice(node.parent.children.indexOf(node), 1);
    node.parent = null;
  },
});

const flush = async () => { await new Promise(setImmediate); await nextTick(); };
const textOf = (node) => node.type === "#comment" ? "" : node.text + node.children.map(textOf).join("");
function find(node, predicate) {
  if (predicate(node)) return node;
  for (const child of node.children) {
    const found = find(child, predicate);
    if (found) return found;
  }
  return null;
}
const button = (tree, label) => find(tree, (node) => node.type === "button" && (node.props["aria-label"] === label || textOf(node).trim() === label));
function click(node) {
  assert.ok(node, "button must be rendered");
  if (node.props.disabled) return;
  const event = { stopPropagation() {}, preventDefault() {} };
  for (const handler of [node.props.onClick].flat()) handler(event);
}
async function typeDraft(tree, value) {
  const textarea = find(tree, (node) => node.type === "textarea");
  assert.ok(textarea, "composer must be rendered");
  textarea.value = value;
  for (const handler of textarea.listeners.input) handler({ target: textarea });
  await flush();
}
function session(status = "running", tags = [], protocol = "terminal") {
  return scopeSession(SERVER, { ...fixture, status, protocol, branch: { ...fixture.branch, tags } });
}

async function mount(t, Component, initialProps, liveTurn = null, options = {}) {
  const calls = [];
  const errors = [];
  const props = reactive(initialProps);
  const bridge = {
    geometry: options.geometry,
    snapshot: { blocks: [], metadata: { config_options: [], commands: [], modes: [], steering_supported: false }, live_turn: liveTurn, pending_prompt: null, older_cursor: null, ...options.snapshot },
    action: async () => {},
    async invoke(command, args) {
      calls.push({ command, args });
      if (this.fetch && command === "fetch_chat") return this.fetch(args);
      if (command === "fetch_chat") return this.snapshot;
      if (command === "chat_older_cursor" || command === "work_summary") return null;
      if (["interrupt", "send_input", "clear_attention", "publish_track"].includes(command)) return this.action(command, args);
      throw new Error(`Unexpected native command: ${command}`);
    },
    async listen(_name, callback) { this.onFrame = callback; return () => {}; },
  };
  globalThis.__composerControls = bridge;
  const tree = element("root");
  const app = renderer.createApp({ render: () => h(Component, { ...props, onError: (error) => errors.push(error) }) });
  app.config.errorHandler = (error) => errors.push(error);
  t.after(() => { app.unmount(); assert.deepEqual(errors, options.expectedErrors ?? [], "component must not fail during lifecycle or actions"); });
  app.mount(tree);
  await flush();
  calls.length = 0;
  return { tree, props, bridge, calls, errors, app };
}

function threadProps(current) {
  return { session: current, fleet: [current], topic: current, launchOptions: null, loomUrl: "http://127.0.0.1:7878", unarchiving: false };
}

test("chat renders time separators only for visible activity and keeps full dates in tooltips", async (t) => {
  const blocks = [
    { kind: "turn_end", turn: 0, seq: 0, stop_reason: "end_turn", created_at: "2026-10-14T11:00:00Z" },
    { kind: "plan", turn: 1, seq: 0, entries: [], created_at: "2026-10-14T12:00:00Z" },
    { kind: "usage", turn: 1, seq: 1, created_at: "2026-10-14T12:09:00Z" },
    { kind: "other", turn: 1, seq: 2, created_at: "2026-10-14T12:09:30Z" },
    { kind: "plan", turn: 2, seq: 0, entries: [], created_at: "2026-10-14T12:10:00Z" },
    { kind: "plan", turn: 2, seq: 1, entries: [], created_at: "invalid" },
  ];
  const { tree } = await mount(t, ThreadView, threadProps(session()), null, { snapshot: { blocks } });
  const labels = conversation(tree).children.filter((node) => node.type === "time");
  assert.deepEqual(labels.map((node) => node.props.datetime), [
    "2026-10-14T12:00:00.000Z", "2026-10-14T12:10:00.000Z",
  ]);
  for (const label of labels) {
    assert.equal(label.props.class, "chat-timestamp");
    assert.equal(label.props.title, new Date(label.props.datetime).toLocaleString());
    assert.ok(textOf(label).trim());
  }
});

test("message rows hover their own local time and hide the tooltip when the journal time is unusable", async (t) => {
  // Pin "now" — the component derives its calendar day from Date.now at
  // setup — so today/other-day tooltip formatting is deterministic.
  const realNow = Date.now;
  const now = new Date(2026, 9, 14, 15, 0);
  Date.now = () => now.getTime();
  t.after(() => { Date.now = realNow; });
  const clock = (value) => value.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  const blocks = [
    { kind: "user_message", turn: 1, seq: 0, text: "hello there", created_at: new Date(2026, 9, 14, 15, 45).toISOString() },
    { kind: "agent_message", turn: 1, seq: 1, text: "hi back", created_at: new Date(2026, 9, 14, 15, 46).toISOString() },
    { kind: "agent_message", turn: 1, seq: 2, text: "no journal time" },
    { kind: "user_message", turn: 2, seq: 0, text: "from yesterday", created_at: new Date(2026, 9, 13, 9, 5).toISOString() },
    { kind: "user_message", turn: 3, seq: 0, text: "bad journal time", created_at: "invalid" },
  ];
  const { tree } = await mount(t, ThreadView, threadProps(session()), null, { snapshot: { blocks } });
  const messageRow = (host) => find(conversation(tree), (node) => node.props["data-copy-host"] === host);
  for (const host of ["block:1:0", "block:1:1", "block:1:2", "block:2:0", "block:3:0"]) assert.ok(messageRow(host), `${host} must render`);
  // Today's messages hover the clock alone.
  assert.equal(messageRow("block:1:0").props.title, clock(new Date(2026, 9, 14, 15, 45)));
  assert.equal(messageRow("block:1:1").props.title, clock(new Date(2026, 9, 14, 15, 46)));
  // A message from another calendar day prefixes its date.
  const yesterday = new Date(2026, 9, 13, 9, 5);
  assert.equal(messageRow("block:2:0").props.title,
    `${yesterday.toLocaleDateString(undefined, { month: "short", day: "numeric" })}, ${clock(yesterday)}`);
  // Missing or unparsable times must not show a bogus "Invalid Date" tooltip.
  assert.equal(messageRow("block:1:2").props.title, undefined);
  assert.equal(messageRow("block:3:0").props.title, undefined);
});

test("Track publish history starts collapsed while current Publish feedback stays visible and routed", async (t) => {
  const previous = "00b79edd72788dc38f6ae27274dd1266019c2cb5";
  const tags = [{ key: "topic", value: "true" }, { key: "track_topic_branch", value: "weaver/workflow-2" },
    { key: "publish_result", value: previous, note: "land-locally: published refs/heads/previous" }];
  const current = session("running", tags);
  const { tree, bridge, calls, errors } = await mount(t, ThreadView, threadProps(current), null, { expectedErrors: ["Selected backend unavailable"] });
  const details = find(tree, node => node.type === "details" && textOf(node).includes("Publish details"));
  assert.ok(details, "native details supplies keyboard-accessible disclosure semantics");
  assert.ok(!details.props.open, "history must be collapsed by default");
  assert.equal(textOf(find(details, node => node.type === "summary")), "Publish details");
  assert.match(textOf(details), /Source branch: weaver\/workflow-2/);
  assert.match(textOf(details), new RegExp(`Previous publish: ${previous}`));
  assert.match(textOf(details), /land-locally: published refs\/heads\/previous/);
  const visibleText = node => node.type === "details" ? textOf(find(node, child => child.type === "summary"))
    : node.type === "#comment" ? "" : node.text + node.children.map(visibleText).join("");
  assert.ok(!visibleText(tree).includes(previous));
  assert.ok(!visibleText(tree).includes("weaver/workflow-2"));
  assert.ok(!visibleText(tree).includes("published refs/heads/previous"));
  let complete;
  bridge.action = () => new Promise(resolve => { complete = resolve; });
  click(button(tree, "Choose publish strategy"));
  await flush();
  click(button(tree, "Squash into local primary current branch"));
  await flush();
  assert.ok(button(tree, "…").props.disabled);
  assert.match(visibleText(tree), /Publishing to local primary current branch…/);
  assert.deepEqual(calls, [{ command: "publish_track", args: { sessionId: fixture.id, strategy: "land-locally", baseUrl: SERVER } }]);
  complete({ local: true, landing: { landed: true, target_branch: "current-primary", commit: "c".repeat(40), commits_squashed: 1, primary_checkout: "/repo" } });
  await flush();
  assert.match(visibleText(tree), /Published locally: squash of 1 commit into current-primary/);
  assert.ok(!details.props.open);
  assert.deepEqual(current.branch.tags, tags, "display must not rewrite historical durable tags");
  bridge.action = async () => { throw new Error("Selected backend unavailable"); };
  click(button(tree, "Publish"));
  await flush();
  assert.deepEqual(errors, ["Selected backend unavailable"]);
  assert.ok(!button(tree, "Publish").props.disabled);
  assert.ok(!visibleText(tree).includes("Published locally"));
  assert.ok(!visibleText(tree).includes(previous), "failure cannot promote historical metadata into current feedback");
});

test("working terminal composer stops once while pending, then follows live idle state", async (t) => {
  const current = session();
  const { tree, props, bridge, calls } = await mount(t, ThreadView, threadProps(current));
  assert.ok(button(tree, "Stop"));
  assert.equal(button(tree, "Interrupt"), null);
  assert.equal(button(tree, "Send message"), null);
  let complete;
  bridge.action = () => new Promise((resolve) => { complete = resolve; });
  click(button(tree, "Stop"));
  await flush();
  assert.equal(button(tree, "Stop").props.disabled, true);
  assert.equal(button(tree, "Stop").props.title, "Stopping…");
  // Call the handler again to verify the in-flight guard as well as disabled UI.
  button(tree, "Stop").props.onClick();
  assert.deepEqual(calls, [{ command: "interrupt", args: { id: splitScopedId(current.id).id, baseUrl: SERVER } }]);
  props.fleet = [session("running", [{ key: "idle", value: "true" }])];
  complete();
  await flush();
  assert.equal(button(tree, "Stop"), null);
  assert.equal(button(tree, "Send message").props.disabled, true);
});

test("typing while working restores Send and preserves its native message wiring", async (t) => {
  const current = session();
  const { tree, calls } = await mount(t, ThreadView, threadProps(current));
  await typeDraft(tree, "  Continue with the next step  ");
  assert.equal(button(tree, "Stop"), null);
  assert.equal(button(tree, "Send message").props.disabled, false);
  click(button(tree, "Send message"));
  await flush();
  assert.deepEqual(calls.find(({ command }) => command === "send_input"), {
    command: "send_input",
    args: { id: splitScopedId(current.id).id, text: "Continue with the next step", topicId: splitScopedId(current.id).id, resourceIds: [], attachments: [], baseUrl: SERVER },
  });
  assert.equal(calls.some(({ command }) => command === "interrupt"), false);
  assert.ok(button(tree, "Stop"), "sending clears the draft, exposing Stop while work remains active");
});

test("idle attention composer switches Dismiss to Send for content and tracks live flag clearing", async (t) => {
  const tags = [{ key: "idle", value: "true" }, { key: "attention", value: "attention" }];
  const { tree, props, calls } = await mount(t, ThreadView, threadProps(session("running", tags)));
  assert.ok(button(tree, "Dismiss attention"));
  assert.equal(button(tree, "Stop"), null);
  await typeDraft(tree, "Follow up");
  assert.equal(button(tree, "Dismiss attention"), null);
  assert.equal(button(tree, "Send message").props.disabled, false);
  await typeDraft(tree, "   ");
  click(button(tree, "Dismiss attention"));
  await flush();
  assert.deepEqual(calls, [{ command: "clear_attention", args: { session: fixture.id, baseUrl: SERVER } }]);
  props.fleet = [session("running", tags.slice(0, 1))];
  await flush();
  assert.equal(button(tree, "Dismiss attention"), null);
  assert.equal(button(tree, "Send message").props.disabled, true);
});

test("ACP composer uses live chat snapshots for Stop and returns to Send after interruption", async (t) => {
  const current = session("running", [], "acp");
  const { tree, bridge, calls } = await mount(t, ThreadView, threadProps(current), 7);
  assert.ok(button(tree, "Stop"));
  bridge.action = async () => { bridge.snapshot = { ...bridge.snapshot, live_turn: null }; };
  click(button(tree, "Stop"));
  await flush();
  assert.deepEqual(calls[0], { command: "interrupt", args: { id: splitScopedId(current.id).id, baseUrl: SERVER } });
  assert.equal(button(tree, "Stop"), null);
  assert.equal(button(tree, "Send message").props.disabled, true);
});

test("archived thread emits Unarchive, disables restoration in flight, and follows live running state", async (t) => {
  const restored = [];
  const current = session("archived", [], "acp");
  const { tree, props, calls } = await mount(t, ThreadView, { ...threadProps(current), onUnarchive: (id) => restored.push(id) });
  assert.equal(button(tree, "Archive"), null);
  assert.equal(button(tree, "Stop"), null);
  click(button(tree, "Unarchive"));
  assert.deepEqual(restored, [current.id]);
  props.unarchiving = true;
  await flush();
  assert.equal(button(tree, "Unarchiving…").props.disabled, true);
  click(button(tree, "Unarchiving…"));
  assert.deepEqual(restored, [current.id]);
  props.unarchiving = false;
  props.fleet = [session("running", [], "acp")];
  await flush();
  assert.equal(button(tree, "Unarchive"), null);
  assert.ok(button(tree, "Archive"));
  assert.equal(calls.length, 0, "restoration routes through the parent event");
});

test("sidebar exposes archived Unarchive through the filter and disables only the pending track", async (t) => {
  const archived = session("archived");
  const second = { ...archived, id: "another-archived", branch: { ...archived.branch, id: "another-branch" } };
  const restored = [];
  let selected = false;
  const { tree, props, calls } = await mount(t, FleetSidebar, {
    fleet: [archived, second], layout: null, selectedId: null, unarchivingIds: new Set(),
    onUnarchive: (id) => restored.push(id), onSelectTopic: () => { selected = true; },
  });
  assert.equal(button(tree, "Unarchive"), null);
  click(button(tree, "Show archived tracks"));
  await flush();
  const restoreButton = button(tree, "Unarchive");
  const restoredCard = restoreButton.parent.parent;
  const event = { stopPropagation() { this.stopped = true; } };
  restoreButton.props.onClick(event);
  assert.equal(event.stopped, true, "restoration must not also select the topic card");
  assert.equal(selected, false);
  const id = restored[0];
  assert.ok([archived.id, second.id].includes(id));
  props.unarchivingIds = new Set([id]);
  await flush();
  assert.equal(button(tree, "Unarchiving…").props.disabled, true);
  assert.equal(button(tree, "Unarchive").props.disabled, false);
  props.fleet = props.fleet.map((item) => item.id === id ? { ...item, status: "running" } : item);
  props.unarchivingIds = new Set();
  await flush();
  assert.equal(button(tree, "Unarchiving…"), null);
  assert.equal(button(restoredCard, "Unarchive"), null, "the restored track loses its archive action");
  assert.ok(button(tree, "Unarchive"), "the other archived track remains restorable");
  assert.equal(calls.length, 0, "restoration routes through the parent event");
});

const cursor = (turn) => ({ turn, seq: 0 });
const historyPage = (turn = 10, older = cursor(9)) => ({
  blocks: [{ kind: "plan", turn, seq: 1, entries: [[`turn ${turn}`, "completed"]] }],
  older_cursor: older,
});
const conversation = (tree) => find(tree, (node) => node.props.class === "conversation");
const hasTurn = (tree, turn) => !!find(tree, (node) => node.props["data-copy-host"] === `block:${turn}:1`);
function scroll(tree, top) {
  const el = conversation(tree);
  el.scrollTop = top;
  el.props.onScroll();
}
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
async function mountHistory(t, options = {}) {
  return mount(t, ThreadView, threadProps(session()), null, { geometry: true, snapshot: historyPage(), ...options });
}
const pageCalls = (calls) => calls.filter(({ command, args }) => command === "fetch_chat" && args.beforeTurn != null);

// Exercise the real scroll handler, native paging boundary, and Vue render
// flush; geometry reflects inserted rows and removal of the history control.
test("near-top upward scroll prefetches once with the snapshot cursor and preserves the visible offset", async (t) => {
  const { tree, bridge, calls } = await mountHistory(t);
  const pending = deferred();
  bridge.fetch = () => pending.promise;
  const el = conversation(tree);
  assert.equal(el.scrollTop, el.scrollHeight - el.clientHeight, "initial visit opens at the latest message");
  scroll(tree, 241);
  assert.equal(pageCalls(calls).length, 0);
  scroll(tree, 240);
  scroll(tree, 240); // identical synthetic events must not start another page
  scroll(tree, 200);
  button(tree, "load older").props.onClick(); // bypass the disabled button
  assert.equal(pageCalls(calls).length, 1);
  assert.deepEqual(pageCalls(calls)[0].args, { id: fixture.id, beforeTurn: 9, beforeSeq: 0, baseUrl: SERVER });
  assert.equal(calls.some(({ command }) => command === "chat_older_cursor"), false, "cursor lookup cannot race or overlap paging");
  const beforeHeight = el.scrollHeight;
  pending.resolve({ ...bridge.snapshot, ...historyPage(8, cursor(7)) });
  await flush();
  assert.ok(hasTurn(tree, 8));
  assert.ok(hasTurn(tree, 10));
  assert.equal(el.scrollTop, 200 + el.scrollHeight - beforeHeight, "anchor uses the position at response time");
  el.props.onScroll();
  el.props.onScroll();
  assert.equal(pageCalls(calls).length, 1, "prepend's scroll events do not chain requests");
  bridge.fetch = async () => ({ ...bridge.snapshot, ...historyPage(6, null) });
  scroll(tree, 240);
  assert.equal(pageCalls(calls).length, 2, "scrolling up again pages from the returned cursor");
  assert.equal(pageCalls(calls)[1].args.beforeTurn, 7);
  await flush();
  assert.ok(hasTurn(tree, 6));
});

test("equal-position events, downward scrolling, and a short bottom-pinned conversation do not prefetch", async (t) => {
  const { tree, bridge, calls } = await mountHistory(t);
  const el = conversation(tree);
  el.props.onScroll();
  el.props.onScroll();
  scroll(tree, 300);
  scroll(tree, 350);
  assert.equal(pageCalls(calls).length, 0);
  el.clientHeight = el.scrollHeight;
  scroll(tree, 0);
  assert.equal(pageCalls(calls).length, 0);
  assert.ok(bridge.snapshot.older_cursor, "history exists but latest-message pinning wins");
});

for (const [name, page] of [
  ["null cursor", historyPage(8, null)],
  ["empty page", { blocks: [], older_cursor: cursor(7) }],
  ["non-advancing cursor", historyPage(8, cursor(9))],
]) {
  test(`older history stops on ${name} and repeated scroll events cannot loop`, async (t) => {
    const { tree, bridge, calls } = await mountHistory(t);
    bridge.fetch = async () => ({ ...bridge.snapshot, ...page });
    const el = conversation(tree);
    scroll(tree, 100);
    const beforeHeight = el.scrollHeight;
    await flush();
    assert.equal(el.scrollTop, 100 + el.scrollHeight - beforeHeight, "final control removal participates in anchoring");
    assert.equal(button(tree, "load older"), null);
    scroll(tree, 0);
    el.props.onScroll();
    scroll(tree, 50);
    scroll(tree, 0);
    assert.equal(pageCalls(calls).length, 1);
  });
}

test("a failed prefetch pauses automatic retries while the manual fallback remains useful", async (t) => {
  const { tree, bridge, calls, errors } = await mountHistory(t, { expectedErrors: ["offline"] });
  bridge.fetch = async () => { throw new Error("offline"); };
  scroll(tree, 200);
  await flush();
  assert.deepEqual(errors, ["offline"]);
  assert.equal(button(tree, "retry older messages").props.disabled, false);
  scroll(tree, 100);
  scroll(tree, 50);
  conversation(tree).props.onScroll();
  await flush();
  assert.equal(pageCalls(calls).length, 1);
  bridge.fetch = async (args) => args.beforeTurn == null ? bridge.snapshot : { ...bridge.snapshot, ...historyPage(8) };
  await resync(bridge);
  scroll(tree, 25);
  assert.equal(pageCalls(calls).length, 1, "live reloads must not silently resume failed prefetches");
  click(button(tree, "retry older messages"));
  await flush();
  assert.ok(hasTurn(tree, 8));
  assert.equal(pageCalls(calls).length, 2);
});

test("thread switches discard stale pages and old completion cannot unlock the new thread's request", async (t) => {
  const { tree, props, bridge, calls } = await mountHistory(t);
  const old = deferred();
  const current = deferred();
  bridge.fetch = (args) => args.beforeTurn == null
    ? { ...bridge.snapshot, ...historyPage(20, cursor(19)) }
    : args.id === fixture.id ? old.promise : current.promise;
  scroll(tree, 200);
  props.session = { ...props.session, id: scopedId(SERVER, "new-thread") };
  await flush();
  scroll(tree, 200);
  await flush();
  assert.equal(button(tree, "loading…").props.disabled, true);
  old.resolve({ ...bridge.snapshot, ...historyPage(8) });
  await flush();
  assert.equal(hasTurn(tree, 8), false);
  assert.ok(hasTurn(tree, 20));
  assert.equal(button(tree, "loading…").props.disabled, true);
  scroll(tree, 100);
  assert.equal(pageCalls(calls).length, 2);
  current.resolve({ ...bridge.snapshot, ...historyPage(18, null) });
  await flush();
  assert.ok(hasTurn(tree, 18));
  assert.equal(hasTurn(tree, 10), false);
});

test("stale page failures after server changes or unmount do not emit errors or mutate history", async (t) => {
  const { tree, props, bridge, app, errors } = await mountHistory(t);
  const old = deferred();
  bridge.fetch = (args) => args.beforeTurn == null ? bridge.snapshot : old.promise;
  scroll(tree, 200);
  props.loomUrl = "http://other-server:7878";
  await flush();
  old.reject(new Error("stale server"));
  await flush();
  assert.deepEqual(errors, []);
  const unmounted = deferred();
  bridge.fetch = () => unmounted.promise;
  scroll(tree, 200);
  app.unmount();
  unmounted.reject(new Error("unmounted"));
  await flush();
  assert.deepEqual(errors, []);
});

async function resync(bridge) {
  bridge.onFrame({ payload: { server_url: SERVER, session_id: fixture.id, event: { topic: "connection", event: "resync" } } });
  await new Promise((resolve) => setTimeout(resolve, 320));
  await flush();
}

test("latest-message reloads still autoscroll at the bottom and leave a reader above it in place", async (t) => {
  const { tree, bridge } = await mountHistory(t);
  const el = conversation(tree);
  bridge.snapshot = { ...bridge.snapshot, blocks: [...bridge.snapshot.blocks, ...historyPage(11).blocks] };
  await resync(bridge);
  assert.equal(el.scrollTop, el.scrollHeight - el.clientHeight);
  scroll(tree, 400);
  bridge.snapshot = { ...bridge.snapshot, blocks: [...bridge.snapshot.blocks, ...historyPage(12).blocks] };
  await resync(bridge);
  assert.equal(el.scrollTop, 400);
  assert.ok(hasTurn(tree, 12));
});

test("a reload invalidates an outstanding older page without causing scroll loops", async (t) => {
  const { tree, bridge, calls } = await mountHistory(t);
  const old = deferred();
  bridge.fetch = (args) => args.beforeTurn == null ? { ...bridge.snapshot, ...historyPage(11) } : old.promise;
  scroll(tree, 200);
  await resync(bridge);
  old.resolve({ ...bridge.snapshot, ...historyPage(8) });
  await flush();
  assert.equal(hasTurn(tree, 8), false);
  assert.ok(hasTurn(tree, 11));
  conversation(tree).props.onScroll();
  conversation(tree).props.onScroll();
  assert.equal(pageCalls(calls).length, 1);
  assert.equal(button(tree, "load older").props.disabled, false);
});

test("latest reloads discard stale thread replies and respect scrolling during a pending request", async (t) => {
  const { tree, props, bridge } = await mountHistory(t);
  const pending = deferred();
  bridge.fetch = () => pending.promise;
  await resync(bridge);
  scroll(tree, 400);
  pending.resolve({ ...bridge.snapshot, ...historyPage(11) });
  await flush();
  assert.equal(conversation(tree).scrollTop, 400, "a reader who moves away during the fetch stays in place");
  const stale = deferred();
  bridge.fetch = () => stale.promise;
  await resync(bridge);
  bridge.fetch = () => ({ ...bridge.snapshot, ...historyPage(20) });
  props.session = { ...props.session, id: scopedId(SERVER, "new-thread") };
  await flush();
  stale.resolve({ ...bridge.snapshot, ...historyPage(12) });
  await flush();
  assert.ok(hasTurn(tree, 20));
  assert.equal(hasTurn(tree, 12), false);
});

test("continuous SSE refreshes coalesce behind a pending tail request without starving its response", async (t) => {
  const { tree, bridge, calls } = await mountHistory(t);
  const first = deferred();
  const second = deferred();
  let fetches = 0;
  bridge.fetch = () => ++fetches === 1 ? first.promise : second.promise;
  await resync(bridge);
  await resync(bridge);
  await resync(bridge);
  assert.equal(calls.filter(({ command }) => command === "fetch_chat").length, 1, "SSE bursts share the current request");
  first.resolve({ ...bridge.snapshot, ...historyPage(11) });
  await flush();
  assert.ok(hasTurn(tree, 11), "the pending response must reach the conversation despite newer events");
  assert.equal(conversation(tree).scrollTop, conversation(tree).scrollHeight - conversation(tree).clientHeight);
  await new Promise((resolve) => setTimeout(resolve, 320));
  assert.equal(fetches, 2, "one trailing refresh catches events received during the fetch");
  second.resolve({ ...bridge.snapshot, ...historyPage(12) });
  await flush();
  assert.ok(hasTurn(tree, 12));
  assert.equal(button(tree, "load older").props.disabled, false);
});

test("a failed tail refresh keeps the displayed history cursor usable without starting an automatic retry", async (t) => {
  const { tree, bridge, calls } = await mountHistory(t, { expectedErrors: ["tail offline"] });
  bridge.fetch = async (args) => {
    if (args.beforeTurn == null) throw new Error("tail offline");
    return { ...bridge.snapshot, ...historyPage(8, null) };
  };
  await resync(bridge);
  assert.ok(hasTurn(tree, 10));
  assert.equal(button(tree, "load older").props.disabled, false);
  conversation(tree).props.onScroll();
  assert.equal(calls.filter(({ command }) => command === "fetch_chat").length, 1);
  click(button(tree, "load older"));
  await flush();
  assert.ok(hasTurn(tree, 8));
  assert.equal(pageCalls(calls).length, 1);
});

test("prepend compensation restores browser anchoring for delayed image height changes", async (t) => {
  const { tree, bridge } = await mountHistory(t);
  const pending = deferred();
  bridge.fetch = () => pending.promise;
  const el = conversation(tree);
  el.style.overflowAnchor = "auto";
  scroll(tree, 200);
  pending.resolve({ ...bridge.snapshot, ...historyPage(8) });
  await flush();
  assert.equal(el.style.overflowAnchor, "auto", "temporary suppression must restore the previous anchor policy");
  const offset = el.scrollHeight - el.scrollTop;
  el.growAboveViewport(240); // native image content arrives after the prepend flush
  assert.equal(el.scrollHeight - el.scrollTop, offset, "the browser can still compensate for delayed content above the reader");
  el.props.onScroll();
  assert.equal(el.style.overflowAnchor, "auto");
});
