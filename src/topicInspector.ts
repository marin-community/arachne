/**
 * Pure logic for the Topic inspector (design.md "Navigation"): the topic's
 * thread tree, per-row states, and the Landing queue. Kept out
 * of the component so it can be tested directly (mirrors chatRows.ts).
 */

import type { SessionSummary } from "./App.vue";
import { isTrackCoordinator, resolveTrack, tagValue } from "./trackPolicy.ts";

// --- Fleet row state ---------------------------------------------------------

// Loom tag semantics (weaver-core/src/tags.rs): the loud keys `attention`
// (agent self-report) and `triage` (outside assessment) carry values
// `attention` | `blocked`; absence is the calm/default state. `idle` is a
// quiet resting mark. Mirrors FleetSidebar's helpers.
export function loudTag(s: SessionSummary): { level: "attention" | "blocked" } | null {
  if (s.status === "archived") return null;
  const tags = s.branch.tags.filter((t) => t.key === "attention" || t.key === "triage");
  if (tags.some((t) => t.value === "blocked")) return { level: "blocked" };
  if (tags.some((t) => t.value === "attention") || s.pending_permissions?.length) {
    return { level: "attention" };
  }
  return null;
}

/** Human-facing reason, never a stale note attached to a cleared tag. */
export function attentionReason(s: SessionSummary): string | null {
  if (s.status === "archived") return null;
  const permission = pendingPermissionSummary(s);
  if (permission) return permission;
  const loud = loudTag(s);
  if (loud) {
    const tags = s.branch.tags.filter((t) =>
      (t.key === "attention" || t.key === "triage") && t.value === loud.level,
    );
    const note = tags.find((t) => t.note?.trim())?.note?.trim();
    return note || s.branch.description?.trim() ||
      (loud.level === "blocked" ? "Agent is blocked. Open the thread to resolve the blocker."
        : "Agent requested your input. Open the thread to review the request.");
  }
  if (s.status === "error") {
    return "Agent stopped with an error. Open the thread to inspect and retry.";
  }
  return null;
}

export function attentionLevel(s: SessionSummary): "blocked" | "attention" | "ok" {
  if (s.status === "archived") return "ok";
  return loudTag(s)?.level ?? "ok";
}

/** Label the concrete next step without assuming every open PR needs a human. */
export function attentionAction(s: SessionSummary): string {
  if (s.pending_permissions?.length) return "Review approval";
  if (attentionLevel(s) === "blocked") return "Resolve blocker";
  if (s.status === "error") return "Inspect error";
  const pr = s.branch.github;
  if (pr?.pr_state?.toLowerCase() === "open" && !pr.is_draft) return `Review PR #${pr.pr_number}`;
  return "Review request";
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

/** The composer's send button doubles as the dismiss button when the
 * thread is flagged, the agent is resting (not working), and nothing is
 * queued to send. Pure gating so the decision is testable: the component
 * supplies its own live working/queue signals (turn-in-flight, busy,
 * queued prompt, attachments uploading) and the current draft/attachments
 * so typing immediately reverts the button to Send. */
export function composerSendAsDismiss(input: {
  dismissible: boolean;
  /** Any live work or queued send keeps the button a Send. */
  working: boolean;
  /** Draft text or attachments keep the button a Send. */
  hasContent: boolean;
}): boolean {
  return input.dismissible && !input.working && !input.hasContent;
}

/** Empty composers offer Stop while a turn can be interrupted. */
export function composerSendAsStop(input: {
  interruptible: boolean;
  hasContent: boolean;
  preparing: boolean;
}): boolean {
  return input.interruptible && !input.hasContent && !input.preparing;
}

/** The quiet `idle` mark: running alone means alive, not working. */
export function isIdle(s: SessionSummary): boolean {
  return s.branch.tags.some((t) => t.key === "idle");
}

export function statusClass(s: SessionSummary): string {
  const loud = loudTag(s);
  if (s.status === "error" || loud?.level === "blocked") return "error";
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

// --- Landing candidates ------------------------------------------------------

/** Readiness is advisory and explicitly reported, never inferred from lifecycle. */
export function isReadyCandidate(s: SessionSummary): boolean {
  if (integrationResult(s) || s.branch.tags.some(t => t.key === "role" && t.value === "reviewer")) return false;
  const state = s.branch.tags.find(t => t.key === "integration_state")?.value;
  // A current failure/staleness assessment supersedes an old ready mark.
  if (state && !["true", "ready"].includes(state)) return false;
  return ["integration_ready", "integration_state"].some((key) => {
    const value = s.branch.tags.find((t) => t.key === key)?.value;
    return value === "true" || value === "ready";
  });
}

export type ThreadFactKind = "archived" | "needs-you" | "parent-help" | "error" | "conflicted" | "stale"
  | "ready" | "reviewing" | "working" | "landed" | "waiting" | "resting";

/** Advisory facts, independent of the process lifecycle and Git ancestry. */
export function threadStatus(s: SessionSummary): { kind: ThreadFactKind; label: string; detail: string } {
  const tag = (key: string) => s.branch.tags.find(t => t.key === key);
  const fact = (kind: ThreadFactKind, label: string, detail = "") => ({ kind, label, detail });
  const result = integrationResult(s);
  if (result) return fact("landed", "landed", result.note || result.value);
  if (s.status === "archived") return fact("archived", "archived");
  if (loudTag(s)) return fact("needs-you", "needs you", attentionReason(s) ?? "");
  if (tag("parent_help")?.value === "requested") return fact("parent-help", "needs parent", tag("parent_help")?.note);
  if (s.status === "error") return fact("error", "error", "Inspect the error; escalate explicitly if human action is needed.");
  const integration = tag("integration_state");
  if (integration?.value === "conflicted") return fact("conflicted", "landing conflict", integration.note);
  if (integration?.value === "stale") return fact("stale", "needs recheck", integration.note);
  if (isReadyCandidate(s)) return fact("ready", "reported ready", tag("integration_ready")?.note || integration?.note);
  if (tag("landing_request")?.value === "pending") return fact("waiting", "Land requested", "Waiting for successful wrap-up and acceptance.");
  const waiting = tag("waiting");
  if (waiting && !["false", "done", "resolved"].includes(waiting.value)) return fact("waiting", "waiting", waiting.note || waiting.value);
  const pr = s.branch.github;
  if (pr?.pr_state?.toLowerCase() === "open") {
    const checks = pr.checks?.toLowerCase();
    return fact("waiting", checks && ["pending", "queued", "in_progress"].includes(checks) ? "waiting on CI" : "waiting on PR", `PR #${pr.pr_number}`);
  }
  if (s.status === "running" && !isIdle(s)) return tag("role")?.value === "reviewer"
    ? fact("reviewing", "reviewing") : fact("working", "working");
  return fact("resting", "resting");
}

/** The recorded integration outcome tag, when Loom verified one. */
export function integrationResult(s: SessionSummary) {
  return s.branch.tags.find((t) => t.key === "integration_result");
}

export interface LandingCandidate {
  session: SessionSummary;
  state: "working" | "waiting" | "ready" | "stale" | "conflicted" | "landed";
  note: string;
}

/** Active workers and advisory reports coexist with durable acceptance outcomes.
 * Lifecycle and pending wrap-up requests never imply successful readiness. */
export function landingCandidates(rows: ThreadRow[], topicId: string): LandingCandidate[] {
  const fleet = rows.map(row => row.session);
  const out: LandingCandidate[] = [];
  for (const { session: s } of rows) {
    if (s.id === topicId || tagValue(s, "role") === "reviewer" || isTrackCoordinator(s)
      || resolveTrack(fleet, s.id)?.id !== topicId) continue;
    const result = integrationResult(s);
    if (result) {
      out.push({ session: s, state: "landed", note: result.note || result.value });
      continue;
    }
    const state = s.branch.tags.find(tag => tag.key === "integration_state");
    if (state?.value === "stale" || state?.value === "conflicted") {
      out.push({ session: s, state: state.value, note: state.note || "Reconcile and revalidate against the canonical branch." });
    } else if (isReadyCandidate(s)) {
      const ready = s.branch.tags.find(tag => ["integration_ready", "integration_state"].includes(tag.key) && ["true", "ready"].includes(tag.value));
      out.push({ session: s, state: "ready", note: ready?.note || "Reported ready; acceptance still requires review and validation." });
    } else {
      const fact = threadStatus(s);
      if (fact.kind === "working" || fact.kind === "waiting") out.push({ session: s, state: fact.kind, note: fact.detail });
      else if (s.status === "running" && !isIdle(s)) out.push({ session: s, state: "working", note: fact.detail || fact.label });
      else if (s.status !== "archived" && s.branch.branch) out.push({
        session: s, state: "waiting", note: fact.detail || `${fact.label}; Land can request wrap-up without implying readiness.`,
      });
    }
  }
  const rank = { conflicted: 0, stale: 1, ready: 2, waiting: 3, working: 4, landed: 5 };
  return out.sort((a, b) => rank[a.state] - rank[b.state] || b.session.last_activity_at.localeCompare(a.session.last_activity_at));
}
