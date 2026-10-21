<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useBackendInvoke } from "../backendInvoke";

import CheckoutEditor from "./CheckoutEditor.vue";
import type { SessionSummary } from "../App.vue";
import { useLaunchSelection } from "../launch";
import { preparationRequest, independentReviewRequest, reviewPreviewEvidence, selectedChangeInstructions } from "../reviewRequest";
import { selectedChangePayload, type ChangeSelection } from "../durableChange.ts";
import { commentFromLine, buildCommentPrompt, type ReviewComment } from "../reviewComments.ts";
import { backendOf } from "../backendIdentity.ts";

const invoke = useBackendInvoke();

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
  source_ref: string;
  accepted_source: boolean;
  version: string | null;
  base: { state: "available"; reference: string; oid: string } | { state: "unavailable"; reference: string; reason: string };
  head_oid: string | null;
  totals: { files: number; additions: number; deletions: number; truncated: boolean };
  files: ChangeFile[];
  truncated: boolean;
}

const props = withDefaults(defineProps<{ sessionId: string; selectedChange?: ChangeSelection | null; selectionError?: string; canEdit?: boolean; isTopic?: boolean | null; integrationTarget?: SessionSummary | null; embedded?: boolean }>(), { canEdit: false, isTopic: false, embedded: false });
const launchSelection = useLaunchSelection(() => backendOf(props.sessionId));
const reviewingTrack = computed(() => props.selectedChange ? false : changes.value?.accepted_source ?? props.isTopic);
const checkoutEditable = computed(() => props.canEdit && reviewingTrack.value === false && !props.selectedChange);
const selectedProblem = computed(() => props.selectionError || (props.selectedChange && props.selectedChange.sourceId !== props.sessionId
  ? "The selected Change belongs to another source or Loom backend." : "") || (props.integrationTarget && backendOf(props.integrationTarget.id) !== backendOf(props.sessionId)
  ? "The canonical target belongs to another Loom backend." : ""));
const emit = defineEmits<{ (e: "requested"): void; (e: "saved"): void }>();
const editPath = ref<string | null>(null);
const editedPaths = ref<string[]>([]);
const fileQuery = ref("");
const fileOptions = ref<string[]>([]);
// GitHub-style inline comments. One open composer at a time, keyed by the
// comment anchor so refreshes of the diff data never lose queued comments.
const activeComment = ref<{ path: string; line: ChangeLine } | null>(null);
const comments = ref<ReviewComment[]>([]);
const commentDraft = ref("");
const commentSendMode = ref(false);
const commentMessage = ref("");
const sendingComments = ref(false);
let commentGeneration = 0;
function anchorOf(path: string, line: ChangeLine): string | null {
  const number = line.new_line ?? line.old_line;
  return number == null ? null : `${path}:${number}`;
}
// Placement distinguishes the diff side: a deletion at old line 2 and an
// addition at new line 2 are different rows even though both anchor as path:2.
function commentLineKey(path: string, line: ChangeLine): string | null {
  if (line.kind === "deletion") return line.old_line == null ? null : `${path}:old:${line.old_line}`;
  return line.new_line == null ? null : `${path}:new:${line.new_line}`;
}
function commentKey(comment: ReviewComment): string {
  return comment.lineKind === "deletion" ? `${comment.path}:old:${comment.line}` : `${comment.path}:new:${comment.line}`;
}
function commentBoxOpen(path: string, line: ChangeLine): boolean {
  return !!activeComment.value && commentLineKey(path, line) === commentLineKey(activeComment.value.path, activeComment.value.line);
}
function commentButtonLabel(path: string, line: ChangeLine): string {
  const anchor = anchorOf(path, line);
  return anchor == null ? "" : `Add an inline comment on ${anchor}${line.kind === "deletion" ? " (deleted line)" : ""}`;
}
function openComment(path: string, line: ChangeLine) {
  if (anchorOf(path, line) == null) return;
  activeComment.value = { path, line };
  commentDraft.value = "";
}
function closeComment() {
  activeComment.value = null;
  commentDraft.value = "";
}
function addComment() {
  if (!activeComment.value) return;
  const comment = commentFromLine(activeComment.value.path, activeComment.value.line, commentDraft.value);
  if (!comment) return;
  comments.value.push(comment);
  closeComment();
}
function removeComment(id: string) {
  comments.value = comments.value.filter(comment => comment.id !== id);
}
function commentsFor(path: string, line: ChangeLine): ReviewComment[] {
  const key = commentLineKey(path, line);
  return key ? comments.value.filter(comment => commentKey(comment) === key) : [];
}
let completionGeneration = 0;
const visibleFiles = computed(() => changes.value?.files.filter(file => file.path.display.toLowerCase().includes(fileQuery.value.toLowerCase())) ?? []);
const requestMode = ref<"review" | "prepare" | null>(null);
const requestText = ref("");
const requestScope = ref("");
// Keep editable requests through comparison refreshes, but never send a brief
// prepared for a different canonical target or preview revision.
const currentRequestScope = computed(() => JSON.stringify([
  reviewingTrack.value, props.integrationTarget?.id,
  props.integrationTarget?.branch.repo_root, props.integrationTarget?.branch.branch,
  props.integrationTarget?.branch.goal,
  JSON.stringify(props.integrationTarget?.branch.tags.filter(tag => ["track_topic_branch", "track_topic_initialized"].includes(tag.key))),
  reviewPreviewEvidence(changes.value, props.selectedChange?.change),
  JSON.stringify(props.selectedChange),
]));
const requestStale = computed(() => requestScope.value !== currentRequestScope.value);
const requesting = ref(false);
const requestNote = ref("");
let requestGeneration = 0;
watch(fileQuery, async query => {
  const current = ++completionGeneration;
  fileOptions.value = [];
  if (!checkoutEditable.value || !query.trim()) return;
  try {
    const files = await invoke<string[]>("complete_files", { id: props.sessionId, query });
    if (current === completionGeneration) fileOptions.value = files.slice(0, 20);
  } catch { /* The diff remains usable when completion is unavailable. */ }
});
function prepareRequest(mode: "review" | "prepare") {
  if (reviewingTrack.value === null || selectedProblem.value || (mode === "prepare" && !reviewingTrack.value && !props.selectedChange)) return;
  requestMode.value = mode;
  requestScope.value = currentRequestScope.value;
  const revision = reviewPreviewEvidence(changes.value, props.selectedChange?.change);
  requestText.value = mode === "review"
    ? independentReviewRequest(revision, props.integrationTarget ?? null, reviewingTrack.value === true, props.selectedChange?.change)
    : preparationRequest(reviewingTrack.value === true, revision, props.integrationTarget ?? null, editedPaths.value, props.selectedChange?.change);
}
async function sendRequest() {
  if (requestStale.value || reviewingTrack.value === null || requesting.value || loading.value || error.value || !requestText.value.trim() || selectedProblem.value || !requestMode.value) return;
  if (requestMode.value === "prepare" && !reviewingTrack.value && !props.selectedChange) return;
  const id = props.sessionId;
  const selected = props.selectedChange ? selectedChangePayload(props.selectedChange) : null;
  const mode = requestMode.value;
  const text = requestText.value + (selected ? `\n${selectedChangeInstructions(selected.expectedChange)}` : "");
  const request = ++requestGeneration;
  requesting.value = true;
  requestNote.value = "";
  try {
    let note: string;
    if (mode === "review") {
      const selection = launchSelection.value;
      const result = await invoke<{ session: { id: string }; warnings: string[] }>("review_session", {
        sourceId: id, brief: text,
        ...(selected ? { changeId: selected.changeId, expectedChange: selected.expectedChange } : {}),
        profile: null, agent: selection.agent || null,
        model: selection.model || null, effort: selection.effort || null,
      });
      note = `Independent reviewer ${result.session.id} launched. Its durable handoff will appear under Handoffs. ${result.warnings.join(" ")}`;
    } else {
      if (selected) {
        // Re-read registry and exact snapshot before delivering selected preparation.
        await invoke("change_changes", selected);
        if (id !== props.sessionId || request !== requestGeneration || selectedProblem.value || requestStale.value) return;
      }
      await invoke("send_input", { id, text, ...(selected ? { changeId: selected.changeId, expectedChange: selected.expectedChange } : {}) });
      note = "Preparation sent to this thread. Validation results will appear in its conversation.";
    }
    if (id !== props.sessionId || request !== requestGeneration) return;
    requestNote.value = note;
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
    if (selectedProblem.value) throw new Error(selectedProblem.value);
    const result = props.selectedChange
      ? await invoke<ChangeSet>("change_changes", selectedChangePayload(props.selectedChange))
      : await invoke<ChangeSet>("work_changes", { sessionId });
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

watch(() => [props.sessionId, JSON.stringify(props.selectedChange), selectedProblem.value], () => {
  changes.value = null;
  editPath.value = null;
  editedPaths.value = [];
  fileQuery.value = "";
  requestMode.value = null;
  requestNote.value = "";
  requestGeneration++;
  requesting.value = false;
  comments.value = [];
  closeComment();
  commentSendMode.value = false;
  commentMessage.value = "";
  commentGeneration++;
  sendingComments.value = false;
  void refresh();
}, { immediate: true });

watch(() => [props.isTopic, props.integrationTarget?.id, props.integrationTarget?.branch.branch,
  props.integrationTarget?.branch.repo_root, props.integrationTarget?.branch.goal,
  JSON.stringify(props.integrationTarget?.branch.tags.filter(tag => ["track_topic_branch", "track_topic_initialized"].includes(tag.key)))], () => {
  changes.value = null;
  if (props.selectedChange) requestMode.value = null;
  requestGeneration++;
  requesting.value = false;
  commentGeneration++;
  sendingComments.value = false;
  void refresh();
});

function lineNumber(value: number | null): string {
  return value == null ? "" : String(value);
}
function commentBoxId(file: ChangeFile, hunkIndex: number, line: ChangeLine): string {
  return `comment-box-${props.sessionId}-${file.path.bytes}-${hunkIndex}-${line.new_line ?? line.old_line}`;
}
function openCommentSend() {
  if (!comments.value.length) return;
  commentSendMode.value = true;
  commentMessage.value = "";
}
async function sendComments() {
  if (!comments.value.length || sendingComments.value || loading.value || error.value) return;
  const id = props.sessionId;
  const prompt = buildCommentPrompt(comments.value);
  const count = comments.value.length;
  const text = `Please address these inline review comments on the current comparison.\n\n${prompt}\n\n${commentMessage.value.trim()}`.trim();
  if (!text.trim()) return;
  const request = ++commentGeneration;
  sendingComments.value = true;
  try {
    await invoke("send_input", { id, text, topicId: null, resourceIds: [], attachments: [] });
    if (id !== props.sessionId || request !== commentGeneration) return;
    comments.value = [];
    commentSendMode.value = false;
    commentMessage.value = "";
    closeComment();
    requestNote.value = `Sent ${count} inline comment${count === 1 ? "" : "s"} to this thread.`;
    emit("requested");
  } catch (e: unknown) {
    if (request === commentGeneration) requestNote.value = (e as { message?: string })?.message ?? String(e);
  } finally {
    if (request === commentGeneration) sendingComments.value = false;
  }
}
</script>

<template>
  <section class="change-review" :class="{ embedded }" aria-label="Review changes">
    <div class="change-review-heading">
      <div>
        <strong>{{ selectedChange ? selectedChange.change.title : 'Review changes' }}</strong>
        <span v-if="changes?.base.state === 'available'" class="change-review-count">{{ changes.totals.files }} files · +{{ changes.totals.additions }} −{{ changes.totals.deletions }}</span>
      </div>
      <button :disabled="loading" @click="refresh">{{ loading ? "Loading…" : "Refresh" }}</button>
    </div>
    <div class="review-toolbar">
      <button :disabled="loading || requesting || reviewingTrack === null || !!selectedProblem || !!error" @click="prepareRequest('review')">Launch reviewer</button>
      <button v-if="reviewingTrack || selectedChange" :disabled="loading || requesting || reviewingTrack === null || !!selectedProblem || !!error" @click="prepareRequest('prepare')">Prepare {{ reviewingTrack ? "publishing" : "selected Change" }}</button>
      <span v-if="comments.length" class="change-review-comments-chip" role="status">{{ comments.length }} comment{{ comments.length === 1 ? "" : "s" }}</span>
      <button v-if="comments.length" class="change-review-comments-send" :disabled="loading || requesting || sendingComments || !!error" @click="openCommentSend">Send to agent</button>
    </div>
    <div v-if="commentSendMode" class="review-request change-review-comments-composer">
      <label :for="`review-comments-${sessionId}`">Send {{ comments.length }} inline comment{{ comments.length === 1 ? "" : "s" }} to this thread</label>
      <pre class="change-review-comments-summary">{{ buildCommentPrompt(comments) }}</pre>
      <textarea :id="`review-comments-${sessionId}`" v-model="commentMessage" :disabled="sendingComments" rows="3" placeholder="Add an optional message for this thread…" />
      <div><button :disabled="sendingComments" @click="commentSendMode = false">Cancel</button><button class="primary" :disabled="loading || sendingComments || !!error" @click="sendComments">{{ sendingComments ? "Sending…" : "Send to agent" }}</button></div>
    </div>
    <div v-if="requestMode" class="review-request">
      <label :for="`review-request-${sessionId}`">{{ requestMode === 'review' ? "Review request" : "Preparation request" }}</label>
      <textarea :id="`review-request-${sessionId}`" v-model="requestText" :disabled="requesting" rows="5" />
      <p v-if="requestStale && !loading" role="status">The comparison changed. Keep your draft or replace it with a request for the current comparison.</p>
      <button v-if="requestStale" :disabled="loading || requesting || reviewingTrack === null" @click="prepareRequest(requestMode)">Replace with current comparison</button>
      <div><button :disabled="requesting" @click="requestMode = null">Cancel</button><button class="primary" :disabled="loading || requestStale || requesting || !!selectedProblem || !!error || !requestText.trim()" @click="sendRequest">{{ requesting ? "Sending…" : (requestMode === 'review' ? "Launch reviewer" : "Send to agent") }}</button></div>
    </div>
    <div v-if="requestNote" class="change-review-state" role="status">{{ requestNote }}</div>
    <div v-if="selectedChange" class="change-review-state">{{ selectedChange.change.source_session }}/{{ selectedChange.change.id }} · {{ selectedChange.change.scope.kind }} · {{ selectedChange.change.scope.paths.length ? selectedChange.change.scope.paths.join(', ') : 'entire pinned range' }}</div>
    <div v-else-if="!reviewingTrack" class="change-review-state">Select an explicit Change in the inspector’s Changes tab to prepare or Land an output.</div>
    <div class="review-file-picker">
      <input v-model="fileQuery" :list="`review-files-${sessionId}`" placeholder="Filter changes or open a file…" aria-label="Find a checkout file" @keydown.enter.prevent="checkoutEditable && fileQuery.trim() && (editPath = fileQuery.trim())" />
      <datalist :id="`review-files-${sessionId}`"><option v-for="path in fileOptions" :key="path" :value="path" /></datalist>
      <button v-if="checkoutEditable" :disabled="!fileQuery.trim()" @click="editPath = fileQuery.trim()">Edit file</button>
    </div>
    <div v-if="loading && !changes" class="change-review-state" role="status">Loading the current comparison…</div>
    <div v-else-if="error" class="change-review-state change-review-error" role="alert">Could not load changes: {{ error }}</div>
    <template v-else-if="changes">
      <div v-if="changes.base.state === 'unavailable'" class="change-review-state" role="status">
        <template v-if="changes.base.reason === 'remote_canonical_preview_unavailable'">Canonical preview is unavailable for this remote repository. Launch a reviewer to compare {{ changes.source_ref }} with {{ changes.base.reference }} on the repository host and report exact revisions and coverage.</template>
        <template v-else>Base {{ changes.base.reference }} is unavailable ({{ changes.base.reason.replaceAll('_', ' ') }}). There is no reliable comparison yet.</template>
      </div>
      <template v-else>
        <div class="change-review-base" :title="`Base ${changes.base.oid}${changes.head_oid ? ` · HEAD ${changes.head_oid}` : ''}`">
          Compared with {{ changes.base.reference }} · {{ changes.base.oid.slice(0, 10) }}
        </div>
        <div v-if="changes.truncated || changes.totals.truncated" class="change-review-state" role="status">
          Large diff: this preview is limited. Open the checkout for the complete changes.
        </div>
        <div v-if="changes.head_oid" class="change-review-base">Source {{ changes.source_ref }} · {{ changes.head_oid.slice(0, 10) }} · {{ selectedChange ? (selectedChange.change.scope.kind === 'range' ? 'pinned committed range' : 'bounded working snapshot') : changes.accepted_source ? "committed accepted state" : "includes worker checkout edits" }}</div>
        <div v-if="changes.files.length === 0" class="change-review-state">No changes against this comparison ref.</div>
        <div v-else-if="!visibleFiles.length" class="change-review-state">No changed files match this filter.</div>
        <div v-else class="change-review-files">
          <details v-for="(file, index) in visibleFiles" :key="file.path.bytes" class="change-review-file" :open="index === 0">
            <summary>
              <span class="change-review-status">{{ file.status.replaceAll('_', ' ') }}</span>
              <span class="change-review-path" :title="file.path.display">{{ file.old_path ? `${file.old_path.display} → ` : "" }}{{ file.path.display }}</span>
              <span class="change-review-file-stats">{{ file.additions == null ? "" : `+${file.additions}` }} {{ file.deletions == null ? "" : `−${file.deletions}` }}</span>
            </summary>
            <div v-if="checkoutEditable && file.content === 'text' && file.status !== 'deleted'" class="review-file-actions"><button @click="editPath = file.path.display">Edit file</button><span>Changes save to this thread’s checkout</span></div>
            <div v-if="file.content !== 'text'" class="change-review-state">{{ file.content }} content cannot be previewed here.</div>
            <div v-else-if="file.hunks.length === 0" class="change-review-state">No text hunks available for this file.</div>
            <div v-else class="change-review-hunks">
              <div v-for="(hunk, hunkIndex) in file.hunks" :key="hunkIndex" class="change-review-hunk">
                <div class="change-review-hunk-header">{{ hunk.header }}</div>
                <template v-for="(line, lineIndex) in hunk.lines" :key="lineIndex">
                  <div class="change-review-line" :class="line.kind">
                    <span class="change-review-number">{{ lineNumber(line.old_line) }}</span>
                    <span class="change-review-number">{{ lineNumber(line.new_line) }}</span>
                    <span class="change-review-marker">{{ line.kind === 'addition' ? '+' : line.kind === 'deletion' ? '−' : ' ' }}</span>
                    <span class="change-review-text">{{ line.text }}</span>
                    <button v-if="anchorOf(file.path.display, line) != null" class="change-review-comment-button" type="button" :aria-label="commentButtonLabel(file.path.display, line)" :title="`Comment on ${anchorOf(file.path.display, line)}`" @click="openComment(file.path.display, line)">+</button>
                  </div>
                  <div v-if="activeComment && commentBoxOpen(file.path.display, line)" class="change-review-comment-box">
                    <label :for="commentBoxId(file, hunkIndex, line)">Comment on <span class="change-review-comment-anchor">{{ anchorOf(activeComment.path, activeComment.line) }}</span></label>
                    <textarea :id="commentBoxId(file, hunkIndex, line)" v-model="commentDraft" rows="3" placeholder="Leave a comment…" />
                    <div><button type="button" @click="closeComment">Cancel</button><button class="primary" type="button" :disabled="!commentDraft.trim()" @click="addComment">Comment</button></div>
                  </div>
                  <div v-for="comment in commentsFor(file.path.display, line)" :key="comment.id" class="change-review-comment-card">
                    <div class="change-review-comment-card-header"><span class="change-review-comment-anchor">{{ comment.anchor }}</span><span class="change-review-comment-kind">{{ comment.lineKind }}</span><button class="change-review-comment-remove" type="button" :aria-label="`Remove comment on ${comment.anchor}`" title="Remove comment" @click="removeComment(comment.id)">×</button></div>
                    <p class="change-review-comment-text">{{ comment.text }}</p>
                  </div>
                </template>
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
.change-review.embedded { flex: 1; max-height: none; border: 0; }
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
.change-review-comment-button { opacity: 0; position: sticky; right: 0; margin: 0 4px; padding: 0 7px; line-height: 1.6; border-color: var(--border); background: var(--bg-raised); color: var(--text-dim); }
.change-review-line:hover .change-review-comment-button, .change-review-comment-button:focus-visible { opacity: 1; }
.change-review-comment-box, .change-review-comment-card { display: grid; gap: 5px; margin: 4px 16px; padding: 8px; border: 1px solid var(--border); border-left: 2px solid var(--accent); border-radius: 5px; background: rgba(110, 168, 254, .07); font: 11px/1.45 var(--mono); }
.change-review-comment-box { min-width: max-content; width: min(520px, calc(100% - 32px)); max-width: min(520px, calc(100% - 32px)); white-space: normal; }
.change-review-comment-box label { color: var(--text-dim); }
.change-review-comment-box textarea { width: 100%; box-sizing: border-box; resize: vertical; min-height: 58px; font-family: inherit; font-size: 12px; line-height: 1.5; color: var(--text); background: var(--bg); border: 1px solid var(--border); border-radius: 5px; padding: 6px 8px; }
.change-review-comment-box textarea:focus-visible { border-color: var(--accent); outline: none; }
.change-review-comment-box > div { display: flex; justify-content: flex-end; gap: 6px; }
.change-review-comment-card { min-width: 0; max-width: min(560px, calc(100% - 32px)); }
.change-review-comment-card-header { display: flex; align-items: center; gap: 8px; }
.change-review-comment-anchor { color: var(--accent); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.change-review-comment-kind { color: var(--text-dim); }
.change-review-comment-remove { margin-left: auto; padding: 0 5px; line-height: 1.3; color: var(--text-dim); }
.change-review-comment-text { margin: 0; color: var(--text); user-select: text; white-space: pre-wrap; overflow-wrap: anywhere; }
.change-review-comments-chip { color: var(--text-dim); font-size: 11px; padding: 3px 8px; border: 1px solid var(--border); border-radius: 10px; white-space: nowrap; }
.change-review-comments-send { flex: none; }
.change-review-comments-composer .change-review-comments-summary { margin: 0; padding: 8px; border: 1px solid var(--border); border-radius: 5px; background: var(--bg); color: var(--text-dim); font: 11px/1.45 var(--mono); overflow: auto; max-height: 180px; white-space: pre-wrap; }
.change-review-comments-composer textarea { min-height: 58px; }
</style>
