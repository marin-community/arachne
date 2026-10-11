import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { readFile, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";
import { build } from "esbuild";
import { parse, compileScript } from "@vue/compiler-sfc";
import { createRenderer, h, nextTick, reactive } from "vue";
import { scopedId } from "../src/backendIdentity.ts";

const localUrl = "http://127.0.0.1:7878";
const emptyLayout = { revision: 0, spaces: [], defaults: [] };

const root = fileURLToPath(new URL("../", import.meta.url));
const fixture = JSON.parse(await readFile(new URL("../src-tauri/tests/fixtures/launch-response.json", import.meta.url), "utf8"));
let directory;
let App;

// Compile the real client templates and replace native calls and unrelated panels.
// Vue's custom renderer lets these tests exercise updates and click handlers
// without adding a browser, DOM emulator, or a separate test runner.
before(async () => {
  directory = await mkdtemp(path.join(tmpdir(), "arachne-inspector-"));
  const result = await build({
    stdin: {
      contents: 'export { default as App } from "./src/App.vue";',
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
          export const invoke = (...args) => globalThis.__inspectorLayout.invoke(...args);
          export const listen = (...args) => globalThis.__inspectorLayout.listen(...args);
          export const isTauri = () => false;
          export const isPermissionGranted = async () => false;
          export const requestPermission = async () => "denied";
          export const sendNotification = () => {};
          export const open = () => { throw new Error("Unexpected shell open"); };
        ` }));
        builder.onLoad({ filter: /\.vue$/ }, async ({ path: filename }) => {
          const name = path.basename(filename);
          // Keep the real App, ThreadView, inspector, review, editor and code menu.
          // Replace unrelated data-heavy panels with observable event boundaries.
          if (["FleetSidebar.vue", "HomeView.vue"].includes(name)) return {
            contents: `import { h } from "vue"; export default { props: ["fleet"], emits: ["select"], setup(props, { emit }) { return () => h("nav", (globalThis.__inspectorLayout.availableSessions ?? props.fleet).map(s => h("button", { onClick: () => emit("select", globalThis.__inspectorLayout.selectionId(s.id)) }, "Select " + s.id))); } };`, loader: "js",
          };
          if (["ResourcePanel.vue", "TrackActivity.vue", "AutolandControl.vue", "AttentionBanner.vue"].includes(name)) return {
            contents: `import { h } from "vue"; export default { props: ["artifactRequest"], setup(props) { return () => h("div", { class: "panel-boundary" }, props.artifactRequest?.name || "Panel"); } };`, loader: "js",
          };
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
  ({ App } = await import(pathToFileURL(filename).href));
  globalThis.Document = class {};
  globalThis.ShadowRoot = class {};
  globalThis.document = { activeElement: null, addEventListener() {}, removeEventListener() {} };
  globalThis.localStorage = { getItem: () => null, setItem() {}, removeItem() {} };
  globalThis.window = {
    listeners: new Map(),
    addEventListener(name, handler) { this.listeners.set(name, handler); },
    removeEventListener(name) { this.listeners.delete(name); },
    confirm: () => true,
  };
  globalThis.FileReader = class {
    readAsDataURL(file) { this.result = "data:text/plain;base64," + file.base64; this.onload(); }
  };
  globalThis.requestAnimationFrame = (callback) => setImmediate(callback);
  globalThis.cancelAnimationFrame = clearImmediate;
});

after(async () => {
  delete globalThis.__inspectorLayout;
  delete globalThis.document;
  delete globalThis.window;
  delete globalThis.FileReader;
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
    focus() { this.focused = true; },
  };
}

const renderer = createRenderer({
  querySelector: () => globalThis.__inspectorLayout.tree,
  createElement: element,
  createText: (text) => element("#text", text),
  createComment: (text) => element("#comment", text),
  setText: (node, text) => { node.text = text; },
  setElementText: (node, text) => { node.text = text; node.children = []; },
  patchProp: (node, key, _previous, value) => {
    node.props[key] = value;
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
  return { ...fixture, status, protocol, branch: { ...fixture.branch, tags } };
}

async function mount(t, Component, initialProps, liveTurn = null, options = {}) {
  const calls = [];
  const errors = [];
  const props = reactive(initialProps);
  const bridge = {
    availableSessions: options.sessions ?? options.fleet,
    selectionUrl: localUrl,
    selectionId(id) { return scopedId(this.selectionUrl, id); },
    fleetByUrl: new Map([[localUrl, options.fleet]]),
    snapshot: { blocks: [], metadata: { config_options: [], commands: [], modes: [], steering_supported: false }, live_turn: liveTurn, pending_prompt: null, older_cursor: null },
    listeners: new Map(),
    async invoke(command, args) {
      calls.push({ command, args });
      if (command === "load_token") return null;
      if (command === "connect") return;
      if (command === "refresh_fleet") return { server_url: args.baseUrl,
        sessions: this.fleetByUrl.get(args.baseUrl) ?? [], layout: emptyLayout };
      if (command === "launch_options") return { agents: [], profiles: [], default_agent: "" };
      if (command === "open_session") {
        const { parent_id, parent_session_id, ...view } = (options.sessions ?? options.fleet).find(s => s.id === args.id);
        return view; // SessionView's wire shape has no ancestry fields.
      }
      if (command === "fetch_chat") return this.snapshot;
      if (command === "chat_older_cursor") return null;
      if (command === "work_summary") return { files: 1, additions: 1, deletions: 0, has_commits: true };
      if (command === "topic_todos") return { todos: [], revision: 0 };
      if (command === "work_changes" && this.workChanges) return this.workChanges(args);
      if (command === "work_changes") return {
        accepted_source: args.sessionId === "track", source_ref: args.sessionId === "track" ? "arachne/tracks/test" : "weaver/" + args.sessionId,
        base: { state: "available", reference: args.sessionId === "worker" ? "arachne/tracks/test" : "main", oid: "abc123456789" },
        head_oid: "def123456789", totals: { files: 0, additions: 0, deletions: 0, truncated: false }, files: [], truncated: false,
      };
      if (command === "complete_files") return ["src/test.ts"];
      if (command === "read_checkout_file") return { content: "before", checkout: "/repo/" + args.sessionId };
      if (command === "review_session") return { session: { ...session(), id: "reviewer" }, warnings: [] };
      if (["save_checkout_file", "send_input", "save_token"].includes(command)) return;
      throw new Error(`Unexpected native command: ${command}`);
    },
    async listen(name, callback) { this.listeners.set(name, callback); return () => this.listeners.delete(name); },
  };
  globalThis.__inspectorLayout = bridge;
  const tree = element("root");
  bridge.tree = tree;
  const app = renderer.createApp({ render: () => h(Component, { ...props, onError: (error) => errors.push(error) }) });
  app.config.errorHandler = (error) => errors.push(error);
  t.after(() => { app.unmount(); assert.deepEqual(errors, options.expectedErrors ?? [], "component must not fail during lifecycle or actions"); });
  app.mount(tree);
  await flush();
  calls.length = 0;
  return { tree, props, bridge, calls, errors, app };
}


const track = { ...session("running"), id: "track", parent_session_id: null, parent_id: null,
  branch: { ...fixture.branch, id: "track-branch", title: "Track", branch: "weaver/track-scratch", repo_root: "/repo", goal: "", tags: [{ key: "track_topic_branch", value: "arachne/tracks/test" }] } };
const worker = { ...session("running"), id: "worker", parent_session_id: "track", parent_id: "track-branch",
  branch: { ...fixture.branch, id: "worker-branch", title: "Worker", branch: "weaver/worker", repo_root: "/repo", goal: "", tags: [] } };
const fleet = [track, worker];
const hasClass = (node, name) => node.props.class?.split(" ").includes(name);
async function openThread(t, id = "worker", sessions = fleet) {
  const mounted = await mount(t, App, {}, null, { fleet: sessions });
  click(button(mounted.tree, "Select " + id));
  await flush();
  return mounted;
}

test("review opens the inspector and expansion/restore/close preserve mounted composer, attachments and scroll", async t => {
  const { tree, calls } = await openThread(t);
  const textarea = find(tree, n => n.type === "textarea");
  await typeDraft(tree, "Follow up draft");
  const input = find(tree, n => n.type === "input" && n.props.type === "file");
  await input.props.onChange({ target: { files: [{ name: "notes.txt", size: 2, type: "text/plain", base64: "aGk=" }] } });
  await flush();
  const history = find(tree, n => hasClass(n, "thread-history"));
  const conversation = find(tree, n => hasClass(n, "conversation"));
  conversation.scrollTop = 155;
  // Starting closed also checks that a fresh inspector consumes the request.
  click(button(tree, "Close inspector")); await flush();
  click(button(tree, "Review diff")); await flush();
  assert.equal(button(tree, "Review").props["aria-selected"], true);
  assert.equal(find(tree, n => hasClass(n, "change-review")).parent.parent.props.id, "topic-inspector");
  assert.equal(calls.filter(c => c.command === "work_changes").at(-1).args.sessionId, "worker");
  click(button(tree, "Expand inspector")); await flush();
  assert.equal(button(tree, "Restore inspector").props["aria-expanded"], true);
  assert.equal(history.style.display, "none");
  assert.equal(find(tree, n => n.type === "textarea"), textarea);
  assert.equal(textarea.value, "Follow up draft");
  assert.ok(button(tree, "Remove notes.txt"));
  conversation.scrollTop = 0; // Hidden-layout geometry must not lose the reader position.
  click(button(tree, "Restore inspector")); await flush();
  assert.notEqual(history.style.display, "none");
  assert.equal(conversation.scrollTop, 155);
  click(button(tree, "Expand inspector")); await flush();
  click(button(tree, "Close inspector")); await flush();
  assert.notEqual(history.style.display, "none");
  assert.equal(find(tree, n => n.type === "textarea"), textarea);
  assert.equal(textarea.value, "Follow up draft");
  assert.ok(button(tree, "Remove notes.txt"));
  click(button(tree, "Review diff")); await flush();
  click(button(tree, "Expand inspector")); await flush();
  click(button(tree, "Send message")); await flush();
  assert.deepEqual(calls.find(c => c.command === "send_input").args, {
    baseUrl: localUrl, id: "worker", text: "Follow up draft", topicId: "track", resourceIds: [], attachments: [{ name: "notes.txt", contentBase64: "aGk=" }],
  });
});

test("Changes menu and Landing review route to the selected worker without remounting it", async t => {
  const { tree, calls } = await openThread(t);
  const textarea = find(tree, n => n.type === "textarea");
  await typeDraft(tree, "Keep me");
  click(button(tree, "More code actions and checkout details")); await flush();
  const changes = find(tree, n => n.type === "button" && textOf(n).startsWith("1 files"));
  click(changes); await flush();
  assert.equal(button(tree, "Review").props["aria-selected"], true);
  click(button(tree, "Landing")); await flush();
  const candidate = find(tree, n => hasClass(n, "inspector-candidate"));
  click(button(candidate, "Review")); await flush();
  assert.equal(button(tree, "Review").props["aria-selected"], true);
  assert.equal(find(tree, n => n.type === "textarea"), textarea);
  assert.equal(textarea.value, "Keep me");
  assert.equal(calls.filter(c => c.command === "open_session").length, 1);
});

test("expanded selected-session changes update review identity and composer send target", async t => {
  const { tree, calls } = await openThread(t, "track");
  click(button(tree, "Review diff")); await flush();
  assert.ok(textOf(tree).includes("Source arachne/tracks/test"));
  assert.ok(textOf(tree).includes("Compared with main"));
  click(button(tree, "Expand inspector")); await flush();
  await typeDraft(tree, "Track draft");
  const oldComposer = find(tree, n => n.type === "textarea");
  click(button(tree, "Select worker")); await flush();
  assert.ok(hasClass(tree.children[0], "inspector-expanded"));
  assert.notEqual(find(tree, n => n.type === "textarea"), oldComposer);
  assert.equal(find(tree, n => n.type === "textarea").value, "");
  assert.ok(textOf(tree).includes("Source weaver/worker"));
  assert.ok(textOf(tree).includes("Compared with arachne/tracks/test"));
  assert.equal(calls.filter(c => c.command === "work_changes").at(-1).args.sessionId, "worker");
  await typeDraft(tree, "Worker follow up");
  click(button(tree, "Send message")); await flush();
  assert.equal(calls.find(c => c.command === "send_input").args.id, "worker");
});

test("standalone review opens inspector as a worker checkout, retaining edit/save refresh", async t => {
  const standalone = { ...worker, id: "standalone", parent_id: null, parent_session_id: null, branch: { ...worker.branch, tags: [{ key: "topic", value: "false" }] } };
  const { tree, calls } = await openThread(t, "standalone", [standalone]);
  assert.equal(button(tree, "Close inspector"), null);
  click(button(tree, "Review diff")); await flush();
  assert.equal(button(tree, "Review").props["aria-selected"], true);
  assert.ok(button(tree, "Prepare landing"));
  const picker = find(tree, n => n.type === "input" && n.props["aria-label"] === "Find a checkout file");
  picker.props["onUpdate:modelValue"]("src/test.ts"); await flush();
  click(button(tree, "Edit file")); await flush();
  // The editor has its own textarea; keep the composer independently mounted.
  const editorText = find(tree, n => n.type === "textarea" && n.value === "before");
  assert.ok(editorText);
  editorText.value = "after";
  for (const handler of editorText.listeners.input) handler({ target: editorText });
  await flush();
  click(button(tree, "Save")); await flush();
  assert.equal(calls.find(c => c.command === "save_checkout_file").args.sessionId, "standalone");
  assert.ok(calls.filter(c => c.command === "work_changes").length >= 2);
  assert.ok(calls.filter(c => c.command === "work_summary").length >= 2);
});


test("delayed fleet snapshots cannot redirect the selected worker review to the coordinator", async t => {
  const { tree, bridge, calls } = await openThread(t);
  bridge.listeners.get("loom://fleet")({ payload: { server_url: "http://127.0.0.1:7878", sessions: [track], layout: emptyLayout } });
  await flush();
  click(button(tree, "Review diff")); await flush();
  assert.equal(calls.filter(c => c.command === "work_changes").at(-1).args.sessionId, "worker");
  assert.ok(textOf(tree).includes("Source weaver/worker"));
  assert.ok(button(tree, "Land"));
  assert.equal(button(tree, "Publish"), null);
});


for (const entry of ["inspector", "shortcut"]) test(`expanded delegation from ${entry} restores visible, focused controls without losing the composer`, async t => {
  const { tree, calls } = await openThread(t);
  await typeDraft(tree, "Keep composer draft");
  const composer = find(tree, n => n.type === "textarea");
  click(button(tree, "Expand inspector")); await flush();
  assert.equal(find(tree, n => hasClass(n, "thread-history")).style.display, "none");
  if (entry === "inspector") click(button(tree, "+ New thread"));
  else globalThis.window.listeners.get("keydown")({ key: "d", ctrlKey: true, shiftKey: true, preventDefault() {} });
  await flush();
  assert.notEqual(find(tree, n => hasClass(n, "thread-history")).style.display, "none");
  assert.equal(button(tree, "Restore inspector"), null);
  const delegate = find(tree, n => hasClass(n, "delegate-box"));
  assert.ok(delegate);
  assert.equal(find(delegate, n => n.type === "input").focused, true);
  assert.ok(button(delegate, "Start worker"));
  assert.equal(find(tree, n => n.type === "textarea"), composer);
  assert.equal(composer.value, "Keep composer draft");
  assert.equal(calls.filter(c => c.command === "open_session").length, 1);
});

async function openBeforeFleet(t, selected, sessions = [track, selected]) {
  // Navigation/deep links can know the session ID before any fleet summary.
  const mounted = await mount(t, App, {}, null, { fleet: [], sessions });
  click(button(mounted.tree, "Select " + selected.id)); await flush();
  assert.ok(find(mounted.tree, n => n.type === "textarea"));
  assert.equal(button(mounted.tree, "Publish"), null, "unknown ancestry cannot imply a coordinator");
  click(button(mounted.tree, "Review diff")); await flush();
  assert.equal(button(mounted.tree, "Review").props["aria-selected"], true);
  assert.equal(mounted.calls.filter(c => c.command === "work_changes").at(-1).args.sessionId, selected.id);
  assert.equal(find(mounted.tree, n => hasClass(n, "canonical-policy")), null, "unknown ancestry cannot imply a canonical Track scope");
  return mounted;
}

const standalone = { ...worker, id: "standalone", parent_id: null, parent_session_id: null,
  branch: { ...worker.branch, tags: [{ key: "topic", value: "false" }] } };

for (const selected of [standalone, track, worker]) test(`pre-fleet ${selected.id} review follows server comparison, then retains arriving ancestry across gaps`, async t => {
  const sessions = selected === standalone ? [standalone] : fleet;
  const { tree, bridge, calls } = await openBeforeFleet(t, selected, sessions);
  assert.ok(button(tree, selected === track ? "Prepare publishing" : "Prepare landing"));
  assert.ok(textOf(tree).includes(selected === track ? "Source arachne/tracks/test" : "Source weaver/" + selected.id));
  assert.ok(textOf(tree).includes(selected === worker ? "Compared with arachne/tracks/test" : "Compared with main"));
  click(button(tree, "Launch reviewer")); await flush();
  const request = find(tree, n => n.type === "textarea" && n.props.id?.startsWith("review-request-"));
  assert.ok(request.value.includes("No same-repository Track is resolved"));
  click(button(tree, "Cancel")); await flush();
  await typeDraft(tree, "Draft before fleet");
  const composer = find(tree, n => n.type === "textarea");
  const snapshot = sessions => bridge.listeners.get("loom://fleet")({ payload: { server_url: "http://127.0.0.1:7878", sessions, layout: emptyLayout } });
  snapshot(sessions); await flush();
  // The newly arriving summary was not present when selectSession ran.
  snapshot(selected === worker ? [track] : []); await flush();
  assert.equal(find(tree, n => n.type === "textarea"), composer);
  assert.equal(composer.value, "Draft before fleet");
  assert.equal(button(tree, "Review").props["aria-selected"], true);
  assert.equal(calls.filter(c => c.command === "work_changes").at(-1).args.sessionId, selected.id);
  click(button(tree, "Launch reviewer")); await flush();
  const resolvedRequest = find(tree, n => n.type === "textarea" && n.props.id?.startsWith("review-request-"));
  assert.ok(resolvedRequest.value.includes(selected === standalone ? "No same-repository Track is resolved"
    : selected === track ? "Canonical accepted source: arachne/tracks/test" : "Compare against canonical accepted branch: arachne/tracks/test"));
  click(button(tree, "Cancel")); await flush();
  if (selected === worker) {
    // If the ancestor disappears too, keep source identity and defer target
    // resolution rather than presenting this worker as a Track coordinator.
    snapshot([]); await flush();
    assert.equal(calls.filter(c => c.command === "work_changes").at(-1).args.sessionId, "worker");
    assert.equal(find(tree, n => hasClass(n, "canonical-policy")), null);
  }
  if (selected === worker) { assert.ok(button(tree, "Land")); assert.equal(button(tree, "Publish"), null); }
  else if (selected === track) assert.ok(button(tree, "Publish"));
  else { assert.equal(button(tree, "Publish"), null); assert.equal(find(tree, n => hasClass(n, "canonical-policy")), null); }
});

test("unknown pre-fleet ancestry stays unclassified until the server comparison resolves", async t => {
  const { tree, bridge } = await openBeforeFleet(t, worker, fleet);
  click(button(tree, "Close inspector")); await flush();
  let resolve;
  bridge.workChanges = () => new Promise(done => { resolve = done; });
  click(button(tree, "Review diff")); await flush();
  assert.ok(button(tree, "Prepare changes").props.disabled);
  assert.ok(button(tree, "Launch reviewer").props.disabled);
  assert.equal(button(tree, "Edit file"), null);
  resolve({ accepted_source: false, source_ref: "weaver/worker", base: { state: "available", reference: "arachne/tracks/test", oid: "abc123456789" },
    head_oid: "def123456789", totals: { files: 0, additions: 0, deletions: 0, truncated: false }, files: [], truncated: false });
  await flush();
  assert.ok(button(tree, "Prepare landing"));
});

const emitFleet = (bridge, sessions) => bridge.listeners.get("loom://fleet")({ payload: {
  server_url: "http://127.0.0.1:7878", sessions, layout: emptyLayout,
} });
const reviewRequestText = tree => find(tree, n => n.type === "textarea" && n.props.id?.startsWith("review-request-"));

test("latest Track metadata survives a selected-row gap and reviewer/preparation briefs match backend scope", async t => {
  const { tree, bridge, calls } = await openThread(t, "track");
  const composer = find(tree, n => n.type === "textarea");
  await typeDraft(tree, "Canonical follow-up");
  click(button(tree, "Review")); await flush();
  click(button(tree, "Expand inspector")); await flush();
  click(button(tree, "Launch reviewer")); await flush();
  const oldRequest = reviewRequestText(tree);
  assert.ok(oldRequest.value.includes("Canonical accepted source: arachne/tracks/test"));
  const updated = { ...track, branch: { ...track.branch, title: "Updated Track", branch: "weaver/latest-scratch",
    tags: [{ key: "track_topic_branch", value: "arachne/tracks/updated" }] } };
  bridge.workChanges = () => ({ accepted_source: true, source_ref: "arachne/tracks/updated",
    base: { state: "available", reference: "main", oid: "abc123456789" }, head_oid: "def123456789",
    totals: { files: 0, additions: 0, deletions: 0, truncated: false }, files: [], truncated: false });
  emitFleet(bridge, [updated, worker]); await flush();
  emitFleet(bridge, [worker]); await flush();
  const policy = find(tree, n => hasClass(n, "canonical-policy"));
  assert.ok(textOf(policy).includes("arachne/tracks/updated"));
  assert.ok(textOf(tree).includes("Source arachne/tracks/updated"));
  assert.equal(reviewRequestText(tree), oldRequest, "retain the old editable draft until explicitly replaced");
  assert.ok(button(oldRequest.parent, "Launch reviewer").props.disabled, "an old canonical brief cannot be sent");
  click(button(tree, "Replace with current comparison")); await flush();
  const request = reviewRequestText(tree).value;
  assert.ok(request.includes("preview showed arachne/tracks/updated"));
  assert.ok(request.includes("Canonical accepted source: arachne/tracks/updated"));
  assert.ok(!request.includes("arachne/tracks/test"));
  click(button(reviewRequestText(tree).parent, "Launch reviewer")); await flush();
  assert.deepEqual(calls.find(c => c.command === "review_session").args.brief, request);
  assert.equal(calls.find(c => c.command === "review_session").args.sourceId, "track");
  click(button(tree, "Prepare publishing")); await flush();
  assert.ok(reviewRequestText(tree).value.includes("Canonical publish source: arachne/tracks/updated"));
  assert.equal(find(tree, n => n.type === "textarea"), composer);
  assert.equal(composer.value, "Canonical follow-up");
  assert.equal(calls.filter(c => c.command === "open_session").length, 1);
});

for (const entry of ["direct tab", "Landing candidate", "Review diff"]) test(`${entry} review preserves expanded inspector/editor/request/composer through ancestor gaps and rearrival`, async t => {
  const { tree, bridge, calls } = await openThread(t);
  await typeDraft(tree, "Composer survives ancestor gaps");
  const composer = find(tree, n => n.type === "textarea");
  const input = find(tree, n => n.type === "input" && n.props.type === "file");
  await input.props.onChange({ target: { files: [{ name: "gap.txt", size: 2, type: "text/plain", base64: "aGk=" }] } });
  if (entry === "Landing candidate") {
    click(button(tree, "Landing")); await flush();
    click(button(find(tree, n => hasClass(n, "inspector-candidate")), "Review"));
  } else click(button(tree, entry === "direct tab" ? "Review" : "Review diff"));
  await flush();
  click(button(tree, "Expand inspector")); await flush();
  const inspector = find(tree, n => n.props.id === "topic-inspector");
  const review = find(tree, n => hasClass(n, "change-review"));
  click(button(tree, "Prepare landing")); await flush();
  const request = reviewRequestText(tree);
  request.props["onUpdate:modelValue"](request.value + " User-specific instructions to retain."); await flush();
  const draft = request.value;
  const picker = find(tree, n => n.props["aria-label"] === "Find a checkout file");
  picker.props["onUpdate:modelValue"]("src/test.ts"); await flush();
  click(button(tree, "Edit file")); await flush();
  const editor = find(tree, n => n.type === "textarea" && n.value === "before");
  editor.value = "Unsaved editor content";
  for (const handler of editor.listeners.input) handler({ target: editor });
  await flush();
  const editorReads = calls.filter(c => c.command === "read_checkout_file").length;
  for (const sessions of [[worker], [], fleet]) {
    emitFleet(bridge, sessions); await flush();
    assert.equal(find(tree, n => n.props.id === "topic-inspector"), inspector);
    assert.equal(find(tree, n => hasClass(n, "change-review")), review);
    assert.equal(button(tree, "Review").props["aria-selected"], true);
    assert.ok(hasClass(tree.children[0], "inspector-expanded"));
    assert.equal(find(tree, n => hasClass(n, "thread-history")).style.display, "none");
    assert.equal(reviewRequestText(tree), request);
    assert.equal(request.value, draft);
    assert.equal(find(tree, n => n.type === "textarea" && n.value === "Unsaved editor content"), editor);
    assert.equal(find(tree, n => n.type === "textarea"), composer);
    assert.equal(composer.value, "Composer survives ancestor gaps");
    assert.ok(button(tree, "Remove gap.txt"));
    assert.equal(calls.filter(c => c.command === "work_changes").at(-1).args.sessionId, "worker");
    if (!sessions.some(s => s.id === "track")) {
      assert.equal(find(tree, n => hasClass(n, "canonical-policy")), null);
      assert.ok(button(request.parent, "Send to agent").props.disabled, "brief's missing canonical target is unverified");
      click(button(request.parent, "Send to agent")); await flush();
      assert.equal(calls.filter(c => c.command === "send_input").length, 0);
    }
  }
  assert.equal(calls.filter(c => c.command === "read_checkout_file").length, editorReads);
  assert.equal(button(request.parent, "Send to agent").props.disabled, false, "same verified scope restored");
  click(button(tree, "Save")); await flush();
  assert.equal(calls.find(c => c.command === "save_checkout_file").args.sessionId, "worker");
  assert.equal(calls.find(c => c.command === "save_checkout_file").args.content, "Unsaved editor content");
  click(button(tree, "Restore inspector")); await flush();
  assert.notEqual(find(tree, n => hasClass(n, "thread-history")).style.display, "none");
  click(button(tree, "Close inspector")); await flush();
  assert.equal(find(tree, n => n.type === "textarea"), composer);
  assert.equal(composer.value, "Composer survives ancestor gaps");
});

test("navigation away during an ancestor gap clears retained inspector scope and routes the new session independently", async t => {
  const { tree, bridge, calls } = await openThread(t, "worker", [track, worker, standalone]);
  click(button(tree, "Review")); await flush();
  click(button(tree, "Expand inspector")); await flush();
  const oldInspector = find(tree, n => n.props.id === "topic-inspector");
  const oldComposer = find(tree, n => n.type === "textarea");
  emitFleet(bridge, [worker]); await flush();
  click(button(tree, "Select standalone")); await flush();
  assert.equal(find(tree, n => n.props.id === "topic-inspector"), null);
  assert.equal(hasClass(tree.children[0], "inspector-expanded"), false);
  assert.ok(find(tree, n => n.type === "textarea") !== oldComposer);
  click(button(tree, "Review diff")); await flush();
  assert.ok(find(tree, n => n.props.id === "topic-inspector") !== oldInspector);
  assert.equal(find(tree, n => hasClass(n, "canonical-policy")), null);
  assert.equal(calls.filter(c => c.command === "work_changes").at(-1).args.sessionId, "standalone");
  click(button(tree, "Launch reviewer")); await flush();
  assert.ok(reviewRequestText(tree).value.includes("No same-repository Track is resolved"));
});

for (const arrival of ["together", "selected then ancestor"]) test(`pre-fleet worker inspector retains live state when first ancestry arrives ${arrival}`, async t => {
  const { tree, bridge, calls } = await openBeforeFleet(t, worker, fleet);
  await typeDraft(tree, "Composer during ancestry discovery");
  const composer = find(tree, n => n.type === "textarea");
  click(button(tree, "Expand inspector")); await flush();
  click(button(tree, "Prepare landing")); await flush();
  const request = reviewRequestText(tree);
  request.props["onUpdate:modelValue"](request.value + " Customized request to preserve."); await flush();
  const requestDraft = request.value;
  const picker = find(tree, n => n.props["aria-label"] === "Find a checkout file");
  picker.props["onUpdate:modelValue"]("src/enrichment.ts"); await flush();
  click(button(tree, "Edit file")); await flush();
  const editor = find(tree, n => n.type === "textarea" && n.props["aria-label"] === "Contents of src/enrichment.ts");
  editor.value = "Unsaved enrichment edit";
  for (const handler of editor.listeners.input) handler({ target: editor });
  await flush();
  const inspector = find(tree, n => n.props.id === "topic-inspector");
  const review = find(tree, n => hasClass(n, "change-review"));
  const reads = calls.filter(c => c.command === "read_checkout_file").length;
  for (const sessions of arrival === "together" ? [fleet] : [[worker], fleet]) {
    emitFleet(bridge, sessions); await flush();
    assert.ok(find(tree, n => n.props.id === "topic-inspector") === inspector, "same inspector during enrichment");
    assert.ok(find(tree, n => hasClass(n, "change-review")) === review, "same review during enrichment");
    assert.ok(reviewRequestText(tree) === request, "same customized request textarea");
    assert.equal(request.value, requestDraft);
    assert.ok(find(tree, n => n.props["aria-label"] === "Contents of src/enrichment.ts") === editor);
    assert.equal(editor.value, "Unsaved enrichment edit");
    assert.ok(find(tree, n => n.props["aria-label"] === "Find a checkout file") === picker);
    assert.equal(picker.value, "src/enrichment.ts");
    assert.ok(find(tree, n => n.type === "textarea") === composer);
    assert.equal(composer.value, "Composer during ancestry discovery");
    assert.ok(hasClass(tree.children[0], "inspector-expanded"));
    assert.equal(find(tree, n => hasClass(n, "thread-history")).style.display, "none");
    assert.equal(button(tree, "Review").props["aria-selected"], true);
    assert.equal(calls.filter(c => c.command === "work_changes").at(-1).args.sessionId, "worker");
    const resolved = sessions.some(s => s.id === "track");
    assert.equal(!!find(tree, n => hasClass(n, "canonical-policy")), resolved);
    assert.equal(button(request.parent, "Send to agent").props.disabled, resolved,
      "a brief prepared without a Track becomes stale only when the target is resolved");
  }
  assert.equal(calls.filter(c => c.command === "read_checkout_file").length, reads);
  click(button(request.parent, "Send to agent")); await flush();
  assert.equal(calls.filter(c => c.command === "send_input").length, 0);
  click(button(tree, "Replace with current comparison")); await flush();
  assert.ok(request.value.includes("branch arachne/tracks/test"));
  assert.equal(button(request.parent, "Send to agent").props.disabled, false);
  click(button(tree, "Save")); await flush();
  assert.equal(calls.find(c => c.command === "save_checkout_file").args.content, "Unsaved enrichment edit");
  assert.equal(calls.find(c => c.command === "save_checkout_file").args.sessionId, "worker");
});

const otherTrack = { ...track, id: "other-track", branch: { ...track.branch, id: "other-track-branch", repo_root: "/other",
  tags: [{ key: "track_topic_branch", value: "arachne/tracks/other" }] } };
const sibling = { ...worker, id: "sibling", branch: { ...worker.branch, id: "sibling-branch", branch: "weaver/sibling" } };
const otherWorker = { ...worker, id: "other-worker", parent_session_id: otherTrack.id, parent_id: otherTrack.branch.id,
  branch: { ...worker.branch, id: "other-worker-branch", repo_root: "/other", branch: "weaver/other-worker" } };

test("source navigation retains the tab within a known Track and resets the inspector for a new Track", async t => {
  const { tree, calls } = await openThread(t, "worker", [...fleet, sibling, otherTrack, otherWorker]);
  click(button(tree, "Review")); await flush();
  click(button(tree, "Expand inspector")); await flush();
  click(button(tree, "Prepare landing")); await flush();
  reviewRequestText(tree).props["onUpdate:modelValue"]("Old source request"); await flush();
  const picker = find(tree, n => n.props["aria-label"] === "Find a checkout file");
  picker.props["onUpdate:modelValue"]("src/navigation.ts"); await flush();
  click(button(tree, "Edit file")); await flush();
  const inspector = find(tree, n => n.props.id === "topic-inspector");
  const oldComposer = find(tree, n => n.type === "textarea");
  click(button(tree, "Select sibling")); await flush();
  assert.ok(find(tree, n => n.props.id === "topic-inspector") === inspector);
  assert.equal(button(tree, "Review").props["aria-selected"], true);
  assert.equal(reviewRequestText(tree), null, "new source resets the previous source's request");
  assert.equal(find(tree, n => n.props["aria-label"] === "Contents of src/navigation.ts"), null);
  assert.equal(picker.value, "", "new source resets its diff filter");
  assert.ok(find(tree, n => n.type === "textarea") !== oldComposer);
  assert.equal(calls.filter(c => c.command === "work_changes").at(-1).args.sessionId, "sibling");
  click(button(tree, "Prepare landing")); await flush();
  assert.ok(reviewRequestText(tree).value.includes("branch arachne/tracks/test"));
  click(button(tree, "Select other-worker")); await flush();
  assert.ok(find(tree, n => n.props.id === "topic-inspector") !== inspector);
  assert.equal(button(tree, "Threads").props["aria-selected"], true);
  assert.equal(reviewRequestText(tree), null);
  assert.ok(textOf(find(tree, n => hasClass(n, "canonical-policy"))).includes("arachne/tracks/other"));
  click(button(tree, "Review")); await flush();
  assert.equal(calls.filter(c => c.command === "work_changes").at(-1).args.sessionId, "other-worker");
  click(button(tree, "Prepare landing")); await flush();
  assert.ok(reviewRequestText(tree).value.includes("branch arachne/tracks/other"));
});

test("switching selected backends clears cached ancestry and inspector identity even when the source ID is reused", async t => {
  const { tree, bridge } = await openThread(t);
  click(button(tree, "Review")); await flush();
  click(button(tree, "Expand inspector")); await flush();
  click(button(tree, "Prepare landing")); await flush();
  const inspector = find(tree, n => n.props.id === "topic-inspector");
  // Retain a source snapshot just before switching to a server with no summary.
  emitFleet(bridge, []); await flush();
  bridge.fleetByUrl.set("http://127.0.0.1:8787", []);
  click(find(tree, n => n.type === "button" && n.props.title === "Connection settings")); await flush();
  const url = find(tree, n => n.type === "input" && n.props.placeholder === "http://127.0.0.1:7878");
  url.props["onUpdate:modelValue"]("http://127.0.0.1:8787"); await flush();
  await new Promise(resolve => setTimeout(resolve, 250)); await flush();
  const form = find(tree, n => n.type === "form" && hasClass(n, "server-settings"));
  form.props.onSubmit({ preventDefault() {} }); await flush();
  assert.equal(find(tree, n => n.props.id === "topic-inspector"), inspector,
    "changing the default leaves the existing selection on its own backend");
  bridge.selectionUrl = "http://127.0.0.1:8787";
  click(button(tree, "Select worker")); await flush();
  assert.equal(button(tree, "Close inspector"), null, "no previous-server ancestry can resolve a Track");
  click(button(tree, "Review diff")); await flush();
  assert.ok(find(tree, n => n.props.id === "topic-inspector") !== inspector);
  assert.equal(reviewRequestText(tree), null);
  assert.equal(find(tree, n => hasClass(n, "canonical-policy")), null);
  click(button(tree, "Prepare landing")); await flush();
  assert.ok(reviewRequestText(tree).value.includes("No landing target is resolved"));
  emitFleet(bridge, fleet); await flush(); // The old backend remains live but cannot enrich this selection.
  assert.equal(find(tree, n => hasClass(n, "canonical-policy")), null);
  const currentInspector = find(tree, n => n.props.id === "topic-inspector");
  bridge.listeners.get("loom://fleet")({ payload: { server_url: "http://127.0.0.1:8787", sessions: fleet, layout: emptyLayout } });
  await flush();
  assert.ok(find(tree, n => n.props.id === "topic-inspector") === currentInspector);
  assert.ok(textOf(find(tree, n => hasClass(n, "canonical-policy"))).includes("arachne/tracks/test"));
  assert.ok(button(reviewRequestText(tree).parent, "Send to agent").props.disabled);
});

async function registerRemote(tree, bridge, url, sessions) {
  bridge.fleetByUrl.set(url, sessions);
  click(find(tree, n => n.type === "button" && n.props.title === "Connection settings")); await flush();
  const input = find(tree, n => n.type === "input" && n.props.placeholder === localUrl);
  input.props["onUpdate:modelValue"](url); await flush();
  await new Promise(resolve => setTimeout(resolve, 250)); await flush();
  find(tree, n => n.type === "form" && hasClass(n, "server-settings")).props.onSubmit({ preventDefault() {} });
  await flush();
}

test("remote inspector comparisons, reviewer requests and expanded composer keep the selected backend owner", async t => {
  const remote = "https://remote.example";
  const { tree, bridge, calls } = await openThread(t);
  await typeDraft(tree, "Local draft");
  click(button(tree, "Review")); await flush();
  click(button(tree, "Expand inspector")); await flush();
  click(button(tree, "Prepare landing")); await flush();
  const localComposer = find(tree, n => n.type === "textarea");
  const localInspector = find(tree, n => n.props.id === "topic-inspector");
  const localRequest = reviewRequestText(tree);
  await registerRemote(tree, bridge, remote, fleet);
  assert.equal(find(tree, n => n.type === "textarea"), localComposer);
  assert.equal(localComposer.value, "Local draft");
  assert.equal(find(tree, n => n.props.id === "topic-inspector"), localInspector);
  assert.equal(reviewRequestText(tree), localRequest);
  assert.ok(hasClass(tree.children[0], "inspector-expanded"), "default change keeps the existing expanded layout");
  bridge.selectionUrl = remote;
  click(button(tree, "Select worker")); await flush();
  assert.notEqual(find(tree, n => n.props.id === "topic-inspector"), localInspector);
  assert.equal(reviewRequestText(tree), null);
  click(button(tree, "Review diff")); await flush();
  assert.equal(button(tree, "Edit file"), null, "remote checkout cannot inherit loopback edit permission");
  assert.equal(calls.filter(c => c.command === "work_changes").at(-1).args.baseUrl, remote);
  click(button(tree, "Launch reviewer")); await flush();
  click(button(reviewRequestText(tree).parent, "Launch reviewer")); await flush();
  assert.equal(calls.filter(c => c.command === "review_session").at(-1).args.baseUrl, remote);
  assert.equal(calls.filter(c => c.command === "review_session").at(-1).args.sourceId, "worker");
  assert.ok(button(tree, "Restore inspector"), "the new selected composer uses the expanded inspector layout");
  await typeDraft(tree, "Remote follow up");
  click(button(tree, "Send message")); await flush();
  const sent = calls.filter(c => c.command === "send_input").at(-1).args;
  assert.equal(sent.baseUrl, remote);
  assert.equal(sent.id, "worker");
  assert.equal(sent.topicId, "track");
  assert.equal(sent.text, "Remote follow up");
});

test("an unavailable selected backend hides inspector actions while another backend remains available", async t => {
  const remote = "https://healthy.example";
  const { tree, bridge, calls } = await openThread(t);
  await registerRemote(tree, bridge, remote, fleet);
  click(button(tree, "Review")); await flush();
  click(button(tree, "Expand inspector")); await flush();
  bridge.listeners.get("loom://error")({ payload: { server_url: localUrl, message: "Offline", unreachable: true } });
  await flush();
  assert.equal(find(tree, n => n.props.id === "topic-inspector"), null);
  assert.equal(button(tree, "Send message"), null);
  assert.ok(textOf(tree).includes("Backend unavailable"));
  assert.equal(hasClass(tree.children[0], "inspector-expanded"), false);
  click(button(tree, "Reconnect")); await flush();
  assert.equal(calls.filter(c => c.command === "refresh_fleet").at(-1).args.baseUrl, localUrl);
  assert.equal(calls.filter(c => c.command === "open_session").at(-1).args.baseUrl, localUrl);
  assert.ok(find(tree, n => n.type === "textarea"), "selected backend reconnect restores its composer");
  assert.ok(button(tree, "Expand inspector"));
});
