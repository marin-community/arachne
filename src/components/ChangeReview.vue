<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import CheckoutEditor from "./CheckoutEditor.vue";
import type { SessionSummary } from "../App.vue";
import { preparationRequest } from "../reviewRequest";

interface ChangeLine {
  kind: "context" | "addition" | "deletion";
  old_line: number | null;
  new_line: number | null;
  text: string;
}
interface ChangeHunk {
  header: string;
  lines: ChangeLine[];
  truncated: boolean;
}
interface ChangeFile {
  status: string;
  path: { display: string; bytes: string };
  old_path: { display: string; bytes: string } | null;
  sources: string[];
  additions: number | null;
  deletions: number | null;
  content: "text" | "binary" | "oversize" | "unsupported";
  hunks: ChangeHunk[];
  truncated: boolean;
}
interface ChangeSet {
  version: string | null;
  base: { state: "available"; reference: string; oid: string } | { state: "unavailable"; reference: string; reason: string };
  head_oid: string | null;
  totals: { files: number; additions: number; deletions: number; truncated: boolean };
  files: ChangeFile[];
  truncated: boolean;
}

const props = withDefaults(defineProps<{ sessionId: string; canEdit?: boolean; isTopic?: boolean; integrationTarget?: SessionSummary | null; embedded?: boolean }>(), { canEdit: false, isTopic: false, embedded: false });
const emit = defineEmits<{ (e: "requested"): void; (e: "saved"): void }>();
const editPath = ref<string | null>(null);
const editedPaths = ref<string[]>([]);
const fileQuery = ref("");
const fileOptions = ref<string[]>([]);
let completionGeneration = 0;
const visibleFiles = computed(() => changes.value?.files.filter(file => file.path.display.toLowerCase().includes(fileQuery.value.toLowerCase())) ?? []);
const requestMode = ref<"review" | "prepare" | null>(null);
const requestText = ref("");
const requesting = ref(false);
const requestNote = ref("");
let requestGeneration = 0;
watch(fileQuery, async query => {
  const current = ++completionGeneration;
  fileOptions.value = [];
  if (!props.canEdit || !query.trim()) return;
  try {
    const files = await invoke<string[]>("complete_files", { id: props.sessionId, query });
    if (current === completionGeneration) fileOptions.value = files.slice(0, 20);
  } catch { /* The diff remains usable when completion is unavailable. */ }
});
function prepareRequest(mode: "review" | "prepare") {
  requestMode.value = mode;
  const revision = changes.value?.head_oid ? `The preview showed HEAD ${changes.value.head_oid}; re-read the current checkout, including uncommitted manual edits.` : "Inspect the current checkout, including uncommitted manual edits.";
  requestText.value = mode === "review"
    ? `Review this thread's changes for correctness, regressions, and missing validation. ${revision} Use a review worker if useful. Report actionable findings with file locations and distinguish verified checks from unverified claims. Do not integrate, land, push, or open a PR as part of this review.`
    : preparationRequest(props.isTopic, revision, props.integrationTarget ?? null, editedPaths.value);
}
async function sendRequest() {
  if (requesting.value || !requestText.value.trim()) return;
  const id = props.sessionId;
  const request = ++requestGeneration;
  requesting.value = true;
  requestNote.value = "";
  try {
    await invoke("send_input", { id, text: requestText.value });
    if (id !== props.sessionId || request !== requestGeneration) return;
    requestNote.value = "Request sent to this thread. Review and validation results will appear in its conversation.";
    requestMode.value = null;
    emit("requested");
  } catch (e: unknown) {
    if (request === requestGeneration) requestNote.value = (e as { message?: string })?.message ?? String(e);
  } finally { if (request === requestGeneration) requesting.value = false; }
}
function onSaved() {
  if (editPath.value && !editedPaths.value.includes(editPath.value)) editedPaths.value.push(editPath.value);
  void refresh(); emit("saved");
}
const changes = ref<ChangeSet | null>(null);
const loading = ref(false);
const error = ref("");
let generation = 0;

async function refresh() {
  const current = ++generation;
  const sessionId = props.sessionId;
  loading.value = true;
  error.value = "";
  try {
    const result = await invoke<ChangeSet>("work_changes", { sessionId });
    if (current === generation && sessionId === props.sessionId) changes.value = result;
  } catch (e: unknown) {
    if (current === generation) {
      changes.value = null;
      error.value = (e as { message?: string })?.message ?? String(e);
    }
  } finally {
    if (current === generation) loading.value = false;
  }
}

watch(() => props.sessionId, () => {
  changes.value = null;
  editPath.value = null;
  editedPaths.value = [];
  fileQuery.value = "";
  requestMode.value = null;
  requestNote.value = "";
  requestGeneration++;
  requesting.value = false;
  void refresh();
}, { immediate: true });

function lineNumber(value: number | null): string {
  return value == null ? "" : String(value);
}
</script>

<template>
  <section class="change-review" :class="{ embedded }" aria-label="Review changes">
    <div class="change-review-heading">
      <div>
        <strong>Review changes</strong>
        <span v-if="changes" class="change-review-count">{{ changes.totals.files }} files · +{{ changes.totals.additions }} −{{ changes.totals.deletions }}</span>
      </div>
      <button :disabled="loading" @click="refresh">{{ loading ? "Loading…" : "Refresh" }}</button>
    </div>
    <div class="review-toolbar">
      <button :disabled="loading || requesting" @click="prepareRequest('review')">Review with agent</button>
      <button :disabled="loading || requesting" @click="prepareRequest('prepare')">Prepare {{ isTopic ? "landing" : "integration" }}</button>
    </div>
    <div v-if="requestMode" class="review-request">
      <label :for="`review-request-${sessionId}`">{{ requestMode === 'review' ? "Review request" : "Preparation request" }}</label>
      <textarea :id="`review-request-${sessionId}`" v-model="requestText" :disabled="requesting" rows="5" />
      <div><button :disabled="requesting" @click="requestMode = null">Cancel</button><button class="primary" :disabled="requesting || !requestText.trim()" @click="sendRequest">{{ requesting ? "Sending…" : "Send to agent" }}</button></div>
    </div>
    <div v-if="requestNote" class="change-review-state" role="status">{{ requestNote }}</div>
    <div class="review-file-picker">
      <input v-model="fileQuery" :list="`review-files-${sessionId}`" placeholder="Filter changes or open a file…" aria-label="Find a checkout file" @keydown.enter.prevent="canEdit && fileQuery.trim() && (editPath = fileQuery.trim())" />
      <datalist :id="`review-files-${sessionId}`"><option v-for="path in fileOptions" :key="path" :value="path" /></datalist>
      <button v-if="canEdit" :disabled="!fileQuery.trim()" @click="editPath = fileQuery.trim()">Edit file</button>
    </div>
    <div v-if="loading && !changes" class="change-review-state" role="status">Loading the current checkout diff…</div>
    <div v-else-if="error" class="change-review-state change-review-error" role="alert">Could not load changes: {{ error }}</div>
    <template v-else-if="changes">
      <div v-if="changes.base.state === 'unavailable'" class="change-review-state" role="status">
        Base {{ changes.base.reference }} is unavailable ({{ changes.base.reason.replaceAll('_', ' ') }}). There is no reliable comparison yet.
      </div>
      <template v-else>
        <div class="change-review-base" :title="`Base ${changes.base.oid}${changes.head_oid ? ` · HEAD ${changes.head_oid}` : ''}`">
          Compared with {{ changes.base.reference }} · {{ changes.base.oid.slice(0, 10) }}
        </div>
        <div v-if="changes.truncated || changes.totals.truncated" class="change-review-state" role="status">
          Large diff: Loom limited this preview. Open the checkout for the complete changes.
        </div>
        <div v-if="changes.head_oid" class="change-review-base">Source HEAD · {{ changes.head_oid.slice(0, 10) }} · includes current checkout edits</div>
        <div v-if="changes.files.length === 0" class="change-review-state">No changes against this branch’s base.</div>
        <div v-else-if="!visibleFiles.length" class="change-review-state">No changed files match this filter.</div>
        <div v-else class="change-review-files">
          <details v-for="(file, index) in visibleFiles" :key="file.path.bytes" class="change-review-file" :open="index === 0">
            <summary>
              <span class="change-review-status">{{ file.status.replaceAll('_', ' ') }}</span>
              <span class="change-review-path" :title="file.path.display">{{ file.old_path ? `${file.old_path.display} → ` : "" }}{{ file.path.display }}</span>
              <span class="change-review-file-stats">{{ file.additions == null ? "" : `+${file.additions}` }} {{ file.deletions == null ? "" : `−${file.deletions}` }}</span>
            </summary>
            <div v-if="canEdit && file.content === 'text' && file.status !== 'deleted'" class="review-file-actions"><button @click="editPath = file.path.display">Edit file</button><span>Changes save to this thread’s checkout</span></div>
            <div v-if="file.content !== 'text'" class="change-review-state">{{ file.content }} content cannot be previewed here.</div>
            <div v-else-if="file.hunks.length === 0" class="change-review-state">No text hunks available for this file.</div>
            <div v-else class="change-review-hunks">
              <div v-for="(hunk, hunkIndex) in file.hunks" :key="hunkIndex" class="change-review-hunk">
                <div class="change-review-hunk-header">{{ hunk.header }}</div>
                <div v-for="(line, lineIndex) in hunk.lines" :key="lineIndex" class="change-review-line" :class="line.kind">
                  <span class="change-review-number">{{ lineNumber(line.old_line) }}</span>
                  <span class="change-review-number">{{ lineNumber(line.new_line) }}</span>
                  <span class="change-review-marker">{{ line.kind === 'addition' ? '+' : line.kind === 'deletion' ? '−' : ' ' }}</span>
                  <span class="change-review-text">{{ line.text }}</span>
                </div>
                <div v-if="hunk.truncated" class="change-review-state">Hunk preview truncated.</div>
              </div>
            </div>
            <div v-if="file.truncated" class="change-review-state">File preview truncated.</div>
          </details>
        </div>
      </template>
    </template>
    <CheckoutEditor v-if="editPath" :key="`${sessionId}:${editPath}`" :session-id="sessionId" :path="editPath" @close="editPath = null" @saved="onSaved" />
  </section>
</template>

<style scoped>
.change-review.embedded { max-height: none; height: 100%; border: 0; }
.review-toolbar, .review-file-picker, .review-file-actions { display: flex; gap: 6px; align-items: center; padding: 8px 12px; flex-wrap: wrap; }
.review-toolbar button { flex: 1; white-space: nowrap; }
.review-file-picker input { flex: 1; min-width: 100px; padding: 6px 8px; color: var(--text); background: var(--bg); border: 1px solid var(--border); border-radius: 5px; font-size: 11px; }
.review-file-actions { padding: 5px 16px; }
.review-file-actions span { color: var(--text-dim); font-size: 10px; }
.review-request { display: grid; gap: 6px; padding: 10px 12px; border-block: 1px solid var(--border); background: var(--bg); }
.review-request label { font-size: 11px; color: var(--text-dim); }
.review-request textarea { width: 100%; box-sizing: border-box; resize: vertical; min-height: 100px; font-family: inherit; font-size: 12px; line-height: 1.5; color: var(--text); background: var(--bg-raised); border: 1px solid var(--border); border-radius: 5px; padding: 8px; }
.review-request > div { display: flex; justify-content: flex-end; gap: 6px; }
.change-review { border-bottom: 1px solid var(--border); min-height: 0; max-height: min(52vh, 620px); overflow: auto; background: var(--bg-raised); }
.change-review-heading { position: sticky; top: 0; z-index: 1; display: flex; align-items: center; justify-content: space-between; gap: 10px; padding: 8px 16px; background: var(--bg-raised); border-bottom: 1px solid var(--border); }
.change-review-heading strong { font-size: 12px; }
.change-review-count, .change-review-base { margin-left: 10px; color: var(--text-dim); font-size: 11px; }
.change-review-base { margin: 0; padding: 6px 16px; font-family: var(--mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.change-review-state { padding: 10px 16px; color: var(--text-dim); font-size: 12px; }
.change-review-error { color: var(--blocked); overflow-wrap: anywhere; }
.change-review-files { padding-bottom: 6px; }
.change-review-file { border-top: 1px solid var(--border); }
.change-review-file summary { display: flex; align-items: center; gap: 8px; padding: 7px 16px; cursor: pointer; font-size: 11px; }
.change-review-file summary:hover { background: var(--bg-hover); }
.change-review-status { color: var(--text-dim); text-transform: capitalize; min-width: 54px; }
.change-review-path { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--mono); }
.change-review-file-stats { margin-left: auto; color: var(--text-dim); white-space: nowrap; font-family: var(--mono); }
.change-review-hunks { overflow-x: auto; font: 11px/1.45 var(--mono); }
.change-review-hunk-header { padding: 4px 16px; color: var(--accent); background: rgba(110, 168, 254, .07); white-space: pre; }
.change-review-line { display: flex; min-width: max-content; white-space: pre; }
.change-review-line.addition { background: rgba(63, 185, 80, .11); }
.change-review-line.deletion { background: rgba(248, 81, 73, .11); }
.change-review-number { width: 38px; flex: none; padding: 0 4px; text-align: right; color: var(--text-dim); border-right: 1px solid var(--border); user-select: none; }
.change-review-marker { width: 20px; flex: none; text-align: center; }
.change-review-text { padding-right: 16px; user-select: text; }
</style>
