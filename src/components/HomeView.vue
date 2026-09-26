<script setup lang="ts">
import { computed } from "vue";
import type { SessionSummary } from "../App.vue";

// The attention-first home screen: the default answer to "what needs me?"
// before any session is opened. Mirrors loom's canonical attention_level
// (crates/loom/src/web/sessions.rs):
//   archived            → ok (not listed)
//   any tag value       → blocked   (needs the user now)
//     "blocked"
//   status error/orphan → attention (needs the user soon)
//   or tag "attention"
//   anything else       → ok
// `idle` is a quiet mark (loom never puts it on the loud ladder): an agent
// that finished its turn is resting, not needing.
const props = defineProps<{
  fleet: SessionSummary[];
  selectedId: string | null;
}>();

const emit = defineEmits<{
  (e: "select", id: string): void;
}>();

function loudValue(s: SessionSummary): "blocked" | "attention" | null {
  // Both `attention` (the agent's self-report) and `triage` (a watch's
  // outside assessment) are loud keys; blocked outranks attention.
  let seen: "blocked" | "attention" | null = null;
  for (const key of ["attention", "triage"]) {
    const tag = s.branch.tags.find((t) => t.key === key);
    if (tag?.value === "blocked") return "blocked";
    if (tag?.value === "attention") seen = "attention";
  }
  return seen;
}

/// The canonical attention level of a row.
function level(s: SessionSummary): "blocked" | "attention" | "ok" {
  if (s.status === "archived") return "ok";
  const loud = loudValue(s);
  if (loud === "blocked") return "blocked";
  if (loud === "attention") return "attention";
  if (s.status === "error" || s.status === "orphaned") return "attention";
  return "ok";
}

function isIdle(s: SessionSummary): boolean {
  return s.branch.tags.some((t) => t.key === "idle");
}

// --- Buckets --------------------------------------------------------------

interface HomeRow {
  s: SessionSummary;
  when: string;
  why: string;
}

/// "3m ago" / "2h ago" / "4d ago" — compact relative time.
function ago(iso: string): string {
  const ms = Date.now() - new Date(iso).getTime();
  if (ms < 0) return "now";
  const m = Math.floor(ms / 60000);
  if (m < 1) return "now";
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  return `${Math.floor(h / 24)}d ago`;
}

function why(s: SessionSummary): string {
  if (s.status === "orphaned") return "orphaned — agent process lost";
  if (s.status === "error") return "errored";
  const tag = s.branch.tags.find(
    (t) => t.key === "attention" || t.key === "triage",
  );
  if (tag?.note) return tag.note;
  return s.branch.description || s.branch.title || "needs you";
}

const live = computed(() => props.fleet.filter((s) => s.status !== "archived"));
const byRecency = (a: SessionSummary, b: SessionSummary) =>
  a.last_activity_at < b.last_activity_at ? 1 : -1;

// NEEDS YOU: blocked first, then attention; each by recency.
const needs = computed<HomeRow[]>(() =>
  live.value
    .filter((s) => level(s) !== "ok")
    .sort((a, b) => {
      const la = level(a) === "blocked" ? 0 : 1;
      const lb = level(b) === "blocked" ? 0 : 1;
      return la - lb || byRecency(a, b);
    })
    .map((s) => ({ s, when: ago(s.last_activity_at), why: why(s) })),
);

// WORKING: running and not needing attention (agents mid-turn, or a calm
// session with work in flight). `idle` agents are resting, not working.
const working = computed<HomeRow[]>(() =>
  live.value
    .filter((s) => s.status === "running" && level(s) === "ok" && !isIdle(s))
    .sort(byRecency)
    .map((s) => ({ s, when: ago(s.last_activity_at), why: subtitle(s) })),
);

// RESTING: idle marks and finished/error-free sessions that don't need you.
const resting = computed<HomeRow[]>(() =>
  live.value
    .filter(
      (s) =>
        level(s) === "ok" && (s.status !== "running" || isIdle(s)),
    )
    .sort(byRecency)
    .map((s) => ({
      s,
      when: ago(s.last_activity_at),
      why: isIdle(s) ? "idle" : subtitle(s),
    })),
);

function subtitle(s: SessionSummary): string {
  return s.branch.description || s.branch.title || "—";
}

function classFor(row: HomeRow): string {
  if (props.selectedId === row.s.id) return "selected";
  return "";
}
</script>

<template>
  <section class="home">
    <div class="home-scroll">
      <h1>🕸 Arachne</h1>
      <div v-if="needs.length" class="section">
        <div class="section-title">Needs you</div>
        <div
          v-for="row in needs"
          :key="row.s.id"
          class="home-row"
          :class="[classFor(row), level(row.s)]"
          @click="emit('select', row.s.id)"
        >
          <span class="level-dot" :class="level(row.s)"></span>
          <div class="row-main">
            <div class="row-name">{{ row.s.branch.name }}</div>
            <div class="row-why">{{ row.why }}</div>
          </div>
          <span class="row-when">{{ row.when }}</span>
        </div>
      </div>
      <div v-else class="all-calm">
        <div class="calm-big">🕸</div>
        <div>Nothing needs you.</div>
      </div>

      <div v-if="working.length" class="section">
        <div class="section-title">Working</div>
        <div
          v-for="row in working"
          :key="row.s.id"
          class="home-row"
          :class="classFor(row)"
          @click="emit('select', row.s.id)"
        >
          <span class="level-dot ok"></span>
          <div class="row-main">
            <div class="row-name">{{ row.s.branch.name }}</div>
            <div class="row-why">{{ row.why }}</div>
          </div>
          <span class="row-when">{{ row.when }}</span>
        </div>
      </div>

      <div v-if="resting.length" class="section">
        <div class="section-title">Resting</div>
        <div
          v-for="row in resting"
          :key="row.s.id"
          class="home-row dim"
          :class="classFor(row)"
          @click="emit('select', row.s.id)"
        >
          <span class="level-dot dim"></span>
          <div class="row-main">
            <div class="row-name">{{ row.s.branch.name }}</div>
            <div class="row-why">{{ row.why }}</div>
          </div>
          <span class="row-when">{{ row.when }}</span>
        </div>
      </div>
    </div>
  </section>
</template>
