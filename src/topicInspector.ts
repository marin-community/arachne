/**
 * Pure logic for the Topic inspector (design.md "Navigation"): the topic's
 * thread tree, per-row states, and the integration candidate queue. Kept out
 * of the component so it can be tested directly (mirrors chatRows.ts).
 */

import type { SessionSummary } from "./App.vue";

// --- Fleet row state ---------------------------------------------------------

// Loom tag semantics (weaver-core/src/tags.rs): the loud keys `attention`
// (agent self-report) and `triage` (outside assessment) carry values
// `attention` | `blocked`; absence is the calm/default state. `idle` is a
// quiet resting mark. Mirrors FleetSidebar's helpers.
export function loudTag(s: SessionSummary): { level: "attention" | "blocked" } | null {
  for (const key of ["attention", "triage"]) {
    const tag = s.branch.tags.find((t) => t.key === key);
    if (tag && (tag.value === "attention" || tag.value === "blocked")) {
      return { level: tag.value };
    }
  }
  if (s.pending_permissions?.length && s.status !== "archived") return { level: "attention" };
  return null;
}

/** Compact reason for a session with one or more unanswered tool approvals. */
export function pendingPermissionSummary(s: SessionSummary): string | null {
  const pending = s.pending_permissions ?? [];
  if (!pending.length) return null;
  const titles = pending.map((request) => request.title || "permission requested");
  return pending.length === 1
    ? `Approve tool use: ${titles[0]}`
    : `${pending.length} tool approvals: ${titles.join(" · ")}`;
}

/** Dismissal surface rule: a row can offer Dismiss only for TAG-raised
 * attention — the `attention` self-report or the `triage` assessment.
 * Permission-raised attention (unanswered ACP tool approvals) is not
 * dismissable: the request stays live until someone answers its options,
 * and hiding it would strand the agent mid-turn. Structural over the
 * summary and view shapes (both carry `status` + `branch.tags`). */
export function dismissibleAttention(s: {
  status: string;
  branch: { tags: { key: string; value: string }[] };
}): boolean {
  if (s.status === "archived") return false;
  return s.branch.tags.some(
    (t) =>
      (t.key === "attention" || t.key === "triage") &&
      (t.value === "attention" || t.value === "blocked"),
  );
}

/** The quiet `idle` mark: running alone means alive, not working. */
export function isIdle(s: SessionSummary): boolean {
  return s.branch.tags.some((t) => t.key === "idle");
}

export function statusClass(s: SessionSummary): string {
  const loud = loudTag(s);
  if (loud?.level === "blocked") return "error";
  if (loud?.level === "attention") return "attention";
  return "done";
}

/** The text badge for a row, or null when it should stay quiet. */
export function badgeLabel(s: SessionSummary): string | null {
  if (s.status === "archived") return "done";
  if (s.status === "orphaned") return null;
  const loud = loudTag(s);
  if (loud) return loud.level;
  if (s.status === "running") return null;
  return s.status;
}

// --- Topic thread tree -------------------------------------------------------

export interface ThreadRow {
  session: SessionSummary;
  depth: number;
  childCount: number;
  isCollapsed: boolean;
}

/**
 * The topic's coordinator + worker fleet as render rows: the topic session
 * is the root and workers nest under their parents at arbitrary depth.
 * Archived children stay visible (dimmed) — a topic's finished work is part
 * of its shape, the same rule the sidebar's Inbox tree applies.
 */
export function topicThreadRows(
  fleet: SessionSummary[],
  topicId: string,
  collapsed: ReadonlySet<string>,
): ThreadRow[] {
  const byId = new Map(fleet.map((s) => [s.id, s]));
  const byBranch = new Map(fleet.map((s) => [s.branch.id, s]));
  const parentOf = (s: SessionSummary) =>
    (s.parent_session_id ? byId.get(s.parent_session_id) : undefined) ??
    (s.parent_id ? byBranch.get(s.parent_id) : undefined);

  const root = byId.get(topicId);
  if (!root) return [];

  const children = new Map<string, SessionSummary[]>();
  for (const s of fleet) {
    const parent = parentOf(s);
    if (!parent || parent.id === s.id) continue;
    const siblings = children.get(parent.id) ?? [];
    siblings.push(s);
    children.set(parent.id, siblings);
  }

  const rows: ThreadRow[] = [];
  const seen = new Set<string>();
  const walk = (node: SessionSummary, depth: number) => {
    if (seen.has(node.id)) return; // cycle guard
    seen.add(node.id);
    const isCollapsed = collapsed.has(node.id);
    const kids = (children.get(node.id) ?? [])
      .slice()
      .sort((a, b) => b.last_activity_at.localeCompare(a.last_activity_at));
    rows.push({ session: node, depth, childCount: kids.length, isCollapsed });
    if (!isCollapsed) kids.forEach((kid) => walk(kid, depth + 1));
  };
  walk(root, 0);
  return rows;
}

// --- Integration candidates --------------------------------------------------

/**
 * Verified candidate state from Loom. A worker is ready ONLY when it carries
 * an explicit readiness tag — design.md: "Ready to Integrate requires
 * explicit verified candidate state from Loom; a stopped or sleeping worker
 * is not sufficient evidence." Worker lifecycle (stopped/sleeping/archived)
 * never implies readiness.
 */
export function isReadyCandidate(s: SessionSummary): boolean {
  return ["integration_ready", "integration_state"].some((key) => {
    const value = s.branch.tags.find((t) => t.key === key)?.value;
    return value === "true" || value === "ready";
  });
}

/** The recorded integration outcome tag, when Loom verified one. */
export function integrationResult(s: SessionSummary) {
  return s.branch.tags.find((t) => t.key === "integration_result");
}

export interface IntegrationCandidate {
  session: SessionSummary;
  state: "ready" | "integrated";
  /** Readiness revision / outcome reference from the tag. */
  note: string;
}

/**
 * The Topic-owned candidate queue from a topic's thread rows: verified
 * ready candidates first, then integrated outcomes. The topic coordinator
 * itself is never a candidate.
 */
export function integrationCandidates(
  rows: ThreadRow[],
  topicId: string,
): IntegrationCandidate[] {
  const out: IntegrationCandidate[] = [];
  for (const { session: s } of rows) {
    if (s.id === topicId) continue;
    const result = integrationResult(s);
    if (result) {
      out.push({ session: s, state: "integrated", note: result.note || result.value });
      continue;
    }
    if (isReadyCandidate(s)) {
      // The readiness tag's note carries the recorded target revision
      // ("clean against <topic>@<rev>", spec "Candidate readiness").
      const ready = s.branch.tags.find(
        (t) =>
          (t.key === "integration_ready" || t.key === "integration_state") &&
          (t.value === "true" || t.value === "ready"),
      );
      out.push({ session: s, state: "ready", note: ready?.note || ready?.value || "" });
    }
  }
  return out.sort((a, b) => {
    if (a.state !== b.state) return a.state === "ready" ? -1 : 1;
    return b.session.last_activity_at.localeCompare(a.session.last_activity_at);
  });
}
