import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { loomArtifactTarget, routeArtifactClick } from '../src/artifactLinks.ts';
import { createArtifactPreview } from '../src/artifactPreview.ts';

const base = 'http://127.0.0.1:7878';
const path = '/s/28kvicam/artifacts/track-brief';
const target = { key: '28kvicam', name: 'track-brief' };

test('configured artifact links accept UI URLs and API endpoint settings', () => {
  for (const endpoint of [base, `${base}/`, `${base}/api`, `${base}/api/`]) {
    for (const href of [`${base}${path}`, path, `//127.0.0.1:7878${path}`, `${base}${path}/?tab=artifacts#heading`]) {
      assert.deepEqual(loomArtifactTarget(href, endpoint), target);
    }
  }
  assert.deepEqual(loomArtifactTarget('https://loom.example/s/other/artifacts/a%20%5Bb%5D.md', 'https://loom.example:443/api'), { key: 'other', name: 'a [b].md' });
  assert.deepEqual(loomArtifactTarget('http://[::1]:7878/s/other/artifacts/brief', 'http://[::1]:7878/'), { key: 'other', name: 'brief' });
  assert.deepEqual(loomArtifactTarget(`${base}/s/other/artifacts/docs%2Fdesign.md`, base), { key: 'other', name: 'docs/design.md' });
});

test('artifacts.url canonical branch keys stay intact through click routing and preview reads', async () => {
  const canonical = { key: '99ne4k2x', name: 'track-brief' };
  const url = `${base}/s/99ne4k2x/artifacts/track-brief`;
  assert.deepEqual(loomArtifactTarget(url, base), canonical);
  let prevented = false;
  let opened: Promise<void> | undefined;
  const reads: unknown[] = [];
  const preview = createArtifactPreview(async value => {
    reads.push(value);
    return { title: 'Track brief', content: 'Canonical artifact' };
  });
  assert.equal(routeArtifactClick({ defaultPrevented: false, preventDefault() { prevented = true; } }, url, base, value => {
    opened = preview.open(value);
  }), true);
  await opened;
  assert.equal(prevented, true);
  assert.deepEqual(reads, [canonical]);
  assert.equal(preview.view.value?.content, 'Canonical artifact');
});

test('external origins, host aliases, unrelated routes and unsafe URLs stay external', () => {
  for (const href of [
    `https://example.com${path}`, `http://localhost:7878${path}`,
    `http://127.0.0.1:7879${path}`, `https://127.0.0.1:7878${path}`,
    `http://127.0.0.1:7878.evil.example${path}`, `http://user:secret@127.0.0.1:7878${path}`,
    `${base}/s/28kvicam`, `${base}${path}/extra`, `${base}/api/artifacts/get`,
    's/28kvicam/artifacts/track-brief', 'mailto:me@example.com', 'javascript:alert(1)',
    '/s/x/artifacts/', '/s/x/artifacts/%zz', '/s/x/artifacts/%00', '/s/%2f/artifacts/brief',
    '/s/x/artifacts/%2e%2e', String.raw`/s/x/artifacts/a\b`,
  ]) assert.equal(loomArtifactTarget(href, base), null, href);
  assert.equal(loomArtifactTarget(`${base}${path}`, 'not a URL'), null);
  assert.equal(loomArtifactTarget(`${base}${path}`, 'file:///'), null);
});

test('only recognized artifact clicks prevent browser navigation, before emitting', () => {
  let prevented = false;
  let opened: unknown = null;
  const event = { defaultPrevented: false, preventDefault() { prevented = true; } };
  assert.equal(routeArtifactClick(event, 'https://example.com', base, value => { opened = value; }), false);
  assert.equal(prevented, false);
  assert.equal(opened, null);
  assert.equal(routeArtifactClick(event, `${base}${path}`, base, value => {
    assert.equal(prevented, true);
    opened = value;
  }), true);
  assert.deepEqual(opened, target);
  assert.equal(routeArtifactClick({ ...event, defaultPrevented: true }, path, base, () => assert.fail()), false);
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test('preview reads the linked cross-session target and clears stale content', async () => {
  const requests: unknown[] = [];
  const preview = createArtifactPreview(async value => {
    requests.push(value);
    return { title: 'Track brief', content: '# Correct artifact' };
  });
  await preview.open(target);
  assert.deepEqual(requests, [target]);
  assert.equal(preview.view.value?.content, '# Correct artifact');
  assert.equal(preview.loading.value, false);
  assert.equal(preview.error.value, '');
  preview.close();
  assert.equal(preview.target.value, null);
  assert.equal(preview.view.value, null);
});

test('missing artifacts show a local error and subsequent navigation can recover', async () => {
  const preview = createArtifactPreview(async value => {
    if (value.name === 'missing') throw { message: 'artifact not found (404)' };
    return { title: value.name, content: 'Found' };
  });
  await preview.open({ key: 'archived-worker', name: 'missing' });
  assert.equal(preview.error.value, 'Artifact unavailable: artifact not found (404)');
  assert.equal(preview.view.value, null);
  assert.equal(preview.loading.value, false);
  await preview.open(target);
  assert.equal(preview.error.value, '');
  assert.equal(preview.view.value?.content, 'Found');
});

test('late responses and failures cannot overwrite a newer artifact or its loading state', async () => {
  const first = deferred<{ title: string; content: string }>();
  const second = deferred<{ title: string; content: string }>();
  const preview = createArtifactPreview(value => value.name === 'first' ? first.promise : second.promise);
  const older = preview.open({ key: 'worker', name: 'first' });
  const newer = preview.open(target);
  first.reject(new Error('old failure'));
  await older;
  assert.equal(preview.loading.value, true);
  assert.equal(preview.error.value, '');
  second.resolve({ title: 'Newest', content: 'Newest content' });
  await newer;
  assert.equal(preview.view.value?.content, 'Newest content');
  assert.deepEqual(preview.target.value, target);
});

test('closing or unmounting a pending preview discards its result', async () => {
  const pending = deferred<{ title: string; content: string }>();
  const preview = createArtifactPreview(() => pending.promise);
  const opened = preview.open(target);
  preview.close();
  pending.resolve({ title: 'Late', content: 'Late content' });
  await opened;
  assert.equal(preview.target.value, null);
  assert.equal(preview.view.value, null);
  assert.equal(preview.loading.value, false);
});

test('chat and activity surfaces route artifacts through the inspector and preserve external anchor attributes', () => {
  const source = (file: string) => readFileSync(new URL(`../src/${file}`, import.meta.url), 'utf8');
  const markdown = source('components/ChatMarkdown.vue');
  assert.match(markdown, /@click="onClick"/);
  assert.match(markdown, /link\.target = "_blank"/);
  assert.match(markdown, /link\.rel = "noopener noreferrer"/);
  for (const file of ['components/ThreadView.vue', 'components/TrackActivity.vue']) {
    const chatTags = source(file).match(/<ChatMarkdown\b[^>]*\/?>/g) ?? [];
    assert.ok(chatTags.length > 0);
    for (const tag of chatTags) {
      assert.match(tag, /:loom-url="loomUrl"/);
      assert.match(tag, /@open-artifact=/);
    }
  }
  const app = source('App.vue');
  assert.match(app, /function openArtifact\(target: ArtifactTarget\)\s*{\s*artifactRequest\.value = { \.\.\.target };\s*showResources\.value = true;/);
  assert.match(app, /watch\(\[inspectorServer, selectedId, selectedTopicId\]/);
  assert.match(source('components/TopicInspector.vue'), /if \(target\) tab\.value = "resources"/);
  assert.match(source('components/ResourcePanel.vue'), /onUnmounted\(artifact\.close\)/);
});

test('standalone artifact inspector visibility reserves the same grid column as the rendered panel', () => {
  const app = readFileSync(new URL('../src/App.vue', import.meta.url), 'utf8');
  assert.match(app, /'with-resources': inspectorVisible/);
  assert.match(app, /<TopicInspector v-if="inspectorVisible && inspectorTopic"/);
  const expression = app.match(/const inspectorVisible = computed\(\(\) => ([\s\S]*?)\);/)?.[1];
  assert.ok(expression);
  const evaluate = new Function('showNewThread', 'showNewTopic', 'showProjectSheet', 'viewMode', 'inspectorTopic', 'showResources', 'currentAvailable', `return (${expression});`);
  const visible = (overrides: Record<string, unknown> = {}) => {
    const state = { showNewThread: false, showNewTopic: false, showProjectSheet: false, viewMode: 'thread', inspectorTopic: { id: 'standalone' }, showResources: true, currentAvailable: true, ...overrides };
    return evaluate(...Object.values(state).map(value => ({ value })));
  };
  assert.equal(visible(), true);
  for (const override of [{ showNewThread: true }, { showNewTopic: true }, { showProjectSheet: true }, { viewMode: 'home' }, { inspectorTopic: null }, { showResources: false }, { currentAvailable: false }]) {
    assert.equal(visible(override), false);
  }
});
