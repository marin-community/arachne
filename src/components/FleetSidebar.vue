<script setup lang="ts">
import { ref } from "vue";
import type { SessionSummary } from "../App.vue";

const props = defineProps<{
  fleet: SessionSummary[];
  selectedId: string | null;
}>();

const emit = defineEmits<{
  (e: "select", id: string): void;
  (e: "launch", task: string, repo: string): void;
}>();

const task = ref("");
const repo = ref("marin-community/arachne");

function attention(s: SessionSummary): "ok" | "attention" | "blocked" {
  const tag = s.branch.tags.find((t) => t.key === "attention");
  return (tag?.value as any) ?? "ok";
}

function submit() {
  const t = task.value.trim();
  if (!t) return;
  emit("launch", t, repo.value.trim());
  task.value = "";
}
</script>

<template>
  <aside class="sidebar">
    <div class="new-task">
      <input v-model="task" placeholder="New task…" @keydown.enter.prevent="submit" />
      <button class="primary" :disabled="!task.trim()" @click="submit">Launch</button>
    </div>
    <div class="new-task" style="margin-top: -4px">
      <input v-model="repo" placeholder="owner/name" spellcheck="false" style="font-family: var(--mono); font-size: 11px" />
    </div>
    <div class="session-list">
      <div
        v-for="s in fleet"
        :key="s.id"
        class="session-item"
        :class="{ selected: s.id === selectedId }"
        @click="emit('select', s.id)"
      >
        <div class="row1">
          <span class="name">{{ s.branch.name || s.id }}</span>
          <span v-if="s.status === 'running'" class="badge running">run</span>
          <span v-else-if="s.status === 'done'" class="badge done">done</span>
          <span v-else-if="s.status === 'error'" class="badge error">error</span>
          <span v-else-if="s.status === 'orphaned'" class="badge orphaned">orphan</span>
          <span v-if="attention(s) === 'attention'" class="badge attention">attn</span>
          <span v-else-if="attention(s) === 'blocked'" class="badge blocked">blocked</span>
        </div>
        <div class="title">{{ s.branch.title || s.branch.goal || "—" }}</div>
      </div>
      <div v-if="fleet.length === 0" class="session-item" style="color: var(--text-dim)">
        No sessions yet — launch one above.
      </div>
    </div>
  </aside>
</template>
