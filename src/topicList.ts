// The Topics list (docs/design.md "Navigation"): which sessions are topics
// (top-level threads — every new top-level conversation is a topic,
// including legacy single-prompt launches), how many threads each one
// delegated, and which of them the list shows.
//
// Archived topics disappear from the list by default: archiving in loom is
// a teardown (worktree removed, terminal killed), so a done topic is
// history, not fleet. The Archived toggle is a view over the same leaders,
// not a second walk — and archived child threads keep a live topic's shape
// (they stay counted and visible, dimmed).
//
// Everything here is pure and free of .vue imports so the logic runs under
// plain `node --test` with no build step (tests/topicList.test.mjs),
// mirroring src/topic-view.ts and src/topicOrder.ts.

import type { SessionSummary } from "./App.vue";

/** A topic's list entry: the leader session plus its delegated count. */
export interface TopicEntry<T extends SessionSummary = SessionSummary> {
  session: T;
  childCount: number;
}

/**
 * The parent of a session, resolved by either link loom records —
 * `parent_session_id` (preferred) or `parent_id` (branch). Self-parents
 * and dangling links yield null.
 */
export function parentOf<T extends SessionSummary>(
  fleet: readonly T[],
  session: T,
): T | null {
  if (session.parent_session_id) {
    const parent = fleet.find((s) => s.id === session.parent_session_id);
    if (parent && parent.id !== session.id) return parent;
  }
  if (session.parent_id) {
    const parent = fleet.find((s) => s.branch.id === session.parent_id);
    if (parent && parent.id !== session.id) return parent;
  }
  return null;
}

/**
 * All top-level topics (leaders) — archived included — ordered by the
 * caller's sort (byTopicRecency(topicRecencyMap(fleet)), src/topicOrder.ts:
 * newest last user message first). Child counts cover every session that
 * names this one as parent, archived children included — a topic's
 * finished work is part of its shape.
 */
export function buildTopicList<T extends SessionSummary>(
  fleet: readonly T[],
  compare: (a: T, b: T) => number,
): TopicEntry<T>[] {
  const childCount = new Map<string, number>();
  for (const session of fleet) {
    const parent = parentOf(fleet, session);
    if (parent) childCount.set(parent.id, (childCount.get(parent.id) ?? 0) + 1);
  }
  return fleet
    .filter((s) => !parentOf(fleet, s))
    .map((s) => ({ session: s, childCount: childCount.get(s.id) ?? 0 }))
    .sort((a, b) => compare(a.session, b.session));
}

/**
 * The visible view over the topic list: archived leaders hidden unless
 * `showArchived` (the toolbar toggle). Child threads are never filtered
 * here — they hang off their leader's card, and the card decides.
 */
export function visibleTopics<T extends SessionSummary>(
  topics: readonly TopicEntry<T>[],
  showArchived: boolean,
): TopicEntry<T>[] {
  return topics.filter((t) => showArchived || t.session.status !== "archived");
}

/**
 * The children of each session, siblings sorted newest-activity-first (the
 * existing sidebar ordering for nested threads). Archived children are
 * kept — a topic's finished work is part of its shape.
 */
export function childrenMap<T extends SessionSummary>(
  fleet: readonly T[],
): Map<string, T[]> {
  const children = new Map<string, T[]>();
  for (const session of fleet) {
    const parent = parentOf(fleet, session);
    if (!parent) continue;
    const siblings = children.get(parent.id) ?? [];
    siblings.push(session);
    children.set(parent.id, siblings);
  }
  for (const siblings of children.values()) {
    siblings.sort((a, b) => b.last_activity_at.localeCompare(a.last_activity_at));
  }
  return children;
}

/** How many leaders the default filter is hiding (the Archived toggle's). */
export function archivedTopicCount<T extends SessionSummary>(
  topics: readonly TopicEntry<T>[],
): number {
  return topics.filter((t) => t.session.status === "archived").length;
}
