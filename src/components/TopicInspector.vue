<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { backendOf } from "../backendIdentity";
import { useBackendInvoke } from "../backendInvoke";

import type { SessionSummary, SessionView } from "../App.vue";
import ResourcePanel from "./ResourcePanel.vue";
import type { ArtifactTarget } from "../artifactLinks";
import ChangeReview from "./ChangeReview.vue";
import { reviewIntegrationTarget } from "../reviewRequest";
import TrackActivity from "./TrackActivity.vue";
import AutolandControl from "./AutolandControl.vue";
import { canonicalTopicBranch, isTrackCoordinator, resolveTrack, type IntegrationTarget } from "../trackPolicy.ts";
import { selectionKey, changeRegistry, pinSelection, sameChange, selectedChangePayload, selectionProblem, type ChangeSelection } from "../durableChange.ts";
import { landStrategies, strategyMemoryKey } from "../trackStrategies.ts";
import SplitButton from "./SplitButton.vue";
import { compareQueueSnapshots, projectChangeQueue, queuedLandFeedback, reorderedDeclarations, validateQueueView, type QueueView, type TimelineRow } from "../changeQueue.ts";
import {
  landingCandidates,
  type LandingCandidate,
  pendingPermissionSummary,
  statusClass,
  topicThreadRows,
  threadStatus,
} from "../topicInspector";

const invoke = useBackendInvoke();

// Track inspector: Changes, workers, and review.

const props = defineProps<{
  topic: SessionSummary | SessionView;
  selectedView?: SessionView | null;
  scopeId?: string;
  fleet: SessionSummary[];
  selectedId: string | null;
  loomUrl?: string;
  artifactRequest?: ArtifactTarget | null;
  reviewRequest?: { sessionId: string } | null;
  expanded?: boolean;
  canExpand?: boolean;
  landingRequest?: number;
}>();

const emit = defineEmits<{
  (e: "open-artifact", target: ArtifactTarget): void;
  (e: "close"): void;
  (e: "toggle-expanded"): void;
  (e: "error", message: string): void;
  (e: "select", id: string): void;
  (e: "new-thread"): void;
  (e: "checkout-saved", sessionId: string): void;
  (e: "policy-updated", session: SessionView): void;
}>();

const tab = ref<"threads" | "review" | "resources" | "changes" | "todos" | "activity" | "handoffs">("changes");
// A SessionView is enough to request the server comparison. Only fleet summaries
// carry ancestry; never classify an absent summary as a root/coordinator.
const topicScope = computed(() => props.fleet.find(session => session.id === (props.scopeId ?? props.topic.id)) ?? null);
const reviewSession = computed(() => props.fleet.find(session => session.id === props.selectedId)
  ?? (props.selectedView?.id === props.selectedId ? props.selectedView : null) ?? props.topic);
const reviewSummary = computed(() => props.fleet.find(session => session.id === reviewSession.value.id) ?? null);
const canEdit = computed(() => {
  try { return ["localhost", "127.0.0.1", "::1", "[::1]"].includes(new URL(backendOf(reviewSession.value.id) ?? "").hostname); }
  catch { return false; }
});
const selectedChange = ref<ChangeSelection | null>(null);
const selectedProblem = computed(() => {
  const selected = selectedChange.value;
  if (!selected) return "";
  if (resolveTrack(props.fleet, selected.sourceId)?.id !== props.topic.id) return "The selected source no longer belongs to this same-repository Track.";
  const candidate = candidates.value.find(candidate => candidate.key === selectionKey(selected));
  return selectionProblem(selected, candidate?.session ?? props.fleet.find(session => session.id === selected.sourceId));
});
function selectCandidate(candidate: LandingCandidate) {
  selectedChange.value = pinSelection(candidate.session.id, candidate.change);
  emit("select", candidate.session.id);
}
function reviewCandidate(candidate: LandingCandidate) { selectCandidate(candidate); tab.value = "review"; }
watch(() => props.selectedId, id => {
  if (selectedChange.value && selectedChange.value.sourceId !== id) selectedChange.value = null;
});


// --- Threads tab -----------------------------------------------------------
//
// The topic's coordinator + worker fleet with attention/working/readiness/
// resting states. The row semantics (loud tags → badge; running && !idle →
// spinner; quiet otherwise) mirror FleetSidebar's Inbox tab, scoped to the
// topic subtree rather than the lane filing system.

function subtitle(s: SessionSummary): string {
  const permission = pendingPermissionSummary(s);
  if (permission) return permission;
  return threadStatus(s).detail || s.branch.description || s.branch.title || "—";
}

const collapsed = ref(new Set<string>());

const topicRows = computed(() => topicScope.value ? topicThreadRows(props.fleet, props.topic.id, collapsed.value) : []);

function toggle(id: string) {
  if (collapsed.value.has(id)) collapsed.value.delete(id);
  else collapsed.value.add(id);
}

// Queue membership does not depend on the Threads tree's collapse state.
const candidates = computed(() => topicScope.value ? landingCandidates(topicThreadRows(props.fleet, props.topic.id, new Set()), props.topic.id, props.fleet) : []);
const registryErrors = computed(() => props.fleet.filter(session => session.id !== props.topic.id && resolveTrack(props.fleet, session.id)?.id === props.topic.id)
  .flatMap(session => changeRegistry(session).errors));
const landOptions = landStrategies();

const sessionRepo = (s: SessionSummary) => s.github_repo || s.branch.repo_root;
const integratingId = ref<string | null>(null);
const integrationNote = ref("");

// Queue reads are driven by Track/fleet updates, never a client drain loop.
const queueView = ref<QueueView | null>(null);
const queueLoading = ref(false);
const queueBusy = ref(false);
const queueError = ref("");
const orderNote = ref("");
const discovered = ref<string[]>([]);
const draggedKey = ref<string | null>(null);
let viewEpoch = 0;
let readSequence = 0;
let appliedQueue: QueueView | null = null;
let conflictingRevision: number | null = null;
let alive = true;
onBeforeUnmount(() => { alive = false; viewEpoch++; readSequence++; });
const captureOwner = () => ({ topicId: props.topic.id, backend: backendOf(props.topic.id), epoch: viewEpoch });
const isCurrent = (owner: ReturnType<typeof captureOwner>) => alive && owner.epoch === viewEpoch && owner.topicId === props.topic.id && owner.backend === backendOf(props.topic.id);
const timeline = computed(() => projectChangeQueue(queueView.value, candidates.value, props.topic.id, discovered.value));
const canReorder = computed(() => timeline.value.canReorder && !queueLoading.value && !queueBusy.value && !integratingId.value && !queueError.value);

function applyQueueSnapshot(next: QueueView, allowRestore = true) {
  try {
    if (conflictingRevision !== null && next.revision <= conflictingRevision) {
      throw new Error("Conflicting Changes queue data. Reconcile the queue before continuing.");
    }
    const relation = compareQueueSnapshots(appliedQueue, next);
    if (relation === "older" || (relation === "same" && !allowRestore)) return relation;
    appliedQueue = next;
    conflictingRevision = null;
    queueView.value = next;
    queueError.value = "";
    return relation;
  } catch (error: any) {
    conflictingRevision = Math.max(conflictingRevision ?? 0, appliedQueue?.revision ?? 0, next.revision);
    queueView.value = null;
    queueError.value = error?.message ?? String(error);
    throw error;
  }
}

async function refreshQueue(owner = captureOwner()) {
  if (!isCurrent(owner) || !topicScope.value || !isTrackCoordinator(topicScope.value)) return;
  const sequence = ++readSequence;
  const topic = topicScope.value;
  if (!topic) return;
  queueLoading.value = true;
  try {
    const next = validateQueueView(await invoke<QueueView>("change_queue", { topicId: owner.topicId }, owner.backend), topic);
    if (!isCurrent(owner)) return;
    // Revision determines freshness; request order only owns loading/errors.
    applyQueueSnapshot(next, sequence === readSequence);
  } catch (error: any) {
    if (!isCurrent(owner) || sequence !== readSequence) return;
    queueView.value = null;
    queueError.value = error?.message ?? String(error);
  } finally {
    if (isCurrent(owner) && sequence === readSequence) queueLoading.value = false;
  }
}

async function movePending(from: number, to: number) {
  if (!canReorder.value || from === to || from < 0 || to < 0 || from >= timeline.value.pending.length || to >= timeline.value.pending.length) return;
  const owner = captureOwner();
  const topic = topicScope.value;
  if (!topic) return;
  const revision = queueView.value!.revision;
  const order = reorderedDeclarations(timeline.value.pending, from, to);
  queueBusy.value = true;
  orderNote.value = "";
  let receivedReply = false;
  try {
    const next = await invoke<QueueView>("reorder_change_queue", { topicId: owner.topicId, expectedRevision: revision, order }, owner.backend);
    receivedReply = true;
    if (!isCurrent(owner)) return;
    // A committed write may arrive after a newer coordinator claim. It must
    // neither roll back that snapshot nor retire an in-flight fleet read.
    const relation = applyQueueSnapshot(validateQueueView(next, topic));
    orderNote.value = relation === "older" ? "Order saved; the queue has since advanced."
      : `Moved ${order[to].title} to position ${to + 1}.`;
  } catch (error: any) {
    if (!isCurrent(owner)) return;
    await refreshQueue(owner);
    if (isCurrent(owner)) orderNote.value = `${receivedReply ? "Could not verify the updated queue." : "Order was not saved."} ${error?.message ?? String(error)} ${queueError.value ? "Queue refresh also failed." : "Refreshed the queue; try again."}`;
  } finally {
    if (isCurrent(owner)) queueBusy.value = false;
  }
}
function startDrag(row: TimelineRow, event: DragEvent) {
  if (!canReorder.value) { event.preventDefault(); return; }
  draggedKey.value = row.key;
  if (event.dataTransfer) { event.dataTransfer.effectAllowed = "move"; event.dataTransfer.setData("text/plain", row.key); }
}
function dropPending(to: number) {
  const from = timeline.value.pending.findIndex(row => row.key === draggedKey.value);
  draggedKey.value = null;
  if (from >= 0) void movePending(from, to);
}
function canLand(candidate: LandingCandidate) {
  const fresh = candidates.value.find(item => item.key === candidate.key);
  return !!queueView.value && !queueError.value && !queueLoading.value && !queueBusy.value && !integratingId.value
    && !!fresh && sameChange(fresh.change, candidate.change) && !["blocked", "unavailable", "landed"].includes(fresh.state)
    && resolveTrack(props.fleet, fresh.session.id)?.id === props.topic.id
    && timeline.value.active?.key !== candidate.key
    && !timeline.value.pending.find(row => row.key === candidate.key)?.problem;
}

// Each click captures its own exact declaration; review selection never substitutes it.
async function onLand(candidate: LandingCandidate, strategy: string) {
  if (!canLand(candidate)) return;
  const selected = pinSelection(candidate.session.id, candidate.change);
  if (selectionProblem(selected, candidates.value.find(item => item.key === candidate.key)?.session)) return;
  const owner = captureOwner();
  integratingId.value = candidate.key;
  integrationNote.value = "";
  try {
    const target = await invoke<IntegrationTarget>("land_change", { ...selectedChangePayload(selected), strategy }, owner.backend);
    if (!isCurrent(owner)) return;
    try { localStorage.setItem(strategyMemoryKey("land", sessionRepo(candidate.session)), strategy); } catch { /* unavailable storage */ }
    integrationNote.value = queuedLandFeedback(target);
    await refreshQueue(owner);
  } catch (error: any) {
    if (!isCurrent(owner)) return;
    integrationNote.value = error?.message ?? String(error);
    emit("error", integrationNote.value);
    await refreshQueue(owner);
  } finally {
    if (isCurrent(owner)) integratingId.value = null;
  }
}

watch(() => [props.topic.id, props.topic.branch.repo_root, canonicalTopicBranch(props.topic)], (next, previous) => {
  // Fleet snapshots can replace the summary object without changing ownership.
  if (previous && next.every((value, index) => value === previous[index])) return;
  viewEpoch++;
  readSequence++;
  queueView.value = null;
  appliedQueue = null;
  conflictingRevision = null;
  queueError.value = "";
  queueBusy.value = false;
  queueLoading.value = false;
  integratingId.value = null;
  integrationNote.value = "";
  orderNote.value = "";
  discovered.value = [];
  draggedKey.value = null;
}, { immediate: true, flush: "sync" });
watch(() => [props.topic.id, props.topic.branch.repo_root, canonicalTopicBranch(props.topic), props.fleet], () => {
  const known = new Set(discovered.value);
  discovered.value = [...discovered.value, ...candidates.value.map(candidate => candidate.key).filter(key => !known.has(key)).sort()];
  void refreshQueue();
}, { immediate: true, deep: true });
watch(() => [props.scopeId ?? props.topic.id, props.artifactRequest, props.landingRequest, props.reviewRequest] as const, ([scopeId, target, request, review], previous) => {
  const initial = !previous;
  const changedTrack = initial || (!props.scopeId && scopeId !== previous[0]);
  const openedArtifact = !!target && (initial || target !== previous[1]);
  const openedReview = review?.sessionId === props.selectedId && (initial || review !== previous[3]);
  // Explicit navigation wins; ancestry enrichment keeps the existing review
  // and composer. A later Changes request can replace a retained artifact.
  if (openedArtifact || (changedTrack && target)) tab.value = "resources";
  else if (openedReview) tab.value = "review";
  else if (changedTrack || request !== previous[2]) tab.value = "changes";
}, { immediate: true, flush: "post" });

// --- Todos tab --------------------------------------------------------------
//
// A topic-filtered view of the durable cross-topic user todo list
// (`arachne-todos` branch artifact). Topic plans and worker-internal
// checklists remain separate — this is the person's list.

interface TodoItem {
  id: string;
  text: string;
  done: boolean;
  topic_id: string;
  created_at: string;
}
interface TodoTopicView {
  todos: TodoItem[];
  revision: number;
}

const todos = ref<TodoTopicView>({ todos: [], revision: 0 });
const todosLoading = ref(false);
const todoDraft = ref("");
const todoBusy = ref(false);
const todoError = ref("");

async function refreshTodos() {
  if (!topicScope.value) return;
  const topicId = props.topic.id;
  todosLoading.value = true;
  todoError.value = "";
  try {
    const next = await invoke<TodoTopicView>("topic_todos", { topicId });
    if (topicId !== props.topic.id) return;
    if (!next || !Array.isArray(next.todos)) throw new Error("Loom returned an invalid todo list");
    todos.value = next;
  } catch (error: any) {
    if (topicId === props.topic.id) todoError.value = error?.message ?? String(error);
  } finally {
    todosLoading.value = false;
  }
}

async function addTodo() {
  const text = todoDraft.value.trim();
  if (!text || todoBusy.value) return;
  todoBusy.value = true;
  todoError.value = "";
  const topicId = props.topic.id;
  try {
    const next = await invoke<TodoTopicView>("add_todo", {
      topicId,
      text,
      expectedRevision: todos.value.revision,
    });
    if (topicId !== props.topic.id) return;
    todos.value = next;
    todoDraft.value = "";
  } catch (error: any) {
    if (topicId !== props.topic.id) return;
    await refreshTodos();
    todoError.value = error?.message ?? String(error);
  } finally {
    todoBusy.value = false;
  }
}

async function toggleTodo(todo: TodoItem) {
  if (todoBusy.value) return;
  todoBusy.value = true;
  todoError.value = "";
  const topicId = props.topic.id;
  try {
    const next = await invoke<TodoTopicView>("toggle_todo", {
      topicId,
      todoId: todo.id,
      expectedRevision: todos.value.revision,
    });
    if (topicId !== props.topic.id) return;
    todos.value = next;
  } catch (error: any) {
    if (topicId !== props.topic.id) return;
    await refreshTodos();
    todoError.value = error?.message ?? String(error);
  } finally {
    todoBusy.value = false;
  }
}

async function removeTodo(todo: TodoItem) {
  if (todoBusy.value) return;
  todoBusy.value = true;
  todoError.value = "";
  const topicId = props.topic.id;
  try {
    const next = await invoke<TodoTopicView>("remove_todo", {
      topicId,
      todoId: todo.id,
      expectedRevision: todos.value.revision,
    });
    if (topicId !== props.topic.id) return;
    todos.value = next;
  } catch (error: any) {
    if (topicId !== props.topic.id) return;
    await refreshTodos();
    todoError.value = error?.message ?? String(error);
  } finally {
    todoBusy.value = false;
  }
}

// Reset per-topic state when the inspected topic changes: the todo slice,
// collapse state, and the integration note all belong to one topic.
watch(() => topicScope.value?.id, () => {
  todos.value = { todos: [], revision: 0 };
  todoDraft.value = "";
  todoError.value = "";
  integrationNote.value = "";
  collapsed.value = new Set();
  void refreshTodos();
}, { immediate: true });
watch(() => props.scopeId ?? props.topic.id, () => { selectedChange.value = null; });
</script>

<template>
  <aside id="topic-inspector" class="topic-inspector" aria-label="Track inspector">
    <header class="topic-inspector-head">
      <div class="topic-inspector-topic">
        <strong>{{ topic.branch.title || topic.branch.name }}</strong>
      </div>
      <button v-if="canExpand" class="inspector-expand-button" type="button" :aria-expanded="!!expanded" aria-controls="topic-inspector"
        :aria-label="expanded ? 'Restore inspector' : 'Expand inspector'"
        :title="expanded ? 'Restore inspector beside conversation' : 'Expand inspector across conversation'"
        @click="emit('toggle-expanded')">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
          stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path :d="expanded ? 'M4 14h6v6 M10 14l-7 7 M20 10h-6V4 M14 10l7-7' : 'M15 3h6v6 M21 3l-7 7 M9 21H3v-6 M3 21l7-7'" />
        </svg>
      </button>
      <button title="Close inspector" aria-label="Close inspector" @click="emit('close')">×</button>
    </header>
    <div v-if="topicScope && isTrackCoordinator(topicScope)" class="canonical-policy">
      <span>Canonical destination: <code>{{ canonicalTopicBranch(topicScope) }}</code></span>
      <AutolandControl :session="topicScope" :track="topicScope" coordinator @updated="emit('policy-updated', $event)" @error="emit('error', $event)" />
    </div>
    <div class="tab-bar inspector-tab-bar" role="tablist">
      <button class="tab" :class="{ active: tab === 'changes' }" role="tab" :aria-selected="tab === 'changes'" :disabled="!topicScope" @click="tab = 'changes'">Changes</button>
      <button class="tab" :class="{ active: tab === 'threads' }" role="tab" :aria-selected="tab === 'threads'" :disabled="!topicScope" @click="tab = 'threads'">Threads</button>
      <button class="tab" :class="{ active: tab === 'review' }" role="tab" :aria-selected="tab === 'review'" @click="tab = 'review'">Review</button>
      <button class="tab" :class="{ active: tab === 'handoffs' }" role="tab" :aria-selected="tab === 'handoffs'" :disabled="!topicScope" @click="tab = 'handoffs'">Handoffs</button>
      <button class="tab" :class="{ active: tab === 'activity' }" role="tab" :aria-selected="tab === 'activity'" :disabled="!topicScope" @click="tab = 'activity'">Activity</button>
      <button class="tab" :class="{ active: tab === 'resources' }" role="tab" :aria-selected="tab === 'resources'" :disabled="!topicScope" @click="tab = 'resources'">Resources</button>
      <button class="tab" :class="{ active: tab === 'todos' }" role="tab" :aria-selected="tab === 'todos'" :disabled="!topicScope" @click="tab = 'todos'">Todos</button>
    </div>

    <div v-if="!topicScope" class="inspector-note" role="status">Session details are loading. Review uses this session’s server comparison.</div>

    <!-- Threads: the coordinator + worker fleet. Row click switches the
         main chat; New Thread lives here (design.md "Place New Thread
         here"). -->
    <div v-if="tab === 'threads'" class="inspector-body">
      <button class="inspector-new-thread" @click="emit('new-thread')">+ New thread</button>
      <div class="inspector-threads">
        <div
          v-for="row in topicRows"
          :key="row.session.id"
          class="session-item inspector-thread"
          :class="{
            selected: row.session.id === selectedId || (row.depth === 0 && !selectedId),
            child: row.depth > 0,
            archived: row.session.status === 'archived',
          }"
          :style="row.depth > 0 ? { marginLeft: `${8 + row.depth * 12}px` } : {}"
          role="button"
          tabindex="0"
          :aria-label="`Open ${row.session.branch.title || row.session.branch.name}`"
          @click="emit('select', row.session.id)"
          @keydown.enter.prevent="emit('select', row.session.id)"
          @keydown.space.prevent="emit('select', row.session.id)"
        >
          <div class="row1">
            <span v-if="row.childCount > 0" class="chevron" role="button" tabindex="-1"
              :aria-label="`${row.isCollapsed ? 'Expand' : 'Collapse'} threads under ${row.session.branch.name}`"
              @click.stop="toggle(row.session.id)">{{ row.isCollapsed ? "▸" : "▾" }}</span>
            <span class="name">{{ row.session.branch.name || row.session.id }}</span>
            <span v-if="['working', 'reviewing'].includes(threadStatus(row.session).kind)"
              class="spinner mini" :title="threadStatus(row.session).label" aria-hidden="true"></span>
            <span class="badge" :class="statusClass(row.session)" :title="threadStatus(row.session).detail">{{ threadStatus(row.session).label }}</span>
            <span v-if="row.childCount > 0" class="badge dim" :title="`${row.childCount} delegated children`">{{ row.childCount }}</span>
          </div>
          <div class="title">{{ subtitle(row.session) }}</div>
          <div v-if="row.session.id !== topic.id && resolveTrack(fleet, row.session.id)?.id === topic.id" @click.stop @keydown.stop>
            <AutolandControl :session="row.session" :track="topic" @updated="emit('policy-updated', $event)" @error="emit('error', $event)" />
          </div>
        </div>
        <div v-if="!topicRows.length" class="inspector-empty">No threads in this track yet.</div>
      </div>
    </div>

    <div v-else-if="tab === 'handoffs' && topicScope" class="inspector-body inspector-body-flush">
      <TrackActivity :loom-url="loomUrl" @open-artifact="emit('open-artifact', $event)" :topic="topicScope" results-only @select="emit('select', $event)" />
    </div>

    <!-- Resources: the topic's attached resources and refs (ResourcePanel's
         content embedded here as the tab body). -->
    <div v-else-if="tab === 'review'" class="inspector-body inspector-body-flush">
      <div class="inspector-review-context">{{ selectedChange?.change.title || reviewSession.branch.title || reviewSession.branch.name }}</div>
      <ChangeReview :session-id="selectedChange?.sourceId || reviewSession.id" :selected-change="selectedChange" :selection-error="selectedProblem" :is-topic="selectedChange ? false : reviewSummary ? isTrackCoordinator(reviewSummary) : null" :integration-target="reviewIntegrationTarget(fleet, selectedChange?.sourceId || reviewSession.id)" :can-edit="canEdit" embedded @saved="emit('checkout-saved', reviewSession.id)" />
    </div>
    <div v-else-if="tab === 'activity' && topicScope" class="inspector-body inspector-body-flush"><TrackActivity :loom-url="loomUrl" @open-artifact="emit('open-artifact', $event)" :topic="topicScope" @select="emit('select', $event)" /></div>
    <div v-else-if="tab === 'resources' && topicScope" class="inspector-body inspector-body-flush">
      <ResourcePanel :key="loomUrl" :artifact-request="artifactRequest" :topic="topicScope" :can-edit="canEdit" embedded @checkout-saved="emit('checkout-saved', $event)" @error="(message: string) => emit('error', message)" />
    </div>

    <div v-else-if="tab === 'changes' && topicScope" class="inspector-body changes-body">
      <div v-if="integrationNote" class="inspector-note" role="status">{{ integrationNote }}</div>
      <div v-if="orderNote" class="inspector-note" role="status">{{ orderNote }}</div>
      <div v-if="queueError" class="inspector-note inspector-note-error" role="alert">
        {{ queueError }} <button class="link" :disabled="queueLoading" @click="refreshQueue()">Refresh</button>
      </div>
      <div v-if="selectedProblem" class="inspector-note inspector-note-error" role="alert">{{ selectedProblem }}</div>
      <div v-for="message in registryErrors" :key="message" class="inspector-note inspector-note-error" role="alert">{{ message }}</div>
      <div v-if="queueLoading && !queueView" class="inspector-empty" role="status">Loading Changes…</div>
      <section v-if="timeline.active" class="changes-section" aria-label="Active acceptance">
        <h3>In progress</h3>
        <div class="change-row change-active">
          <span class="change-marker"><span class="spinner mini" aria-hidden="true"></span></span>
          <div class="change-content">
            <strong>{{ timeline.active.change.title }}</strong>
            <span class="change-status">Acceptance in progress</span>
            <div class="change-meta">{{ timeline.active.candidate?.session.branch.name || 'Source unavailable' }} · <code>{{ timeline.active.change.scope.head_oid.slice(0, 8) }}</code></div>
            <p v-if="timeline.active.problem" class="change-problem">{{ timeline.active.problem }}</p>
            <details><summary>Details</summary><p><code>{{ timeline.active.change.scope.base_oid.slice(0, 8) }} → {{ timeline.active.change.scope.head_oid }}</code></p><p>{{ timeline.active.change.source_branch }} · {{ timeline.active.change.scope.paths.join(', ') || 'Entire pinned range' }}</p></details>
            <button v-if="timeline.active.candidate" class="link" :disabled="!!timeline.active.problem" @click="reviewCandidate(timeline.active.candidate)">Review</button>
          </div>
        </div>
      </section>
      <section v-if="timeline.pending.length" class="changes-section" aria-label="Pending Changes">
        <h3>Pending <span>{{ timeline.pending.length }}</span></h3>
        <ol class="changes-timeline">
          <li v-for="(row, index) in timeline.pending" :key="row.key" class="change-row change-pending"
            :class="{ 'change-blocked': !!row.problem, selected: selectedChange && row.key === selectionKey(selectedChange) }"
            :draggable="canReorder" @dragstart="startDrag(row, $event)" @dragend="draggedKey = null"
            @dragover.prevent @drop.prevent="dropPending(index)">
            <span class="change-marker" aria-hidden="true">{{ index + 1 }}</span>
            <div class="change-content">
              <button v-if="row.candidate" class="link name" :aria-pressed="!!selectedChange && row.key === selectionKey(selectedChange)" @click="selectCandidate(row.candidate)">{{ row.change.title }}</button>
              <strong v-else>{{ row.change.title }}</strong>
              <span class="change-status">{{ row.status }}</span>
              <div class="change-meta">{{ row.candidate?.session.branch.name || 'Source unavailable' }} · <code>{{ row.change.scope.head_oid.slice(0, 8) }}</code></div>
              <p v-if="row.problem" class="change-problem">{{ row.problem }}</p>
              <details><summary>Details</summary><p><code>{{ row.change.scope.base_oid.slice(0, 8) }} → {{ row.change.scope.head_oid }}</code></p><p>{{ row.change.source_branch }} · {{ row.change.scope.paths.join(', ') || 'Entire pinned range' }}</p><p>{{ row.candidate?.note }}</p></details>
              <div class="change-actions">
                <button v-if="row.candidate" class="link" :disabled="!!row.problem" @click="reviewCandidate(row.candidate)">Select / review</button>
                <SplitButton v-if="row.candidate && !row.problem && row.candidate.state !== 'landed'"
                  kind="land" :repo="sessionRepo(row.candidate.session)" :options="landOptions" label="Land"
                  :busy="integratingId === row.key" :disabled="!canLand(row.candidate)"
                  @run="(strategy: string) => row.candidate && onLand(row.candidate, strategy)" />
              </div>
            </div>
            <div class="change-order" role="group" :aria-label="`Reorder ${row.change.title}`">
              <button :aria-label="`Move ${row.change.title} up`" :disabled="!canReorder || index === 0" @click="movePending(index, index - 1)">↑</button>
              <button :aria-label="`Move ${row.change.title} down`" :disabled="!canReorder || index === timeline.pending.length - 1" @click="movePending(index, index + 1)">↓</button>
            </div>
          </li>
        </ol>
      </section>
      <section v-if="timeline.history.length" class="changes-section changes-history" aria-label="Landed history">
        <h3>Landed</h3>
        <ol class="changes-timeline">
          <li v-for="row in timeline.history" :key="row.key" class="change-row change-landed">
            <span class="change-marker" aria-hidden="true">✓</span>
            <div class="change-content">
              <strong>{{ row.change.title }}</strong><span class="change-status">Landed</span>
              <div class="change-meta">{{ row.candidate?.session.branch.name }} · <code>{{ row.change.scope.head_oid.slice(0, 8) }}</code></div>
              <details><summary>Details</summary><p>{{ row.candidate?.note }}</p></details>
            </div>
          </li>
        </ol>
      </section>
      <div v-if="!queueLoading && !timeline.active && !timeline.pending.length && !timeline.history.length" class="inspector-empty">
        No declared Changes.<br /><small>Threads with no explicit outputs require no Land.</small>
      </div>
    </div>

    <!-- Todos: a topic-filtered view of the durable cross-topic user todo
         list. Topic plans and worker checklists stay separate. -->
    <div v-else-if="topicScope" class="inspector-body">
      <div class="todo-form">
        <input
          v-model="todoDraft"
          placeholder="Add a todo for this track…"
          aria-label="Add a todo"
          :disabled="todoBusy"
          @keydown.enter.prevent="addTodo"
        />
        <button class="primary" :disabled="!todoDraft.trim() || todoBusy" @click="addTodo">{{ todoBusy ? "…" : "Add" }}</button>
      </div>
      <div v-if="todoError" class="inspector-note inspector-note-error">{{ todoError }}</div>
      <div class="todo-list">
        <div v-if="todosLoading && !todos.todos.length" class="inspector-empty">Loading…</div>
        <div v-for="todo in todos.todos" :key="todo.id" class="todo-item" :class="{ done: todo.done }">
          <label class="todo-check">
            <input type="checkbox" :checked="todo.done" :disabled="todoBusy" @change="toggleTodo(todo)" />
            <span class="todo-text">{{ todo.text }}</span>
          </label>
          <button class="link todo-remove" :aria-label="`Remove ${todo.text}`" :disabled="todoBusy" @click="removeTodo(todo)">✕</button>
        </div>
        <div v-if="!todosLoading && !todos.todos.length" class="inspector-empty">Nothing to do here. A finished item stays checked until removed.</div>
      </div>
    </div>
  </aside>
</template>

<style scoped>
.inspector-expand-button { display: inline-flex; align-items: center; justify-content: center; flex: none; width: 28px; height: 28px; padding: 0; }
.inspector-tab-bar { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); }
.inspector-tab-bar .tab { padding: 7px 6px; }
.inspector-review-context { padding: 8px 12px; font-size: 12px; color: var(--text-dim); border-bottom: 1px solid var(--border); }
.canonical-policy { display: flex; flex-direction: column; gap: 7px; padding: 10px 12px; font-size: 12px; overflow-wrap: anywhere; }
.changes-body { padding: 10px 12px; }
.changes-section { margin-bottom: 16px; }
.changes-section h3 { display: flex; gap: 6px; margin: 0 0 7px; color: var(--text-dim); font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: .04em; }
.changes-section h3 span { font-weight: 400; }
.changes-timeline { list-style: none; padding: 0; margin: 0; }
.change-row { position: relative; display: flex; align-items: flex-start; gap: 9px; padding: 9px 4px; border-radius: 5px; }
.change-row + .change-row::before { content: ""; position: absolute; left: 14px; top: -9px; height: 18px; border-left: 1px solid var(--border); }
.change-marker { display: flex; align-items: center; justify-content: center; flex: 0 0 21px; height: 21px; border: 1px solid var(--border); border-radius: 50%; color: var(--text-dim); font-size: 10px; background: var(--bg); z-index: 1; }
.change-content { flex: 1; min-width: 0; font-size: 12px; }
.change-content > strong, .change-content > .name { display: block; text-align: left; color: var(--text); line-height: 1.4; font-size: 12px; font-weight: 600; overflow-wrap: anywhere; }
.change-status { display: block; margin-top: 2px; font-size: 11px; color: var(--text-dim); }
.change-meta { margin-top: 3px; color: var(--text-dim); font-size: 10px; overflow-wrap: anywhere; }
.change-meta code { font-size: inherit; }
.change-actions { display: flex; align-items: center; gap: 9px; margin-top: 5px; }
.change-order { display: flex; flex-direction: column; gap: 2px; }
.change-order button { min-width: 25px; min-height: 25px; padding: 2px 5px; font-size: 13px; }
.change-order button:disabled { opacity: .3; }
.change-row.selected { outline: 1px solid var(--accent); }
.change-active { background: color-mix(in srgb, var(--accent) 9%, transparent); }
.change-active .change-marker { border-color: var(--accent); }
.change-active .change-status { color: var(--accent); }
.change-landed .change-marker { border-color: var(--ok); color: var(--ok); }
.change-landed .change-status { color: var(--ok); }
.changes-history { padding-top: 12px; border-top: 1px solid var(--border); }
.change-content details { margin-top: 5px; color: var(--text-dim); font-size: 11px; overflow-wrap: anywhere; }
.change-content summary { cursor: pointer; }
.change-problem { margin: 4px 0; color: var(--blocked); font-size: 11px; overflow-wrap: anywhere; }
.change-blocked .change-marker { border-color: var(--blocked); }
.change-row:focus-within { background: color-mix(in srgb, var(--accent) 6%, transparent); }
.change-row button:focus-visible, .change-content summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
