<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { SessionSummary } from "../App.vue";
import ResourcePanel from "./ResourcePanel.vue";
import SplitButton from "./SplitButton.vue";
import {
  badgeLabel,
  integrationCandidates,
  type IntegrationCandidate,
  isIdle,
  loudTag,
  statusClass,
  topicThreadRows,
} from "../topicInspector";

// The Topic inspector (docs/design.md "Navigation"): the right pane while a
// Topic is open, with one tab per view of that topic — Threads, Resources,
// Integrations, Todos. Land is deliberately NOT here: it is a Topic action
// toward upstream, not an inspector view. The pure row/candidate logic
// lives in src/topicInspector.ts so it stays testable.

const props = defineProps<{
  topic: SessionSummary;
  fleet: SessionSummary[];
  selectedId: string | null;
}>();

const emit = defineEmits<{
  (e: "close"): void;
  (e: "error", message: string): void;
  (e: "select", id: string): void;
  (e: "new-thread"): void;
}>();

const tab = ref<"threads" | "resources" | "integrations" | "todos">("threads");

// --- Threads tab -----------------------------------------------------------
//
// The topic's coordinator + worker fleet with attention/working/readiness/
// resting states. The row semantics (loud tags → badge; running && !idle →
// spinner; quiet otherwise) mirror FleetSidebar's Inbox tab, scoped to the
// topic subtree rather than the lane filing system.

function subtitle(s: SessionSummary): string {
  return s.branch.description || s.branch.title || "—";
}

const collapsed = ref(new Set<string>());

const topicRows = computed(() => topicThreadRows(props.fleet, props.topic.id, collapsed.value));

function toggle(id: string) {
  if (collapsed.value.has(id)) collapsed.value.delete(id);
  else collapsed.value.add(id);
}

// --- Integrations tab -------------------------------------------------------
//
// The Topic-owned candidate queue. A worker qualifies ONLY on verified
// candidate state reported by Loom (design.md: "Ready to Integrate requires
// explicit verified candidate state from Loom; a stopped or sleeping worker
// is not sufficient evidence"). Workers with a recorded `integration_result`
// are shown as integrated outcomes. Land is not here — it is a Topic action.

const candidates = computed(() => integrationCandidates(topicRows.value, props.topic.id));

const integrateOptions = [
  { value: "squash", label: "Squash into topic" },
  { value: "merge", label: "Merge into topic" },
  { value: "rebase", label: "Rebase onto topic" },
  { value: "cherry-pick", label: "Cherry-pick commits" },
  { value: "open-pr", label: "Open PR into topic" },
  { value: "ask", label: "Ask coordinator to decide" },
];

const sessionRepo = (s: SessionSummary) => s.github_repo || s.branch.repo_root;
const integratingId = ref<string | null>(null);
const integrationNote = ref("");

// The integrate path mirrors ThreadView.onIntegrate: the structured request
// goes to the worker's coordinator (the topic here), strategy memory is
// per-repo, and a sent request is not a success state.
async function onIntegrate(candidate: IntegrationCandidate, strategy: string) {
  if (integratingId.value) return;
  integratingId.value = candidate.session.id;
  integrationNote.value = "";
  try {
    const target = await invoke<{ coordinator_id: string; target_branch: string; coordinator_name: string }>(
      "integrate_session",
      { sessionId: candidate.session.id, strategy },
    );
    localStorage.setItem(`arachne:strategy:integrate:${sessionRepo(candidate.session)}`, strategy);
    integrationNote.value = `Sent to ${target.coordinator_name} (${target.target_branch}). Follow the integration in that thread.`;
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
  <aside class="topic-inspector" aria-label="Topic inspector">
    <header class="topic-inspector-head">
      <div class="topic-inspector-topic">
        <strong>{{ topic.branch.title || topic.branch.name }}</strong>
      </div>
      <button title="Close inspector" aria-label="Close inspector" @click="emit('close')">×</button>
    </header>
    <div class="tab-bar inspector-tab-bar" role="tablist">
      <button class="tab" :class="{ active: tab === 'threads' }" role="tab" :aria-selected="tab === 'threads'" @click="tab = 'threads'">Threads</button>
      <button class="tab" :class="{ active: tab === 'resources' }" role="tab" :aria-selected="tab === 'resources'" @click="tab = 'resources'">Resources</button>
      <button class="tab" :class="{ active: tab === 'integrations' }" role="tab" :aria-selected="tab === 'integrations'" @click="tab = 'integrations'">Integrations</button>
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
            <span v-if="row.session.status === 'running' && !isIdle(row.session) && !loudTag(row.session)"
              class="spinner mini" title="working" aria-hidden="true"></span>
            <span v-else-if="badgeLabel(row.session)" class="badge" :class="statusClass(row.session)">{{ badgeLabel(row.session) }}</span>
            <span v-if="row.childCount > 0" class="badge dim" :title="`${row.childCount} delegated children`">{{ row.childCount }}</span>
          </div>
          <div class="title">{{ subtitle(row.session) }}</div>
        </div>
        <div v-if="!topicRows.length" class="inspector-empty">No threads in this topic yet.</div>
      </div>
    </div>

    <!-- Resources: the topic's attached resources and refs (ResourcePanel's
         content embedded here as the tab body). -->
    <div v-else-if="tab === 'resources'" class="inspector-body inspector-body-flush">
      <ResourcePanel :topic="topic" embedded @error="(message: string) => emit('error', message)" />
    </div>

    <!-- Integrations: the Topic-owned candidate queue. Only verified
         candidate state from Loom qualifies; a stopped or sleeping worker is
         not evidence of readiness. -->
    <div v-else-if="tab === 'integrations'" class="inspector-body">
      <div v-if="integrationNote" class="inspector-note">{{ integrationNote }}</div>
      <div v-if="!candidates.length" class="inspector-empty">
        No integration candidates yet.<br />
        <small>Loom reports readiness explicitly — a stopped or sleeping worker is not a candidate.</small>
      </div>
      <div v-for="candidate in candidates" :key="candidate.session.id" class="inspector-candidate">
        <div class="inspector-candidate-head">
          <span class="name">{{ candidate.session.branch.title || candidate.session.branch.name }}</span>
          <span class="badge" :class="candidate.state === 'ready' ? 'done' : 'dim'">
            {{ candidate.state === "ready" ? "Ready" : "Integrated" }}
          </span>
        </div>
        <div class="inspector-candidate-sub">
          {{ candidate.note || candidate.session.branch.branch }}
        </div>
        <div class="inspector-candidate-actions">
          <button class="link" @click="emit('select', candidate.session.id)">Review</button>
          <SplitButton
            v-if="candidate.state === 'ready'"
            kind="integrate"
            :repo="sessionRepo(candidate.session)"
            :options="integrateOptions"
            label="Integrate"
            :busy="integratingId === candidate.session.id"
            @run="(strategy: string) => onIntegrate(candidate, strategy)"
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
          placeholder="Add a todo for this topic…"
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
