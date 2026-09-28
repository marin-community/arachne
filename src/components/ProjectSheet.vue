<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted } from "vue";
import { invoke } from "@tauri-apps/api/core";
import RepoBaseFields from "./RepoBaseFields.vue";
import { NEW_TOPIC_DRAFT_KEY, DEFAULT_REPO } from "../newTopicDraft";

// Manage a project's resource bindings directly (design.md "Project defaults
// and resource inheritance"): bind a design document, file, repository, or a
// PR/issue URL to the project, and every new Topic in it inherits them —
// unless its creation form unchecks one or the topic later hides it from
// the Resources panel. The bindings live in the project (`project_bindings`
// → the repo-shared `arachne-projects` store), never copied onto topics.

const props = defineProps<{
  project: { id: string; name: string };
}>();

const emit = defineEmits<{
  (e: "close"): void;
}>();

interface ProjectBinding {
  id: string;
  kind: string;
  title: string;
  repository: string;
  reference: string | null;
  path: string | null;
  url: string | null;
}

// The store is per-repo (`arachne-projects` lives in the repo's shared
// artifact space), so bindings are addressed by project id + repo. The
// field defaults like the topic sheet (the remembered draft's repo, else
// the app default) and stays fully editable — any slug or path loom knows.
const repo = ref("");
const base = ref("");

const bindings = ref<ProjectBinding[]>([]);
const revision = ref(0);
const loading = ref(false);
const error = ref("");
const saving = ref(false);

// --- Bindings load --------------------------------------------------------

async function load() {
  const projectId = props.project.id;
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
    if (projectId !== props.project.id) return;
    bindings.value = view.bindings ?? [];
    revision.value = view.revision ?? 0;
  } catch (e: any) {
    bindings.value = [];
    error.value = e?.message ?? String(e);
  } finally {
    loading.value = false;
  }
}

// Managed repos fill the default; loading failures leave the field free.
async function seedRepoDefault() {
  try {
    const raw = localStorage.getItem(NEW_TOPIC_DRAFT_KEY);
    const parsed = raw ? JSON.parse(raw) as { repo?: string } | null : null;
    repo.value = parsed?.repo?.trim() || DEFAULT_REPO;
  } catch {
    repo.value = DEFAULT_REPO;
  }
}

onMounted(() => {
  seedRepoDefault().then(load);
  document.addEventListener("keydown", onKeydown);
});
onUnmounted(() => document.removeEventListener("keydown", onKeydown));

// A repo change swaps to that repo's store.
watch(repo, () => void load());

// --- Add / remove bindings ------------------------------------------------

const showAttach = ref(false);
const formKind = ref<"design_document" | "file" | "repository" | "pull_request" | "issue">("design_document");
const formTitle = ref("Design document");
const formPath = ref("");
const formUrl = ref("");
const isUrlKind = computed(() => formKind.value === "pull_request" || formKind.value === "issue");

// Repository bindings pick their own repo (design.md: multiple
// repositories per project) — the managed list, so the binding names a real
// repo rather than a guess.
const managedRepos = ref<string[]>([]);
const formRepo = ref("");
const repoOptions = computed(() => managedRepos.value);
watch(repoOptions, (options) => {
  if (!formRepo.value || !options.includes(formRepo.value)) formRepo.value = options[0] ?? "";
}, { immediate: true });

async function loadManagedRepos() {
  try {
    const repos = await invoke<{ slug: string }[] | null>("managed_repos");
    managedRepos.value = Array.isArray(repos) ? repos.map((r) => r.slug) : [];
  } catch {
    managedRepos.value = [];
  }
}
loadManagedRepos();

const attachValid = computed(() =>
  Boolean(
    formTitle.value.trim()
    && (isUrlKind.value
      ? formUrl.value.trim()
      : formKind.value === "repository"
        ? formRepo.value.trim()
        : formPath.value.trim()),
  ),
);

function chooseKind() {
  if (formKind.value === "design_document") {
    formTitle.value = "Design document";
    formPath.value = "docs/design.md";
    formUrl.value = "";
  } else {
    formTitle.value = "";
    formPath.value = "";
    formUrl.value = "";
  }
}

async function attach() {
  const projectId = props.project.id;
  const repoInput = repo.value.trim();
  const title = formTitle.value.trim();
  const path = formPath.value.trim();
  const url = formUrl.value.trim();
  const targetRepo = formRepo.value.trim();
  if (!repoInput || saving.value || !attachValid.value) return;
  saving.value = true;
  error.value = "";
  try {
    const view = await invoke<{ bindings: ProjectBinding[]; revision: number }>("add_project_binding", {
      projectId,
      repo: repoInput,
      resource: isUrlKind.value
        ? { kind: formKind.value, title, repository: "", reference: null, path: null, url }
        : formKind.value === "repository"
          ? { kind: formKind.value, title, repository: targetRepo, reference: null, path: null, url: null }
          : { kind: formKind.value, title, repository: "", reference: null, path, url: null },
      expectedRevision: revision.value,
    });
    bindings.value = view.bindings ?? [];
    revision.value = view.revision ?? 0;
    showAttach.value = false;
    formTitle.value = "Design document";
    formPath.value = "";
    formUrl.value = "";
    formKind.value = "design_document";
  } catch (e: any) {
    // Conflict or failure: reload so the next edit starts from truth.
    await load();
    error.value = e?.message ?? String(e);
  } finally {
    saving.value = false;
  }
}

async function remove(binding: ProjectBinding) {
  const projectId = props.project.id;
  const repoInput = repo.value.trim();
  if (!repoInput || saving.value) return;
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

// --- Sheet chrome ---------------------------------------------------------

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    event.preventDefault();
    emit("close");
  }
}
</script>

<template>
  <section class="new-thread-sheet" aria-label="Project resources" @keydown="onKeydown">
    <div class="nts-inner">
      <header class="nts-head">
        <h1>Project resources</h1>
        <span class="nts-project" :title="`Bindings every new topic in ${props.project.name} inherits`">{{ props.project.name }}</span>
        <button class="nts-close" title="Close (esc)" aria-label="Close project resources" @click="emit('close')">✕</button>
      </header>

      <RepoBaseFields v-model:repo="repo" v-model:base="base" @submit="attach" />

      <div class="nts-field nts-project-bindings">
        <span class="nts-field-name">Bound resources <em class="nts-opt">inherited by new topics</em></span>
        <div v-if="loading" class="nts-project-hint">Loading project bindings…</div>
        <template v-else-if="bindings.length">
          <div v-for="binding in bindings" :key="binding.id" class="nts-binding-row">
            <span class="nts-binding-kind" :title="binding.kind">{{ binding.kind }}</span>
            <span class="nts-binding-text">
              <strong>{{ binding.title }}</strong>
              <small>{{ binding.path || binding.url || binding.repository }}</small>
            </span>
            <button
              type="button"
              class="nts-binding-remove"
              :aria-label="`Remove ${binding.title} from project ${props.project.name}`"
              :disabled="saving"
              title="Remove from the project (existing topics keep their own bindings)"
              @click="remove(binding)"
            >×</button>
          </div>
        </template>
        <div v-else-if="!error" class="nts-project-hint">
          Nothing bound yet — new topics in {{ props.project.name }} inherit what you bind here.
        </div>
        <div v-if="error" class="nts-project-hint">{{ error }}</div>
        <div class="nts-binding-actions">
          <button type="button" class="link" :disabled="saving" @click="showAttach = !showAttach">
            {{ showAttach ? "Cancel" : "+ Bind a resource" }}
          </button>
        </div>
        <div v-if="showAttach" class="nts-binding-form">
          <label>Kind
            <select v-model="formKind" @change="chooseKind">
              <option value="design_document">Design document</option>
              <option value="file">File</option>
              <option value="repository">Repository</option>
              <option value="pull_request">Pull request</option>
              <option value="issue">Issue</option>
            </select>
          </label>
          <label>Title <input v-model="formTitle" placeholder="Design document" /></label>
          <!-- A repository binding names its own repo — the managed list;
               this is how a project comes to span several repos. -->
          <label v-if="formKind === 'repository'">Repository
            <select v-model="formRepo">
              <option v-for="repo in repoOptions" :key="repo" :value="repo">{{ repo }}</option>
            </select>
          </label>
          <label v-if="isUrlKind">GitHub URL <input v-model="formUrl" placeholder="https://github.com/OWNER/REPO/pull/13" spellcheck="false" /></label>
          <label v-else-if="formKind !== 'repository'">Path in repo <input v-model="formPath" placeholder="docs/design.md" spellcheck="false" /></label>
          <button type="button" class="primary" :disabled="saving || !attachValid" @click="attach">Bind to project</button>
        </div>
      </div>

      <span class="nts-hint">
        Bound resources are inherited by every new topic in {{ props.project.name }}; a topic can hide one from
        its Resources panel without removing it here. Existing topics keep supplying these bindings live.
      </span>
    </div>
  </section>
</template>
