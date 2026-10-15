<script setup lang="ts">
import { ref, reactive, computed, watch, onMounted, onUnmounted } from "vue";
import { useBackendInvoke } from "../backendInvoke";
import type { BackendFleet } from "../backendFleet";
import type { SessionLayout } from "../App.vue";
import { saveNewProject, type ProjectBinding, type ProjectBindingsView, type ProjectCreationDraft } from "../projectCreation";
import { readProjectDefaults, saveProjectDefaults, recentRepositories } from "../launchDefaults";
import RepoBaseFields from "./RepoBaseFields.vue";
import AttachResourceMenu, { type AttachDraft } from "./AttachResourceMenu.vue";
import { readDraft, DEFAULT_REPO } from "../newTopicDraft";

// Manage a project's resource bindings directly (design.md "Project defaults
// and resource inheritance"): bind a design document, file, repository, or a
// PR/issue URL to the project, and every new Topic in it inherits them —
// unless its creation form unchecks one or the topic later hides it from
// the Resources panel. The bindings live in the project (`project_bindings`
// → the repo-shared `arachne-projects` store), never copied onto topics.

const props = defineProps<{
  backendUrl: string;
  backends: BackendFleet[];
  available: boolean;
  project?: { id: string; name: string } | null;
  layout?: SessionLayout | null;
}>();

const emit = defineEmits<{
  (e: "backend", url: string): void;
  (e: "close"): void;
  (e: "created", project: { id: string; name: string }): void;
  (e: "layout", layout: SessionLayout): void;
}>();

const invoke = useBackendInvoke();
const preferenceStorage = {
  getItem: (key: string) => key === "loomUrl" ? props.backendUrl : localStorage.getItem(key),
  setItem: (key: string, value: string) => localStorage.setItem(key, value),
  removeItem: (key: string) => localStorage.removeItem(key),
};

const creation = reactive<ProjectCreationDraft>({ project: null, bindings: [] });
const creating = computed(() => !props.project);
const project = computed(() => props.project ?? creation.project);
const name = ref("");
const nameInput = ref<HTMLInputElement | null>(null);
const projectName = computed(() => project.value?.name || name.value.trim() || "this project");
const visibleBindings = computed(() => [...bindings.value, ...creation.bindings]);

// The store is per-repo (`arachne-projects` lives in the repo's shared
// artifact space), so bindings are addressed by project id + repo. The
// field defaults like the new-track chat (the remembered draft's repo, else
// the app default) and stays fully editable — any slug or path loom knows.
const repo = ref("");
const base = ref("");

const bindings = ref<ProjectBinding[]>([]);
const revision = ref(0);
const loading = ref(false);
const error = ref("");
const saving = ref(false);
const defaultsSaved = ref(false);
let active = true;
onUnmounted(() => { active = false; });
function saveDefaults() {
  if (!active || !project.value) return;
  saveProjectDefaults(localStorage, project.value.id, { repo: repo.value.trim(), base: base.value.trim() });
  defaultsSaved.value = true;
}
watch([repo, base], () => { defaultsSaved.value = false; });

// --- Bindings load --------------------------------------------------------

let loadRequest = 0;
async function load() {
  const request = ++loadRequest;
  const projectId = project.value?.id;
  const repoInput = repo.value.trim();
  if (!projectId || !repoInput) return;
  loading.value = true;
  error.value = "";
  try {
    const view = await invoke<{ bindings: ProjectBinding[]; revision: number }>("project_bindings", {
      projectId,
      repo: repoInput,
    });
    // Guard a project switch racing the reply.
    if (request !== loadRequest || projectId !== project.value?.id || repoInput !== repo.value.trim()) return;
    bindings.value = view.bindings ?? [];
    revision.value = view.revision ?? 0;
  } catch (e: any) {
    if (request !== loadRequest || repoInput !== repo.value.trim()) return;
    bindings.value = [];
    error.value = e?.message ?? String(e);
  } finally {
    if (request === loadRequest) loading.value = false;
  }
}

// Managed repos fill the default; loading failures leave the field free.
async function seedRepoDefault() {
  const saved = project.value ? readProjectDefaults(localStorage, project.value.id) : null;
  if (saved) { repo.value = saved.repo; base.value = saved.base; return; }
  try {
    const parsed = readDraft(preferenceStorage);
    repo.value = parsed?.repo?.trim() || recentRepositories(preferenceStorage)[0] || DEFAULT_REPO;
  } catch {
    repo.value = DEFAULT_REPO;
  }
}

onMounted(() => {
  void seedRepoDefault();
  if (creating.value) nameInput.value?.focus();
});

// A repo change swaps to that repo's store.
let loadTimer: ReturnType<typeof setTimeout> | undefined;
watch(repo, () => {
  loadRequest++;
  bindings.value = [];
  revision.value = 0;
  error.value = "";
  if (creating.value) return;
  loading.value = !!repo.value.trim();
  clearTimeout(loadTimer);
  loadTimer = setTimeout(() => void load(), 300);
});
onUnmounted(() => { loadRequest++; clearTimeout(loadTimer); });

// --- Add / remove bindings ------------------------------------------------

// Repository bindings pick their own repo (design.md: multiple
// repositories per project) — the managed list, so the binding names a real
// repo rather than a guess.
const managedRepos = ref<string[]>([]);
async function loadManagedRepos() {
  try {
    const repos = await invoke<{ slug: string }[] | null>("managed_repos");
    managedRepos.value = Array.isArray(repos) ? repos.map((r) => r.slug) : [];
  } catch {
    managedRepos.value = [];
  }
}
loadManagedRepos();

// The GitHub search needs a repo to search: the repo field's value when
// it carries a slug, else the managed list's first entry.
const ghRepo = computed(() => {
  const repoInput = repo.value.trim();
  if (repoInput && !repoInput.startsWith("/") && repoInput.includes("/")) return [repoInput];
  const fallback = managedRepos.value[0];
  return fallback ? [fallback] : [];
});

/** Attach the menu's chosen draft as a project binding (design.md: every
 *  new track in the project inherits what you bind here). The menu hands
 *  back a complete draft; this stamps the pieces only the project context
 *  knows — the store's repo root for file/GitHub rows. */
async function onAttachDraft(draft: AttachDraft) {
  const projectId = project.value?.id;
  const repoInput = repo.value.trim();
  if (!props.available || !repoInput || saving.value) return;
  const resource = draft.kind === "github"
    ? { kind: draft.kind === "github" ? (draft.url!.includes("/pull/") ? "pull_request" : "issue") : draft.kind, title: draft.title, repository: "", reference: null, path: null, url: draft.url }
    : draft.kind === "repository"
      ? { kind: "repository", title: draft.title, repository: draft.repository, reference: null, path: null, url: null }
      : { kind: draft.kind, title: draft.title, repository: "", reference: null, path: draft.path, url: null };
  if (creating.value) {
    creation.bindings.push({ id: crypto.randomUUID(), ...resource });
    return;
  }
  if (!projectId) return;
  saving.value = true;
  error.value = "";
  try {
    const view = await invoke<{ bindings: ProjectBinding[]; revision: number }>("add_project_binding", {
      projectId,
      repo: repoInput,
      resource,
      expectedRevision: revision.value,
    });
    bindings.value = view.bindings ?? [];
    revision.value = view.revision ?? 0;
  } catch (e: any) {
    // Conflict or failure: reload so the next edit starts from truth.
    await load();
    error.value = e?.message ?? String(e);
  } finally {
    saving.value = false;
  }
}

async function remove(binding: ProjectBinding) {
  if (!props.available || saving.value || loading.value) return;
  const pendingIndex = creation.bindings.findIndex((b) => b.id === binding.id);
  if (pendingIndex !== -1) {
    creation.bindings.splice(pendingIndex, 1);
    return;
  }
  const projectId = project.value?.id;
  const repoInput = repo.value.trim();
  if (!projectId || !repoInput) return;
  saving.value = true;
  error.value = "";
  try {
    const view = await invoke<{ bindings: ProjectBinding[]; revision: number }>("remove_project_binding", {
      projectId,
      repo: repoInput,
      bindingId: binding.id,
      expectedRevision: revision.value,
    });
    bindings.value = view.bindings ?? [];
    revision.value = view.revision ?? 0;
  } catch (e: any) {
    await load();
    error.value = e?.message ?? String(e);
  } finally {
    saving.value = false;
  }
}

async function create() {
  if (!props.available || saving.value || !name.value.trim()) return;
  saving.value = true;
  error.value = "";
  try {
    const savedProject = await saveNewProject(
      creation,
      { name: name.value, repo: repo.value, layout: props.layout ?? null, isActive: () => active },
      invoke,
      (layout) => emit("layout", layout),
      (view: ProjectBindingsView) => {
        bindings.value = view.bindings ?? [];
        revision.value = view.revision ?? 0;
      },
    );
    saveDefaults();
    emit("created", savedProject);
  } catch (e: any) {
    error.value = e?.message ?? String(e);
    if (creation.project) saveDefaults();
  } finally {
    saving.value = false;
  }
}

// --- Sheet chrome ---------------------------------------------------------

function onKeydown(event: KeyboardEvent) {
  if (event.defaultPrevented) return;
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    if (!saving.value) emit("close");
  }
}
</script>

<template>
  <section id="project-sheet" class="new-thread-sheet" :aria-label="creating ? 'New project' : 'Project settings'" @keydown="onKeydown">
    <div class="nts-inner">
      <header class="nts-head">
        <h1>{{ creating ? 'New project' : 'Project settings' }}</h1>
        <span v-if="!creating" class="nts-project" :title="`Bindings every new track in ${projectName} inherits`">{{ projectName }}</span>
        <button class="nts-close" :disabled="saving" title="Close (esc)" aria-label="Close project settings" @click="emit('close')">✕</button>
      </header>

      <label class="nts-field">
        <span class="nts-field-name">Backend</span>
        <select v-if="creating" :value="backendUrl" :disabled="saving || !!creation.project || !!creation.bindings.length" @change="emit('backend', ($event.target as HTMLSelectElement).value)">
          <option v-for="backend in backends" :key="backend.url" :value="backend.url">{{ backend.name }} · {{ backend.url }}{{ backend.available ? '' : ' — Unavailable' }}</option>
        </select>
        <span v-else>{{ backends.find(backend => backend.url === backendUrl)?.name }} · {{ backendUrl }}</span>
        <span class="nts-hint">The project and its resource bindings stay on this backend.</span>
        <span v-if="!available" class="nts-hint" role="status">Backend unavailable. Reconnect from the sidebar to continue.</span>
      </label>
      <label v-if="creating" class="nts-field">
        <span class="nts-field-name">Project name</span>
        <input ref="nameInput" v-model="name" :disabled="!available || saving || !!creation.project" placeholder="Project name" @keydown.enter.prevent="create" />
      </label>

      <RepoBaseFields v-model:repo="repo" v-model:base="base" :disabled="!available || saving || (creating && !!creation.savedRepo)" @submit="creating ? create() : saveDefaults()" />
      <div v-if="!creating" class="nts-binding-actions">
        <button type="button" :disabled="!repo.trim() || saving" @click="saveDefaults">{{ defaultsSaved ? 'Defaults saved' : 'Save launch defaults' }}</button>
        <span class="nts-hint">Repository and base for new tracks on this device. Existing work keeps its settings.</span>
      </div>
      <span v-else class="nts-hint">Set the repository and base for new tracks, and bind the resources they should inherit.</span>

      <div class="nts-field nts-project-bindings">
        <span class="nts-field-name">Bound resources <em class="nts-opt">inherited by new tracks</em></span>
        <div v-if="loading" class="nts-project-hint">Loading project bindings…</div>
        <template v-else-if="visibleBindings.length">
          <div v-for="binding in visibleBindings" :key="binding.id" class="nts-binding-row">
            <span class="nts-binding-kind" :title="binding.kind">{{ binding.kind }}</span>
            <span class="nts-binding-text">
              <strong>{{ binding.title }}</strong>
              <small>{{ binding.path || binding.url || binding.repository }}</small>
            </span>
            <button
              type="button"
              class="nts-binding-remove"
              :aria-label="`Remove ${binding.title} from project ${projectName}`"
              :disabled="!available || saving || loading"
              title="Remove from the project (existing tracks keep their own bindings)"
              @click="remove(binding)"
            >×</button>
          </div>
        </template>
        <div v-else-if="!error" class="nts-project-hint">
          Nothing bound yet — new tracks in {{ projectName }} inherit what you bind here.
        </div>
        <div class="nts-binding-actions">
          <!-- One attach affordance (design.md "Resource attachment"): the
               same menu as the track panel — search GitHub, complete a file
               path, or pick a repository — bound here to the project's
               store so every new track in it inherits the choice. -->
          <AttachResourceMenu
            :gh-repos="ghRepo"
            :repo-options="managedRepos"
            :files-repo="repo.trim() || null"
            :disabled="!available || saving || !repo.trim()"
            :label="creating ? '+ Add resource' : '+ Bind a resource'"
            @attach="onAttachDraft"
          />
        </div>
      </div>

      <div v-if="error" class="nts-error" role="alert">{{ error }}</div>
      <span v-if="creating && creation.project && !saving" class="nts-hint">The project was created. Retry to save the remaining resources, or close to keep its saved settings.</span>
      <span v-if="creating && creation.savedRepo && !saving" class="nts-hint">Finish saving resources in this repository. You can edit launch defaults in Project settings afterward.</span>
      <span class="nts-hint">
        Bound resources are inherited by every new track in {{ projectName }}; a track can hide one from
        its Resources panel without removing it here. Existing tracks keep supplying these bindings live.
      </span>
      <div v-if="creating" class="nts-actions">
        <button type="button" :disabled="saving" @click="emit('close')">{{ creation.project ? 'Close' : 'Cancel' }}</button>
        <button type="button" class="primary" :disabled="!available || saving || !name.trim() || (!!creation.bindings.length && !repo.trim())" @click="create">
          {{ saving ? 'Saving…' : creation.project ? 'Save remaining resources' : 'Create project' }}
        </button>
      </div>
    </div>
  </section>
</template>
