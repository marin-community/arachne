import type { SessionSummary } from "./App.vue";
import { attentionAction, attentionLevel, attentionReason } from "./topicInspector.ts";
import { topicRootOf } from "./topic-view.ts";

export interface AttentionNotice {
  key: string;
  sessionId: string;
  topicId: string;
  title: string;
  reason: string;
  action: string;
  level: "attention" | "blocked";
}

/** Derive notices from Loom snapshots. Activity alone never creates a notice.
 * The identity includes the request, not last_activity_at, so streaming and
 * reconnects do not repeatedly announce an unchanged decision. */
export function attentionNotices(fleet: readonly SessionSummary[]): AttentionNotice[] {
  return fleet.filter((s) => attentionLevel(s) !== "ok")
    .slice().sort((a, b) => {
      const priority = (s: SessionSummary) => s.pending_permissions?.length ? 0
        : attentionLevel(s) === "blocked" ? 1 : 2;
      return priority(a) - priority(b) || b.last_activity_at.localeCompare(a.last_activity_at);
    }).map((s) => {
      const reason = attentionReason(s)!;
      const level = attentionLevel(s) as "attention" | "blocked";
      const topic = topicRootOf(fleet, s.id) ?? s;
      const title = s.branch.title || s.branch.name;
      const topicTitle = topic.branch.title || topic.branch.name;
      const requests = (s.pending_permissions ?? []).map((p) => p.request_id).sort();
      const tags = s.branch.tags.filter((t) =>
        (t.key === "attention" || t.key === "triage") &&
        (t.value === "attention" || t.value === "blocked"),
      ).map((t) => [t.key, t.value, t.note, t.set_at]).sort();
      return {
        key: JSON.stringify([s.id, level, reason, requests, tags]),
        sessionId: s.id,
        topicId: topic.id,
        title: topic.id === s.id ? title : `${topicTitle} · ${title}`,
        reason,
        action: attentionAction(s),
        level,
      };
    });
}

/** Resolved requests can notify again if they recur, even with the same text. */
export function retainActiveDismissals(
  dismissed: ReadonlySet<string>, notices: readonly AttentionNotice[],
): Set<string> {
  const active = new Set(notices.map((n) => n.key));
  return new Set([...dismissed].filter((key) => active.has(key)));
}
