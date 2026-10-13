import type { SessionSummary } from "./App.vue";
import { scopedId, splitScopedId } from "./backendIdentity.ts";
import { changeKey, parseChange, pinChange, sameChange, type DurableChange } from "./durableChange.ts";
import { canonicalTopicBranch, type IntegrationTarget } from "./trackPolicy.ts";
import type { LandingCandidate } from "./topicInspector.ts";

export interface QueueEntry {
  change: DurableChange;
  strategy: string;
  requested_by: null | "user" | "autoland";
}
export interface QueueState {
  version: 1;
  track_session: string;
  repo_root: string;
  target_branch: string;
  active: QueueEntry | null;
  pending: QueueEntry[];
}
export interface QueueView { queue: QueueState; revision: number }
const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);
const keys = (value: Record<string, unknown>, names: string[]) => Object.keys(value).length === names.length && names.every(name => Object.prototype.hasOwnProperty.call(value, name));
const wireKey = (change: DurableChange) => JSON.stringify([change.source_session, change.id]);

/** Keep queue evidence server-local; resolve navigation ownership separately. */
export function queueSourceId(topicId: string, change: DurableChange): string {
  const owner = splitScopedId(topicId);
  return owner ? scopedId(owner.baseUrl, change.source_session) : change.source_session;
}

/** Malformed or foreign queue metadata cannot enable ordering or Land. */
export function validateQueueView(value: unknown, topic: SessionSummary): QueueView {
  const invalid = () => new Error("The Changes queue is invalid or belongs to another Track. Refresh or reconcile it before continuing.");
  if (!record(value) || !keys(value, ["queue", "revision"]) || !Number.isSafeInteger(value.revision) || (value.revision as number) < 0 || !record(value.queue)) throw invalid();
  const queue = value.queue;
  if (!keys(queue, ["version", "track_session", "repo_root", "target_branch", "active", "pending"])
    || queue.version !== 1 || queue.track_session !== (splitScopedId(topic.id)?.id ?? topic.id)
    || queue.repo_root !== topic.branch.repo_root || queue.target_branch !== canonicalTopicBranch(topic) || !queue.target_branch
    || !Array.isArray(queue.pending)) throw invalid();
  const seen = new Set<string>();
  function entry(value: unknown): QueueEntry {
    if (!record(value) || !keys(value, ["change", "strategy", "requested_by"])) throw invalid();
    const change = parseChange(value.change);
    if (!change || change.repo_root !== queue.repo_root || change.source_session === queue.track_session || splitScopedId(change.source_session)
      || !["squash", "merge", "rebase", "cherry-pick"].includes(String(value.strategy))
      || ![null, "user", "autoland"].includes(value.requested_by as QueueEntry["requested_by"]) || seen.has(wireKey(change))) throw invalid();
    seen.add(wireKey(change));
    return { change: pinChange(change), strategy: value.strategy as string, requested_by: value.requested_by as QueueEntry["requested_by"] };
  }
  const active = queue.active === null ? null : entry(queue.active);
  return { revision: value.revision as number, queue: { version: 1, track_session: queue.track_session as string,
    repo_root: queue.repo_root as string, target_branch: queue.target_branch as string, active, pending: queue.pending.map(entry) } };
}

/** Compare validated snapshots in one owning Track/backend/canonical epoch. */
export function compareQueueSnapshots(previous: QueueView | null, next: QueueView): "newer" | "same" | "older" {
  if (!previous || next.revision > previous.revision) return "newer";
  if (next.revision < previous.revision) return "older";
  if (JSON.stringify(next.queue) !== JSON.stringify(previous.queue)) {
    throw new Error("Conflicting Changes queue data. Reconcile the queue before continuing.");
  }
  return "same";
}

export interface TimelineRow {
  key: string;
  change: DurableChange;
  candidate: LandingCandidate | null;
  entry: QueueEntry | null;
  phase: "active" | "pending" | "landed";
  problem: string;
  status: string;
}
export interface ChangeTimeline { active: TimelineRow | null; pending: TimelineRow[]; history: TimelineRow[]; canReorder: boolean }

/** Queue order wins. New declarations append in stable discovery order, never activity order. */
export function projectChangeQueue(view: QueueView | null, candidates: LandingCandidate[], topicId: string, discovered: readonly string[] = []): ChangeTimeline {
  const byKey = new Map(candidates.map(candidate => [candidate.key, candidate]));
  const included = new Set<string>();
  function row(change: DurableChange, entry: QueueEntry | null, phase: TimelineRow["phase"]): TimelineRow {
    const key = changeKey(change, queueSourceId(topicId, change));
    included.add(key);
    const candidate = byKey.get(key) ?? null;
    const problem = !candidate ? "Source unavailable; refresh before continuing."
      : !sameChange(candidate.change, change) ? "The declaration changed; reconcile this queued output before continuing."
      : ["blocked", "unavailable"].includes(candidate.state) ? candidate.note
      : phase === "pending" && candidate.state === "landed" ? "Already landed; awaiting queue reconciliation." : "";
    const status = phase === "active" ? "Acceptance in progress" : phase === "landed" ? "Landed"
      : problem ? (candidate?.state === "unavailable" || !candidate ? "Identity unavailable" : "Needs reconciliation")
      : entry?.requested_by ? (change.scope.kind === "working" ? "Queued · awaiting completion" : "Queued")
      : change.scope.kind === "working" ? "Working output" : "Declared output";
    return { key, change, candidate, entry, phase, problem, status };
  }
  const active = view?.queue.active ? row(view.queue.active.change, view.queue.active, "active") : null;
  const pending = view?.queue.pending.map(entry => row(entry.change, entry, "pending")) ?? [];
  const order = new Map(discovered.map((key, index) => [key, index]));
  const remaining = candidates.filter(candidate => !included.has(candidate.key)).sort((a, b) =>
    (order.get(a.key) ?? Infinity) - (order.get(b.key) ?? Infinity) || a.key.localeCompare(b.key));
  const history: TimelineRow[] = [];
  for (const candidate of remaining) {
    const next = row(candidate.change, null, candidate.state === "landed" ? "landed" : "pending");
    (next.phase === "landed" ? history : pending).push(next);
  }
  // A receipt can coexist with an active claim while the coordinator clears it.
  // The claim remains visible and immovable; no queue state synthesizes history.
  for (const candidate of candidates.filter(candidate => candidate.state === "landed" && included.has(candidate.key))) {
    if (!history.some(row => row.key === candidate.key)) history.push({ ...row(candidate.change, null, "landed"), problem: "" });
  }
  return { active, pending, history, canReorder: !!view && pending.every(row => !row.problem && row.candidate?.state !== "landed") };
}

export function reorderedDeclarations(rows: TimelineRow[], from: number, to: number): DurableChange[] {
  if (!Number.isInteger(from) || !Number.isInteger(to) || from < 0 || to < 0 || from >= rows.length || to >= rows.length
    || rows.some(row => row.phase !== "pending" || row.problem || !row.candidate || row.candidate.state === "landed")) throw new Error("Only available pending Changes can be reordered.");
  const order = rows.map(row => pinChange(row.change));
  order.splice(to, 0, order.splice(from, 1)[0]);
  return order;
}

export function queuedLandFeedback(target: IntegrationTarget): string {
  return `Queued for ${target.target_branch}. Waiting for successful completion, review and acceptance.${target.warnings?.length ? ` ${target.warnings.join(" ")}` : ""}`;
}
