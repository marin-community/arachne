<script setup lang="ts">
import { ref, computed, onMounted } from "vue";
import type { SessionSummary } from "../App.vue";
import { searchThreads } from "../navigation";
import { isStandalone } from "../threadKind";
const props = defineProps<{ fleet: SessionSummary[] }>();
const emit = defineEmits<{ (e: "close"): void; (e: "select", id: string): void; (e: "action", action: string): void }>();
const query = ref("");
const input = ref<HTMLInputElement | null>(null);
const selected = ref(0);
const actions = [
  { id: "thread", title: "New thread", detail: "A quick one-off conversation", shortcut: "⌘N" },
  { id: "track", title: "New track", detail: "A durable goal with a coordinator and workers", shortcut: "⌘⇧N" },
  { id: "home", title: "Needs You / Home", detail: "Review attention across projects", shortcut: "⌘⇧H" },
  { id: "delegate", title: "Delegate work", detail: "Create a worker under the current conversation", shortcut: "⌘⇧D" },
  { id: "inspector", title: "Toggle inspector", detail: "Threads, resources, review and todos", shortcut: "⌘⇧E" },
  { id: "settings", title: "Switch Loom server / Settings", detail: "Connections and saved servers", shortcut: "⌘," },
];
const results = computed(() => [
  ...actions.filter(a => `${a.title} ${a.detail}`.toLowerCase().includes(query.value.toLowerCase())).map(a => ({...a, kind: "action"})),
  ...searchThreads(props.fleet, query.value).slice(0, 40).map(s => ({ id: s.id, title: s.branch.title || s.branch.name,
    detail: [s.placement?.group_name, s.parent_session_id || s.parent_id ? "Worker" : isStandalone(s) ? "Thread" : "Track", s.github_repo || s.branch.repo_root,
      s.branch.github?.pr_number ? `PR #${s.branch.github.pr_number}` : "", s.status === "archived" ? "Archived" : ""].filter(Boolean).join(" · "), shortcut: "", kind: "session" })),
]);
function choose(index: number) { const row = results.value[index]; if (!row) return; if (row.kind === "session") emit("select", row.id); else emit("action", row.id); emit("close"); }
function move(by: number) { selected.value = Math.max(0, Math.min(results.value.length - 1, selected.value + by)); document.getElementById(`command-${selected.value}`)?.scrollIntoView({block: "nearest"}); }
onMounted(() => input.value?.focus());
</script>
<template>
  <div class="command-overlay" @click.self="emit('close')" @keydown.esc.stop.prevent="emit('close')">
    <section class="command-palette" role="dialog" aria-modal="true" aria-label="Search and commands">
      <div class="command-input"><input ref="input" v-model="query" placeholder="Search tracks, threads, repositories, PRs…" aria-label="Search tracks, threads, repositories, PRs" @input="selected = 0" @keydown.down.prevent="move(1)" @keydown.up.prevent="move(-1)" @keydown.enter.prevent="choose(selected)" /><button @click="emit('close')" aria-label="Close search">Esc</button></div>
      <div class="command-results" role="listbox" aria-label="Search results">
        <button v-for="(row, i) in results" :id="`command-${i}`" :key="`${row.kind}-${row.id}`" class="command-row" :class="{ active: selected === i }" role="option" :aria-selected="selected === i" @click="choose(i)">
          <span><strong>{{ row.title }}</strong><small>{{ row.detail }}</small></span><kbd>{{ row.shortcut }}</kbd>
        </button>
        <p v-if="!results.length">No matching threads. Try a title, branch, repository or PR number.</p>
      </div>
      <footer>↑↓ to choose · Enter to open · Searches thread metadata, including archived work</footer>
    </section>
  </div>
</template>
<style scoped>
.command-overlay { position: fixed; inset: 0; z-index: 150; background: #0008; display: flex; align-items: flex-start; justify-content: center; padding: 12vh 20px 20px; }
.command-palette { width: min(700px, 100%); background: var(--bg-raised); border: 1px solid var(--border); border-radius: 12px; box-shadow: 0 20px 70px #0008; overflow: hidden; }
.command-input { display: flex; gap: 10px; padding: 14px; border-bottom: 1px solid var(--border); }
.command-input input { flex: 1; min-width: 0; padding: 10px; }
.command-results { max-height: 52vh; overflow: auto; padding: 6px; }
.command-row { width: 100%; display: flex; align-items: center; justify-content: space-between; text-align: left; border: 0; background: transparent; padding: 11px; gap: 10px; }
.command-row.active { background: var(--bg-hover); outline: 1px solid var(--accent); outline-offset: -1px; }
.command-row span { min-width: 0; } .command-row strong, .command-row small { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.command-row small, footer { color: var(--text-dim); font-size: 11px; } footer { padding: 10px 14px; border-top: 1px solid var(--border); }
</style>
