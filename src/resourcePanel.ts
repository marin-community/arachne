// Pure row logic for the Resources panel (docs/design.md "Resource slice").
//
// The panel's live set is the spec's full resource set: the topic's
// repository, the PRs of every thread in its subtree, the GitHub issues its
// subtree works, and the checkout. Durable manifest attachments follow below
// them. Kept out of the component so it runs under plain `node --test`
// (mirrors topicInspector.ts), with structural slices of App.vue's types.

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
}

/** The slice of a topic session the checkout row needs. */
export interface CheckoutSubject {
  worktree_present: boolean;
  work_dir: string;
  branch: { branch: string; repo_root: string };
  github_repo: string | null;
}

/** A sorted, render-ready live row. `key` is unique and stable per row. */
export type LiveRow =
  | { kind: "repository"; key: string; label: string; url: string | null }
  | { kind: "pr"; key: string; pr: PanelPr; ownedByTopic: boolean }
  | { kind: "issue"; key: string; issue: PanelIssue; url: string | null; open: boolean };

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
