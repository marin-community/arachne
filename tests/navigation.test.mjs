import test from 'node:test';
import assert from 'node:assert/strict';
import { searchThreads } from '../src/navigation.ts';
import { isStandalone, isTrack } from '../src/threadKind.ts';
const make = (id, extra = {}) => ({id, status: 'running', last_activity_at: '2026-10-01', branch: {id, title: id, name: id, tags: [], ...extra}});
test('search locates PR provenance and requires all terms', () => {
  const worker = make('worker', {github: {pr_number: 42, pr_url: 'https://github.com/org/repo/pull/42', pr_title: 'Fix reader'}});
  assert.deepEqual(searchThreads([worker, make('other')], 'repo #42'), [worker]);
  assert.deepEqual(searchThreads([worker], 'repo unrelated'), []);
});
test('search retains archived provenance but prioritizes live work', () => {
  const archived = {...make('old'), status: 'archived', last_activity_at: '2099'};
  const live = make('live');
  assert.deepEqual(searchThreads([archived, live], '').map(s => s.id), ['live', 'old']);
});
test('explicit standalone marker distinguishes one-offs while preserving legacy tracks', () => {
  const oneOff = make('quick', {tags: [{key: 'topic', value: 'false'}]});
  assert.equal(isStandalone(oneOff), true);
  assert.equal(isTrack(make('legacy')), true);
  oneOff.branch.tags[0].value = 'true';
  assert.equal(isTrack(oneOff), true);
});
