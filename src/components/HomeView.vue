<script setup lang="ts">
import { computed, ref } from "vue";
import type { SessionSummary } from "../App.vue";

const props = defineProps<{ fleet: SessionSummary[]; topic?: SessionSummary | null }>();
const emit = defineEmits<{
  (e: "select", id: string): void;
  (e: "new-thread"): void;
  (e: "open-zed", id: string): void;
  (e: "home"): void;
}>();

const restingOpen = ref(false);
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
  let attention = false;
  for (const key of ["attention", "triage"]) {
    const value = s.branch.tags.find((tag) => tag.key === key)?.value;
    if (value === "blocked") return "blocked";
    if (value === "attention") attention = true;
  }
  return attention || s.status === "error" ? "attention" : "ok";
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
  if (s.status === "error") return "errored";
  const tag = s.branch.tags.find((t) => t.key === "attention" || t.key === "triage");
  return tag?.note || s.branch.description || s.branch.goal || "—";
}

const scoped = computed(() => props.fleet.filter((s) =>
  (s.status !== "archived" || isReady(s)) && (!props.topic || rootOf(s).id === props.topic.id),
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
          <h1 v-else>Topics home</h1>
        </div>
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
        <div v-for="s in section.rows" :key="s.id" class="home-row" :class="level(s)"
          role="button" tabindex="0" :aria-label="`Open ${rowTitle(s)}: ${description(s)}`"
          @click="emit('select', s.id)" @keydown.enter.prevent="emit('select', s.id)" @keydown.space.prevent="emit('select', s.id)">
          <span class="level-dot" :class="section.kind === 'ready' ? 'ready' : level(s)"></span>
          <div class="row-main"><div class="row-name">{{ rowTitle(s) }}</div><div class="row-why">{{ description(s) }}</div></div>
          <span class="row-when">{{ ago(s.last_activity_at) }}</span>
        </div>
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
