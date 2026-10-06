import type { SessionSummary } from "./App.vue";

/** Search durable fleet metadata, including PR provenance; never imply transcript search. */
export function searchThreads(fleet: readonly SessionSummary[], query: string): SessionSummary[] {
  const words = query.toLocaleLowerCase().trim().split(/\s+/).filter(Boolean);
  return fleet.filter(s => {
    const b = s.branch;
    const haystack = [s.id, b.title, b.name, b.description, b.goal, b.branch, b.repo_root,
      s.github_repo, s.placement?.group_name, b.github?.pr_url, b.github?.pr_title,
      b.github?.pr_number && `#${b.github.pr_number}`, b.github_pr && `#${b.github_pr}`,
      ...b.tags.map(t => `${t.key} ${t.value} ${t.note}`)].filter(Boolean).join(" ").toLocaleLowerCase();
    return words.every(word => haystack.includes(word));
  }).sort((a, b) => Number(a.status === "archived") - Number(b.status === "archived") ||
    (b.last_user_message_at || b.last_activity_at).localeCompare(a.last_user_message_at || a.last_activity_at));
}
