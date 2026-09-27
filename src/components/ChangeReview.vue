<script setup lang="ts">
import { ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";

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

const props = defineProps<{ sessionId: string }>();
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
      error.value = e instanceof Error ? e.message : String(e);
    }
  } finally {
    if (current === generation) loading.value = false;
  }
}

watch(() => props.sessionId, () => {
  changes.value = null;
  void refresh();
}, { immediate: true });

function lineNumber(value: number | null): string {
  return value == null ? "" : String(value);
}
</script>

<template>
  <section class="change-review" aria-label="Review changes">
    <div class="change-review-heading">
      <div>
        <strong>Review changes</strong>
        <span v-if="changes" class="change-review-count">{{ changes.totals.files }} files · +{{ changes.totals.additions }} −{{ changes.totals.deletions }}</span>
      </div>
      <button :disabled="loading" @click="refresh">{{ loading ? "Loading…" : "Refresh" }}</button>
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
        <div v-if="changes.files.length === 0" class="change-review-state">No changes against this branch’s base.</div>
        <div v-else class="change-review-files">
          <details v-for="(file, index) in changes.files" :key="file.path.bytes" class="change-review-file" :open="index === 0">
            <summary>
              <span class="change-review-status">{{ file.status.replaceAll('_', ' ') }}</span>
              <span class="change-review-path" :title="file.path.display">{{ file.old_path ? `${file.old_path.display} → ` : "" }}{{ file.path.display }}</span>
              <span class="change-review-file-stats">{{ file.additions == null ? "" : `+${file.additions}` }} {{ file.deletions == null ? "" : `−${file.deletions}` }}</span>
            </summary>
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
  </section>
</template>

<style scoped>
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
