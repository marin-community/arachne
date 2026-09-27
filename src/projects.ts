// Projects group the sidebar's Topics tab. A Project is a non-system Loom
// layout group — pure filing, with no coordinator or execution state
// (docs/design.md "User model"). Topics whose coordinator session sits in a
// non-system group render under that project's heading; everything else —
// no placement, a system group such as the Inbox, or a group that no longer
// exists — is unfiled and renders under "Ungrouped".

import type { SessionLayout, SessionSummary } from "./App.vue";

/** A project reference as seen by the UI: a layout group, or Ungrouped. */
export interface ProjectRef {
  id: string | null;
  name: string;
}

export interface ProjectSection<T> {
  id: string | null;
  name: string;
  topics: T[];
}

/** The non-system layout groups, in space-then-group rank order. */
export function layoutProjects(
  layout: SessionLayout | null | undefined,
): { id: string; name: string; order: number }[] {
  const groups: { id: string; name: string; order: number }[] = [];
  for (const space of layout?.spaces ?? []) {
    for (const group of space.groups) {
      if (group.system_key) continue; // system groups (Inboxes) aren't Projects
      groups.push({
        id: group.id,
        name: group.name,
        order: space.rank * 10000 + group.rank,
      });
    }
  }
  return groups.sort((a, b) => a.order - b.order);
}

/**
 * A topic's home project: its coordinator's placement group when that group
 * is a non-system group. System groups and unknown ids are unfiled — a
 * placement pointing at a deleted group must not strand the topic.
 */
export function topicProjectId(
  session: SessionSummary,
  projectIds: Set<string>,
): string | null {
  const groupId = session.placement?.group_id ?? null;
  return groupId && projectIds.has(groupId) ? groupId : null;
}

/**
 * File topics into project sections (layout order), with "Ungrouped" last.
 * Empty sections are kept: a project with no topics still has a heading to
 * file under and a Topics [+] affordance.
 */
export function buildProjectSections<T extends { session: SessionSummary }>(
  topics: T[],
  layout: SessionLayout | null | undefined,
): ProjectSection<T>[] {
  const projects = layoutProjects(layout);
  const ids = new Set(projects.map((project) => project.id));
  const byProject = new Map<string | null, T[]>();
  for (const project of projects) byProject.set(project.id, []);
  byProject.set(null, []);
  for (const topic of topics) {
    byProject.get(topicProjectId(topic.session, ids))!.push(topic);
  }
  const sections: ProjectSection<T>[] = projects.map((project) => ({
    id: project.id,
    name: project.name,
    topics: byProject.get(project.id)!,
  }));
  sections.push({ id: null, name: "Ungrouped", topics: byProject.get(null)! });
  return sections;
}
