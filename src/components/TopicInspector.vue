<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { backendOf } from "../backendIdentity";
import { useBackendInvoke } from "../backendInvoke";

import type { SessionSummary, SessionView } from "../App.vue";
import ResourcePanel from "./ResourcePanel.vue";
import type { ArtifactTarget } from "../artifactLinks";
import ChangeReview from "./ChangeReview.vue";
import { reviewIntegrationTarget } from "../reviewRequest";
import TrackActivity from "./TrackActivity.vue";
import AutolandControl from "./AutolandControl.vue";
import { canonicalTopicBranch, isTrackCoordinator, resolveTrack, threadLandFeedback, type IntegrationTarget } from "../trackPolicy.ts";
import { selectionKey, changeRegistry, pinSelection, sameChange, selectedChangePayload, selectionProblem, type ChangeSelection } from "../durableChange.ts";
import { landStrategies, strategyMemoryKey } from "../trackStrategies.ts";
import SplitButton from "./SplitButton.vue";
import {
  landingCandidates,
  type LandingCandidate,
  pendingPermissionSummary,
  statusClass,
  topicThreadRows,
  threadStatus,
} from "../topicInspector";

const invoke = useBackendInvoke();

// Track inspector: workers, review, and the canonical Landing queue.

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

const candidateLabels = { working: "Working output", waiting: "Declared output", blocked: "Needs reconciliation", landed: "Landed" };
const tab = ref<"threads" | "review" | "resources" | "landing" | "todos" | "activity" | "handoffs">("threads");
watch(() => props.artifactRequest, target => { if (target) tab.value = "resources"; }, { immediate: true });
watch(() => props.reviewRequest, request => {
  if (request && request.sessionId === props.selectedId) tab.value = "review";
}, { immediate: true });
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
watch(() => props.landingRequest, () => { tab.value = "landing"; }, { immediate: !!props.landingRequest });

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

// Land requests wrap-up and acceptance; only a reported outcome means completion.
async function onLand(candidate: LandingCandidate, strategy: string) {
  if (integratingId.value) return;
  const selected = selectedChange.value;
  if (!selected || selectionKey(selected) !== candidate.key || !sameChange(selected.change, candidate.change) || selectedProblem.value || candidate.state === "blocked" || candidate.state === "landed") return;
  integratingId.value = candidate.key;
  const topicId = props.topic.id;
  integrationNote.value = "";
  try {
    const target = await invoke<IntegrationTarget>(
      "land_change",
      { ...selectedChangePayload(selected), strategy },
    );
    try { localStorage.setItem(strategyMemoryKey("land", sessionRepo(candidate.session)), strategy); } catch { /* unavailable storage */ }
    if (props.topic.id === topicId) integrationNote.value = threadLandFeedback(target);
  } catch (error: any) {
    if (topicId !== props.topic.id) return;
    emit("error", error?.message ?? String(error));
  } finally {
    integratingId.value = null;
  }
}

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
      <button class="tab" :class="{ active: tab === 'threads' }" role="tab" :aria-selected="tab === 'threads'" :disabled="!topicScope" @click="tab = 'threads'">Threads</button>
      <button class="tab" :class="{ active: tab === 'review' }" role="tab" :aria-selected="tab === 'review'" @click="tab = 'review'">Review</button>
      <button class="tab" :class="{ active: tab === 'handoffs' }" role="tab" :aria-selected="tab === 'handoffs'" :disabled="!topicScope" @click="tab = 'handoffs'">Handoffs</button>
      <button class="tab" :class="{ active: tab === 'activity' }" role="tab" :aria-selected="tab === 'activity'" :disabled="!topicScope" @click="tab = 'activity'">Activity</button>
      <button class="tab" :class="{ active: tab === 'resources' }" role="tab" :aria-selected="tab === 'resources'" :disabled="!topicScope" @click="tab = 'resources'">Resources</button>
      <button class="tab" :class="{ active: tab === 'landing' }" role="tab" :aria-selected="tab === 'landing'" :disabled="!topicScope" @click="tab = 'landing'">Landing</button>
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

    <!-- Landing includes active work, advisory readiness, and accepted outcomes. -->
    <div v-else-if="tab === 'landing' && topicScope" class="inspector-body">
      <div v-if="integrationNote" class="inspector-note">{{ integrationNote }}</div>
      <div v-if="selectedProblem" class="inspector-note inspector-note-error" role="alert">{{ selectedProblem }}</div>
      <div v-for="message in registryErrors" :key="message" class="inspector-note inspector-note-error" role="alert">{{ message }}</div>
      <div v-if="!candidates.length" class="inspector-empty">
        No declared Changes.<br />
        <small>Threads with no explicit outputs require no Land.</small>
      </div>
      <div v-for="candidate in candidates" :key="candidate.key" class="inspector-candidate" :class="{ selected: selectedChange && candidate.key === selectionKey(selectedChange) }">
        <div class="inspector-candidate-head">
          <button class="link name" :aria-pressed="!!selectedChange && candidate.key === selectionKey(selectedChange)" @click="selectCandidate(candidate)">{{ candidate.change.title }}</button>
          <span class="badge" :class="candidate.state === 'landed' ? 'done' : 'dim'">
            {{ candidateLabels[candidate.state] }}
          </span>
        </div>
        <div class="inspector-candidate-sub">
          {{ candidate.key }} · {{ candidate.change.scope.kind }}<br />
          {{ candidate.change.scope.base_oid.slice(0, 10) }} → {{ candidate.change.scope.head_oid.slice(0, 10) }} · {{ candidate.change.scope.paths.length ? candidate.change.scope.paths.join(', ') : 'entire pinned range' }}<br />
          {{ candidate.note || candidate.session.branch.branch }}
        </div>
        <AutolandControl :session="candidate.session" :track="topicScope" @updated="emit('policy-updated', $event)" @error="emit('error', $event)" />
        <div class="inspector-candidate-actions">
          <button v-if="candidate.state !== 'landed'" class="link" :disabled="candidate.state === 'blocked'" @click="reviewCandidate(candidate)">Select / review</button>
          <SplitButton
            v-if="candidate.state !== 'landed' && candidate.state !== 'blocked'"
            kind="land"
            :repo="sessionRepo(candidate.session)"
            :options="landOptions"
            label="Land"
            :busy="integratingId === candidate.key"
            :disabled="!!integratingId || !selectedChange || candidate.key !== selectionKey(selectedChange) || !sameChange(selectedChange.change, candidate.change) || !!selectedProblem"
            @run="(strategy: string) => onLand(candidate, strategy)"
          />
        </div>
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
.inspector-candidate.selected { outline: 1px solid var(--accent); }
</style>
