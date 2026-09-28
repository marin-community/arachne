// Pure row logic for the Resources panel (docs/design.md "Resource slice").
//
// The panel's live set is the spec's full resource set: the topic's
// repository, the PRs of every thread in its subtree, the GitHub issues its
// subtree works, and the checkout. Durable manifest attachments follow below
// them. Kept out of the component so it runs under plain `node --test`
// (mirrors topicInspector.ts), with structural slices of App.vue's types.

/** Where an attached-resource row came from (design.md: show each
 *  binding's origin — Project, Topic, or Thread). */
export type ResourceOrigin = "project" | "topic";

/** A slice of the `topic_resources` reply's effective rows: the flat
 *  binding plus origin. `hidden` is true only for hidden inherited
 *  bindings (kept for the restore affordance). */
export interface EffectiveRow {
  id: string;
  kind: string;
  title: string;
  repository: string;
  reference: string | null;
  path: string | null;
  url: string | null;
  origin: ResourceOrigin;
  hidden?: boolean;
}

/** The rows the mention menus offer: effective rows only — hidden
 *  inherited bindings never resolve (design.md: a Topic can hide an
 *  inherited resource without deleting it from the Project). */
export function mentionableRows(rows: EffectiveRow[]): EffectiveRow[] {
  return rows.filter((row) => !row.hidden);
}

/** Origin label for the panel's attached list: what came from the
 *  Project vs the Topic's own addition (design.md: the user can tell). */
export function originLabel(origin: ResourceOrigin): string {
  return origin === "project" ? "project" : "topic";
}

/** The slice of an `issues.board` row the panel renders. */
export interface PanelIssue {
  id: number;
  repo_root: string;
  github_repo: string | null;
  github_issue: number | null;
  claimed_branch: string | null;
  source_branch: string | null;
  title: string;
  status: string;
}

/** The slice of a PR row (`TopicPrRow` from `topic_resources`). */
export interface PanelPr {
  session_id: string;
  session_name: string;
  pr_number: number;
  pr_url: string;
  pr_state: string;
  pr_title: string;
  is_draft: boolean;
  review_decision: string | null;
  checks: string | null;
  mergeable: string | null;
}

/** The slice of a topic session the checkout row needs. */
export interface CheckoutSubject {
  worktree_present: boolean;
  work_dir: string;
  branch: { branch: string; repo_root: string };
  github_repo: string | null;
}

/** The `pr_status` reply for an attached PR URL: the same fields a live
 * row gets from loom's snapshot, so both render the same light. */
export interface AttachedPrStatus {
  state: string;
  mergeable: string | null;
  checks: string | null;
  title: string | null;
}

/** A `PanelPr`-shaped view over an attached PR resource, so `prLight` and the
 * tooltip work on attached rows unchanged. */
export function attachedAsPanelPr(url: string, status: AttachedPrStatus): PanelPr {
  return {
    session_id: "attached",
    session_name: "attached",
    pr_number: prNumberFromUrl(url) ?? 0,
    pr_url: url,
    pr_state: status.state,
    pr_title: status.title ?? "Pull request",
    is_draft: false,
    review_decision: null,
    checks: status.checks,
    mergeable: status.mergeable,
  };
}

/** `https://github.com/<owner>/<repo>/pull/<n>` → n. */
export function prNumberFromUrl(url: string): number | null {
  const m = url.match(/^https:\/\/(?:www\.)?github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)/);
  return m ? Number(m[3]) : null;
}

/** A sorted, render-ready live row. `key` is unique and stable per row. */
export type LiveRow =
  | { kind: "repository"; key: string; label: string; url: string | null }
  | { kind: "pr"; key: string; pr: PanelPr; ownedByTopic: boolean; light: PrLight }
  | { kind: "issue"; key: string; issue: PanelIssue; url: string | null; open: boolean };

/**
 * The PR row's status light — the same read a human does on the PR page.
 * Merged is a win (purple); closed is simply done, so it shows no dot at all.
 * For open PRs, red beats yellow beats green: a merge conflict or failing CI
 * is actionable now, so it wins over in-progress; a green PR also requires the
 * merge fit to be known (GitHub briefly reports UNKNOWN while it computes).
 */
export type PrLight = "green" | "yellow" | "red" | "purple" | null;

export function prLight(pr: PanelPr): PrLight {
  if (pr.pr_state === "MERGED") return "purple";
  if (pr.pr_state === "CLOSED") return null;
  if (pr.mergeable === "CONFLICTING") return "red";
  if (pr.checks === "failing") return "red";
  if (pr.checks === "pending") return "yellow";
  if (pr.pr_state === "OPEN" && pr.checks === "passing" && pr.mergeable === "MERGEABLE") {
    return "green";
  }
  // No checks on the PR, or GitHub hasn't decided mergeability yet: nothing
  // is known to be wrong, but not known to be good either — in progress.
  return "yellow";
}

/**
 * The GitHub URL for an issue, when it carries a repo + number link.
 * Loom issues can be purely local, so absence is normal — the row then just
 * has no link, not an error.
 */
export function issueUrl(issue: PanelIssue): string | null {
  const slug = issue.github_repo;
  return slug && issue.github_issue && issue.github_issue > 0
    ? `https://github.com/${slug}/issues/${issue.github_issue}`
    : null;
}

/** Repository label: the managed slug, else the repo root's last two parts. */
export function repoLabel(topic: CheckoutSubject): string {
  if (topic.github_repo) return topic.github_repo;
  const root = topic.branch.repo_root || "";
  return root ? root.split("/").filter(Boolean).slice(-2).join("/") : "";
}

/** The repository's GitHub URL, or null when it is not a managed repo. */
export function repoUrl(topic: CheckoutSubject): string | null {
  const slug = topic.github_repo;
  return slug && /^[\w.-]+\/[\w.-]+$/.test(slug) ? `https://github.com/${slug}` : null;
}

/**
 * The live rows in render order: repository, PRs (the topic's own first,
 * workers' labeled with their thread), then issues (open before closed, both
 * ordered by id). Capped so a very active topic cannot overflow the panel —
 * PRs at 8 and issues at 12, the same bound the manifest's mention picker
 * keeps.
 */
export function liveRows(
  topic: CheckoutSubject,
  topicId: string,
  prs: PanelPr[],
  issues: PanelIssue[],
): LiveRow[] {
  const rows: LiveRow[] = [
    { kind: "repository", key: "repository", label: repoLabel(topic), url: repoUrl(topic) },
  ];
  const sortedPrs = [...prs].sort(
    (a, b) =>
      Number(b.session_id === topicId) - Number(a.session_id === topicId) ||
      a.session_name.localeCompare(b.session_name),
  );
  for (const pr of sortedPrs.slice(0, 8)) {
    rows.push({
      kind: "pr",
      key: `pr:${pr.session_id}:${pr.pr_number}`,
      pr,
      ownedByTopic: pr.session_id === topicId,
      light: prLight(pr),
    });
  }
  const sortedIssues = [...issues].sort(
    (a, b) =>
      Number(a.status === "open" ? 0 : 1) - Number(b.status === "open" ? 0 : 1) || a.id - b.id,
  );
  for (const issue of sortedIssues.slice(0, 12)) {
    rows.push({
      kind: "issue",
      key: `issue:${issue.id}`,
      issue,
      url: issueUrl(issue),
      open: issue.status === "open",
    });
  }
  return rows;
}
