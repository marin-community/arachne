import type { SessionLayout } from "./App.vue";

export interface ProjectResourceDraft {
  kind: string;
  title: string;
  repository: string;
  reference: string | null;
  path: string | null;
  url: string | null;
}

export interface ProjectBinding extends ProjectResourceDraft {
  id: string;
}

export interface ProjectBindingsView {
  bindings: ProjectBinding[];
  revision: number;
}

export interface ProjectCreationDraft {
  project: { id: string; name: string } | null;
  bindings: ProjectBinding[];
  savedRepo?: string;
}

type Invoke = <T>(command: string, args: Record<string, unknown>) => Promise<T>;

/** Save queued resources, retaining the created group and unsaved entries on failure. */
export async function saveNewProject(
  draft: ProjectCreationDraft,
  options: { name: string; repo: string; layout: SessionLayout | null; isActive?: () => boolean },
  invoke: Invoke,
  onLayout: (layout: SessionLayout) => void,
  onBindings: (view: ProjectBindingsView) => void,
): Promise<{ id: string; name: string }> {
  const name = options.name.trim();
  const repo = options.repo.trim();
  const checkActive = () => {
    if (options.isActive && !options.isActive()) throw new Error("Project setup was closed.");
  };
  checkActive();
  if (!name) throw new Error("Enter a project name.");
  if (draft.bindings.length && !repo) throw new Error("Choose a repository for the bound resources.");
  if (draft.savedRepo && draft.savedRepo !== repo) {
    throw new Error("Finish saving resources in the original repository before changing project defaults.");
  }

  if (!draft.project) {
    const spaces = options.layout?.spaces ?? [];
    const home = spaces.find((s) => s.system_key === "user" || s.name === "User")
      ?? spaces.find((s) => s.groups.some((g) => !g.system_key)) ?? spaces[0];
    if (!home) throw new Error("No space available to create the project in.");
    const existingIds = new Set(home.groups.map((g) => g.id));
    const layout = await invoke<SessionLayout>("create_group", { spaceId: home.id, name });
    const group = layout.spaces.find((s) => s.id === home.id)?.groups
      .find((g) => !existingIds.has(g.id) && g.name === name && !g.system_key);
    if (!group) throw new Error("The created project was missing from the returned layout.");
    draft.project = { id: group.id, name: group.name };
    checkActive();
    onLayout(layout);
  }

  if (draft.bindings.length) {
    const projectId = draft.project.id;
    let view = await invoke<ProjectBindingsView>("project_bindings", { projectId, repo });
    checkActive();
    onBindings(view);
    while (draft.bindings.length) {
      const { id: _draftId, ...resource } = draft.bindings[0];
      view = await invoke<ProjectBindingsView>("add_project_binding", {
        projectId, repo, resource, expectedRevision: view.revision,
      });
      // Remove only acknowledged entries, so a retry never creates a second
      // project or repeats bindings that have already been saved.
      draft.bindings.shift();
      draft.savedRepo = repo;
      checkActive();
      onBindings(view);
    }
  }
  return draft.project;
}
