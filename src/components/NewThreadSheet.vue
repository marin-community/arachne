<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { LaunchOptions, ResourceMention, SessionSummary } from "../App.vue";
import { addAttachments, filesFromClipboard, imagePreviewUrl, MAX_LAUNCH_TOTAL_BYTES, type FileAttachment } from "../attachments";
import { pasteAsPlainText } from "../composerPaste";
import { recentRepositories, rememberRepository } from "../launchDefaults";
import { launchSelection } from "../launch";
import { useFileCompletion } from "../useFileCompletion";
import RepoBaseFields from "./RepoBaseFields.vue";

// New thread opens in the main pane with the same header, conversation,
// and composer as New track. Repository/base sit in a setup card; the
// first send launches the standalone thread through App's launchTask →
// selectSession flow. Failed launches leave the draft here for retry.

const props = defineProps<{
  fleet: SessionSummary[];
  launching?: boolean;
  error?: string | null;
  launchOptions: LaunchOptions | null;
}>();

const emit = defineEmits<{
  (e: "close"): void;
  (
    e: "launch",
    task: string,
    repo: string,
    meta?: { base?: string; mentions?: ResourceMention[]; attachments?: FileAttachment[]; profile?: string; agent?: string; model?: string; effort?: string },
  ): void;
}>();

const task = ref("");
const repositorySuggestions = computed(() => [...new Set(props.fleet.map((s) => s.github_repo || s.branch.repo_root).filter(Boolean))]);
const repo = ref(recentRepositories(localStorage)[0] || (repositorySuggestions.value.length === 1 ? repositorySuggestions.value[0] : ""));
const base = ref("");
const taskEl = ref<HTMLTextAreaElement | null>(null);
const completion = useFileCompletion(task, computed(() => repo.value.trim() ? { repo: repo.value.trim() } : null), taskEl);
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
  // The goal body is markdown (a copied turn pastes its source); take the
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

// --- Launch config -----------------------------------------------------------

const profile = ref("default");
const agent = ref(launchSelection.value.agent);
const model = ref(launchSelection.value.model);
const effort = ref(launchSelection.value.effort);
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
  completion.updateCaret();
  const caret = taskEl.value?.selectionStart ?? task.value.length;
  const match = /(?:^|\s)@\{?([^@{}\n]{0,64})$/.exec(task.value.slice(0, caret));
  const wasOpen = !!mentionRange.value;
  mentionRange.value = match ? { start: task.value.slice(0, caret).lastIndexOf("@"), end: caret, query: match[1] } : null;
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

// --- Chat lifecycle ---------------------------------------------------------------

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
  const t = task.value.trim();
  if ((!t && !attachments.value.length) || props.launching || attachmentLoading.value) return;
  // Success unmounts this chat; failure keeps the text for a retry.
  rememberRepository(localStorage, repo.value);
  emit("launch", t || `Review ${attachments.value[0].name}`, repo.value.trim(), {
    ...launchConfig(),
    base: base.value.trim() || undefined,
    mentions: mentions.value.filter((mention) => t.includes(mention.token))
      .map(({ topicId, resourceId }) => ({ topicId, resourceId })),
    attachments: attachments.value,
  });
}

// ⌘/Ctrl+Enter launches (the chat composer's send convention); bare Enter
// inserts a newline so a goal can be multi-line. The mention menu owns
// the arrow keys and Escape while it's open.
function onGoalKeydown(event: KeyboardEvent) {
  if (completion.onKeydown(event)) { mentionRange.value = null; return; }
  if (event.key === "Enter" && !event.shiftKey && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    submit();
    return;
  }
  if (!mentionRange.value) return;
  if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); mentionRange.value = null; }
  else if (event.key === "ArrowDown" && matchingResources.value.length) {
    event.preventDefault(); mentionIndex.value = (mentionIndex.value + 1) % matchingResources.value.length;
  } else if (event.key === "ArrowUp" && matchingResources.value.length) {
    event.preventDefault(); mentionIndex.value = (mentionIndex.value - 1 + matchingResources.value.length) % matchingResources.value.length;
  } else if ((event.key === "Tab" || (event.key === "Enter" && !event.shiftKey)) && matchingResources.value.length) {
    event.preventDefault(); chooseMention(matchingResources.value[mentionIndex.value] || matchingResources.value[0]);
  }
}
</script>

<template>
  <section class="main new-thread" aria-label="New thread">
    <div class="thread-header">
      <div class="meta">
        <div class="name">New thread</div>
        <div class="sub">first send launches the thread</div>
      </div>
      <button
        class="nts-close"
        title="Close (esc)"
        aria-label="Close new thread"
        @click="emit('close')"
      >
        ✕
      </button>
    </div>

    <div class="conversation">
      <div class="block new-thread-setup">
        <div class="who">Thread setup <em>· goes away at launch</em></div>
        <div class="body">
          <RepoBaseFields v-model:repo="repo" v-model:base="base" :repositories="repositorySuggestions" />
        </div>
      </div>
      <div v-if="props.error" class="nts-error new-thread-error" role="alert">{{ props.error }}</div>
    </div>

    <div class="composer-wrap" @dragover.prevent @drop.prevent="($event) => $event.dataTransfer?.files && addFiles($event.dataTransfer.files)">
      <div v-if="completion.visible.value" class="mention-menu" role="listbox" aria-label="Repository files">
        <button v-for="(path, index) in completion.matches.value" :key="path" type="button" role="option"
          :aria-selected="index === completion.selected.value" :class="{ selected: index === completion.selected.value }"
          @mousedown.prevent="completion.choose(path); mentionRange = null">
          <strong>@{{ path }}</strong>
        </button>
      </div>
      <div v-else-if="mentionRange" class="mention-menu" role="listbox" aria-label="Existing track resources">
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
            ref="taskEl"
            v-model="task"
            aria-label="First message"
            placeholder="Describe the goal, context, and what a good result looks like… Use @ to mention a repository file or resource."
            @input="updateMention"
            @click="updateMention"
            @keyup="completion.updateCaret"
            @keydown="onGoalKeydown"
            @paste="onPaste"
          ></textarea>
        </div>
        <div class="composer-utility-row">
          <label class="attachment-pick composer-attach" title="Attach files or images" aria-label="Attach files or images">
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 12.5 14.5 6a3 3 0 0 1 4.25 4.25l-8.5 8.5a5 5 0 0 1-7.07-7.07l8.5-8.5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
            <input type="file" multiple :disabled="attachmentLoading" aria-label="Attach files or images to the thread's first message" @change="onFileInput" />
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
              list="nts-thread-models"
              :placeholder="agent ? 'Model' : (selectedProfile?.model || 'Model')"
              aria-label="Model override"
              spellcheck="false"
            />
            <datalist id="nts-thread-models">
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
            :disabled="(!task.trim() && !attachments.length) || props.launching || attachmentLoading"
            :title="props.launching ? 'Launching…' : 'Send — launches the thread (⌘/Ctrl + Enter)'"
            aria-label="Send and launch thread"
            @click="submit"
          >
            <span v-if="props.launching">…</span><span v-else aria-hidden="true">↑</span>
          </button>
        </div>
      </div>
      <div class="composer-hint">⌘/Ctrl + Enter to send and launch · Enter for a new line</div>
    </div>
  </section>
</template>
