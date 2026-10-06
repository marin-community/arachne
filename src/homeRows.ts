// Home row names. On the aggregate Topics home every row names its Project
// and parent Topic before the row's own title (docs/design.md: "With no
// Project or Topic selected … Every row names its Project and parent Topic
// and opens its Thread"). Scoped views prefix less: a Topic home shows the
// row's title alone, a Project home keeps the parent Topic (all rows share
// the project, which the dashboard heading already names).
import type { SessionSummary } from "./App.vue";
import { isStandalone } from "./threadKind.ts";

/** A home row's title: the row's Project · Topic · row name. */
export function homeRowTitle(
  session: SessionSummary,
  root: SessionSummary,
  projectName: string,
  scope: { topic: boolean; project: boolean },
): string {
  if (root.id === session.id && isStandalone(session)) {
    const name = session.branch.title || session.branch.name;
    return scope.topic || scope.project ? name : `${projectName} · ${name}`;
  }
  const rowName = root.id === session.id ? "Coordinator thread" : session.branch.title || session.branch.name;
  if (scope.topic) return rowName;
  const title = root.branch.title || root.branch.name;
  const names = scope.project ? [title, rowName] : [projectName, title, rowName];
  return names.join(" · ");
}
