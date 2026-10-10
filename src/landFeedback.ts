export interface LandResult {
  local: boolean;
  landing?: {
    landed: boolean;
    target_branch: string;
    commit?: string;
    commits_squashed: number;
    primary_checkout: string;
  };
}

/** Queued requests acknowledge explicit publishing intent, not completion. */
export function landResultFeedback(result: LandResult): string {
  if (result.local && result.landing) {
    const done = result.landing;
    return done.landed
      ? `Published locally: squash of ${done.commits_squashed} commit${done.commits_squashed === 1 ? "" : "s"} into ${done.target_branch} · ${done.commit}`
      : `${done.target_branch} already contained this track's changes — nothing to do.`;
  }
  return "Publish requested. Follow the actual result in this thread.";
}
