// Chat-first topic navigation (docs/design.md, "Navigation").
//
// Selecting a Topic opens a chat: the coordinator thread on a first
// visit, the last thread opened within that Topic on later visits (when
// it is still available and still part of the Topic), the coordinator
// again when one of the Topic's threads is already on screen (the click
// asked for the Topic, not the thread already showing), and the open
// chat is kept when the Topic itself — its coordinator — is selected.
// The scoped dashboard stays reachable as an explicit Overview detour,
// never as the default route.
//
// Everything here is pure and free of .vue imports so the decision logic
// runs under plain `node --test` with no build step
// (tests/topicView.test.mjs), mirroring src/chatRows.ts.

/** localStorage key holding the topicId -> threadId map. */
export const TOPIC_THREAD_STORAGE_KEY = "arachne.topicThread";

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

/** The slice of localStorage these helpers need — trivial to stub in tests. */
export interface TopicThreadStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** Last-opened-thread memory: topicId -> threadId. */
export type TopicThreadMemory = Record<string, string>;
function memoryKey(storage: TopicThreadStorage): string {
  const server = storage.getItem("loomUrl");
  return server ? `${TOPIC_THREAD_STORAGE_KEY}@${server.replace(/\/+$/, "")}` : TOPIC_THREAD_STORAGE_KEY;
}

/** Why a topic view resolved to a particular thread. */
export type TopicThreadSource = "current" | "remembered" | "coordinator";

export interface TopicThreadChoice {
  threadId: string;
  source: TopicThreadSource;
}

export interface ResolveTopicThreadInput {
  /** The topic (root/coordinator session) being opened. */
  topicId: string;
  fleet: readonly FleetThread[];
  /** The thread currently open, if any — the Topic keeps it only when it is
   *  the Topic's own coordinator; a worker sends the click to the
   *  coordinator instead. */
  currentThreadId?: string | null;
  /** The thread last opened in this topic, read from the memory map. */
  rememberedThreadId?: string | null;
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
 * Decide which thread a Topic selection opens.
 *
 * 1. The coordinator is already open: clicking the Topic again keeps it.
 *    It always qualifies, even if the fleet snapshot has not caught up
 *    with it yet.
 * 2. A thread of this Topic is already open: the click asked for the
 *    Topic, and that thread is already on screen — open the coordinator,
 *    the Topic's voice, so the click always goes somewhere. (Resolving
 *    to the open worker instead would make the click a no-op with no
 *    way to reach the Topic's own chat.)
 * 3. Entering from elsewhere: restore the thread last opened within the
 *    Topic, but only if it is still available (present and not archived
 *    — the home and dashboard surfaces treat archived workers as
 *    finished) and still belongs to this Topic (it may have been
 *    reparented or removed).
 * 4. Otherwise open the coordinator: the Topic's voice, not one item
 *    buried in a dashboard.
 */
export function resolveTopicThread(input: ResolveTopicThreadInput): TopicThreadChoice {
  const { topicId, fleet, currentThreadId, rememberedThreadId } = input;
  if (currentThreadId && currentThreadId === topicId) {
    return { threadId: currentThreadId, source: "current" };
  }
  if (currentThreadId && threadBelongsToTopic(fleet, topicId, currentThreadId)) {
    return { threadId: topicId, source: "coordinator" };
  }
  if (rememberedThreadId && isRestorable(fleet, topicId, rememberedThreadId)) {
    return { threadId: rememberedThreadId, source: "remembered" };
  }
  return { threadId: topicId, source: "coordinator" };
}

function isRestorable(
  fleet: readonly FleetThread[],
  topicId: string,
  threadId: string,
): boolean {
  const thread = fleet.find((session) => session.id === threadId);
  if (!thread || thread.status === "archived") return false;
  return threadBelongsToTopic(fleet, topicId, threadId);
}

/**
 * Read the topicId -> threadId map. Corrupt, hand-edited, or non-object
 * data yields an empty map so it can never steer navigation.
 */
export function readTopicThreadMemory(
  storage: TopicThreadStorage | null | undefined,
): TopicThreadMemory {
  let raw: string | null = null;
  try {
    raw = storage ? storage.getItem(memoryKey(storage)) : null;
  } catch {
    return {};
  }
  if (!raw) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return {};
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return {};
  const memory: TopicThreadMemory = {};
  for (const [topicId, threadId] of Object.entries(parsed)) {
    if (typeof threadId === "string" && threadId) memory[topicId] = threadId;
  }
  return memory;
}

/** Record the last thread opened within a topic. Never throws. */
export function rememberTopicThread(
  storage: TopicThreadStorage | null | undefined,
  topicId: string,
  threadId: string,
): void {
  if (!storage || !topicId || !threadId) return;
  const memory = readTopicThreadMemory(storage);
  if (memory[topicId] === threadId) return;
  memory[topicId] = threadId;
  try {
    storage.setItem(memoryKey(storage), JSON.stringify(memory));
  } catch {
    // A full or blocked store must never break navigation.
  }
}
