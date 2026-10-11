import assert from 'node:assert/strict';
import test from 'node:test';
import { scopedId } from '../src/backendIdentity.ts';
const SERVER = 'http://127.0.0.1:7878';
const wireSession = id => ({ id, status: 'running', parent_id: null, parent_session_id: null,
  branch: { id: `branch-${id}`, branch: `weaver/${id}`, name: id, title: id, repo_root: '/repo', tags: [] }, placement: null });
import { readFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve, basename } from 'node:path';
import { pathToFileURL } from 'node:url';
import { build } from 'esbuild';
import { parse, compileScript } from '@vue/compiler-sfc';

// Mount the real App and launch composers with an in-memory Vue renderer.
// Stub unrelated panels and the native bridge; exercise the actual event
// handlers and Vue's asynchronous prop updates without a desktop session.
const directory = await mkdtemp(join(tmpdir(), 'arachne-track-launch-'));
const output = join(directory, 'app.mjs');
await build({
  stdin: {
    contents: `export { default as App } from './src/App.vue'; export { createRenderer, nextTick } from 'vue';`,
    resolveDir: resolve('.'),
  },
  bundle: true, format: 'esm', platform: 'node', outfile: output,
  plugins: [{
    name: 'component-test',
    setup(builder) {
      builder.onResolve({ filter: /^@tauri-apps\// }, args => ({ path: args.path, namespace: 'native' }));
      builder.onLoad({ filter: /.*/, namespace: 'native' }, () => ({ contents: `
        export const invoke = (command, args) => {
          if (command === "load_token") return Promise.resolve(null);
          if (command === "connect") return Promise.resolve();
          if (command === "refresh_fleet") return Promise.resolve({ server_url: args.baseUrl, sessions: [], layout: { revision: 0, spaces: [], defaults: [] } });
          if (command === "launch_options") return Promise.resolve({ agents: [], profiles: [], default_agent: "" });
          return globalThis.trackLaunchInvoke(command, args);
        };
        export const listen = async (event, callback) => {
          globalThis.trackLaunchListeners.set(event, callback);
          return () => globalThis.trackLaunchListeners.delete(event);
        };
      ` }));
      builder.onLoad({ filter: /\.vue$/ }, async ({ path }) => {
        if (!['App.vue', 'NewTrackChat.vue', 'NewThreadSheet.vue'].includes(basename(path))) {
          return { contents: `export default { name: ${JSON.stringify(basename(path))}, render() { return null; } };` };
        }
        const { descriptor } = parse(await readFile(path, 'utf8'), { filename: path });
        return { contents: compileScript(descriptor, { id: basename(path), inlineTemplate: true }).content, loader: 'ts' };
      });
    },
  }],
});
const { App, createRenderer, nextTick } = await import(pathToFileURL(output));
await rm(directory, { recursive: true, force: true });

const renderer = createRenderer({
  createElement: tag => ({
    tag, tagName: tag.toUpperCase(), props: {}, children: [], focus() {}, setSelectionRange() {},
    addEventListener() {}, removeEventListener() {},
    getRootNode() { return globalThis.document; },
    get options() { return this.children.filter(child => child.tag === 'option'); },
  }),
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
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const flush = async () => { await new Promise(resolve => setImmediate(resolve)); await nextTick(); };

async function mount(launcher = 'track', draftOverrides = {}) {
  const values = new Map([['arachne.newTopicDraft', JSON.stringify({ body: 'Fix Linux launch', repo: 'org/repo', ...draftOverrides })]]);
  globalThis.localStorage = {
    getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key),
  };
  globalThis.window = { addEventListener() {}, removeEventListener() {} };
  globalThis.Document = class {};
  globalThis.ShadowRoot = class {};
  globalThis.document = Object.assign(new Document(), { addEventListener() {}, removeEventListener() {} });
  globalThis.trackLaunchListeners = new Map();
  const root = { children: [] };
  const app = renderer.createApp(App);
  app.mount(root);
  await flush();
  // Open through the actual sidebar event handler; only the sidebar is stubbed.
  if (launcher) {
    const event = launcher === 'track' ? 'onNewTopic' : 'onNewThread';
    app._instance.subTree.children.find(node => node.props?.[event]).props[event]();
  }
  await flush();
  return { root, app, values };
}

test('rapid submissions preserve canonical branch choice, launch once and retain the composer until chat opens', async () => {
  const launched = deferred(), opened = deferred();
  const calls = [];
  globalThis.trackLaunchInvoke = (command, args) => {
    calls.push({ command, args });
    if (command === 'launch_session') return launched.promise;
    if (command === 'open_session') return opened.promise;
    return Promise.resolve(null);
  };
  const { root, app, values } = await mount('track', { topicBranch: 'arachne/tracks/linux-launch' });
  try {
    await nextTick();
    const composer = find(root, node => node.tag === 'textarea');
    const send = find(root, node => node.props?.['aria-label'] === 'Send and launch track');
    composer.props.onKeydown({ key: 'Enter', ctrlKey: true, preventDefault() {} });
    send.props.onClick();
    composer.props.onKeydown({ key: 'Enter', ctrlKey: true, repeat: true, preventDefault() {} });
    assert.equal(calls.filter(call => call.command === 'launch_session').length, 1);
    assert.equal(calls.find(call => call.command === 'launch_session').args.topicBranch, 'arachne/tracks/linux-launch');
    assert.equal(calls.find(call => call.command === 'launch_session').args.asTrack, true);
    await nextTick();
    assert.equal(send.props.disabled, true);
    launched.resolve(wireSession('new-track'));
    await flush();
    assert.equal(calls.find(call => call.command === 'open_session').args.id, 'new-track');
    assert.ok(find(root, node => node.props?.['aria-label'] === 'New track'));
    opened.resolve(wireSession('new-track'));
    await flush();
    assert.equal(find(root, node => node.props?.['aria-label'] === 'New track'), undefined);
    assert.equal([...values.keys()].some(key => key.startsWith('arachne.newTopicDraft')), false);
  } finally { app.unmount(); }
});

test('a failed launch preserves the composer draft and allows a single retry', async () => {
  const first = deferred(), retry = deferred();
  let launches = 0;
  globalThis.trackLaunchInvoke = command => {
    if (command === 'launch_session') return ++launches === 1 ? first.promise : retry.promise;
    return Promise.resolve(null);
  };
  const { root, app } = await mount();
  try {
    await nextTick();
    const send = find(root, node => node.props?.['aria-label'] === 'Send and launch track');
    send.props.onClick();
    first.reject(new Error('Loom unavailable'));
    await flush();
    assert.equal(send.props.disabled, false);
    assert.equal(find(root, node => node.tag === 'textarea').value, 'Fix Linux launch');
    send.props.onClick();
    send.props.onClick();
    assert.equal(launches, 2);
    retry.reject(new Error('Still unavailable'));
    await flush();
  } finally { app.unmount(); }
});

test('failed activation shows the created track instead of offering another launch', async () => {
  const opened = deferred();
  let launches = 0;
  globalThis.trackLaunchInvoke = command => {
    if (command === 'launch_session') {
      launches++;
      return Promise.resolve(wireSession('created-track'));
    }
    if (command === 'open_session') return opened.promise;
    return Promise.resolve(null);
  };
  const { root, app, values } = await mount();
  try {
    await nextTick();
    find(root, node => node.props?.['aria-label'] === 'Send and launch track').props.onClick();
    await flush();
    opened.reject(new Error('Could not activate chat'));
    await flush();
    assert.equal(launches, 1);
    assert.equal(find(root, node => node.props?.['aria-label'] === 'New track'), undefined);
    const thread = app._instance.subTree.children.find(node => node.props?.session);
    assert.equal(thread.props.session.id, scopedId(SERVER, 'created-track'));
    assert.ok(find(root, node => node.text === 'Could not activate chat'));
    assert.equal([...values.keys()].some(key => key.startsWith('arachne.newTopicDraft')), false);
  } finally { app.unmount(); }
});

for (const phase of ['creation', 'activation']) {
  for (const destination of ['thread', 'project']) {
  test(`delayed ${phase} preserves a new ${destination} panel opened while it was pending`, async () => {
    const created = deferred();
    const opened = deferred();
    globalThis.trackLaunchInvoke = command => {
      if (command === 'launch_session') return created.promise;
      if (command === 'open_session') return opened.promise;
      return Promise.resolve(null);
    };
    const { root, app, values } = await mount();
    try {
      await nextTick();
      find(root, node => node.props?.['aria-label'] === 'Send and launch track').props.onClick();
      if (phase === 'activation') created.resolve(wireSession('created-track'));
      await flush();
      const event = destination === 'thread' ? 'onNewThread' : 'onNewProject';
      app._instance.subTree.children.find(node => node.props?.[event]).props[event]();
      await nextTick();
      if (phase === 'creation') created.resolve(wireSession('created-track'));
      opened.resolve(wireSession('created-track'));
      await flush();
      if (destination === 'thread') assert.ok(find(root, node => node.props?.['aria-label'] === 'New thread'));
      else assert.ok(app._instance.subTree.children.find(node => node.type?.name === 'ProjectSheet.vue'));
      assert.equal([...values.keys()].some(key => key.startsWith('arachne.newTopicDraft')), false);
    } finally { app.unmount(); }
  });
  }
}

test('successful creation preserves a newer saved track draft', async () => {
  const created = deferred();
  globalThis.trackLaunchInvoke = command => {
    if (command === 'launch_session') return created.promise;
    if (command === 'open_session') return Promise.resolve(wireSession('created-track'));
    return Promise.resolve(null);
  };
  const { root, app, values } = await mount();
  try {
    await nextTick();
    find(root, node => node.props?.['aria-label'] === 'Send and launch track').props.onClick();
    app._instance.subTree.children.find(node => node.props?.onNewThread).props.onNewThread();
    await nextTick();
    app._instance.subTree.children.find(node => node.props?.onNewTopic).props.onNewTopic();
    await nextTick();
    find(root, node => node.tag === 'textarea').props['onUpdate:modelValue']('A different track draft');
    app._instance.subTree.children.find(node => node.props?.onLaunch && node.props?.layout !== undefined).props.onClose();
    await nextTick();
    created.resolve(wireSession('created-track'));
    await flush();
    const saved = [...values.entries()].find(([key]) => key.startsWith('arachne.newTopicDraft'));
    assert.equal(JSON.parse(saved[1]).body, 'A different track draft');
  } finally { app.unmount(); }
});

test('new thread submits once and stays visible until its live chat opens', async () => {
  const launched = deferred(), opened = deferred();
  const calls = [];
  globalThis.trackLaunchInvoke = (command, args) => {
    calls.push({ command, args });
    if (command === 'launch_session') return launched.promise;
    if (command === 'open_session') return opened.promise;
    return Promise.resolve(null);
  };
  const { root, app } = await mount('thread');
  try {
    await nextTick();
    const goal = find(root, node => node.tag === 'textarea');
    goal.props['onUpdate:modelValue']('Fix the thread transition');
    await nextTick();
    const launch = find(root, node => node.text === 'Launch thread');
    goal.props.onKeydown({ key: 'Enter', ctrlKey: true, preventDefault() {} });
    launch.props.onClick();
    assert.equal(calls.filter(call => call.command === 'launch_session').length, 1);
    assert.equal(calls.find(call => call.command === 'launch_session').args.asTrack, false);
    launched.resolve(wireSession('new-thread'));
    await flush();
    assert.ok(find(root, node => node.props?.['aria-label'] === 'New thread'));
    opened.resolve(wireSession('new-thread'));
    await flush();
    assert.equal(find(root, node => node.props?.['aria-label'] === 'New thread'), undefined);
    assert.equal(app._instance.subTree.children.find(node => node.props?.session).props.session.id, scopedId(SERVER, 'new-thread'));
  } finally { app.unmount(); }
});

const archiveSession = wireSession;
async function openForArchive(app, id = 'current') {
  await flush();
  globalThis.trackLaunchListeners.get('loom://fleet')({ payload: {
    server_url: 'http://127.0.0.1:7878',
    sessions: [archiveSession('current'), archiveSession('other')],
    layout: { revision: 0, spaces: [], defaults: [] },
  } });
  app._instance.subTree.children.find(node => node.props?.onSelect).props.onSelect(scopedId(SERVER, id));
  await flush();
}

test('archiving keeps the current chat visible until success then switches directly to home', async () => {
  const archived = deferred();
  globalThis.trackLaunchInvoke = (command, args) => {
    if (command === 'open_session') return Promise.resolve(archiveSession(args.id));
    if (command === 'archive_session') return archived.promise;
    return Promise.resolve(null);
  };
  const { app } = await mount(null);
  try {
    await openForArchive(app);
    const thread = app._instance.subTree.children.find(node => node.props?.session);
    thread.props.onArchive(scopedId(SERVER, 'current'));
    await nextTick();
    assert.equal(app._instance.subTree.children.find(node => node.props?.session).component.uid, thread.component.uid);
    archived.resolve();
    await flush();
    const home = app._instance.subTree.children.find(node => node.type?.name === 'HomeView.vue');
    assert.ok(home);
    assert.equal(home.props.fleet.find(session => session.id === scopedId(SERVER, 'current')).status, 'archived');
    assert.equal(app._instance.subTree.children.some(node => node.props?.session), false);
  } finally { app.unmount(); }
});

test('archive failure or archiving another thread preserves the current chat', async () => {
  const failed = deferred();
  let archiveCalls = 0;
  globalThis.trackLaunchInvoke = (command, args) => {
    if (command === 'open_session') return Promise.resolve(archiveSession(args.id));
    if (command === 'archive_session') return ++archiveCalls === 1 ? failed.promise : Promise.resolve();
    return Promise.resolve(null);
  };
  const { root, app } = await mount(null);
  try {
    await openForArchive(app);
    const thread = app._instance.subTree.children.find(node => node.props?.session);
    thread.props.onArchive(scopedId(SERVER, 'current'));
    failed.reject(new Error('Archive failed'));
    await flush();
    assert.equal(app._instance.subTree.children.find(node => node.props?.session).component.uid, thread.component.uid);
    assert.ok(find(root, node => node.text?.endsWith('Archive failed')));
    app._instance.subTree.children.find(node => node.props?.onSelect).props.onArchive(scopedId(SERVER, 'other'));
    await flush();
    assert.equal(app._instance.subTree.children.find(node => node.props?.session).component.uid, thread.component.uid);
    assert.equal(thread.props.fleet.find(session => session.id === scopedId(SERVER, 'other')).status, 'archived');
  } finally { app.unmount(); }
});
