<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { LaunchOptions, ResourceMention, SessionSummary } from "../App.vue";
import { addAttachments, filesFromClipboard, imagePreviewUrl, MAX_LAUNCH_TOTAL_BYTES, type FileAttachment } from "../attachments";
import RepoBaseFields from "./RepoBaseFields.vue";

// The new-thread sheet: composing a thread happens in the main display
// panel — the thread home — not in a cramped sidebar composer or a
// floating modal. The sheet takes over grid-area `main` while open;
// launching routes through App's launchTask → selectSession flow, which
// swaps the sheet for the live thread. On failure the sheet stays and the
// draft survives, so a retry is one edit away.
//
// The draft carries the same affordances the chat composer has: file
// attachments (uploaded into the new session's Scratch directory) and
// @-mentions of existing topic resources (the same `topic_resources`
// menu the thread composer and the topic sheet use), plus the launch
// config (profile/agent/model/effort). These moved here when the
// sidebar's single-prompt composer was removed — this sheet is now the
// only single-prompt launch surface.

// The optional preselected project (Topics [+] on a project heading) is
// shown as context in the sheet and forwarded on launch.
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
    meta?: { base?: string; mentions?: ResourceMention[]; attachments?: FileAttachment[]; profile?: string; agent?: string; model?: string; effort?: string; project?: { id: string | null; name: string } },
  ): void;
}>();

const task = ref("");
const repo = ref("marin-community/arachne");
const base = ref("");
const taskEl = ref<HTMLTextAreaElement | null>(null);
const attachments = ref<FileAttachment[]>([]);
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
  const files = filesFromClipboard(event.clipboardData);
  if (!files.length) return;
  event.preventDefault();
  void addFiles(files);
}

// --- Launch config -----------------------------------------------------------

const profile = ref("default");
const agent = ref("");
const model = ref("");
const effort = ref("");
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

// Declared above alongside the other draft state: `attachments`,
// `attachmentError`, `attachmentLoading`, `addFiles`, `onFileInput`, and
// `onPaste` (which also accepts pasted images via filesFromClipboard).

// --- @-mentions of existing topic resources -------------------------------------

interface TopicMentionResource {
  topicId: string;
  id: string;
  title: string;
  kind: string;
  path: string | null;
  url: string | null;
  repository: string;
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
const mentions = ref<{ token: string; topicId: string; resourceId: string }[]>([]);
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
      return (view.resources ?? []).map((resource) => ({ ...resource, topicId: id }));
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
  const caret = taskEl.value?.selectionStart ?? task.value.length;
  const match = /(?:^|\s)@([^@{}\n]{0,64})$/.exec(task.value.slice(0, caret));
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
  task.value = task.value.slice(0, range.start) + token + " " + task.value.slice(range.end);
  mentions.value.push({ token, topicId: resource.topicId, resourceId: resource.id });
  mentionRange.value = null;
  nextTick(() => {
    const caret = range.start + token.length + 1;
    taskEl.value?.focus();
    taskEl.value?.setSelectionRange(caret, caret);
  });
}

// --- Sheet lifecycle ---------------------------------------------------------------

// Keyboard-first: focus the goal on open, and Esc always closes — unless
// the mention menu is open (it owns Esc first).
function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") emit("close");
}
onMounted(async () => {
  document.addEventListener("keydown", onKeydown);
  await nextTick();
  taskEl.value?.focus();
});
onUnmounted(() => document.removeEventListener("keydown", onKeydown));

function submit() {
  if (mentionRange.value && matchingResources.value.length) {
    chooseMention(matchingResources.value[mentionIndex.value] || matchingResources.value[0]);
  }
  const t = task.value.trim();
  if ((!t && !attachments.value.length) || props.launching || attachmentLoading.value) return;
  // The draft is deliberately not cleared: success unmounts the sheet
  // (App closes it), while failure keeps the text for a retry.
  emit("launch", t || `Review ${attachments.value[0].name}`, repo.value.trim(), {
    ...launchConfig(),
    base: base.value.trim() || undefined,
    mentions: mentions.value.filter((mention) => t.includes(mention.token))
      .map(({ topicId, resourceId }) => ({ topicId, resourceId })),
    attachments: attachments.value,
    project: props.project ?? undefined,
  });
}

// ⌘/Ctrl+Enter launches (the chat composer's send convention); bare Enter
// inserts a newline so a goal can be multi-line. The mention menu owns
// the arrow keys and Escape while it's open.
function onGoalKeydown(event: KeyboardEvent) {
  if (event.key === "Enter" && !event.shiftKey && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    if (mentionRange.value && matchingResources.value.length) {
      chooseMention(matchingResources.value[mentionIndex.value] || matchingResources.value[0]);
    } else {
      submit();
    }
    return;
  }
  if (!mentionRange.value) return;
  if (event.key === "Escape") { event.preventDefault(); mentionRange.value = null; }
  else if (event.key === "ArrowDown" && matchingResources.value.length) {
    event.preventDefault(); mentionIndex.value = (mentionIndex.value + 1) % matchingResources.value.length;
  } else if (event.key === "ArrowUp" && matchingResources.value.length) {
    event.preventDefault(); mentionIndex.value = (mentionIndex.value - 1 + matchingResources.value.length) % matchingResources.value.length;
  }
}
</script>

<template>
  <section class="new-thread-sheet" aria-label="New thread">
    <div class="nts-inner">
      <header class="nts-head">
        <h1>New thread</h1>
        <span v-if="props.project" class="nts-project" :title="`Files under project ${props.project.name}`">{{ props.project.name }}</span>
        <button
          class="nts-close"
          title="Close (esc)"
          aria-label="Close new thread"
          @click="emit('close')"
        >
          ✕
        </button>
      </header>

      <label class="nts-field">
        <span class="nts-field-name">Goal</span>
        <div class="nts-body-wrap">
          <textarea
            ref="taskEl"
            v-model="task"
            rows="6"
            placeholder="Describe the goal — it becomes the agent's first message… Use @ to mention an existing resource."
            @input="updateMention"
            @click="updateMention"
            @keydown="onGoalKeydown"
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
        <span class="nts-hint">⌘/Ctrl+Enter to launch · Enter for a new line</span>
      </label>

      <div class="attachment-row" @dragover.prevent @drop.prevent="($event) => $event.dataTransfer?.files && addFiles($event.dataTransfer.files)">
        <label class="attachment-pick">+ Attach files or images<input type="file" multiple :disabled="attachmentLoading" aria-label="Attach files or images to new thread" @change="onFileInput" /></label>
        <span v-for="(file, index) in attachments" :key="file.name" class="attachment-chip">
          <img v-if="imagePreviewUrl(file)" :src="imagePreviewUrl(file)!" class="attachment-preview" alt="" />
          {{ file.name }} <button type="button" :aria-label="`Remove ${file.name}`" @click="attachments.splice(index, 1)">×</button>
        </span>
        <span v-if="attachmentError" class="attachment-error">{{ attachmentError }}</span>
        <span v-if="attachmentLoading" class="attachment-hint">Reading files…</span>
      </div>

      <RepoBaseFields v-model:repo="repo" v-model:base="base" @submit="submit" />

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
            list="nts-thread-models"
            :placeholder="agent ? 'Model · runtime default' : (selectedProfile?.model || 'Model · runtime default')"
            aria-label="Model override"
            spellcheck="false"
          />
          <datalist id="nts-thread-models">
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
          :disabled="(!task.trim() && !attachments.length) || props.launching || attachmentLoading"
          @click="submit"
        >
          {{ props.launching ? "Launching…" : "Launch thread" }}
        </button>
      </div>
    </div>
  </section>
</template>

<style scoped>
/* Launch config row (profile/agent/model/effort) — same look as the
   topic sheet's controls. */
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
