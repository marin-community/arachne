import assert from "node:assert/strict";
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
        builder.onLoad({ filter: /.*/, namespace: "native-test" }, () => ({ contents: `
          export const invoke = (...args) => globalThis.__composerControls.invoke(...args);
          export const listen = (...args) => globalThis.__composerControls.listen(...args);
          export const open = () => { throw new Error("Unexpected shell open"); };
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
});

after(async () => {
  delete globalThis.__composerControls;
  delete globalThis.document;
  delete globalThis.Document;
  delete globalThis.ShadowRoot;
  delete globalThis.localStorage;
  if (directory) await rm(directory, { recursive: true, force: true });
});

function element(type, text = "") {
  return {
    type, text, props: {}, children: [], parent: null, listeners: {},
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
  patchProp: (node, key, _previous, value) => { node.props[key] = value; },
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
  return { ...fixture, status, protocol, branch: { ...fixture.branch, tags } };
}

async function mount(t, Component, initialProps, liveTurn = null) {
  const calls = [];
  const errors = [];
  const props = reactive(initialProps);
  const bridge = {
    snapshot: { blocks: [], metadata: { config_options: [], commands: [], modes: [], steering_supported: false }, live_turn: liveTurn, pending_prompt: null },
    action: async () => {},
    async invoke(command, args) {
      calls.push({ command, args });
      if (command === "fetch_chat") return this.snapshot;
      if (command === "chat_older_cursor" || command === "work_summary") return null;
      if (["interrupt", "send_input", "clear_attention"].includes(command)) return this.action(command, args);
      throw new Error(`Unexpected native command: ${command}`);
    },
    async listen() { return () => {}; },
  };
  globalThis.__composerControls = bridge;
  const tree = element("root");
  const app = renderer.createApp({ render: () => h(Component, { ...props, onError: (error) => errors.push(error) }) });
  app.config.errorHandler = (error) => errors.push(error);
  t.after(() => { app.unmount(); assert.deepEqual(errors, [], "component must not fail during lifecycle or actions"); });
  app.mount(tree);
  await flush();
  calls.length = 0;
  return { tree, props, bridge, calls };
}

function threadProps(current) {
  return { session: current, fleet: [current], topic: current, launchOptions: null, loomUrl: "http://127.0.0.1:7878", unarchiving: false };
}

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
  assert.deepEqual(calls, [{ command: "interrupt", args: { id: current.id } }]);
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
    args: { id: current.id, text: "Continue with the next step", topicId: current.id, resourceIds: [], attachments: [] },
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
  assert.deepEqual(calls, [{ command: "clear_attention", args: { session: fixture.id } }]);
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
  assert.deepEqual(calls[0], { command: "interrupt", args: { id: current.id } });
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
