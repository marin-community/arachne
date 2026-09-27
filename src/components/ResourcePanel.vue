<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-shell";
import MarkdownIt from "markdown-it";
import DOMPurify from "dompurify";
import type { SessionSummary } from "../App.vue";

interface TopicResource {
  id: string;
  kind: string;
  title: string;
  repository: string;
  reference: string | null;
  path: string | null;
  url: string | null;
}
interface TopicResourcesView {
  resources: TopicResource[];
  revision: number;
}
interface TopicResourceContent {
  resource: TopicResource;
  content: string;
}

const props = defineProps<{ topic: SessionSummary; embedded?: boolean }>();
const emit = defineEmits<{ (e: "close"): void; (e: "error", message: string): void }>();
const snapshot = ref<TopicResourcesView>({ resources: [], revision: 0 });
const selectedId = ref<string | null>(null);
const content = ref("");
const markdown = new MarkdownIt({ html: false, linkify: true, breaks: false });
const renderedContent = computed(() => DOMPurify.sanitize(markdown.render(content.value)));
const loading = ref(false);
const reading = ref(false);
const saving = ref(false);
const showAttach = ref(false);
const formKind = ref<"design_document" | "file">("design_document");
const formTitle = ref("Design document");
const formPath = ref("docs/design.md");
const message = ref("");

const selected = computed(() => snapshot.value.resources.find((resource) => resource.id === selectedId.value) ?? null);
const sortedResources = computed(() => [...snapshot.value.resources].sort((a, b) =>
  Number(b.kind === "design_document") - Number(a.kind === "design_document") || a.title.localeCompare(b.title),
));

async function refresh() {
  const topicId = props.topic.id;
  loading.value = true;
  message.value = "";
  try {
    const next = await invoke<TopicResourcesView>("topic_resources", { topicId });
    if (!next || !Array.isArray(next.resources)) throw new Error("Loom returned an invalid resource list");
    if (topicId !== props.topic.id) return;
    snapshot.value = next;
    if (!next.resources.some((resource) => resource.id === selectedId.value)) {
      selectedId.value = next.resources.find((resource) => resource.kind === "design_document")?.id
        ?? next.resources[0]?.id ?? null;
    } else if (selectedId.value) {
      void selectResource(selectedId.value);
    }
  } catch (error: any) {
    message.value = error?.message ?? String(error);
  } finally {
    loading.value = false;
  }
}

async function selectResource(id: string) {
  selectedId.value = id;
  content.value = "";
  reading.value = true;
  try {
    const view = await invoke<TopicResourceContent>("read_topic_resource", { topicId: props.topic.id, resourceId: id });
    if (selectedId.value === id) content.value = view.content;
  } catch (error: any) {
    if (selectedId.value === id) content.value = `Preview unavailable: ${error?.message ?? String(error)}`;
  } finally {
    reading.value = false;
  }
}

watch(() => props.topic.id, () => {
  snapshot.value = { resources: [], revision: 0 };
  selectedId.value = null;
  content.value = "";
  void refresh();
}, { immediate: true });
watch(selectedId, (id) => {
  if (id) void selectResource(id);
  else content.value = "";
});

function chooseKind() {
  if (formKind.value === "design_document") {
    formTitle.value = "Design document";
    formPath.value = "docs/design.md";
  } else {
    formTitle.value = "";
    formPath.value = "";
  }
}

async function attach() {
  const title = formTitle.value.trim();
  const path = formPath.value.trim();
  if (!title || !path || saving.value) return;
  saving.value = true;
  message.value = "";
  try {
    const next = await invoke<TopicResourcesView>("attach_topic_resource", {
      topicId: props.topic.id,
      resource: {
        kind: formKind.value,
        title,
        repository: props.topic.branch.repo_root,
        reference: props.topic.branch.branch,
        path,
        url: null,
      },
      expectedRevision: snapshot.value.revision,
    });
    snapshot.value = next;
    const attached = next.resources.find((resource) => resource.kind === formKind.value && resource.path === path);
    selectedId.value = attached?.id ?? selectedId.value;
    showAttach.value = false;
    message.value = "Resource attached to this topic.";
  } catch (error: any) {
    await refresh();
    message.value = error?.message ?? String(error);
  } finally {
    saving.value = false;
  }
}

async function detach(resource: TopicResource) {
  if (saving.value) return;
  saving.value = true;
  message.value = "";
  try {
    snapshot.value = await invoke<TopicResourcesView>("detach_topic_resource", {
      topicId: props.topic.id,
      resourceId: resource.id,
      expectedRevision: snapshot.value.revision,
    });
    if (selectedId.value === resource.id) selectedId.value = null;
    message.value = "Attachment removed; the file was not changed.";
  } catch (error: any) {
    await refresh();
    message.value = error?.message ?? String(error);
  } finally {
    saving.value = false;
  }
}

async function openInZed(resource: TopicResource) {
  try {
    await invoke("open_topic_resource_in_zed", { topicId: props.topic.id, resourceId: resource.id });
  } catch (error: any) {
    emit("error", error?.message ?? String(error));
  }
}

async function onPreviewClick(event: MouseEvent) {
  const link = (event.target as HTMLElement).closest("a[href]");
  if (!link) return;
  event.preventDefault();
  const href = link.getAttribute("href") ?? "";
  if (!/^https?:\/\//i.test(href)) {
    message.value = "Open related repository files in Zed.";
    return;
  }
  try {
    await open(href);
  } catch (error: any) {
    emit("error", error?.message ?? String(error));
  }
}
</script>

<template>
  <aside class="resource-panel" :class="{ embedded }" :aria-label="embedded ? 'Topic resources tab' : 'Topic resources'">
    <header v-if="!embedded" class="resource-panel-head">
      <div>
        <strong>Resources</strong>
        <div class="resource-panel-topic">{{ topic.branch.title || topic.branch.name }}</div>
      </div>
      <button v-if="!embedded" title="Refresh resources" :disabled="loading" @click="refresh">↻</button>
      <button v-if="!embedded" title="Close resources" aria-label="Close resources" @click="emit('close')">×</button>
    </header>
    <div class="resource-panel-list">
      <div v-if="loading && !snapshot.resources.length" class="resource-panel-empty">Loading…</div>
      <div v-else-if="!snapshot.resources.length" class="resource-panel-empty">
        No resources attached yet. Attach a design document or file so it stays with this topic.
      </div>
      <button v-for="resource in sortedResources" :key="resource.id" class="resource-panel-item"
        :class="{ selected: resource.id === selectedId }" @click="selectedId = resource.id">
        <span class="resource-panel-icon">{{ resource.kind === 'design_document' ? '◇' : '▤' }}</span>
        <span class="resource-panel-item-text">
          <strong>{{ resource.title }}</strong>
          <small>{{ resource.path || resource.url || resource.reference }}</small>
        </span>
      </button>
    </div>
    <button class="resource-panel-add" @click="showAttach = !showAttach">{{ showAttach ? 'Cancel attachment' : '+ Attach resource' }}</button>
    <div v-if="showAttach" class="resource-panel-form">
      <label>Kind
        <select v-model="formKind" @change="chooseKind">
          <option value="design_document">Design document</option>
          <option value="file">File</option>
        </select>
      </label>
      <label>Title <input v-model="formTitle" placeholder="Design document" /></label>
      <label>Path in topic branch <input v-model="formPath" placeholder="docs/design.md" spellcheck="false" /></label>
      <button class="primary" :disabled="saving || !formTitle.trim() || !formPath.trim()" @click="attach">Attach</button>
    </div>
    <div v-if="message" class="resource-panel-message">{{ message }}</div>
    <div v-if="selected" class="resource-panel-preview">
      <div class="resource-panel-preview-head">
        <strong>{{ selected.title }}</strong>
        <button :disabled="!selected.path" @click="openInZed(selected)">Open in Zed</button>
        <button class="danger" :disabled="saving" title="Remove attachment; keep the file" @click="detach(selected)">Remove</button>
      </div>
      <div class="resource-panel-location" :title="`${selected.repository} · ${selected.reference} · ${selected.path}`">
        {{ selected.reference }} · {{ selected.path }}
      </div>
      <div v-if="reading" class="resource-panel-empty">Loading preview…</div>
      <div v-else class="resource-panel-content" @click="onPreviewClick" v-html="renderedContent"></div>
    </div>
  </aside>
</template>

<style scoped>
.resource-panel { grid-area: resources; display: flex; flex-direction: column; min-width: 0; overflow: hidden; border-left: 1px solid var(--border); background: var(--bg-raised); }
/* Embedded inside the inspector's Resources tab: the inspector owns the
   column, the header, and the border-left; only the panel body renders. */
.resource-panel.embedded { grid-area: auto; flex: 1; border-left: 0; }
.resource-panel-head { display: flex; align-items: center; gap: 6px; padding: 10px 12px; border-bottom: 1px solid var(--border); }
.resource-panel-head > div { flex: 1; min-width: 0; }
.resource-panel-head strong { font-size: 13px; }
.resource-panel-topic { color: var(--text-dim); font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.resource-panel-list { overflow-y: auto; max-height: 35%; padding: 7px; }
.resource-panel.embedded .resource-panel-list { max-height: none; flex: 1; }
.resource-panel-item { width: 100%; display: flex; align-items: center; gap: 8px; text-align: left; border: 0; background: transparent; padding: 8px; }
.resource-panel-item:hover, .resource-panel-item.selected { background: var(--bg-hover); }
.resource-panel-icon { font-size: 17px; color: var(--accent); }
.resource-panel-item-text { display: flex; flex-direction: column; min-width: 0; }
.resource-panel-item-text strong, .resource-panel-item-text small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.resource-panel-item-text small, .resource-panel-empty, .resource-panel-location { color: var(--text-dim); font-size: 11px; }
.resource-panel-empty { padding: 12px; }
.resource-panel-add { margin: 6px 10px; }
.resource-panel-form { display: grid; gap: 8px; padding: 10px; border-bottom: 1px solid var(--border); }
.resource-panel-form label { display: grid; gap: 3px; color: var(--text-dim); font-size: 11px; }
.resource-panel-form input, .resource-panel-form select { width: 100%; min-width: 0; background: var(--bg); color: var(--text); border: 1px solid var(--border); border-radius: 5px; padding: 6px; }
.resource-panel-message { padding: 6px 12px; color: var(--accent); font-size: 11px; }
.resource-panel-preview { display: flex; flex-direction: column; min-height: 0; flex: 1; border-top: 1px solid var(--border); }
.resource-panel-preview-head { display: flex; align-items: center; gap: 5px; padding: 10px; }
.resource-panel-preview-head strong { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.resource-panel-preview-head button { white-space: nowrap; }
.resource-panel-location { padding: 0 10px 7px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.resource-panel-content { margin: 0; padding: 12px 16px; overflow: auto; flex: 1; user-select: text; overflow-wrap: anywhere; font: 12px/1.6 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; background: var(--bg); }
.resource-panel-content :deep(h1), .resource-panel-content :deep(h2), .resource-panel-content :deep(h3) { line-height: 1.25; margin: 1.2em 0 .5em; }
.resource-panel-content :deep(h1) { font-size: 20px; margin-top: .2em; }
.resource-panel-content :deep(h2) { font-size: 16px; border-bottom: 1px solid var(--border); padding-bottom: .3em; }
.resource-panel-content :deep(h3) { font-size: 13px; }
.resource-panel-content :deep(p), .resource-panel-content :deep(ul), .resource-panel-content :deep(ol) { margin: .6em 0; }
.resource-panel-content :deep(ul), .resource-panel-content :deep(ol) { padding-left: 1.6em; }
.resource-panel-content :deep(code) { font: 11px/1.4 var(--mono); background: var(--bg-hover); padding: 1px 3px; border-radius: 3px; }
.resource-panel-content :deep(pre) { overflow-x: auto; padding: 10px; background: var(--bg-hover); border-radius: 5px; }
.resource-panel-content :deep(pre code) { background: transparent; padding: 0; }
.resource-panel-content :deep(a) { color: var(--accent); }
</style>
