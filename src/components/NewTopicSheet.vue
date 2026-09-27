<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { LaunchOptions, ResourceMention, SessionSummary } from "../App.vue";
import { addAttachments, MAX_LAUNCH_TOTAL_BYTES, type FileAttachment } from "../attachments";

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
    meta?: { title?: string; description?: string; mentions?: ResourceMention[]; attachments?: FileAttachment[]; profile?: string; agent?: string; model?: string; effort?: string },
  ): void;
}>();

// --- Draft state ---------------------------------------------------------------

const title = ref("");
const body = ref("");
const repo = ref("marin-community/arachne");
const titleEl = ref<HTMLInputElement | null>(null);
const bodyEl = ref<HTMLTextAreaElement | null>(null);

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
  if (!event.clipboardData?.files.length) return;
  event.preventDefault();
  void addFiles(event.clipboardData.files);
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
  mentions.value.push({ token, topicId: resource.topicId, resourceId: resource.id });
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

// --- Sheet lifecycle ---------------------------------------------------------------

// Keyboard-first: focus the title on open, and Esc always closes — unless
// the mention menu is open (it owns Esc first).
function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") emit("close");
}
onMounted(async () => {
  document.addEventListener("keydown", onKeydown);
  await nextTick();
  titleEl.value?.focus();
});
onUnmounted(() => document.removeEventListener("keydown", onKeydown));

function submit() {
  const t = title.value.trim();
  const b = body.value.trim();
  if ((!t && !b && !attachments.value.length) || props.launching || attachmentLoading.value) return;
  // The body is both the agent's initial goal and the durable branch
  // description; either alone can seed a topic. Drafts are deliberately
  // not cleared here — success unmounts the sheet, failure keeps them.
  emit("launch", b || t || `Review ${attachments.value[0].name}`, repo.value.trim(), {
    title: t || undefined,
    description: b || undefined,
    mentions: mentions.value.filter((mention) => b.includes(mention.token))
      .map(({ topicId, resourceId }) => ({ topicId, resourceId })),
    attachments: attachments.value,
    ...launchConfig(),
  });
}
</script>

<template>
  <section class="new-thread-sheet" aria-label="New topic">
    <div class="nts-inner">
      <header class="nts-head">
        <h1>New topic</h1>
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

      <label class="nts-field">
        <span class="nts-field-name">Repository</span>
        <input
          v-model="repo"
          placeholder="owner/name"
          spellcheck="false"
          @keydown.enter.prevent="submit"
        />
        <span class="nts-hint">
          A fresh worktree + branch is created from this repo.
        </span>
      </label>

      <div
        class="attachment-row"
        @dragover.prevent
        @drop.prevent="($event) => $event.dataTransfer?.files && addFiles($event.dataTransfer.files)"
      >
        <label class="attachment-pick">+ Attach files<input type="file" multiple :disabled="attachmentLoading" aria-label="Attach files to new topic" @change="onFileInput" /></label>
        <span v-for="(file, index) in attachments" :key="file.name" class="attachment-chip">
          {{ file.name }} <button type="button" :aria-label="`Remove ${file.name}`" @click="attachments.splice(index, 1)">×</button>
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
   sidebar's quick-task controls, scoped here alongside its sibling. */
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
