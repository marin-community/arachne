import assert from 'node:assert/strict';
import test from 'node:test';
import { scopedId } from '../src/backendIdentity.ts';
const SERVER = 'http://127.0.0.1:7878';
import { readFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { build } from 'esbuild';
import { parse, compileScript } from '@vue/compiler-sfc';

// Exercise the real HomeView template and lifecycle with the same in-memory
// Vue renderer approach as trackLaunch.test.mjs; no desktop bridge is needed.
const directory = await mkdtemp(join(tmpdir(), 'arachne-home-todos-'));
const output = join(directory, 'home.mjs');
await build({
  stdin: {
    contents: `export { default as HomeView } from './src/components/HomeView.vue'; export { createRenderer, h, reactive, nextTick } from 'vue';`,
    resolveDir: resolve('.'),
  },
  bundle: true, format: 'esm', platform: 'node', outfile: output,
  plugins: [{
    name: 'home-component-test',
    setup(builder) {
      builder.onResolve({ filter: /^@tauri-apps\// }, args => ({ path: args.path, namespace: 'native' }));
      builder.onLoad({ filter: /.*/, namespace: 'native' }, () => ({ contents: `
        export const invoke = (...args) => globalThis.homeTodosInvoke(...args);
        export const listen = async (event, callback) => {
          globalThis.homeTodosListeners.set(event, callback);
          return () => globalThis.homeTodosListeners.delete(event);
        };
      ` }));
      builder.onLoad({ filter: /\.vue$/ }, async ({ path }) => {
        const { descriptor } = parse(await readFile(path, 'utf8'), { filename: path });
        return { contents: compileScript(descriptor, { id: 'home-todos', inlineTemplate: true }).content, loader: 'ts' };
      });
    },
  }],
});
const { HomeView, createRenderer, h, reactive, nextTick } = await import(pathToFileURL(output));
await rm(directory, { recursive: true, force: true });

const renderer = createRenderer({
  createElement: tag => ({ tag, props: {}, children: [] }),
  createText: text => ({ text }), createComment: text => ({ text }),
  setText: (node, text) => { node.text = text; },
  setElementText: (node, text) => { node.text = text; node.children = []; },
  patchProp: (node, key, old, value) => { node.props[key] = value; },
  parentNode: node => node.parent,
  nextSibling: node => node.parent?.children[node.parent.children.indexOf(node) + 1],
  insert(node, parent, anchor) {
    if (node.parent) node.parent.children.splice(node.parent.children.indexOf(node), 1);
    node.parent = parent;
    const index = anchor ? parent.children.indexOf(anchor) : -1;
    parent.children.splice(index < 0 ? parent.children.length : index, 0, node);
  },
  remove(node) {
    node.parent.children.splice(node.parent.children.indexOf(node), 1);
    node.parent = null;
  },
});

function find(node, predicate) {
  if (predicate(node)) return node;
  for (const child of node.children ?? []) {
    const found = find(child, predicate);
    if (found) return found;
  }
}
const textNode = (root, text) => find(root, node => node.tag && node.text?.trim() === text);
const flush = async () => { await new Promise(resolve => setImmediate(resolve)); await nextTick(); };
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const session = (id, project = 'alpha') => ({
  id: scopedId(SERVER, id), status: 'idle', parent_id: null, parent_session_id: null,
  last_activity_at: '2026-10-10T00:00:00Z', placement: { group_id: scopedId(SERVER, project) },
  branch: { id: scopedId(SERVER, `branch-${id}`), title: `Track ${id}`, name: id, tags: [] },
});
const todo = (id, topic_id = 'a') => ({ id, topic_id, text: `Todo ${id}`, revision: 1 });
const empty = () => ({ todos: [], warnings: [] });
const layout = { spaces: [{ rank: 0, groups: [
  { id: scopedId(SERVER, 'alpha'), name: 'Alpha', rank: 0 },
  { id: scopedId(SERVER, 'beta'), name: 'Beta', rank: 1 },
] }] };

async function mount(t, overrides = {}) {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const requests = [];
  globalThis.homeTodosListeners = new Map();
  globalThis.homeTodosInvoke = (command, args) => {
    assert.equal(command, 'user_todos');
    const request = deferred();
    requests.push({ ...request, args });
    return request.promise;
  };
  const props = reactive({ fleet: [session('a')], layout, project: { id: scopedId(SERVER, 'alpha'), name: 'Alpha' }, ...overrides });
  const root = { children: [] };
  const app = renderer.createApp({ render: () => h(HomeView, props) });
  app.provide("backendUrl", SERVER);
  app.mount(root);
  t.after(() => app.unmount());
  const fleetEvent = () => globalThis.homeTodosListeners.get('loom://fleet')({ payload: {} });
  const tick = async () => { t.mock.timers.tick(600); await flush(); };
  await flush();
  return { root, app, props, requests, fleetEvent, tick };
}

function assertEmpty(root) {
  assert.ok(textNode(root, 'Nothing needs you.'));
  assert.ok(textNode(root, 'No open user Todos.'));
}
function assertUnsettled(root) {
  assert.equal(textNode(root, 'Nothing needs you.'), undefined);
  assert.equal(textNode(root, 'No open user Todos.'), undefined);
  assert.equal(textNode(root, 'Loading…')?.props.disabled, true);
}

test('settled empty inbox nodes survive repeated fleet refreshes and queued pending requests', async t => {
  const { root, props, requests, fleetEvent, tick } = await mount(t);
  await flush();
  assert.equal(requests.length, 1);
  assertUnsettled(root);
  requests[0].resolve(empty());
  await flush();
  assertEmpty(root);
  const calm = textNode(root, 'Nothing needs you.');
  const noTodos = textNode(root, 'No open user Todos.');
  fleetEvent();
  await tick();
  assert.equal(requests.length, 2);
  for (let i = 0; i < 3; i++) {
    // Fresh fleet objects with the same topic membership must stay settled.
    props.fleet = [session('a')];
    fleetEvent();
    await tick();
    assert.equal(requests.length, 2, 'refresh requests are serialized');
    assert.equal(textNode(root, 'Nothing needs you.'), calm);
    assert.equal(textNode(root, 'No open user Todos.'), noTodos);
    assert.equal(textNode(root, 'Loading…').props.disabled, true);
  }
  requests[1].resolve(empty());
  await flush();
  await tick();
  assert.equal(requests.length, 3, 'events during a request queue a follow-up');
  assert.equal(textNode(root, 'Nothing needs you.'), calm);
  assert.equal(textNode(root, 'No open user Todos.'), noTodos);
  requests[2].resolve(empty());
  await flush();
  assert.equal(textNode(root, 'Nothing needs you.'), calm);
  assert.equal(textNode(root, 'No open user Todos.'), noTodos);
  assert.equal(textNode(root, 'Refresh').props.disabled, false);
});

test('background results show added and removed Todos, retain rows while pending, and surface failures', async t => {
  const { root, props, requests, fleetEvent, tick } = await mount(t);
  requests[0].resolve(empty());
  await flush();
  fleetEvent();
  await tick();
  assertEmpty(root);
  requests[1].resolve({ todos: [todo('review')], warnings: ["Partial Todo results"] });
  await flush();
  assert.ok(textNode(root, 'Todo review'));
  assert.equal(textNode(root, `${SERVER}: Partial Todo results`).props.role, 'alert');
  assert.equal(textNode(root, 'Nothing needs you.'), undefined);
  assert.equal(textNode(root, 'No open user Todos.'), undefined);
  fleetEvent();
  await tick();
  assert.ok(textNode(root, 'Todo review'));
  requests[2].reject(new Error('Loom unavailable'));
  await flush();
  assert.ok(textNode(root, 'Todo review'));
  assert.equal(textNode(root, `${SERVER}: Could not load user Todos: Loom unavailable`).props.role, 'alert');
  textNode(root, 'Refresh').props.onClick();
  await flush();
  assert.ok(textNode(root, `${SERVER}: Could not load user Todos: Loom unavailable`));
  requests[3].resolve(empty());
  await flush();
  assertEmpty(root);
  assert.equal(textNode(root, 'Todo review'), undefined);
  assert.equal(textNode(root, `${SERVER}: Could not load user Todos: Loom unavailable`), undefined);
  props.fleet = [{ ...session('a'), branch: { ...session('a').branch, tags: [{ key: 'attention', value: 'attention', note: 'Please decide' }] } }];
  await flush();
  assert.ok(textNode(root, 'Please decide'));
  assert.equal(textNode(root, 'Nothing needs you.'), undefined);
  assert.ok(textNode(root, 'No open user Todos.'));
});

test('an initial failure is visible and a manual retry can settle empty', async t => {
  const { root, requests } = await mount(t);
  requests[0].reject(new Error('Initial load failed'));
  await flush();
  assert.equal(textNode(root, `${SERVER}: Could not load user Todos: Initial load failed`).props.role, 'alert');
  assert.equal(textNode(root, 'Nothing needs you.'), undefined);
  assert.equal(textNode(root, 'No open user Todos.'), undefined);
  textNode(root, 'Refresh').props.onClick();
  await flush();
  assert.ok(textNode(root, `${SERVER}: Could not load user Todos: Initial load failed`));
  requests[1].resolve(empty());
  await flush();
  assertEmpty(root);
});

for (const staleResult of ['success', 'failure']) {
  test(`scope change clears previous data and ignores a late ${staleResult}`, async t => {
    const { root, props, requests, fleetEvent, tick } = await mount(t, { fleet: [session('a'), session('b', 'beta')] });
    requests[0].resolve({ todos: [todo('old')], warnings: ['Old scope warning'] });
    await flush();
    fleetEvent();
    await tick();
    props.project = { id: scopedId(SERVER, 'beta'), name: 'Beta' };
    await flush();
    assertUnsettled(root);
    assert.equal(textNode(root, 'Todo old'), undefined);
    assert.equal(textNode(root, 'Old scope warning'), undefined);
    await tick();
    if (staleResult === 'success') requests[1].resolve({ todos: [todo('stale', 'b')], warnings: ['Stale warning'] });
    else requests[1].reject(new Error('Old scope failure'));
    await flush();
    assertUnsettled(root);
    assert.equal(textNode(root, 'Todo stale'), undefined);
    assert.equal(textNode(root, 'Stale warning'), undefined);
    assert.equal(textNode(root, 'Could not load user Todos: Old scope failure'), undefined);
    await tick();
    assert.deepEqual(requests[2].args.topicIds, ['b']);
    requests[2].resolve(empty());
    await flush();
    assertEmpty(root);
  });
}

test('scope identity changes reload even when both projects have no topics', async t => {
  const { root, props, requests, tick } = await mount(t, { fleet: [] });
  requests[0].resolve(empty());
  await flush();
  assertEmpty(root);
  props.project = { id: scopedId(SERVER, 'beta'), name: 'Beta' };
  await flush();
  assertUnsettled(root);
  await tick();
  assert.deepEqual(requests[1].args.topicIds, []);
  requests[1].resolve(empty());
  await flush();
  assertEmpty(root);
});

test('new topic membership unsettles the current project until its Todos load', async t => {
  const { root, props, requests, tick } = await mount(t);
  requests[0].resolve(empty());
  await flush();
  props.fleet = [session('a'), session('new')];
  await flush();
  assertUnsettled(root);
  await tick();
  assert.deepEqual(requests[1].args.topicIds, ['a', 'new']);
  requests[1].resolve({ todos: [todo('new-task', 'new')], warnings: [] });
  await flush();
  assert.ok(textNode(root, 'Todo new-task'));
  assert.equal(textNode(root, 'Nothing needs you.'), undefined);
});
