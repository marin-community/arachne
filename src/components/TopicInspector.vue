<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { SessionSummary, SessionView } from "../App.vue";
import ResourcePanel from "./ResourcePanel.vue";
import type { ArtifactTarget } from "../artifactLinks";
import ChangeReview from "./ChangeReview.vue";
import { reviewIntegrationTarget } from "../reviewRequest";
import TrackActivity from "./TrackActivity.vue";
import AutolandControl from "./AutolandControl.vue";
import { canonicalTopicBranch, isTrackCoordinator, threadLandFeedback, type IntegrationTarget } from "../trackPolicy.ts";
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

// Track inspector: workers, review, and the canonical Landing queue.

const props = defineProps<{
  topic: SessionSummary;
  fleet: SessionSummary[];
  selectedId: string | null;
  loomUrl?: string;
  artifactRequest?: ArtifactTarget | null;
}>();

const emit = defineEmits<{
  (e: "open-artifact", target: ArtifactTarget): void;
  (e: "close"): void;
  (e: "error", message: string): void;
  (e: "select", id: string): void;
  (e: "new-thread"): void;
  (e: "checkout-saved", sessionId: string): void;
  (e: "policy-updated", session: SessionView): void;
}>();

const candidateLabels = { working: "Working", waiting: "Waiting", ready: "Reported ready", stale: "Needs recheck", conflicted: "Conflict", landed: "Landed" };
const tab = ref<"threads" | "review" | "resources" | "landing" | "todos" | "activity" | "handoffs">("threads");
watch(() => props.artifactRequest, target => { if (target) tab.value = "resources"; }, { immediate: true });
const reviewSession = computed(() => props.fleet.find(session => session.id === props.selectedId) ?? props.topic);
const canEdit = computed(() => {
  try { return ["localhost", "127.0.0.1", "::1", "[::1]"].includes(new URL(props.loomUrl ?? "").hostname); }
  catch { return false; }
});
function reviewCandidate(id: string) { emit("select", id); tab.value = "review"; }

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

const topicRows = computed(() => topicThreadRows(props.fleet, props.topic.id, collapsed.value));

function toggle(id: string) {
  if (collapsed.value.has(id)) collapsed.value.delete(id);
  else collapsed.value.add(id);
}

// Queue membership does not depend on the Threads tree's collapse state.
const candidates = computed(() => landingCandidates(topicThreadRows(props.fleet, props.topic.id, new Set()), props.topic.id));
const landOptions = landStrategies();

const sessionRepo = (s: SessionSummary) => s.github_repo || s.branch.repo_root;
const integratingId = ref<string | null>(null);
const integrationNote = ref("");

// Land requests wrap-up and acceptance; only a reported outcome means completion.
async function onLand(candidate: LandingCandidate, strategy: string) {
  if (integratingId.value) return;
  integratingId.value = candidate.session.id;
  const topicId = props.topic.id;
  integrationNote.value = "";
  try {
    const target = await invoke<IntegrationTarget>(
      "land_thread",
      { sessionId: candidate.session.id, strategy },
    );
    try { localStorage.setItem(strategyMemoryKey("land", sessionRepo(candidate.session)), strategy); } catch { /* unavailable storage */ }
    if (props.topic.id === topicId) integrationNote.value = threadLandFeedback(target);
  } catch (error: any) {
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
  const topicId = props.topic.id;
  todosLoading.value = true;
  todoError.value = "";
  try {
    const next = await invoke<TodoTopicView>("topic_todos", { topicId });
    if (topicId !== props.topic.id) return;
    if (!next || !Array.isArray(next.todos)) throw new Error("Loom returned an invalid todo list");
    todos.value = next;
  } catch (error: any) {
    todoError.value = error?.message ?? String(error);
  } finally {
    todosLoading.value = false;
  }
}

async function addTodo() {
  const text = todoDraft.value.trim();
  if (!text || todoBusy.value) return;
  todoBusy.value = true;
  todoError.value = "";
  try {
    const next = await invoke<TodoTopicView>("add_todo", {
      topicId: props.topic.id,
      text,
      expectedRevision: todos.value.revision,
    });
    todos.value = next;
    todoDraft.value = "";
  } catch (error: any) {
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
  try {
    todos.value = await invoke<TodoTopicView>("toggle_todo", {
      topicId: props.topic.id,
      todoId: todo.id,
      expectedRevision: todos.value.revision,
    });
  } catch (error: any) {
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
  try {
    todos.value = await invoke<TodoTopicView>("remove_todo", {
      topicId: props.topic.id,
      todoId: todo.id,
      expectedRevision: todos.value.revision,
    });
  } catch (error: any) {
    await refreshTodos();
    todoError.value = error?.message ?? String(error);
  } finally {
    todoBusy.value = false;
  }
}

// Reset per-topic state when the inspected topic changes: the todo slice,
// collapse state, and the integration note all belong to one topic.
watch(() => props.topic.id, () => {
  todos.value = { todos: [], revision: 0 };
  todoDraft.value = "";
  todoError.value = "";
  integrationNote.value = "";
  collapsed.value = new Set();
  void refreshTodos();
}, { immediate: true });
</script>

<template>
  <aside class="topic-inspector" aria-label="Track inspector">
    <header class="topic-inspector-head">
      <div class="topic-inspector-topic">
        <strong>{{ topic.branch.title || topic.branch.name }}</strong>
      </div>
      <button title="Close inspector" aria-label="Close inspector" @click="emit('close')">×</button>
    </header>
    <div v-if="isTrackCoordinator(topic)" class="canonical-policy">
      <span>Canonical destination: <code>{{ canonicalTopicBranch(topic) }}</code></span>
      <AutolandControl :session="topic" :track="topic" coordinator @updated="emit('policy-updated', $event)" @error="emit('error', $event)" />
    </div>
    <div class="tab-bar inspector-tab-bar" role="tablist">
      <button class="tab" :class="{ active: tab === 'threads' }" role="tab" :aria-selected="tab === 'threads'" @click="tab = 'threads'">Threads</button>
      <button class="tab" :class="{ active: tab === 'review' }" role="tab" :aria-selected="tab === 'review'" @click="tab = 'review'">Review</button>
      <button class="tab" :class="{ active: tab === 'handoffs' }" role="tab" :aria-selected="tab === 'handoffs'" @click="tab = 'handoffs'">Handoffs</button>
      <button class="tab" :class="{ active: tab === 'activity' }" role="tab" :aria-selected="tab === 'activity'" @click="tab = 'activity'">Activity</button>
      <button class="tab" :class="{ active: tab === 'resources' }" role="tab" :aria-selected="tab === 'resources'" @click="tab = 'resources'">Resources</button>
      <button class="tab" :class="{ active: tab === 'landing' }" role="tab" :aria-selected="tab === 'landing'" @click="tab = 'landing'">Landing</button>
      <button class="tab" :class="{ active: tab === 'todos' }" role="tab" :aria-selected="tab === 'todos'" @click="tab = 'todos'">Todos</button>
    </div>

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
        </div>
        <div v-if="!topicRows.length" class="inspector-empty">No threads in this track yet.</div>
      </div>
    </div>

    <div v-else-if="tab === 'handoffs'" class="inspector-body inspector-body-flush">
      <TrackActivity :loom-url="loomUrl" @open-artifact="emit('open-artifact', $event)" :topic="topic" results-only @select="emit('select', $event)" />
    </div>

    <!-- Resources: the topic's attached resources and refs (ResourcePanel's
         content embedded here as the tab body). -->
    <div v-else-if="tab === 'review'" class="inspector-body inspector-body-flush">
      <div class="inspector-review-context">{{ reviewSession.branch.title || reviewSession.branch.name }}</div>
      <ChangeReview :session-id="reviewSession.id" :is-topic="reviewSession.id === topic.id" :integration-target="reviewIntegrationTarget(fleet, reviewSession.id)" :can-edit="canEdit" embedded @saved="emit('checkout-saved', reviewSession.id)" />
    </div>
    <div v-else-if="tab === 'activity'" class="inspector-body inspector-body-flush"><TrackActivity :loom-url="loomUrl" @open-artifact="emit('open-artifact', $event)" :topic="topic" @select="emit('select', $event)" /></div>
    <div v-else-if="tab === 'resources'" class="inspector-body inspector-body-flush">
      <ResourcePanel :key="loomUrl" :artifact-request="artifactRequest" :topic="topic" :can-edit="canEdit" embedded @checkout-saved="emit('checkout-saved', $event)" @error="(message: string) => emit('error', message)" />
    </div>

    <!-- Landing includes active work, advisory readiness, and accepted outcomes. -->
    <div v-else-if="tab === 'landing'" class="inspector-body">
      <div v-if="integrationNote" class="inspector-note">{{ integrationNote }}</div>
      <div v-if="!candidates.length" class="inspector-empty">
        No landing candidates yet.<br />
        <small>Working and waiting threads appear here. Readiness is reported explicitly.</small>
      </div>
      <div v-for="candidate in candidates" :key="candidate.session.id" class="inspector-candidate" :class="{ selected: candidate.session.id === selectedId }">
        <div class="inspector-candidate-head">
          <button class="link name" :aria-pressed="candidate.session.id === selectedId" @click="emit('select', candidate.session.id)">{{ candidate.session.branch.title || candidate.session.branch.name }}</button>
          <span class="badge" :class="candidate.state === 'landed' ? 'done' : 'dim'">
            {{ candidateLabels[candidate.state] }}
          </span>
        </div>
        <div class="inspector-candidate-sub">
          {{ candidate.note || candidate.session.branch.branch }}
        </div>
        <AutolandControl :session="candidate.session" :track="topic" @updated="emit('policy-updated', $event)" @error="emit('error', $event)" />
        <div class="inspector-candidate-actions">
          <button class="link" @click="reviewCandidate(candidate.session.id)">{{ candidate.state === 'stale' || candidate.state === 'conflicted' ? "Reconcile / review" : "Review" }}</button>
          <SplitButton
            v-if="candidate.state !== 'landed'"
            kind="land"
            :repo="sessionRepo(candidate.session)"
            :options="landOptions"
            label="Land"
            :busy="integratingId === candidate.session.id"
            :disabled="!!integratingId || candidate.session.status === 'archived'"
            @run="(strategy: string) => onLand(candidate, strategy)"
          />
        </div>
      </div>
    </div>

    <!-- Todos: a topic-filtered view of the durable cross-topic user todo
         list. Topic plans and worker checklists stay separate. -->
    <div v-else class="inspector-body">
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
.inspector-tab-bar { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); }
.inspector-tab-bar .tab { padding: 7px 6px; }
.inspector-review-context { padding: 8px 12px; font-size: 12px; color: var(--text-dim); border-bottom: 1px solid var(--border); }
.canonical-policy { display: flex; flex-direction: column; gap: 7px; padding: 10px 12px; font-size: 12px; overflow-wrap: anywhere; }
.inspector-candidate.selected { outline: 1px solid var(--accent); }
</style>
