import type { SessionSummary } from "./App.vue";

/** Mirrors the backend's ancestor routing: a live track in the same repo wins,
 * otherwise the nearest live parent in that repo. Never default to main. */
export function reviewIntegrationTarget(fleet: SessionSummary[], sourceId: string): SessionSummary | null {
  const byId = new Map(fleet.map(session => [session.id, session]));
  const byBranch = new Map(fleet.map(session => [session.branch.id, session]));
  let current = byId.get(sourceId);
  const repo = current?.branch.repo_root;
  let nearest: SessionSummary | null = null;
  const seen = new Set<string>();
  for (let depth = 0; current && depth < 32 && !seen.has(current.id); depth++) {
    seen.add(current.id);
    const parent = (current.parent_session_id ? byId.get(current.parent_session_id) : undefined)
      ?? (current.parent_id ? byBranch.get(current.parent_id) : undefined);
    if (!parent) break;
    if (parent.branch.repo_root === repo && parent.status !== "archived" && parent.branch.branch) {
      nearest ??= parent;
      if (parent.branch.tags.some(tag => tag.key === "topic" && tag.value !== "false")) return parent;
    }
    current = parent;
  }
  return nearest;
}

export function preparationRequest(isTrack: boolean, revision: string, target: SessionSummary | null, editedPaths: string[] = []): string {
  const destination = isTrack
    ? "A landing strategy and destination have not been selected yet. Inspect repository policy and report the appropriate destination options; do not assume main."
    : target
      ? `Integration destination: coordinator ${target.branch.name} (session ${target.id}), branch ${target.branch.branch}, repository ${target.branch.repo_root}. Re-verify this exact target and its current revision; do not substitute main or the primary checkout's branch.`
      : "No integration target is resolved. Identify the same-repository track coordinator before claiming readiness; do not assume main.";
  const edits = editedPaths.length ? ` Files manually saved through this review: ${editedPaths.join(", ")}. Treat these as intentional user edits; inspect their relevance and explicitly report anything excluded.` : "";
  return `Prepare this ${isTrack ? "track" : "worker"} for ${isTrack ? "landing" : "integration"}. ${revision} ${destination}${edits} Review the result, fix straightforward issues, and run the repository's required validation. Delegate review or repair where useful. Stabilize and commit the intended source changes once validation passes, preserving unrelated manual edits. Report the exact source revision, target revision, validation evidence, and git status after preparation. If any uncommitted edits remain, list the files and report preparation as incomplete; do not mark the source ready. This is agent preparation, not control-plane verified readiness: do not set integration_ready or claim a verified ready-at-SHA result. Do not integrate, land, push, or open a PR until I choose that action.`;
}
