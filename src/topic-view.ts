// Chat-first topic navigation (docs/design.md, "Navigation").
//
// Selecting a Topic opens a chat — its coordinator thread, always: the
// Topic's voice, not one item buried in a dashboard. Worker threads
// inside the Topic are opened deliberately (sidebar sub-thread rows,
// inspector Threads tab, home rows), never by clicking the Topic, and
// they never hijack the next Topic visit. The scoped dashboard stays
// reachable as an explicit Overview detour, never as the default route.
//
// Everything here is pure and free of .vue imports so the decision logic
// runs under plain `node --test` with no build step
// (tests/topicView.test.mjs), mirroring src/chatRows.ts.

/**
 * The slice of a fleet session this module needs. App.vue's
 * SessionSummary satisfies it structurally, so the app can pass its
 * fleet straight in while tests build tiny fixtures.
 */
export interface FleetThread {
  id: string;
  status: string;
  parent_session_id: string | null;
  parent_id: string | null;
  branch: { id: string };
}

/** Why a topic view resolved to a particular thread. */
export type TopicThreadSource = "current" | "coordinator";

export interface TopicThreadChoice {
  threadId: string;
  source: TopicThreadSource;
}

export interface ResolveTopicThreadInput {
  /** The topic (root/coordinator session) being opened. */
  topicId: string;
  /** The thread currently open, if any — kept only when it is the
   *  Topic's own coordinator (clicking an already-selected Topic). */
  currentThreadId?: string | null;
}

/**
 * The ancestor chain of `sessionId`, nearest first, excluding the session
 * itself. Cycles and dangling links stop the walk; an unknown id yields
 * an empty chain.
 */
export function threadAncestors<T extends FleetThread>(
  fleet: readonly T[],
  sessionId: string | null | undefined,
): T[] {
  if (!sessionId) return [];
  let node = fleet.find((session) => session.id === sessionId);
  if (!node) return [];
  const ancestors: T[] = [];
  const seen = new Set<string>([node.id]);
  while (node) {
    const parent = fleet.find(
      (session) =>
        session.id === node?.parent_session_id || session.branch.id === node?.parent_id,
    );
    if (!parent || seen.has(parent.id)) break;
    seen.add(parent.id);
    ancestors.push(parent);
    node = parent;
  }
  return ancestors;
}

/**
 * The root (topic/coordinator) session of `sessionId`, following
 * parent_session_id and parent_id (branch) links. Cycles and missing
 * links stop the walk; an unknown id yields null.
 */
export function topicRootOf<T extends FleetThread>(
  fleet: readonly T[],
  sessionId: string | null | undefined,
): T | null {
  let node: T | undefined = sessionId
    ? fleet.find((session) => session.id === sessionId)
    : undefined;
  if (!node) return null;
  const seen = new Set<string>();
  while (node && !seen.has(node.id)) {
    seen.add(node.id);
    const parent: T | undefined = fleet.find(
      (session) =>
        session.id === node?.parent_session_id ||
        session.branch.id === node?.parent_id,
    );
    if (!parent) break;
    node = parent;
  }
  return node ?? null;
}

/** Whether `threadId` lives inside `topicId`'s delegation subtree. */
export function threadBelongsToTopic(
  fleet: readonly FleetThread[],
  topicId: string,
  threadId: string | null | undefined,
): boolean {
  if (!threadId) return false;
  return topicRootOf(fleet, threadId)?.id === topicId;
}

/**
 * Decide which thread a Topic selection opens: the Topic's coordinator
 * thread, always — the Topic is its coordinator's chat. The only thing
 * kept is an already-open coordinator (clicking an already-selected
 * Topic is a no-op, not a refetch); an open worker thread resolves to
 * the coordinator, the click having asked for the Topic while its
 * worker was on screen. Restoring the thread last opened within the
 * Topic is deliberately absent: a Topic click must mean the Topic's
 * chat, never whichever worker happened to be open before (an
 * inconsistent “sometimes coordinator, sometimes first thread” was
 * exactly what made clicking a Track feel unreliable). Worker threads
 * are opened deliberately through their own affordances (sidebar
 * sub-thread rows, inspector Threads tab, home rows).
 */
export function resolveTopicThread(input: ResolveTopicThreadInput): TopicThreadChoice {
  const { topicId, currentThreadId } = input;
  if (currentThreadId && currentThreadId === topicId) {
    return { threadId: currentThreadId, source: "current" };
  }
  return { threadId: topicId, source: "coordinator" };
}
