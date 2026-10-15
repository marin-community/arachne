<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useBackendInvoke } from "../backendInvoke";

// One attach affordance for every surface that binds resources
// (docs/design.md "Resource attachment"): a single "Attach" button whose
// menu names what can be attached — a GitHub PR or issue (found by
// number or words), a repository file or design document (found by
// path autocomplete), or another repository. Every choice leads to the
// same shape: an autocomplete box whose chosen entry attaches directly.
// There is no separate manual URL/title form — search is the form.

export type AttachDraft = {
  kind: "github" | "design_document" | "file" | "repository";
  title: string;
  repository: string;
  reference: string | null;
  path: string | null;
  url: string | null;
};

const props = defineProps<{
  /** Repos the GitHub picker offers (owner/name slugs). */
  ghRepos: string[];
  /** Repos the repository picker offers (slugs or roots). */
  repoOptions: string[];
  /** Repo whose files the path completion searches (slug or root). */
  filesRepo: string | null;
  /** The sheet-level disabled state (backend unavailable, saving, …). */
  disabled?: boolean;
  /** Button label; defaults to "Attach". */
  label?: string;
  /** Hook for tests: file/path completion lookup. */
  lookupFiles?: (source: { repo?: string }, query: string) => Promise<string[]>;
}>();

const emit = defineEmits<{
  (e: "attach", draft: AttachDraft): void;
}>();

const invoke = useBackendInvoke();
const open = ref(false);
const rootEl = ref<HTMLElement | null>(null);
const searchEl = ref<HTMLInputElement | null>(null);

// The menu's modes: first choose WHAT, then find it. The GitHub mode
// finds PRs and issues in one search (they ride the same repo); file and
// design_document complete paths in the named repo; repository picks a
// repo from the managed list.
const mode = ref<"menu" | "github" | "file" | "repository">("menu");
const fileKind = ref<"file" | "design_document">("file");

const query = ref("");
const ghRepo = ref("");
const pickedRepo = ref("");
const results = ref<string[]>([]);
const ghResults = ref<GhSearchRow[]>([]);
const searching = ref(false);
const error = ref("");
const selected = ref(0);

interface GhSearchRow {
  number: number;
  title: string;
  state: string;
  url: string;
}

watch(() => props.ghRepos, (repos) => {
  if (!ghRepo.value || !repos.includes(ghRepo.value)) ghRepo.value = repos[0] ?? "";
}, { immediate: true });
watch(() => props.repoOptions, (repos) => {
  if (!pickedRepo.value || !repos.includes(pickedRepo.value)) pickedRepo.value = repos[0] ?? "";
}, { immediate: true });

const matches = computed(() =>
  mode.value === "github"
    ? ghResults.value.map((row) => ({ key: row.url, label: `#${row.number}`, title: row.title, meta: row.state.toLowerCase() }))
    : mode.value === "file"
      ? results.value.map((path) => ({ key: path, label: "", title: path, meta: "" }))
      : [],
);

function reset() {
  mode.value = "menu";
  fileKind.value = "file";
  query.value = "";
  results.value = [];
  ghResults.value = [];
  error.value = "";
  selected.value = 0;
}

function toggle() {
  if (props.disabled) return;
  open.value = !open.value;
  if (!open.value) reset();
}

function onDocClick(event: MouseEvent) {
  if (open.value && rootEl.value && !rootEl.value.contains(event.target as Node)) {
    open.value = false;
    reset();
  }
}

onMounted(() => document.addEventListener("mousedown", onDocClick));
onBeforeUnmount(() => document.removeEventListener("mousedown", onDocClick));

function chooseMode(next: "menu" | "github" | "file" | "repository", design = false) {
  mode.value = next;
  if (next === "file") fileKind.value = design ? "design_document" : "file";
  query.value = "";
  results.value = [];
  ghResults.value = [];
  error.value = "";
  selected.value = 0;
  nextTick(() => searchEl.value?.focus());
}

// --- Autocomplete ---------------------------------------------------------

let searchSeq = 0;
let searchTimer: ReturnType<typeof setTimeout> | null = null;

function dropSearch() {
  searchSeq++;
  if (searchTimer) { clearTimeout(searchTimer); searchTimer = null; }
  results.value = [];
  ghResults.value = [];
  searching.value = false;
}

async function runSearch() {
  const q = query.value.trim();
  if (!q) { dropSearch(); return; }
  if (mode.value === "repository") return; // picked, not searched
  const seq = ++searchSeq;
  if (searchTimer) clearTimeout(searchTimer);
  searchTimer = setTimeout(async () => {
    searching.value = true;
    error.value = "";
    try {
      if (mode.value === "github") {
        const repo = ghRepo.value;
        if (!repo) { dropSearch(); return; }
        // One search covers PRs and issues: GitHub treats them as
        // searchable either way, and the chosen row's URL names its kind.
        const rows = await invoke<GhSearchRow[]>("gh_search", { repo, kind: "pull_request", query: q });
        if (seq === searchSeq) ghResults.value = rows;
      } else if (mode.value === "file") {
        const repo = props.filesRepo;
        if (!repo) { dropSearch(); return; }
        const files = props.lookupFiles
          ? await props.lookupFiles({ repo }, q)
          : await invoke<string[]>("complete_files", { repo, query: q });
        if (seq === searchSeq) results.value = files;
      }
    } catch (e: any) {
      if (seq === searchSeq) {
        ghResults.value = [];
        results.value = [];
        error.value = e?.message ?? String(e);
      }
    } finally {
      if (seq === searchSeq) searching.value = false;
    }
  }, 180);
}

watch(query, () => { selected.value = 0; });

// --- Attach ----------------------------------------------------------

function attachGh(row: GhSearchRow) {
  emit("attach", {
    kind: "github",
    title: row.title,
    repository: "",
    reference: null,
    path: null,
    url: row.url,
  });
  open.value = false;
  reset();
}

function attachPath(path: string) {
  emit("attach", {
    kind: fileKind.value,
    title: path.split("/").pop() || path,
    repository: "",
    reference: null,
    path,
    url: null,
  });
  open.value = false;
  reset();
}

function attachRepo() {
  const repo = pickedRepo.value.trim();
  if (!repo) return;
  emit("attach", {
    kind: "repository",
    title: repo,
    repository: repo,
    reference: null,
    path: null,
    url: null,
  });
  open.value = false;
  reset();
}

function onSearchKeydown(event: KeyboardEvent) {
  // Escape backs out one level (kind menu, then closes) — it must not
  // bubble to a sheet's close-on-Escape while the menu is open.
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    if (mode.value === "repository" || !matches.value.length) { open.value = false; reset(); }
    else chooseMode("menu");
    return;
  }
  if (mode.value === "repository") return;
  if (!matches.value.length) return;
  if (event.key === "ArrowDown") {
    event.preventDefault();
    selected.value = (selected.value + 1) % matches.value.length;
  } else if (event.key === "ArrowUp") {
    event.preventDefault();
    selected.value = (selected.value - 1 + matches.value.length) % matches.value.length;
  } else if (event.key === "Tab" || event.key === "Enter") {
    event.preventDefault();
    const current = matches.value[selected.value];
    if (!current) return;
    if (mode.value === "github") {
      const row = ghResults.value[selected.value];
      if (row) attachGh(row);
    } else {
      attachPath(current.key);
    }
  }
}

const placeholder = computed(() =>
  mode.value === "github" ? "#27 or words from the title"
    : mode.value === "file" ? "docs/design.md"
      : "",
);
</script>

<template>
  <div ref="rootEl" class="attach-menu-root">
    <button
      type="button"
      class="attach-menu-button"
      :disabled="disabled"
      :title="disabled ? undefined : 'Attach a resource'"
      @click="toggle"
    >{{ label ?? "Attach" }} ▾</button>
    <div v-if="open" class="attach-popover" role="dialog" aria-label="Attach a resource">
      <!-- The kind menu: one list, each row naming what it attaches and
           where the search happens. -->
      <div v-if="mode === 'menu'" class="attach-kinds" role="menu">
        <button type="button" role="menuitem" @click="chooseMode('github')">
          <strong>GitHub PR or issue</strong>
          <small>search by number or words — attaches the PR/issue itself</small>
        </button>
        <button type="button" role="menuitem" @click="chooseMode('file')">
          <strong>File</strong>
          <small>a repository file — autocomplete its path</small>
        </button>
        <button type="button" role="menuitem" @click="chooseMode('file', true)">
          <strong>Design document</strong>
          <small>a durable file the track edits — autocomplete its path</small>
        </button>
        <button type="button" role="menuitem" @click="chooseMode('repository')">
          <strong>Repository</strong>
          <small>another repo this track should see</small>
        </button>
      </div>
      <div v-else class="attach-search">
        <div class="attach-search-head">
          <button type="button" class="attach-back" title="Back to kinds" aria-label="Back to kinds" @click="mode = 'menu'">‹ Back</button>
          <span class="attach-mode-name">
            {{ mode === 'github' ? 'GitHub' : mode === 'file' ? (fileKind === 'design_document' ? 'Design document' : 'File') : 'Repository' }}
          </span>
        </div>
        <div v-if="mode === 'github'" class="attach-search-bar">
          <select v-model="ghRepo" class="attach-repo" title="Search this repo">
            <option v-for="repo in ghRepos" :key="repo" :value="repo">{{ repo }}</option>
          </select>
          <input ref="searchEl" v-model="query" :placeholder="placeholder" spellcheck="false"
            @input="runSearch" @keydown="onSearchKeydown" />
        </div>
        <div v-else-if="mode === 'file'" class="attach-search-bar">
          <input ref="searchEl" v-model="query" :placeholder="placeholder" spellcheck="false"
            @input="runSearch" @keydown="onSearchKeydown" />
        </div>
        <template v-else>
          <label class="attach-repo-label">Repository
            <select v-model="pickedRepo" size="6">
              <option v-for="repo in repoOptions" :key="repo" :value="repo">{{ repo }}</option>
            </select>
          </label>
          <button type="button" class="primary attach-confirm" :disabled="!pickedRepo" @click="attachRepo">Attach</button>
        </template>
        <div v-if="mode !== 'repository'">
          <div v-if="searching" class="attach-hint">Searching…</div>
          <div v-else-if="error" class="attach-error" role="alert">{{ error }}</div>
          <div v-else-if="matches.length" class="attach-results" role="listbox">
            <button v-for="(row, index) in matches" :key="row.key" type="button" role="option"
              :aria-selected="index === selected" :class="{ selected: index === selected }"
              @mousedown.prevent="mode === 'github' ? attachGh(ghResults[index]) : attachPath(row.key)">
              <strong v-if="row.label">{{ row.label }}</strong>
              <span class="attach-result-title">{{ row.title }}</span>
              <small v-if="row.meta">{{ row.meta }}</small>
            </button>
          </div>
          <div v-else-if="query.trim()" class="attach-hint">Nothing found.</div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.attach-menu-root { position: relative; display: inline-flex; }
.attach-menu-button { white-space: nowrap; }
.attach-popover {
  position: absolute;
  bottom: calc(100% + 6px);
  left: 0;
  min-width: 320px;
  max-width: 420px;
  background: var(--bg-raised);
  border: 1px solid var(--border);
  border-radius: 6px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
  z-index: 40;
  overflow: hidden;
}
.attach-kinds { display: grid; }
.attach-kinds button {
  display: grid;
  gap: 2px;
  text-align: left;
  padding: 10px 12px;
  border: 0;
  background: transparent;
  color: var(--text);
}
.attach-kinds button:hover { background: var(--bg-hover); }
.attach-kinds small { color: var(--text-dim); font-size: 11px; }
.attach-search { display: grid; gap: 8px; padding: 8px; }
.attach-search-head { display: flex; align-items: center; gap: 6px; }
.attach-back { padding: 2px 6px; }
.attach-mode-name { font-size: 11px; color: var(--text-dim); text-transform: uppercase; letter-spacing: .04em; }
.attach-search-bar { display: flex; gap: 5px; }
.attach-search-bar input { flex: 1; min-width: 0; }
.attach-repo { width: auto; flex: none; max-width: 40%; }
.attach-repo-label { display: grid; gap: 3px; color: var(--text-dim); font-size: 11px; }
.attach-repo-label select { width: 100%; min-width: 0; background: var(--bg); color: var(--text); border: 1px solid var(--border); border-radius: 5px; padding: 6px; }
.attach-confirm { justify-self: start; }
.attach-results { display: grid; gap: 2px; max-height: 220px; overflow-y: auto; }
.attach-results button {
  display: flex;
  align-items: baseline;
  gap: 6px;
  width: 100%;
  text-align: left;
  border: 0;
  background: transparent;
  padding: 5px 6px;
  border-radius: 5px;
  color: var(--text);
}
.attach-results button:hover, .attach-results button.selected { background: var(--bg-hover); }
.attach-results button strong { flex: none; }
.attach-result-title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.attach-results button small { color: var(--text-dim); flex: none; }
.attach-hint, .attach-error { padding: 4px 6px; font-size: 11px; color: var(--text-dim); }
.attach-error { color: var(--blocked); }
</style>
