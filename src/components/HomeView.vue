<script setup lang="ts">
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { SessionLayout, SessionSummary } from "../App.vue";
import { layoutProjects, topicProjectId } from "../projects";
import { dismissibleAttention, loudTag, pendingPermissionSummary } from "../topicInspector";

const props = defineProps<{
  fleet: SessionSummary[];
  topic?: SessionSummary | null;
  /** Selected project (from the sidebar's Topics tab): filters the home to
   *  that project's topics. `null` = the aggregate Topics home. */
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
function rowTitle(s: SessionSummary): string {
  const root = rootOf(s);
  const thread = root.id === s.id ? "Coordinator thread" : title(s);
  return props.topic ? thread : `${title(root)} · ${thread}`;
}

function level(s: SessionSummary): "blocked" | "attention" | "ok" {
  if (s.status === "archived") return "ok";
  const loud = loudTag(s);
  if (loud?.level === "blocked") return "blocked";
  return loud || s.status === "error" ? "attention" : "ok";
}

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
  const permission = pendingPermissionSummary(s);
  if (permission) return permission;
  if (s.status === "error") return "errored";
  const tag = s.branch.tags.find((t) => t.key === "attention" || t.key === "triage");
  return tag?.note || s.branch.description || s.branch.goal || "—";
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
  rows: SessionSummary[];
}
function groupByProject(rows: SessionSummary[]): ProjectGroup[] {
  const groups = new Map<string, SessionSummary[]>();
  for (const row of rows) {
    const key = projectNameOf(row);
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key)!.push(row);
  }
  return [...groups.entries()]
    .map(([name, groupRows]) => ({ name, rows: groupRows }))
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
            <div class="home-breadcrumb"><button @click="emit('home')">Topics home</button> → {{ title(topic) }}</div>
            <h1 class="topic-dashboard-title">{{ title(topic) }}</h1>
          </template>
          <template v-else-if="project">
            <div class="home-breadcrumb"><button @click="emit('home')">Topics home</button> → {{ project.name }}</div>
            <h1 class="project-dashboard-title">{{ project.name }}</h1>
          </template>
          <h1 v-else>Topics home</h1>
        </div>
        <button class="home-new" title="Start a durable topic — title, description, goal" @click="emit('new-topic')">
          + New topic
        </button>
        <button class="primary home-new" @click="emit('new-thread')">+ New thread</button>
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
        <div class="section-title">{{ section.name }}</div>
        <div v-if="!section.rows.length" :class="section.kind === 'needs' ? 'all-calm compact-calm' : 'home-section-empty'">
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
        </template>
        <template v-else>
          <template v-for="group in section.kind === 'working' ? workingGroups : readyGroups" :key="group.name">
            <div class="project-title">{{ group.name }}</div>
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
