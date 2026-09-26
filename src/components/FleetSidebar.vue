<script setup lang="ts">
import { ref, computed } from "vue";
import type { SessionSummary } from "../App.vue";

const props = defineProps<{
  fleet: SessionSummary[];
  selectedId: string | null;
  launching?: boolean;
}>();

const emit = defineEmits<{
  (e: "select", id: string): void;
  (e: "launch", task: string, repo: string): void;
}>();

const task = ref("");
const repo = ref("marin-community/arachne");

function submit() {
  const t = task.value.trim();
  if (!t) return;
  emit("launch", t, repo.value.trim());
  task.value = "";
}

// Loom tag semantics (weaver-core/src/tags.rs): the loud keys `attention`
// (agent self-report) and `triage` (outside assessment) carry values
// `attention` | `blocked`; absence is the calm/default state. `idle` is a
// quiet resting mark. Prose status lives on branch.description.
type Attention = { level: "attention" | "blocked"; source: string } | null;

function loudTag(s: SessionSummary): Attention {
  for (const key of ["attention", "triage"]) {
    const tag = s.branch.tags.find((t) => t.key === key);
    if (tag && (tag.value === "attention" || tag.value === "blocked")) {
      return { level: tag.value, source: tag.set_by };
    }
  }
  return null;
}

const sorted = computed(() =>
  [...props.fleet].sort((a, b) =>
    a.last_activity_at < b.last_activity_at ? 1 : -1
  )
);

function subtitle(s: SessionSummary): string {
  return s.branch.description || s.branch.title || "—";
}

function statusClass(s: SessionSummary): string {
  if (s.status === "orphaned") return "orphaned";
  if (s.status === "running") {
    const loud = loudTag(s);
    if (loud?.level === "blocked") return "error";
    if (loud?.level === "attention") return "attention";
    return "running";
  }
  return "done";
}

function statusLabel(s: SessionSummary): string {
  if (s.status === "orphaned") return "orphan";
  const loud = loudTag(s);
  if (loud) return loud.level;
  if (s.status === "running") return "run";
  return s.status;
}
</script>

<template>
  <aside class="sidebar">
    <div class="new-task">
      <input v-model="task" placeholder="New task…" @keydown.enter.prevent="submit" />
      <button class="primary" :disabled="!task.trim() || props.launching" @click="submit">
        {{ props.launching ? "…" : "Launch" }}
      </button>
    </div>
    <div class="new-task" style="margin-top: -4px">
      <input v-model="repo" placeholder="owner/name" spellcheck="false" style="font-family: var(--mono); font-size: 11px" />
    </div>
    <div class="session-list">
      <div
        v-for="s in sorted"
        :key="s.id"
        class="session-item"
        :class="{ selected: s.id === selectedId }"
        @click="emit('select', s.id)"
      >
        <div class="row1">
          <span class="name">{{ s.branch.name || s.id }}</span>
          <span class="badge" :class="statusClass(s)">{{ statusLabel(s) }}</span>
          <span v-if="s.branch.tags.some((t) => t.key === 'idle')" class="badge idle">idle</span>
        </div>
        <div class="title">{{ subtitle(s) }}</div>
      </div>
      <div v-if="fleet.length === 0" class="session-item" style="color: var(--text-dim)">
        No sessions yet — launch one above.
      </div>
    </div>
  </aside>
</template>
