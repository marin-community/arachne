<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import type { SessionLayout, SessionSummary } from "../App.vue";
import { homeRowTitle } from "../homeRows";
import { layoutProjects, topicProjectId } from "../projects";
import { attentionAction, attentionLevel, attentionReason, dismissibleAttention } from "../topicInspector";

const props = defineProps<{
  fleet: SessionSummary[];
  topic?: SessionSummary | null;
  /** Selected project (from the sidebar's Topics tab): filters the home to
   *  that project's topics. `null` = the aggregate Tracks home. */
  project?: { id: string | null; name: string } | null;
  /** The session layout (from the fleet snapshot) — supplies the project
   *  group ids. A project is a non-system layout group (src/projects.ts). */
  layout?: SessionLayout | null;
}>();
const emit = defineEmits<{
  (e: "select", id: string): void;
  (e: "new-thread"): void;
  (e: "new-topic"): void;
  (e: "open-zed", id: string): void;
  (e: "home"): void;
  (e: "manage-project-resources", project: { id: string; name: string }): void;
  (e: "error", message: string): void;
}>();

// Dismiss without opening the thread: clear the loud tag axes on the
// session's branch. The Needs You row is the person's inbox — a row is
// handled by answering it in the thread, or by deciding it needs no reply
// at all, and the latter should be one click, not a detour through the
// conversation. Permission-raised attention offers no dismiss (see
// dismissibleAttention); its row still opens the thread to answer.
const dismissingId = ref<string | null>(null);
async function dismiss(s: SessionSummary) {
  if (dismissingId.value) return;
  dismissingId.value = s.id;
  try {
    await invoke("clear_attention", { session: s.id });
  } catch (error: any) {
    emit("error", error?.message ?? String(error));
  } finally {
    dismissingId.value = null;
  }
}

const restingOpen = ref(false);
const projectIds = computed(() => new Set(layoutProjects(props.layout).map((p) => p.id)));
const projects = computed(() => layoutProjects(props.layout));
// A row's project: its root (coordinator) session's placement group when
// that group is a non-system layout group — the same model the sidebar's
// Topics tab uses (src/projects.ts).
const projectIdOf = (s: SessionSummary): string | null =>
  topicProjectId(rootOf(s), projectIds.value);
// A row's project name: the layout group's name when filed under a
// project, else the repo slug (github_repo) or the repo_root directory
// with any .worktrees/<name> suffix peeled, so a session running in a
// worktree still groups with its parent repo. "—" for unknown.
const projectNameOf = (s: SessionSummary): string => {
  const id = projectIdOf(s);
  const project = projects.value.find((p) => p.id === id);
  if (project) return project.name;
  if (s.github_repo) return s.github_repo;
  const root = s.branch.repo_root;
  if (!root) return "—";
  const idx = root.indexOf("/.worktrees/");
  const base = idx >= 0 ? root.slice(0, idx) : root;
  return base.split("/").filter(Boolean).pop() ?? root;
};
const byId = computed(() => new Map(props.fleet.map((s) => [s.id, s])));
const byBranch = computed(() => new Map(props.fleet.map((s) => [s.branch.id, s])));
const parentOf = (s: SessionSummary) =>
  (s.parent_session_id ? byId.value.get(s.parent_session_id) : undefined) ??
  (s.parent_id ? byBranch.value.get(s.parent_id) : undefined);

function rootOf(s: SessionSummary): SessionSummary {
  let node = s;
  const seen = new Set<string>();
  while (!seen.has(node.id)) {
    seen.add(node.id);
    const parent = parentOf(node);
    if (!parent || parent.id === node.id) break;
    node = parent;
  }
  return node;
}

const title = (s: SessionSummary) => s.branch.title || s.branch.name;
// Row naming per docs/design.md: on the aggregate home every row names its
// Project and parent Topic; scoped views drop the parts the heading or the
// dashboard already names. Pure logic lives in src/homeRows.ts.
function rowTitle(s: SessionSummary): string {
  return homeRowTitle(s, rootOf(s), projectNameOf(s), {
    topic: props.topic != null,
    project: props.project?.id != null,
  });
}

const level = attentionLevel;

const isIdle = (s: SessionSummary) => s.branch.tags.some((tag) => tag.key === "idle");
// Do not infer readiness from a stopped worker. Loom must report it explicitly.
const isReady = (s: SessionSummary) =>
  s.branch.tags.some((tag) =>
    (tag.key === "integration_ready" || tag.key === "integration_state") &&
    (tag.value === "true" || tag.value === "ready"),
  ) && !s.branch.tags.some((tag) => tag.key === "integration_result");

function ago(iso: string): string {
  const minutes = Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 60000));
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}h ago` : `${Math.floor(hours / 24)}d ago`;
}

function description(s: SessionSummary): string {
  return attentionReason(s) || s.branch.description || s.branch.goal || "—";
}

const scoped = computed(() => props.fleet.filter((s) =>
  (s.status !== "archived" || isReady(s)) &&
  (!props.topic || rootOf(s).id === props.topic.id) &&
  (props.project?.id == null || projectIdOf(s) === props.project.id),
));
const byRecency = (a: SessionSummary, b: SessionSummary) => b.last_activity_at.localeCompare(a.last_activity_at);
const needs = computed(() => scoped.value.filter((s) => level(s) !== "ok")
  .sort((a, b) => (level(a) === "blocked" ? 0 : 1) - (level(b) === "blocked" ? 0 : 1) || byRecency(a, b)));
const working = computed(() => scoped.value.filter((s) =>
  level(s) === "ok" && s.status === "running" && !isIdle(s) && !isReady(s),
).sort(byRecency));
const ready = computed(() => scoped.value.filter((s) => level(s) === "ok" && isReady(s)).sort(byRecency));
const resting = computed(() => scoped.value.filter((s) =>
  level(s) === "ok" && !isReady(s) && (s.status !== "running" || isIdle(s)),
).sort(byRecency));
interface UserTodo { id: string; text: string; topic_id: string; revision: number }
const userTodos = ref<UserTodo[]>([]);
const todoWarnings = ref<string[]>([]);
const todoBusy = ref<string | null>(null);
const todosLoading = ref(false);
let todoTimer: ReturnType<typeof setTimeout> | undefined;
let todoUnlisten: UnlistenFn | undefined;
let todosDisposed = false;
let todoRequest = 0;
let todosDirty = false;
const todoTopicIds = computed(() => [...new Set(scoped.value.map((s) => rootOf(s).id))].sort());
const visibleTodos = computed(() => userTodos.value.filter((todo) => todoTopicIds.value.includes(todo.topic_id)));
async function refreshUserTodos() {
  if (todosDisposed) return;
  if (todosLoading.value) { todosDirty = true; return; }
  const request = ++todoRequest;
  todosLoading.value = true;
  try {
    const result = await invoke<{ todos: UserTodo[]; warnings: string[] }>("user_todos", { topicIds: todoTopicIds.value });
    if (todosDisposed || request !== todoRequest) return;
    userTodos.value = result.todos;
    todoWarnings.value = result.warnings;
  } catch (error: any) {
    if (!todosDisposed) todoWarnings.value = [`Could not load user Todos: ${error?.message ?? String(error)}`];
  } finally {
    todosLoading.value = false;
    if (todosDirty && !todosDisposed) { todosDirty = false; scheduleTodos(); }
  }
}
function scheduleTodos() {
  if (todoTimer || todosDisposed) return;
  todoTimer = setTimeout(() => { todoTimer = undefined; void refreshUserTodos(); }, 600);
}
async function completeTodo(todo: UserTodo) {
  if (todoBusy.value) return;
  todoBusy.value = todo.id;
  try {
    await invoke("toggle_todo", { topicId: todo.topic_id, todoId: todo.id, expectedRevision: todo.revision });
    userTodos.value = userTodos.value.filter((row) => row.id !== todo.id);
    await refreshUserTodos();
  } catch (error: any) {
    await refreshUserTodos();
    todoWarnings.value = [...todoWarnings.value, error?.message ?? String(error)];
  } finally { todoBusy.value = null; }
}
onMounted(async () => {
  void refreshUserTodos();
  try {
    const stop = await listen("loom://fleet", scheduleTodos);
    if (todosDisposed) stop(); else todoUnlisten = stop;
  } catch { /* Manual refresh remains available without the event bridge. */ }
});
onUnmounted(() => { todosDisposed = true; todoRequest++; todoUnlisten?.(); if (todoTimer) clearTimeout(todoTimer); });
watch(() => todoTopicIds.value.join(","), scheduleTodos);

const sections = computed(() => [
  { name: "Needs You", rows: needs.value, kind: "needs" },
  { name: "Working", rows: working.value, kind: "working" },
  { name: "Ready to Integrate", rows: ready.value, kind: "ready" },
]);

// Below the attention/priority inbox, Working and Ready rows group by
// project (the layout group, else the repo). Groups order by their most
// recent row's activity, so the project with current work rises to the
// top of its section. When a specific project's home is open, every row
// shares one project — the grouping only matters on the aggregate home.
interface ProjectGroup {
  name: string;
  /** The layout-group id when the group is a real project — the id the
   *   project-bindings store is keyed by; null for repo-derived groups. */
  id: string | null;
  rows: SessionSummary[];
}
function groupByProject(rows: SessionSummary[]): ProjectGroup[] {
  const groups = new Map<string, ProjectGroup>();
  for (const row of rows) {
    const key = projectNameOf(row);
    if (!groups.has(key)) {
      groups.set(key, { name: key, id: projectIdOf(row), rows: [] });
    }
    groups.get(key)!.rows.push(row);
  }
  return [...groups.values()]
    .sort((a, b) => b.rows[0].last_activity_at.localeCompare(a.rows[0].last_activity_at));
}
const workingGroups = computed(() => groupByProject(working.value));
const readyGroups = computed(() => groupByProject(ready.value));
</script>

<template>
  <section class="home">
    <div class="home-scroll">
      <div class="home-top">
        <div class="home-heading">
          <template v-if="topic">
            <div class="home-breadcrumb"><button @click="emit('home')">Tracks home</button> → {{ title(topic) }}</div>
            <h1 class="topic-dashboard-title">{{ title(topic) }}</h1>
          </template>
          <template v-else-if="project">
            <div class="home-breadcrumb"><button @click="emit('home')">Tracks home</button> → {{ project.name }}</div>
            <h1 class="project-dashboard-title">{{ project.name }}</h1>
          </template>
          <h1 v-else>Tracks home</h1>
        </div>
        <!-- Project home: manage what this project's new topics inherit
             (design.md "Project defaults and resource inheritance"). The
             sidebar heading offers the same affordance (⚙); this is the
             home-surface entry point. -->
        <button
          v-if="project?.id"
          class="home-manage-resources"
          :title="`Manage resources inherited by new tracks in ${project.name}`"
          :aria-label="`Manage resources for project ${project.name}`"
          @click="emit('manage-project-resources', { id: project.id, name: project.name })"
        >◇ Resources</button>
        <button class="primary home-new" title="Start a durable track — title, description, goal" @click="emit('new-topic')">
          + New track
        </button>
        <button class="home-new" @click="emit('new-thread')">+ New thread</button>
      </div>
      <template v-if="topic">
        <p v-if="topic.branch.description || topic.branch.goal" class="topic-dashboard-summary">{{ topic.branch.description || topic.branch.goal }}</p>
        <div class="topic-dashboard-meta">{{ topic.github_repo || topic.branch.repo_root || "Repository unavailable" }} · {{ topic.branch.branch || topic.branch.name }}</div>
        <div class="topic-dashboard-actions">
          <button @click="emit('select', topic.id)">Open coordinator thread</button>
          <button @click="emit('open-zed', topic.id)">Open in Zed</button>
        </div>
      </template>

      <div v-for="section in sections" :key="section.kind" class="section" :class="`${section.kind}-section`">
        <div class="section-title">{{ section.name }} <span class="home-section-count">{{ section.rows.length }}</span></div>
        <div v-if="!section.rows.length && (section.kind !== 'needs' || (!visibleTodos.length && !todosLoading && !todoWarnings.length))" :class="section.kind === 'needs' ? 'all-calm compact-calm' : 'home-section-empty'">
          {{ section.kind === "needs" ? "Nothing needs you." : section.kind === "ready" ? "No candidates reported by Loom yet." : "No threads working." }}
        </div>
        <!-- Needs You stays flat: it's the priority inbox, one glance. The
             calmer Working / Ready sections group their rows by project. -->
        <template v-if="section.kind === 'needs'">
          <div v-for="s in section.rows" :key="s.id" class="home-row" :class="level(s)"
            role="button" tabindex="0" :aria-label="`Open ${rowTitle(s)}: ${description(s)}`"
            @click="emit('select', s.id)" @keydown.enter.prevent="emit('select', s.id)" @keydown.space.prevent="emit('select', s.id)">
            <span class="level-dot" :class="level(s)"></span>
            <div class="row-main"><div class="row-name">{{ rowTitle(s) }}</div><div class="row-why">{{ description(s) }}</div></div>
            <span class="home-action-label">{{ attentionAction(s) }}</span>
            <button v-if="rootOf(s).id !== s.id" class="home-coordinator-link"
              :title="`Open the coordinator for ${title(rootOf(s))}`"
              @click.stop="emit('select', rootOf(s).id)"
              @keydown.enter.stop @keydown.space.stop>Coordinator</button>
            <button
              v-if="section.kind === 'needs' && dismissibleAttention(s)"
              class="home-row-dismiss"
              type="button"
              :title="`Dismiss attention on ${title(s)} — clears the flag without opening the thread`"
              :aria-label="`Dismiss attention on ${title(s)}`"
              :disabled="dismissingId === s.id"
              @click.stop.prevent="dismiss(s)"
              @keydown.enter.stop.prevent="dismiss(s)"
              @keydown.space.stop.prevent="dismiss(s)"
            >{{ dismissingId === s.id ? "…" : "✕" }}</button>
            <span class="row-when">{{ ago(s.last_activity_at) }}</span>
          </div>
          <section class="home-user-todos section" aria-label="Your Todos">
            <div class="home-todos-heading"><div class="section-title">Your Todos <span class="home-section-count">{{ visibleTodos.length }}</span></div>
              <button type="button" :disabled="todosLoading" @click="refreshUserTodos">{{ todosLoading ? "Loading…" : "Refresh" }}</button></div>
            <p v-for="warning in todoWarnings" :key="warning" class="home-todo-error" role="alert">{{ warning }}</p>
            <div v-if="!visibleTodos.length && !todosLoading && !todoWarnings.length" class="home-section-empty">No open user Todos.</div>
            <div v-for="todo in visibleTodos" :key="todo.id" class="home-todo-row">
              <button type="button" class="home-todo-complete" :disabled="todoBusy !== null" :aria-label="`Complete ${todo.text}`" title="Mark complete" @click="completeTodo(todo)">{{ todoBusy === todo.id ? "…" : "✓" }}</button>
              <div class="row-main"><div>{{ todo.text }}</div><button class="home-todo-track" type="button" @click="emit('select', todo.topic_id)">{{ byId.get(todo.topic_id)?.branch.title || byId.get(todo.topic_id)?.branch.name || "Open Track" }}</button></div>
            </div>
          </section>

        </template>
        <template v-else>
          <template v-for="group in section.kind === 'working' ? workingGroups : readyGroups" :key="group.name">
            <!-- Aggregate home: each project group carries the same manage
                 affordance as the filtered project home — real projects
                 only (repo-derived groups have no binding store). -->
            <div class="project-title">
              {{ group.name }}
              <button
                v-if="group.id"
                class="project-title-manage"
                :title="`Manage resources inherited by new tracks in ${group.name}`"
                :aria-label="`Manage resources for project ${group.name}`"
                @click="emit('manage-project-resources', { id: group.id, name: group.name })"
              >⚙ resources</button>
            </div>
            <div v-for="s in group.rows" :key="s.id" class="home-row" :class="section.kind === 'ready' ? 'ready' : level(s)"
              role="button" tabindex="0" :aria-label="`Open ${rowTitle(s)}: ${description(s)}`"
              @click="emit('select', s.id)" @keydown.enter.prevent="emit('select', s.id)" @keydown.space.prevent="emit('select', s.id)">
              <span class="level-dot" :class="section.kind === 'ready' ? 'ready' : level(s)"></span>
              <div class="row-main"><div class="row-name">{{ rowTitle(s) }}</div><div class="row-why">{{ description(s) }}</div></div>
              <span class="row-when">{{ ago(s.last_activity_at) }}</span>
            </div>
          </template>
        </template>
      </div>

      <div v-if="resting.length" class="section">
        <button class="section-title resting-toggle" :aria-expanded="restingOpen" @click="restingOpen = !restingOpen">
          {{ restingOpen ? "▾" : "▸" }} Waiting / Resting · {{ resting.length }}
        </button>
        <div v-if="restingOpen" v-for="s in resting" :key="s.id" class="home-row dim" role="button" tabindex="0"
          :aria-label="`Open ${rowTitle(s)}`" @click="emit('select', s.id)"
          @keydown.enter.prevent="emit('select', s.id)" @keydown.space.prevent="emit('select', s.id)">
          <span class="level-dot dim"></span>
          <div class="row-main"><div class="row-name">{{ rowTitle(s) }}</div><div class="row-why">{{ description(s) }}</div></div>
          <span class="row-when">{{ ago(s.last_activity_at) }}</span>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.home-todos-heading { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
.home-todos-heading button { font-size: 11px; padding: 3px 7px; }
.home-todo-row { display: flex; gap: 10px; align-items: flex-start; padding: 9px 0; border-bottom: 1px solid var(--border); }
.home-todo-complete { flex-shrink: 0; padding: 3px 7px; }
.home-todo-track { border: 0; background: transparent; padding: 2px 0; color: var(--accent); font-size: 11px; }
.home-todo-error { color: var(--blocked); font-size: 12px; }
.home-section-count { margin-left: 6px; font-variant-numeric: tabular-nums; opacity: .65; }
.home-action-label { flex-shrink: 0; font-size: 11px; color: var(--text-dim); }
.home-coordinator-link { flex-shrink: 0; padding: 4px 7px; font-size: 11px; }
@media (max-width: 850px) { .home-action-label { display: none; } }
</style>
