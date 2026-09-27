// Topic ordering (docs/design.md "Navigation"): a topic's recency is when a
// person last steered it — the newest user message anywhere in its subtree —
// not when its agents were last active. Loom restamps `last_activity_at` on
// every agent frame (thoughts, tool calls, streaming output), so ordering
// topics by it makes any busy worker pull its topic to the top and the list
// "jumps around" while you watch. Ordering by the last user message keeps
// the list still while work runs.
//
// Everything here is pure and free of .vue imports so the decision logic
// runs under plain `node --test` with no build step (tests/topicOrder.test.mjs),
// mirroring src/topic-view.ts.

import { topicRootOf, type FleetThread } from "./topic-view.ts";

/**
 * The slice of a session summary this module needs — a topic-walkable
 * thread plus the recency stamps. `last_user_message_at` is absent on
 * older looms and null when the journal holds no user input; both mean
 * "fall back" (see `sessionRecency`).
 */
export interface OrderableSession extends FleetThread {
  last_activity_at: string;
  last_user_message_at?: string | null;
}

/**
 * A session's user-recency stamp: the last user message when loom reports
 * one, else its last activity. The fallback matters for two cases:
 *
 * - an older loom predates `last_user_message_at` (the field is
 *   `#[serde(default)]`-absent there): ordering degrades to the previous
 *   activity-sort rather than mis-sorting every topic as "never used";
 * - a terminal session (no chat journal at all) and, in principle, a
 *   journal with no user block: `last_activity_at` is always present and
 *   strictly better than dropping the row to the bottom.
 */
export function sessionRecency(session: OrderableSession): string {
  return session.last_user_message_at ?? session.last_activity_at;
}

/**
 * The newest user message under each topic: topicId -> timestamp, covering
 * the coordinator AND every thread in its delegation subtree. A person
 * steering a worker (the remembered-thread route opens workers directly)
 * must still float the topic; a busy agent streaming frames must not.
 *
 * The fallback in `sessionRecency` folds a worker's activity in only when
 * that worker reports no user-message stamp (older loom), preserving the
 * previous activity-order there.
 */
export function topicRecencyMap(
  fleet: readonly OrderableSession[],
): Record<string, string> {
  const max: Record<string, string> = {};
  for (const session of fleet) {
    const root = topicRootOf(fleet, session.id);
    if (!root) continue; // unknown id: topic-view's contract is null
    const stamp = sessionRecency(session);
    if (!max[root.id] || max[root.id] < stamp) max[root.id] = stamp;
  }
  return max;
}

/** A session's recency, folded through its topic root when one is known. */
export function topicRecency(
  session: OrderableSession,
  recency: Record<string, string>,
): string {
  return recency[session.id] ?? sessionRecency(session);
}

/**
 * Order topics by when a person last steered them, newest first. Ties break
 * by id so the sort is total and stable across fleet snapshots (they arrive
 * frequently).
 */
export function byTopicRecency(
  recency: Record<string, string>,
): (a: OrderableSession, b: OrderableSession) => number {
  return (a, b) => {
    const ka = topicRecency(a, recency);
    const kb = topicRecency(b, recency);
    if (ka !== kb) return ka < kb ? 1 : -1;
    if (a.id !== b.id) return a.id < b.id ? -1 : 1;
    return 0;
  };
}
