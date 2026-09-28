<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted, nextTick, reactive } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { LaunchOptions, ResourceMention, SessionSummary } from "../App.vue";
import { addAttachments, filesFromClipboard, imagePreviewUrl, MAX_LAUNCH_TOTAL_BYTES, type FileAttachment } from "../attachments";
import { pasteAsPlainText } from "../composerPaste";
import { readDraft, saveDraft, mergeLaunchConfig, type NewTopicDraft } from "../newTopicDraft";
import RepoBaseFields from "./RepoBaseFields.vue";

// The new-topic sheet: composing a topic takes over the main display panel
// (grid-area main), exactly like the new-thread sheet — never a floating
// modal, never a cramped sidebar composer. A topic is a leader chat with
// card metadata, so this sheet carries everything the old sidebar overlay
// did: a short title, a substantive body (the agent's initial goal and the
// durable branch description), @-mentions of existing topic resources,
// file attachments, and the launch config (profile/agent/model/effort).
//
// Launching routes through App's launchTask → selectSession flow, which
// swaps the sheet for the live thread; on failure the drafts survive.
// Closing the sheet by other means (selecting a topic, Esc, Cancel) also
// keeps the drafts: the composer state is snapshotted to localStorage
// (src/newTopicDraft.ts) and restored when the sheet reopens, so clicking
// away mid-compose never eats a half-written topic.
//
// The optional preselected project (Topics [+] on a project heading) is
// shown as context in the sheet and forwarded on launch — a topic is the
// unit a project files, so the + belongs to this sheet, not the plain
// new-thread one.

const props = defineProps<{
  fleet: SessionSummary[];
  launching?: boolean;
  error?: string | null;
  launchOptions: LaunchOptions | null;
  project?: { id: string | null; name: string } | null;
}>();

const emit = defineEmits<{
  (e: "close"): void;
  (
    e: "launch",
    task: string,
    repo: string,
    meta?: { title?: string; description?: string; base?: string; mentions?: ResourceMention[]; attachments?: FileAttachment[]; profile?: string; agent?: string; model?: string; effort?: string; project?: { id: string | null; name: string }; hiddenBindingKeys?: string[] },
  ): void;
}>();

// --- Draft state ---------------------------------------------------------------
//
// The draft lives in localStorage (src/newTopicDraft.ts): selecting a topic
// closes the sheet mid-compose (the main pane is the topic's chat — see
// App's selectTopic), and closing must not eat a half-written topic. The
// sheet restores the stored draft on mount — this is a reopen, not a blank
// form — reconciling the launch config against the runtime's current
// profiles/agents. A successful launch clears it in App (the topic exists
// as a real branch now, so the draft has served its purpose).

const draft: NewTopicDraft = reactive(readDraft(localStorage));
const title = computed({
  get: () => draft.title,
  set: (value: string) => { draft.title = value; },
});
const body = computed({
  get: () => draft.body,
  set: (value: string) => { draft.body = value; },
});
const repo = computed({
  get: () => draft.repo,
  set: (value: string) => { draft.repo = value; },
});
const base = computed({
  get: () => draft.base,
  set: (value: string) => { draft.base = value; },
});
const attachments = computed({
  get: () => draft.attachments,
  set: (value: FileAttachment[]) => { draft.attachments = value; },
});
const mentions = computed({
  get: () => draft.mentions,
  set: (value: NewTopicDraft["mentions"]) => { draft.mentions = value; },
});

// Reconcile the launch config against the runtime's current
// profiles/agents: a stored profile or agent that no longer exists
// (e.g. the loom config changed while the sheet sat closed) falls back
// to a valid default instead of silently failing at launch.
Object.assign(draft, mergeLaunchConfig(draft, props.launchOptions));

const titleEl = ref<HTMLInputElement | null>(null);
const bodyEl = ref<HTMLTextAreaElement | null>(null);

const profile = computed({
  get: () => draft.profile,
  set: (value: string) => { draft.profile = value; },
});
const agent = computed({
  get: () => draft.agent,
  set: (value: string) => { draft.agent = value; },
});
const model = computed({
  get: () => draft.model,
  set: (value: string) => { draft.model = value; },
});
const effort = computed({
  get: () => draft.effort,
  set: (value: string) => { draft.effort = value; },
});
const profiles = computed(() => props.launchOptions?.profiles.filter((p) => p.class === "interactive") ?? []);
const selectedProfile = computed(() => profiles.value.find((p) => p.name === profile.value));
const selectedAgent = computed(() => props.launchOptions?.agents.find((a) => a.kind === (agent.value || selectedProfile.value?.agent_kind || props.launchOptions?.default_agent)));
const modelChoices = computed(() => selectedAgent.value?.models ?? []);
const effortChoices = computed(() => selectedAgent.value?.efforts ?? []);
const launchConfig = () => ({
  profile: profile.value === "default" ? undefined : profile.value,
  agent: agent.value || undefined,
  model: model.value.trim() || undefined,
  effort: effort.value || undefined,
});
function onProfileChange() {
  agent.value = "";
  model.value = "";
  effort.value = "";
}
function onAgentChange() {
  model.value = "";
  effort.value = "";
}

// --- Attachments ---------------------------------------------------------------

// Restored attachments come back as plain FileAttachment snapshots
// (name/size/payload/mime); re-adding more checks the total budget
// including them, same as a live session.
const attachmentError = ref("");
const attachmentLoading = ref(false);

async function addFiles(files: FileList | File[]) {
  if (attachmentLoading.value) return;
  attachmentLoading.value = true;
  try {
    attachments.value = await addAttachments(attachments.value, files, MAX_LAUNCH_TOTAL_BYTES);
    attachmentError.value = "";
  } catch (error: any) {
    attachmentError.value = error?.message ?? String(error);
  } finally {
    attachmentLoading.value = false;
  }
}
function onFileInput(event: Event) {
  const input = event.target as HTMLInputElement;
  if (input.files) void addFiles(input.files);
  input.value = "";
}
function onPaste(event: ClipboardEvent) {
  if (!event.clipboardData) return;
  // The body is markdown (a copied turn pastes its source); take the
  // text/plain flavor before WebKit flattens the HTML one.
  if (pasteAsPlainText(event)) {
    event.preventDefault();
    const text = event.clipboardData.getData("text/plain");
    document.execCommand("insertText", false, text);
    updateMention();
    return;
  }
  const files = filesFromClipboard(event.clipboardData);
  if (!files.length) return;
  event.preventDefault();
  void addFiles(files);
}

// --- @-mentions of existing topic resources -------------------------------------

interface TopicMentionResource {
  topicId: string;
  id: string;
  title: string;
  kind: string;
  path: string | null;
  url: string | null;
  repository: string;
  /** Inherited rows the topic hid — never offerable as a mention. */
  hidden?: boolean;
}

// Every top-level session is a topic whose resources can be mentioned.
const topics = computed(() => {
  const byId = new Map(props.fleet.map((s) => [s.id, s]));
  const byBranch = new Map(props.fleet.map((s) => [s.branch.id, s]));
  const parentOf = (s: SessionSummary) =>
    (s.parent_session_id ? byId.get(s.parent_session_id) : undefined) ??
    (s.parent_id ? byBranch.get(s.parent_id) : undefined);
  return props.fleet.filter((s) => {
    const p = parentOf(s);
    return !p || p.id === s.id;
  });
});

const mentionRange = ref<{ start: number; end: number; query: string } | null>(null);
const mentionIndex = ref(0);
const mentionResources = ref<TopicMentionResource[]>([]);
const mentionLoading = ref(false);
const mentionError = ref("");
const matchingResources = computed(() => {
  const query = mentionRange.value?.query.trim().toLowerCase() ?? "";
  return mentionResources.value.filter((resource) =>
    !query || [resource.title, resource.kind, resource.path, resource.url, resource.repository]
      .some((value) => value?.toLowerCase().includes(query)),
  ).slice(0, 8);
});

async function loadMentionResources() {
  mentionLoading.value = true;
  mentionError.value = "";
  try {
    const views = await Promise.allSettled(topics.value.slice(0, 24).map(async ({ id }) => {
      const view = await invoke<{ resources: Omit<TopicMentionResource, "topicId">[] }>("topic_resources", { topicId: id });
      // Effective rows only: hidden inherited bindings never resolve.
      return (view.resources ?? []).filter((resource) => !resource.hidden).map((resource) => ({ ...resource, topicId: id }));
    }));
    mentionResources.value = views.flatMap((result) => result.status === "fulfilled" ? result.value : []);
    if (views.length && views.every((result) => result.status === "rejected")) {
      mentionError.value = "Could not load topic resources";
    }
  } finally {
    mentionLoading.value = false;
  }
}

function updateMention() {
  const caret = bodyEl.value?.selectionStart ?? body.value.length;
  const match = /(?:^|\s)@([^@{}\n]{0,64})$/.exec(body.value.slice(0, caret));
  const wasOpen = !!mentionRange.value;
  mentionRange.value = match ? { start: caret - match[1].length - 1, end: caret, query: match[1] } : null;
  mentionIndex.value = 0;
  if (mentionRange.value && !wasOpen) void loadMentionResources();
}

function chooseMention(resource: TopicMentionResource) {
  const range = mentionRange.value;
  if (!range) return;
  const duplicate = mentionResources.value.some((other) => other.id !== resource.id && other.title === resource.title);
  const label = duplicate ? `${resource.title} (${resource.path || resource.url || resource.repository})` : resource.title;
  const token = `@{${label}}`;
  body.value = body.value.slice(0, range.start) + token + " " + body.value.slice(range.end);
  mentions.value = [...mentions.value, { token, topicId: resource.topicId, resourceId: resource.id }];
  mentionRange.value = null;
  nextTick(() => {
    const caret = range.start + token.length + 1;
    bodyEl.value?.focus();
    bodyEl.value?.setSelectionRange(caret, caret);
  });
}

function onBodyKeydown(event: KeyboardEvent) {
  // Sheets launch with bare Enter (the NewThreadSheet convention): Enter
  // submits, Shift+Enter inserts a newline. The mention menu owns Enter
  // first while it's open.
  if (event.key === "Enter" && !event.shiftKey) {
    if (mentionRange.value && matchingResources.value.length) {
      event.preventDefault();
      chooseMention(matchingResources.value[mentionIndex.value] || matchingResources.value[0]);
    } else if (!mentionRange.value) {
      event.preventDefault();
      submit();
    }
    return;
  }
  if (event.key === "Escape") { event.preventDefault(); mentionRange.value = null; }
  else if (mentionRange.value && event.key === "ArrowDown" && matchingResources.value.length) {
    event.preventDefault(); mentionIndex.value = (mentionIndex.value + 1) % matchingResources.value.length;
  } else if (mentionRange.value && event.key === "ArrowUp" && matchingResources.value.length) {
    event.preventDefault(); mentionIndex.value = (mentionIndex.value - 1 + matchingResources.value.length) % matchingResources.value.length;
  }
}

// --- Project resource bindings (design.md "Project defaults and resource
//     inheritance") ----------------------------------------------------------
//
// A preselected project's bindings load here as a checklist — every binding
// checked (inherited) by default; unchecking one overrides the inheritance
// for THIS topic only (recorded as an initial hide, never removing it from
// the project). "+ Add to project" files a new binding onto the project
// itself, where every future topic inherits it.

interface ProjectBinding {
  id: string;
  kind: string;
  title: string;
  repository: string;
  reference: string | null;
  path: string | null;
  url: string | null;
}

const projectBindings = ref<ProjectBinding[]>([]);
const projectRevision = ref(0);
const projectBindingsLoading = ref(false);
const projectBindingsError = ref("");
/** Unchecked binding ids — the initial hides recorded at launch. */
const uncheckedBindings = ref<Set<string>>(new Set());

async function loadProjectBindings() {
  const projectId = props.project?.id;
  const repoInput = repo.value.trim();
  if (!projectId || !repoInput) {
    projectBindings.value = [];
    uncheckedBindings.value = new Set();
    return;
  }
  projectBindingsLoading.value = true;
  projectBindingsError.value = "";
  try {
    const view = await invoke<{ bindings: ProjectBinding[]; revision: number }>("project_bindings", {
      projectId,
      repo: repoInput,
    });
    projectBindings.value = view.bindings ?? [];
    projectRevision.value = view.revision ?? 0;
    // Fresh fetch, fresh defaults: everything checked (inherited).
    uncheckedBindings.value = new Set();
  } catch (error: any) {
    projectBindings.value = [];
    projectBindingsError.value = error?.message ?? String(error);
  } finally {
    projectBindingsLoading.value = false;
  }
}

function toggleBinding(id: string, checked: boolean) {
  const next = new Set(uncheckedBindings.value);
  if (checked) next.delete(id);
  else next.add(id);
  uncheckedBindings.value = next;
}

const showProjectAttach = ref(false);
const projectFormKind = ref<"design_document" | "file" | "repository" | "pull_request" | "issue">("design_document");
const projectFormTitle = ref("Design document");
const projectFormPath = ref("");
const projectFormUrl = ref("");
const projectSaving = ref(false);
const projectIsUrlKind = computed(() => projectFormKind.value === "pull_request" || projectFormKind.value === "issue");

// Repository bindings pick their own repo — the managed list (design.md:
// a project can bind multiple repositories).
const projectManagedRepos = ref<string[]>([]);
const projectFormRepo = ref("");
const projectRepoOptions = computed(() => projectManagedRepos.value);
watch(projectRepoOptions, (options) => {
  if (!projectFormRepo.value || !options.includes(projectFormRepo.value)) projectFormRepo.value = options[0] ?? "";
}, { immediate: true });

async function loadProjectManagedRepos() {
  try {
    const repos = await invoke<{ slug: string }[] | null>("managed_repos");
    projectManagedRepos.value = Array.isArray(repos) ? repos.map((r) => r.slug) : [];
  } catch {
    projectManagedRepos.value = [];
  }
}
loadProjectManagedRepos();

const projectAttachValid = computed(() =>
  projectFormTitle.value.trim()
  && (projectIsUrlKind.value
    ? projectFormUrl.value.trim()
    : projectFormKind.value === "repository"
      ? projectFormRepo.value.trim()
      : projectFormPath.value.trim()),
);

function chooseProjectKind() {
  if (projectFormKind.value === "design_document") {
    projectFormTitle.value = "Design document";
    projectFormPath.value = "docs/design.md";
    projectFormUrl.value = "";
  } else {
    projectFormTitle.value = "";
    projectFormPath.value = "";
    projectFormUrl.value = "";
  }
}

async function attachToProject() {
  const projectId = props.project?.id;
  const repoInput = repo.value.trim();
  const title = projectFormTitle.value.trim();
  const path = projectFormPath.value.trim();
  const url = projectFormUrl.value.trim();
  const targetRepo = projectFormRepo.value.trim();
  if (!projectId || !repoInput || projectSaving.value || !projectAttachValid.value) return;
  projectSaving.value = true;
  projectBindingsError.value = "";
  try {
    const view = await invoke<{ bindings: ProjectBinding[]; revision: number }>("add_project_binding", {
      projectId,
      repo: repoInput,
      resource: projectIsUrlKind.value
        ? { kind: projectFormKind.value, title, repository: "", reference: null, path: null, url }
        : projectFormKind.value === "repository"
          ? { kind: projectFormKind.value, title, repository: targetRepo, reference: null, path: null, url: null }
          : { kind: projectFormKind.value, title, repository: "", reference: null, path, url: null },
      expectedRevision: projectRevision.value,
    });
    projectBindings.value = view.bindings ?? [];
    projectRevision.value = view.revision ?? 0;
    showProjectAttach.value = false;
    projectFormTitle.value = "Design document";
    projectFormPath.value = "";
    projectFormUrl.value = "";
    projectFormKind.value = "design_document";
  } catch (error: any) {
    await loadProjectBindings();
    projectBindingsError.value = error?.message ?? String(error);
  } finally {
    projectSaving.value = false;
  }
}

async function removeFromProject(binding: ProjectBinding) {
  const projectId = props.project?.id;
  const repoInput = repo.value.trim();
  if (!projectId || !repoInput || projectSaving.value) return;
  projectSaving.value = true;
  projectBindingsError.value = "";
  try {
    const view = await invoke<{ bindings: ProjectBinding[]; revision: number }>("remove_project_binding", {
      projectId,
      repo: repoInput,
      bindingId: binding.id,
      expectedRevision: projectRevision.value,
    });
    projectBindings.value = view.bindings ?? [];
    projectRevision.value = view.revision ?? 0;
  } catch (error: any) {
    await loadProjectBindings();
    projectBindingsError.value = error?.message ?? String(error);
  } finally {
    projectSaving.value = false;
  }
}

// The checklist reloads when the preselected project or the repo changes —
// bindings are per-repo, so "arachne" vs "loom" carry different lists.
watch(
  () => [props.project?.id, repo.value.trim()] as const,
  () => void loadProjectBindings(),
  { immediate: true },
);

// --- Sheet lifecycle ---------------------------------------------------------------

// Keyboard-first: focus the title on open, and Esc always closes — unless
// the mention menu is open (it owns Esc first). A restored draft opens
// with focus at the end of the body — the cursor lands where the writing
// stopped, not back at the title.
function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") emit("close");
}
onMounted(async () => {
  document.addEventListener("keydown", onKeydown);
  await nextTick();
  if (!title.value && !body.value) {
    titleEl.value?.focus();
    return;
  }
  const target = title.value ? bodyEl.value : titleEl.value;
  target?.focus();
  if (target === bodyEl.value) target?.setSelectionRange(target.value.length, target.value.length);
});
// Snapshot the draft on unmount — the save happens on every close
// (launch, Cancel, Esc, selecting a topic), so a mid-compose close never
// loses work. App clears the snapshot after a successful launch, once
// this hook has flushed.
onUnmounted(() => {
  document.removeEventListener("keydown", onKeydown);
  saveDraft(localStorage, draft);
});

function submit() {
  const t = title.value.trim();
  const b = body.value.trim();
  if ((!t && !b && !attachments.value.length) || props.launching || attachmentLoading.value) return;
  // The body is both the agent's initial goal and the durable branch
  // description; either alone can seed a topic. The draft is not cleared
  // here: App clears it on successful launch (the sheet unmounts), and
  // every other exit (failure, Esc, navigation) keeps it for reopening.
  emit("launch", b || t || `Review ${attachments.value[0].name}`, repo.value.trim(), {
    title: t || undefined,
    description: b || undefined,
    base: base.value.trim() || undefined,
    mentions: mentions.value.filter((mention) => b.includes(mention.token))
      .map(({ topicId, resourceId }) => ({ topicId, resourceId })),
    attachments: attachments.value,
    ...launchConfig(),
    project: props.project ?? undefined,
    // Unchecked bindings become the topic's initial hides — the project's
    // binding stays attached for every other topic (design.md).
    hiddenBindingKeys: props.project?.id && uncheckedBindings.value.size
      ? [...uncheckedBindings.value]
      : undefined,
  });
}
</script>

<template>
  <section class="new-thread-sheet" aria-label="New topic">
    <div class="nts-inner">
      <header class="nts-head">
        <h1>New topic</h1>
        <span v-if="props.project" class="nts-project" :title="`Files under project ${props.project.name}`">{{ props.project.name }}</span>
        <button
          class="nts-close"
          title="Close (esc)"
          aria-label="Close new topic"
          @click="emit('close')"
        >
          ✕
        </button>
      </header>

      <label class="nts-field">
        <span class="nts-field-name">Title <em class="nts-opt">optional</em></span>
        <input
          ref="titleEl"
          v-model="title"
          placeholder="What is this work about?"
          spellcheck="false"
          @keydown.enter.prevent="bodyEl?.focus()"
        />
        <span class="nts-hint">The sidebar and topic cards show this label.</span>
      </label>

      <label class="nts-field">
        <span class="nts-field-name">Body <em class="nts-opt">optional</em></span>
        <div class="nts-body-wrap">
          <textarea
            ref="bodyEl"
            v-model="body"
            rows="6"
            placeholder="Describe the goal, context, and what a good result looks like… Use @ to mention an existing resource."
            @input="updateMention"
            @click="updateMention"
            @keydown="onBodyKeydown"
            @paste="onPaste"
          ></textarea>
          <div v-if="mentionRange" class="mention-menu nts-mention-menu" role="listbox" aria-label="Existing topic resources">
            <div v-if="mentionLoading" class="mention-hint">Loading resources…</div>
            <div v-else-if="mentionError" class="mention-hint">{{ mentionError }}</div>
            <div v-else-if="!matchingResources.length" class="mention-hint">No matching attached resources</div>
            <button
              v-for="(resource, index) in matchingResources"
              :key="`${resource.topicId}:${resource.id}`"
              role="option"
              :aria-selected="index === mentionIndex"
              :class="{ selected: index === mentionIndex }"
              @mousedown.prevent="chooseMention(resource)"
            >
              <strong>{{ resource.title }}</strong>
              <small>{{ resource.repository }} · {{ resource.path || resource.url || resource.kind }}</small>
            </button>
          </div>
        </div>
        <span class="nts-hint">Enter to launch · Shift+Enter for a new line · the body is the agent's opening message and the durable topic description</span>
      </label>

      <RepoBaseFields v-model:repo="repo" v-model:base="base" @submit="submit" />

      <!-- Project resource bindings (design.md "Project defaults and resource
           inheritance"): every binding inherited (checked) by default;
           unchecking overrides the inheritance for this topic only. -->
      <div v-if="props.project?.id" class="nts-field nts-project-bindings">
        <span class="nts-field-name">Project resources <em class="nts-opt">inherited</em></span>
        <div v-if="projectBindingsLoading" class="nts-project-hint">Loading project bindings…</div>
        <template v-else-if="projectBindings.length">
          <label v-for="binding in projectBindings" :key="binding.id" class="nts-binding-row">
            <input
              type="checkbox"
              :checked="!uncheckedBindings.has(binding.id)"
              @change="toggleBinding(binding.id, ($event.target as HTMLInputElement).checked)"
            />
            <span class="nts-binding-text">
              <strong>{{ binding.title }}</strong>
              <small>{{ binding.path || binding.url || binding.repository }}</small>
            </span>
            <button
              type="button"
              class="nts-binding-remove"
              :aria-label="`Remove ${binding.title} from project ${props.project?.name}`"
              :disabled="projectSaving"
              title="Remove from the project (existing topics keep their own bindings)"
              @click="removeFromProject(binding)"
            >×</button>
          </label>
        </template>
        <div v-else-if="!projectBindingsError" class="nts-project-hint">
          No resources bound to {{ props.project.name }} yet — new topics inherit what you bind here.
        </div>
        <div v-if="projectBindingsError" class="nts-project-hint">{{ projectBindingsError }}</div>
        <div class="nts-binding-actions">
          <button
            type="button"
            class="link"
            :disabled="projectSaving"
            @click="showProjectAttach = !showProjectAttach"
          >{{ showProjectAttach ? "Cancel" : "+ Bind a resource to this project" }}</button>
        </div>
        <!-- Binding a resource to the project: the same four kinds the topic
             panel attaches, stored in the project's store (repo-shared
             arachne-projects) rather than any topic's manifest. -->
        <div v-if="showProjectAttach" class="nts-binding-form">
          <label>Kind
            <select v-model="projectFormKind" @change="chooseProjectKind">
              <option value="design_document">Design document</option>
              <option value="file">File</option>
              <option value="repository">Repository</option>
              <option value="pull_request">Pull request</option>
              <option value="issue">Issue</option>
            </select>
          </label>
          <label>Title <input v-model="projectFormTitle" placeholder="Design document" /></label>
          <!-- A repository binding names its own repo — how a project
               comes to span several repositories. -->
          <label v-if="projectFormKind === 'repository'">Repository
            <select v-model="projectFormRepo">
              <option v-for="repo in projectRepoOptions" :key="repo" :value="repo">{{ repo }}</option>
            </select>
          </label>
          <label v-if="projectIsUrlKind">GitHub URL <input v-model="projectFormUrl" placeholder="https://github.com/OWNER/REPO/pull/13" spellcheck="false" /></label>
          <label v-else-if="projectFormKind !== 'repository'">Path in repo <input v-model="projectFormPath" placeholder="docs/design.md" spellcheck="false" /></label>
          <button type="button" class="primary" :disabled="projectSaving || !projectAttachValid" @click="attachToProject">Bind to project</button>
        </div>
      </div>

      <div
        class="attachment-row"
        @dragover.prevent
        @drop.prevent="($event) => $event.dataTransfer?.files && addFiles($event.dataTransfer.files)"
      >
        <label class="attachment-pick">+ Attach files or images<input type="file" multiple :disabled="attachmentLoading" aria-label="Attach files or images to new topic" @change="onFileInput" /></label>
        <span v-for="(file, index) in attachments" :key="file.name" class="attachment-chip">
          <img v-if="imagePreviewUrl(file)" :src="imagePreviewUrl(file)!" class="attachment-preview" alt="" />
          {{ file.name }} <button type="button" :aria-label="`Remove ${file.name}`" @click="attachments = attachments.filter((_, i) => i !== index)">×</button>
        </span>
        <span v-if="attachmentError" class="attachment-error">{{ attachmentError }}</span>
        <span v-if="attachmentLoading" class="attachment-hint">Reading files…</span>
      </div>

      <div class="nts-field">
        <span class="nts-field-name">Launch config <em class="nts-opt">optional</em></span>
        <div class="launch-controls">
          <select v-model="profile" aria-label="Inference profile" @change="onProfileChange">
            <option v-if="!profiles.some((p) => p.name === 'default')" value="default">Default route</option>
            <option v-for="p in profiles" :key="p.name" :value="p.name">{{ p.name }} · {{ p.agent_kind }}</option>
          </select>
          <select v-model="agent" aria-label="Agent runtime" @change="onAgentChange">
            <option value="">{{ selectedProfile?.agent_kind || launchOptions?.default_agent || 'Default agent' }}</option>
            <option v-for="choice in launchOptions?.agents ?? []" :key="choice.kind" :value="choice.kind">{{ choice.label }}</option>
          </select>
          <select v-if="modelChoices.length && !selectedAgent?.accepts_raw_model" v-model="model" aria-label="Model">
            <option value="">{{ agent ? 'Runtime default model' : (selectedProfile?.model || 'Runtime default model') }}</option>
            <option v-for="choice in modelChoices" :key="choice.id" :value="choice.id">{{ choice.label }}</option>
          </select>
          <input
            v-else
            v-model="model"
            list="nts-launch-models"
            :placeholder="agent ? 'Model · runtime default' : (selectedProfile?.model || 'Model · runtime default')"
            aria-label="Model override"
            spellcheck="false"
          />
          <datalist id="nts-launch-models">
            <option v-for="choice in modelChoices" :key="choice.id" :value="choice.id">{{ choice.label }}</option>
          </datalist>
          <select v-model="effort" aria-label="Reasoning effort">
            <option value="">{{ agent ? 'Default effort' : (selectedProfile?.effort || 'Default effort') }}</option>
            <option v-for="choice in effortChoices" :key="choice.id" :value="choice.id">{{ choice.label }}</option>
          </select>
        </div>
      </div>

      <div v-if="props.error" class="nts-error">{{ props.error }}</div>

      <div class="nts-actions">
        <button @click="emit('close')">Cancel</button>
        <button
          class="primary"
          :disabled="(!title.trim() && !body.trim() && !attachments.length) || props.launching || attachmentLoading"
          @click="submit"
        >
          {{ props.launching ? "Launching…" : "Launch topic" }}
        </button>
      </div>
    </div>
  </section>
</template>

<style scoped>
/* Launch config row (profile/agent/model/effort) — same look as the
   New thread sheet's controls. */
.launch-controls {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.launch-controls select,
.launch-controls input {
  box-sizing: border-box;
  flex: 1 1 118px;
  min-width: 0;
  max-width: 100%;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg);
  color: var(--text);
  padding: 6px 8px;
  font: 11px var(--mono);
}
.launch-controls select:focus,
.launch-controls input:focus {
  outline: 1px solid var(--accent);
}
</style>
