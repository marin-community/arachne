import { splitScopedId } from "./backendIdentity.ts";
import type { SessionSummary } from "./App.vue";
import { canonicalTopicBranch, resolveTrack } from "./trackPolicy.ts";
import type { DurableChange } from "./durableChange.ts";

export function selectedChangeInstructions(change: DurableChange): string {
  return `Selected Durable Change v1 declaration: ${JSON.stringify(change)}. Re-read the source-owned change:${change.id} tag and verify this exact declaration, original source identity, scope, completion and per-Change acceptance receipt. Stop on changed or conflicting metadata; require reselection and fresh review. Review only the declared ${change.scope.kind === "range" ? "immutable base_oid→head_oid range and literal paths; exclude live checkout edits" : "paths at the pinned HEAD and worktree_version; this working snapshot is not readiness"}. Do not expand to sibling outputs, inherited changes or the cumulative branch. Re-read the live same-repository owning Track, canonical ref, initialization and target revision before any acceptance. Preserve foreign edits. Report the qualified Change identity and exact scope in findings.`;
}

/** Reviews use the same-repository canonical Track, never an incidental parent. */
export function reviewIntegrationTarget(fleet: SessionSummary[], sourceId: string): SessionSummary | null {
  return resolveTrack(fleet, sourceId);
}

/** Optional preparation-only action from Review. Land includes its own preparation. */
export function preparationRequest(isTrack: boolean, revision: string, target: SessionSummary | null, editedPaths: string[] = [], selected?: DurableChange | null): string {
  if (!isTrack && !selected) return "No Change is selected. Do not prepare this source for landing; select an explicit output in the inspector first. Do not land, publish, push, or open a PR until I choose that action.";
  const destination = isTrack
    ? `Canonical publish source: ${target ? canonicalTopicBranch(target) : "resolve the Track canonical branch"}. A publishing strategy and destination have not been selected yet. Inspect repository policy and report the appropriate destination options; do not assume main.`
    : target
      ? `Landing destination: coordinator ${target.branch.name} (session ${splitScopedId(target.id)?.id ?? target.id}), branch ${canonicalTopicBranch(target)}, repository ${target.branch.repo_root}. Re-verify this exact target and its current revision; do not substitute main or the primary checkout's branch.`
      : "No landing target is resolved. Identify the same-repository track coordinator before claiming readiness; do not assume main.";
  const edits = !isTrack && editedPaths.length ? ` Files manually saved through this review: ${editedPaths.join(", ")}. Treat these as intentional user edits; inspect their relevance and explicitly report anything excluded.` : "";
  const sourceScope = isTrack
    ? "Inspect and validate committed canonical accepted state in an isolated checkout. Coordinator scratch commits and staged, unstaged or untracked edits are outside the publishing source; keep preparation focused on accepted state, preserving unrelated manual edits. Do not incorporate scratch work into preparation. Report any accepted-state repair separately."
    : selected
      ? `${selectedChangeInstructions(selected)} Prepare only this selected output, preserving unrelated manual edits. A committed range remains immutable when the source branch advances. For working output, commit only its declared paths after validation, retain its ID and original identity, record the finalized range and require fresh scope review. Do not prepare unselected source changes.`
      : "No Change is selected. Do not prepare this source for landing; select an explicit output in the inspector first.";
  return `Prepare this ${isTrack ? "track" : "worker"} for ${isTrack ? "publishing" : "landing"}. ${revision} ${destination}${edits} Review the result, fix straightforward issues, and run the repository's required validation. Delegate review or repair where useful through visible Loom child sessions, including reviewers. ${sourceScope} Re-verify the exact source branch and revision and the exact target before reporting preparation. Report the exact source revision, target revision, validation evidence, and git status after preparation. If any intended source edits remain uncommitted, list the files and report preparation as incomplete; do not mark the source ready. This is optional agent preparation, not control-plane verified readiness: do not set integration_ready or claim a verified ready-at-SHA result. Do not land, publish, push, or open a PR until I choose that action.`;
}

/** Independent reviews compare worker changes with the canonical accepted ref. */
export function independentReviewRequest(revision: string, target: SessionSummary | null, isTrack = false, selected?: DurableChange | null): string {
  const destination = target
    ? `${isTrack ? "Canonical accepted source" : "Compare against canonical accepted branch"}: ${canonicalTopicBranch(target)} in ${target.branch.repo_root} (Track ${splitScopedId(target.id)?.id ?? target.id}). Re-read and verify this exact ref and revision; do not substitute its scratch branch or main.`
    : "No same-repository Track is resolved. Resolve the canonical accepted ref before comparing changes; do not assume main.";
  const sourceScope = isTrack
    ? " Fork the committed canonical accepted ref and compare with its recorded publishing base. Coordinator scratch commits and checkout edits are outside this review; report the exact accepted source and publishing base revisions."
    : selected
      ? ` ${selectedChangeInstructions(selected)} Inspect the pinned scope independently and report its compatibility with the current canonical target separately. Do not replace the selected range with a comparison of the current branch.`
      : " This is an unselected inspection only, with no preparation or landing authorization. Inspect source changes read-only and report coverage separately.";
  return `Review this thread's changes for correctness, regressions, and missing validation. ${revision} ${destination}${sourceScope} Act independently in a fresh review thread. Report actionable findings with file locations and distinguish verified checks from unverified claims. Do not land, publish, push, or open a PR as part of this review.`;
}

/** Preview evidence names its actual Git source and comparison, including scope. */
export function reviewPreviewEvidence(changes: {
  source_ref: string;
  accepted_source: boolean;
  head_oid: string | null;
  base: { state: string; reference: string; oid?: string };
} | null, selected?: DurableChange | null): string {
  if (!changes) return "No preview snapshot is available; resolve and verify the actual review source and comparison.";
  if (changes.base.state !== "available") {
    return `Preview source: ${changes.source_ref}; comparison ${changes.base.reference} was unavailable. No Git diff or source/target revisions were verified by this preview. Perform the actual comparison on the repository host and report its exact refs, revisions and coverage.`;
  }
  const source = `The preview showed ${changes.source_ref}${changes.head_oid ? ` at ${changes.head_oid}` : ""}`;
  const target = ` compared with ${changes.base.reference} at ${changes.base.oid}`;
  return `${source}${target}. ${selected
    ? selected.scope.kind === "range" ? "It covered only the pinned range and declared literal paths; live checkout edits are excluded." : "It covered the declared working paths at the pinned HEAD and worktree version; completion is unverified."
    : changes.accepted_source
    ? "It covered committed canonical accepted state only; coordinator checkout edits are excluded."
    : "It included worker checkout edits; re-read the original worker checkout and report current coverage."} Re-read both refs and revisions before reviewing or preparing.`;
}
