/** Durable Loom policy and canonical accepted refs, shared by all Track UI. */
export interface PolicySession {
  id: string;
  status: string;
  parent_session_id?: string | null;
  parent_id?: string | null;
  branch: {
    id: string;
    branch: string;
    repo_root: string;
    tags: { key: string; value: string; note?: string }[];
  };
}

export type AutolandPolicy = "inherit" | "on" | "off";
export const tagValue = (session: Pick<PolicySession, "branch">, key: string) =>
  session.branch.tags.find(tag => tag.key === key)?.value;

export function isTrackCoordinator(session: PolicySession): boolean {
  const marker = tagValue(session, "topic");
  return marker === "true" || (marker !== "false" && !session.parent_session_id && !session.parent_id);
}

export function canonicalTopicBranch(session: Pick<PolicySession, "branch">): string {
  // A legacy Track keeps its existing accepted branch. Never migrate it implicitly.
  return tagValue(session, "track_topic_branch") ?? session.branch.branch;
}

export function resolveTrack<T extends PolicySession>(fleet: readonly T[], sourceId: string): T | null {
  let current = fleet.find(session => session.id === sourceId);
  const repo = current?.branch.repo_root;
  if (!repo) return null;
  const seen = new Set<string>();
  while (current && !seen.has(current.id)) {
    seen.add(current.id);
    if (current.branch.repo_root === repo && current.status !== "archived" && isTrackCoordinator(current)) return current;
    const parentId = current.parent_session_id;
    const branchId = current.parent_id;
    current = (parentId ? fleet.find(session => session.id === parentId) : undefined)
      ?? (branchId ? fleet.find(session => session.branch.id === branchId) : undefined);
  }
  return null;
}

export function trackAutoland(session: PolicySession): "on" | "off" {
  return tagValue(session, "track_autoland") === "on" ? "on" : "off";
}
export function threadAutoland(session: PolicySession): AutolandPolicy {
  const value = tagValue(session, "thread_autoland");
  return value === "on" || value === "off" ? value : "inherit";
}
export function effectiveAutoland(source: PolicySession, track: PolicySession | null): "on" | "off" {
  if (!track || source.branch.repo_root !== track.branch.repo_root) return "off";
  if (source.id === track.id) return trackAutoland(track);
  const policy = threadAutoland(source);
  return policy === "inherit" ? trackAutoland(track) : policy;
}

export function inferTopicBranch(title: string, task = ""): string {
  const slug = (title.trim() || task.trim()).replace(/[A-Z]/g, char => char.toLowerCase())
    .replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "track";
  return `arachne/tracks/${slug}`;
}

/** Empty requests backend inference and unique disambiguation. */
export function topicBranchError(branch: string): string | null {
  if (!branch) return null;
  if (branch === "HEAD" || branch === "@" || /^refs\//.test(branch)) return "Choose a local branch name outside reserved ref forms.";
  if (branch.startsWith("-") || branch.endsWith(".") || branch.includes("..") || branch.includes("@{")
    || /[\x00-\x20\x7f~^:?*\[\\]/.test(branch)
    || branch.split("/").some(part => !part || part.startsWith(".") || part.endsWith(".lock"))) {
    return "Enter a valid local Git branch name (no spaces, ref expressions, or empty path segments).";
  }
  return null;
}

/** Ref validity differs from an empty draft, which requests automatic inference. */
export function isValidTopicBranch(branch: string): boolean {
  return branch !== "" && topicBranchError(branch) === null;
}

/** Pass inference through to the backend; only root Track launches set this field. */
export function launchTopicBranch(branch: string | undefined, asTrack: boolean): string | null {
  if (!asTrack || !branch) return null;
  const error = topicBranchError(branch);
  if (error) throw new Error(error);
  return branch;
}

export interface IntegrationTarget {
  coordinator_id: string;
  target_branch: string;
  coordinator_name: string;
  warnings?: string[];
}

export function threadLandFeedback(target: IntegrationTarget): string {
  return `Land requested via ${target.coordinator_name} into ${target.target_branch}. Waiting for successful wrap-up and acceptance.${target.warnings?.length ? ` ${target.warnings.join(" ")}` : ""}`;
}
