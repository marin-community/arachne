<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted, nextTick, reactive } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { LaunchOptions, ResourceMention, SessionSummary } from "../App.vue";
import { addAttachments, filesFromClipboard, imagePreviewUrl, MAX_LAUNCH_TOTAL_BYTES, type FileAttachment } from "../attachments";
import { pasteAsPlainText } from "../composerPaste";
import { readDraft, saveDraft, mergeLaunchConfig, type NewTopicDraft } from "../newTopicDraft";
import { recentRepositories, rememberRepository, readProjectDefaults } from "../launchDefaults";
import RepoBaseFields from "./RepoBaseFields.vue";

// The new-track chat (docs/design.md "Navigation"): New track opens a chat
// immediately, not a form sheet. It takes over the main display panel with
// the same shape as a live thread — header, conversation, composer — so
// composing a track looks like the chat it will become.
//
// The setup choices the old sheet carried (title, repository/base, project
// resource bindings) sit in a setup card inside the conversation area,
// above the composer, where the first messages will land. The whole view
// is replaced by the live thread on launch, so the setup disappears the
// moment the model starts. Everything that belongs to a chat box stays in
// the composer: the message text, attachments, @-mentions of existing
// track resources, and the launch-config pills (profile/agent/model/
// effort) beside the send button.
//
// Loom's launch needs the first message, so the session cannot exist
// before the first send: the first ⌘/Ctrl+Enter sends the message AND
// launches the track (App's launchTopic → launchTask), which routes
// straight into the live ThreadView. A failed launch keeps this view and
// its draft; closing it by other means (Esc, selecting a track) also
// keeps the draft — it is snapshotted to localStorage
// (src/newTopicDraft.ts) and restored when the chat reopens.

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
// The draft lives in localStorage (src/newTopicDraft.ts): selecting a
// track while this chat is open closes it (the main pane is that track's
// chat), and closing must not eat a half-written track. The chat restores
// the stored draft on mount — this is a reopen, not a blank composer —
// reconciling the launch config against the runtime's current profiles/
// agents. A successful launch clears it in App (the track exists as a
// real branch now, so the draft has served its purpose).

// Pin persistence to the server that opened this chat, including unmount
// during a server switch. Draft resource IDs must never move to another server.
const draftServer = localStorage.getItem("loomUrl");
const draftStorage = {
  getItem: (key: string) => key === "loomUrl" ? draftServer : localStorage.getItem(key),
  setItem: (key: string, value: string) => localStorage.setItem(key, value),
  removeItem: (key: string) => localStorage.removeItem(key),
};
const draft: NewTopicDraft = reactive(readDraft(draftStorage));
const repositorySuggestions = computed(() => [...new Set(props.fleet.map((s) => s.github_repo || s.branch.repo_root).filter(Boolean))]);
const inheritedDefaults = props.project?.id ? readProjectDefaults(localStorage, props.project.id) : null;
// Preserve a composed draft; project defaults apply only to a fresh launch.
if (!draft.title && !draft.body && !draft.attachments.length && !draft.mentions.length) {
  if (inheritedDefaults) Object.assign(draft, inheritedDefaults);
  else if (!draft.repo) draft.repo = recentRepositories(localStorage)[0] || (repositorySuggestions.value.length === 1 ? repositorySuggestions.value[0] : "");
}
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
// (e.g. the loom config changed while the chat sat closed) falls back
// to a valid default instead of silently failing at launch.
Object.assign(draft, mergeLaunchConfig(draft, props.launchOptions));

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
  // The message is markdown (a copied turn pastes its source); take the
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

// --- @-mentions of existing track resources -------------------------------------

interface TopicMentionResource {
  topicId: string;
  id: string;
  title: string;
  kind: string;
  path: string | null;
  url: string | null;
  repository: string;
  /** Inherited rows the track hid — never offerable as a mention. */
  hidden?: boolean;
}

// Every top-level session is a track whose resources can be mentioned.
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
      mentionError.value = "Could not load track resources";
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

// --- Project resource bindings (design.md "Project defaults and resource
//     inheritance") ----------------------------------------------------------
//
// A preselected project's bindings load here as a checklist — every binding
// checked (inherited) by default; unchecking one overrides the inheritance
// for THIS track only (recorded as an initial hide, never removing it from
// the project). "+ Bind a resource to this project" files a new binding
// onto the project itself, where every future track inherits it.

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

let bindingsRequest = 0;
async function loadProjectBindings() {
  const request = ++bindingsRequest;
  const projectId = props.project?.id;
  const repoInput = repo.value.trim();
  if (!projectId || !repoInput) {
    projectBindings.value = [];
    projectBindingsLoading.value = false;
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
    if (request !== bindingsRequest || projectId !== props.project?.id || repoInput !== repo.value.trim()) return;
    projectBindings.value = view.bindings ?? [];
    projectRevision.value = view.revision ?? 0;
    // Fresh fetch, fresh defaults: everything checked (inherited).
    uncheckedBindings.value = new Set();
  } catch (error: any) {
    if (request !== bindingsRequest || repoInput !== repo.value.trim()) return;
    projectBindings.value = [];
    projectBindingsError.value = error?.message ?? String(error);
  } finally {
    if (request === bindingsRequest) projectBindingsLoading.value = false;
  }
}

function toggleBinding(id: string, checked: boolean) {
  const next = new Set(uncheckedBindings.value);
  if (checked) next.delete(id);
  else next.add(id);
  uncheckedBindings.value = next;
}

const showProjectAttach = ref(false);
const projectFormKind = ref<"design_document" | "file" | "pull_request" | "issue">("design_document");
const projectFormTitle = ref("Design document");
const projectFormPath = ref("docs/design.md");
const projectFormUrl = ref("");
const projectSaving = ref(false);
const projectIsUrlKind = computed(() => projectFormKind.value === "pull_request" || projectFormKind.value === "issue");
const projectAttachValid = computed(() =>
  projectFormTitle.value.trim() && (projectIsUrlKind.value ? projectFormUrl.value.trim() : projectFormPath.value.trim()),
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
  if (!projectId || !repoInput || projectSaving.value || !projectAttachValid.value) return;
  projectSaving.value = true;
  projectBindingsError.value = "";
  try {
    const view = await invoke<{ bindings: ProjectBinding[]; revision: number }>("add_project_binding", {
      projectId,
      repo: repoInput,
      resource: projectIsUrlKind.value
        ? { kind: projectFormKind.value, title, repository: "", reference: null, path: null, url }
        : { kind: projectFormKind.value, title, repository: "", reference: null, path, url: null },
      expectedRevision: projectRevision.value,
    });
    projectBindings.value = view.bindings ?? [];
    projectRevision.value = view.revision ?? 0;
    showProjectAttach.value = false;
    projectFormTitle.value = "Design document";
    projectFormPath.value = "docs/design.md";
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

// --- Chat lifecycle ---------------------------------------------------------------

// Chat-first: focus the composer on open — the cursor lands where the
// writing happens, not in the setup card. A restored draft opens with the
// caret at the end of the message.
onMounted(async () => {
  await nextTick();
  bodyEl.value?.focus();
  if (body.value) bodyEl.value?.setSelectionRange(body.value.length, body.value.length);
});
// Snapshot the draft on unmount — the save happens on every close
// (launch, Esc, selecting a track), so a mid-compose close never
// loses work. App clears the snapshot after a successful launch, once
// this hook has flushed.
onUnmounted(() => {
  saveDraft(draftStorage, draft);
});

function send() {
  const t = title.value.trim();
  const b = body.value.trim();
  if ((!t && !b && !attachments.value.length) || props.launching || attachmentLoading.value) return;
  // The message is both the agent's initial goal and the durable branch
  // description; either alone can seed a track. The draft is not cleared
  // here: App clears it on successful launch (this view unmounts), and
  // every other exit (failure, Esc, navigation) keeps it for reopening.
  rememberRepository(localStorage, repo.value);
  emit("launch", b || t || `Review ${attachments.value[0].name}`, repo.value.trim(), {
    title: t || undefined,
    description: b || undefined,
    base: base.value.trim() || undefined,
    mentions: mentions.value.filter((mention) => b.includes(mention.token))
      .map(({ topicId, resourceId }) => ({ topicId, resourceId })),
    attachments: attachments.value,
    ...launchConfig(),
    project: props.project ?? undefined,
    // Unchecked bindings become the track's initial hides — the project's
    // binding stays attached for every other track (design.md).
    hiddenBindingKeys: props.project?.id && uncheckedBindings.value.size
      ? [...uncheckedBindings.value]
      : undefined,
  });
}

// ⌘/Ctrl+Enter sends (the chat composer's convention); bare Enter inserts
// a newline so the opening message can be multi-line. The mention menu
// owns the arrow keys and Escape while it's open.
function onComposerKeydown(event: KeyboardEvent) {
  if (event.key === "Enter" && !event.shiftKey && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    if (mentionRange.value && matchingResources.value.length) {
      chooseMention(matchingResources.value[mentionIndex.value] || matchingResources.value[0]);
    } else {
      send();
    }
    return;
  }
  if (!mentionRange.value) return;
  if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); mentionRange.value = null; }
  else if (event.key === "ArrowDown" && matchingResources.value.length) {
    event.preventDefault(); mentionIndex.value = (mentionIndex.value + 1) % matchingResources.value.length;
  } else if (event.key === "ArrowUp" && matchingResources.value.length) {
    event.preventDefault(); mentionIndex.value = (mentionIndex.value - 1 + matchingResources.value.length) % matchingResources.value.length;
  }
}
</script>

<template>
  <section class="main new-track" aria-label="New track">
    <div class="thread-header">
      <div class="meta">
        <div class="name">New track</div>
        <div class="sub">
          {{ props.project ? `project ${props.project.name} · ` : "" }}first send launches the track
        </div>
      </div>
      <button
        class="nts-close"
        title="Close (esc)"
        aria-label="Close new track"
        @click="emit('close')"
      >
        ✕
      </button>
    </div>

    <!-- The conversation area: until launch it holds the setup card where
         the chat's messages will land. It disappears once the model starts
         (a successful launch swaps this view for the live thread). -->
    <div class="conversation">
      <div class="block new-track-setup">
        <div class="who">Track setup <em>· goes away at launch</em></div>
        <div class="body new-track-setup-body">
          <label class="nts-field">
            <span class="nts-field-name">Title <em class="nts-opt">optional</em></span>
            <input
              v-model="title"
              placeholder="What is this work about?"
              spellcheck="false"
              @keydown.enter.prevent="bodyEl?.focus()"
            />
            <span class="nts-hint">The sidebar and track cards show this label.</span>
          </label>

          <!-- Enter never launches here (the chat convention): only
               ⌘/Ctrl+Enter or the send button does, so the setup card's
               fields are safe to type through. -->
          <RepoBaseFields v-model:repo="repo" v-model:base="base" :repositories="repositorySuggestions" />
          <span v-if="inheritedDefaults" class="nts-hint">Repository and base default to this project’s saved settings on this device.</span>

          <!-- Project resource bindings (design.md "Project defaults and
               resource inheritance"): every binding inherited (checked) by
               default; unchecking overrides the inheritance for this track
               only. -->
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
                  title="Remove from the project (existing tracks keep their own bindings)"
                  @click="removeFromProject(binding)"
                >×</button>
              </label>
            </template>
            <div v-else-if="!projectBindingsError" class="nts-project-hint">
              No resources bound to {{ props.project.name }} yet — new tracks inherit what you bind here.
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
            <!-- Binding a resource to the project: the same four kinds the
                 track panel attaches, stored in the project's store
                 (repo-shared arachne-projects) rather than any track's
                 manifest. -->
            <div v-if="showProjectAttach" class="nts-binding-form">
              <label>Kind
                <select v-model="projectFormKind" @change="chooseProjectKind">
                  <option value="design_document">Design document</option>
                  <option value="file">File</option>
                  <option value="pull_request">Pull request</option>
                  <option value="issue">Issue</option>
                </select>
              </label>
              <label>Title <input v-model="projectFormTitle" placeholder="Design document" /></label>
              <label v-if="projectIsUrlKind">GitHub URL <input v-model="projectFormUrl" placeholder="https://github.com/OWNER/REPO/pull/13" spellcheck="false" /></label>
              <label v-else>Path in repo <input v-model="projectFormPath" placeholder="docs/design.md" spellcheck="false" /></label>
              <button type="button" class="primary" :disabled="projectSaving || !projectAttachValid" @click="attachToProject">Bind to project</button>
            </div>
          </div>
        </div>
      </div>
      <div v-if="props.error" class="nts-error new-track-error">{{ props.error }}</div>
    </div>

    <!-- The chat box: message text, attachments, @-mentions, launch-config
         pills, and send — the same affordances a live thread's composer
         carries. -->
    <div class="composer-wrap" @dragover.prevent @drop.prevent="($event) => $event.dataTransfer?.files && addFiles($event.dataTransfer.files)">
      <div v-if="mentionRange" class="mention-menu" role="listbox" aria-label="Existing track resources">
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
      <div class="composer-card">
        <div v-if="attachments.length || attachmentError || attachmentLoading" class="attachment-row composer-attachments">
          <span v-for="(file, index) in attachments" :key="file.name" class="attachment-chip">
            <img v-if="imagePreviewUrl(file)" :src="imagePreviewUrl(file)!" class="attachment-preview" alt="" />
            {{ file.name }} <button type="button" :aria-label="`Remove ${file.name}`" @click="attachments = attachments.filter((_, i) => i !== index)">×</button>
          </span>
          <span v-if="attachmentError" class="attachment-error">{{ attachmentError }}</span>
          <span v-if="attachmentLoading" class="attachment-hint">Reading files…</span>
        </div>
        <div class="composer">
          <textarea
            ref="bodyEl"
            v-model="body"
            placeholder="Describe the goal, context, and what a good result looks like… Use @ to mention an existing resource."
            @input="updateMention"
            @click="updateMention"
            @keydown="onComposerKeydown"
            @paste="onPaste"
          ></textarea>
        </div>
        <div class="composer-utility-row">
          <label class="attachment-pick composer-attach" title="Attach files or images" aria-label="Attach files or images">
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 12.5 14.5 6a3 3 0 0 1 4.25 4.25l-8.5 8.5a5 5 0 0 1-7.07-7.07l8.5-8.5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
            <input type="file" multiple :disabled="attachmentLoading" aria-label="Attach files or images to the track's first message" @change="onFileInput" />
          </label>
          <label class="composer-pill" title="Inference profile">
            <span aria-hidden="true">⚡</span>
            <select v-model="profile" aria-label="Inference profile" @change="onProfileChange">
              <option v-if="!profiles.some((p) => p.name === 'default')" value="default">Default route</option>
              <option v-for="p in profiles" :key="p.name" :value="p.name">{{ p.name }}</option>
            </select>
            <span aria-hidden="true">⌄</span>
          </label>
          <label class="composer-pill" title="Agent runtime">
            <span aria-hidden="true">⚙</span>
            <select v-model="agent" aria-label="Agent runtime" @change="onAgentChange">
              <option value="">{{ selectedProfile?.agent_kind || launchOptions?.default_agent || 'Default agent' }}</option>
              <option v-for="choice in launchOptions?.agents ?? []" :key="choice.kind" :value="choice.kind">{{ choice.label }}</option>
            </select>
            <span aria-hidden="true">⌄</span>
          </label>
          <label v-if="modelChoices.length && !selectedAgent?.accepts_raw_model" class="composer-pill" title="Model">
            <span aria-hidden="true">▣</span>
            <select v-model="model" aria-label="Model">
              <option value="">{{ agent ? 'Runtime default model' : (selectedProfile?.model || 'Runtime default model') }}</option>
              <option v-for="choice in modelChoices" :key="choice.id" :value="choice.id">{{ choice.label }}</option>
            </select>
            <span aria-hidden="true">⌄</span>
          </label>
          <label v-else class="composer-pill" title="Model override">
            <span aria-hidden="true">▣</span>
            <input
              v-model="model"
              list="ntc-launch-models"
              :placeholder="agent ? 'Model' : (selectedProfile?.model || 'Model')"
              aria-label="Model override"
              spellcheck="false"
            />
            <datalist id="ntc-launch-models">
              <option v-for="choice in modelChoices" :key="choice.id" :value="choice.id">{{ choice.label }}</option>
            </datalist>
            <span aria-hidden="true">⌄</span>
          </label>
          <label class="composer-pill" title="Reasoning effort">
            <span aria-hidden="true">◉</span>
            <select v-model="effort" aria-label="Reasoning effort">
              <option value="">{{ agent ? 'Default effort' : (selectedProfile?.effort || 'Default effort') }}</option>
              <option v-for="choice in effortChoices" :key="choice.id" :value="choice.id">{{ choice.label }}</option>
            </select>
            <span aria-hidden="true">⌄</span>
          </label>
          <button
            class="composer-send"
            type="button"
            :disabled="(!title.trim() && !body.trim() && !attachments.length) || props.launching || attachmentLoading"
            :title="props.launching ? 'Launching…' : 'Send — launches the track (⌘/Ctrl + Enter)'"
            aria-label="Send and launch track"
            @click="send"
          >
            <span v-if="props.launching">…</span><span v-else aria-hidden="true">↑</span>
          </button>
        </div>
      </div>
      <div class="composer-hint">⌘/Ctrl + Enter to send and launch · Enter for a new line</div>
    </div>
  </section>
</template>
