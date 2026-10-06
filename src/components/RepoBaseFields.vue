<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted, useId } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { recentRepositories, rememberRepository } from "../launchDefaults";

// The launch sheets' Repository + Base pair. Repository is half the row;
// Base is the other half — a combobox populated with the chosen repo's
// local branches (loom `repos.branches`, via Arachne's repo_branches
// command). Base defaults to the last branch chosen for that repo
// (per-repo, in localStorage), and typing inside it fuzzy-searches the
// branch list. Leaving it empty forks from the repo's default base
// (loom's origin/<default branch>), so Base is genuinely optional.

const props = withDefaults(defineProps<{
  repo: string;
  base: string;
  repositories?: string[];
  showBase?: boolean;
  disabled?: boolean;
}>(), { showBase: true });

const emit = defineEmits<{
  (e: "update:repo", repo: string): void;
  (e: "update:base", base: string): void;
  (e: "submit"): void;
}>();

interface RepoBranch {
  name: string;
  worktree: string | null;
  current: boolean;
}

const pickerId = useId();
const recentRepos = ref(recentRepositories(localStorage));
const managedRepos = ref<string[]>([]);
onMounted(async () => {
  try {
    const rows = await invoke<{ slug: string; path: string }[]>("list_repos");
    managedRepos.value = (rows ?? []).flatMap((row) => [row.slug, row.path]).filter(Boolean);
    if (!props.repo && rows?.length === 1) emit("update:repo", rows[0].slug || rows[0].path);
  } catch { /* Typed paths and fleet suggestions still work offline. */ }
});
const repoSuggestions = computed(() => [...new Set([...managedRepos.value, ...(props.repositories ?? []), ...recentRepos.value])].filter(Boolean));
function rememberRepo() {
  rememberRepository(localStorage, props.repo);
  recentRepos.value = recentRepositories(localStorage);
}
function editRepo(event: Event) {
  emit("update:base", "");
  emit("update:repo", (event.target as HTMLInputElement).value);
}
const branches = ref<RepoBranch[]>([]);
const branchesLoading = ref(false);
const branchesError = ref("");
// The dropdown's own focus state, separate from the input's text cursor:
// the menu stays open while the user types a fuzzy query.
const open = ref(false);
const query = ref("");
const rootEl = ref<HTMLDivElement | null>(null);
const menuEl = ref<HTMLDivElement | null>(null);
const inputEl = ref<HTMLInputElement | null>(null);
const highlighted = ref(0);

// --- Branch loading -------------------------------------------------------

let branchRequest = 0;
async function loadBranches() {
  const request = ++branchRequest;
  const repo = props.repo.trim();
  if (!repo) {
    branches.value = [];
    branchesLoading.value = false;
    return;
  }
  branchesLoading.value = true;
  branchesError.value = "";
  try {
    // Guard the wire shape: a null/undefined payload decodes as null, and
    // every downstream consumer (fuzzy filter, menu) assumes an array.
    const result = await invoke<RepoBranch[] | null>("repo_branches", { repo });
    if (request !== branchRequest || repo !== props.repo.trim()) return;
    branches.value = Array.isArray(result) ? result : [];
  } catch (error: any) {
    if (request !== branchRequest || repo !== props.repo.trim()) return;
    branches.value = [];
    branchesError.value = error?.message ?? String(error);
  } finally {
    if (request === branchRequest) branchesLoading.value = false;
  }
}

// Repos change rarely and the list is per-repo: load on open (not on every
// keystroke of the repo field), debounce repo edits while the sheet sits
// open so a fast typist doesn't fire a request per character.
let loadTimer: ReturnType<typeof setTimeout> | null = null;
watch(
  () => props.repo,
  () => {
    branchRequest++;
    branches.value = [];
    branchesError.value = "";
    branchesLoading.value = false;
    open.value = false;
    if (loadTimer) clearTimeout(loadTimer);
    if (props.showBase === false) return;
    loadTimer = setTimeout(loadBranches, 400);
  },
  { immediate: true },
);
onUnmounted(() => {
  branchRequest++;
  if (loadTimer) clearTimeout(loadTimer);
});

// --- Fuzzy search ----------------------------------------------------------

// Subsequence match over the branch name (case-insensitive): every query
// character appears in order. That's the forgiving match a long
// `weaver/some-topic-branch` list needs; the highlighter marks the hits.
function fuzzyScore(name: string, query: string): number[] | null {
  if (!query) return [];
  const target = name.toLowerCase();
  const q = query.toLowerCase();
  const hits: number[] = [];
  let i = 0;
  for (let j = 0; j < target.length && i < q.length; j++) {
    if (target[j] === q[i]) {
      hits.push(j);
      i++;
    }
  }
  return i === q.length ? hits : null;
}

const matching = computed(() => {
  const q = query.value.trim();
  return branches.value
    .map((branch) => ({ branch, hits: fuzzyScore(branch.name, q) }))
    .filter((entry): entry is { branch: RepoBranch; hits: number[] } => entry.hits !== null)
    .map((entry) => ({ ...entry, label: entry.branch.name }));
});

function highlightLabel(name: string, hits: number[]): { text: string; hit: boolean }[] {
  const marks = new Set(hits);
  const parts: { text: string; hit: boolean }[] = [];
  for (let i = 0; i < name.length; i++) {
    const hit = marks.has(i);
    const last = parts[parts.length - 1];
    if (last && last.hit === hit) last.text += name[i];
    else parts.push({ text: name[i], hit });
  }
  return parts;
}

// --- Dropdown --------------------------------------------------------------

function setBase(name: string) {
  emit("update:base", name);
  // Remember the last branch chosen for this repo: the picker reopens with
  // it next time the same repo is selected.
  const repo = props.repo.trim();
  if (repo) {
    try {
      localStorage.setItem(`arachne.base.${repo}`, name);
    } catch {
      // Private mode / storage disabled — a default is a nicety, not a gate.
    }
  }
}

function choose(entry: { branch: RepoBranch }) {
  setBase(entry.branch.name);
  open.value = false;
  query.value = "";
  inputEl.value?.blur();
}

// Restore the remembered default when the repo changes and no base is
// set (or on mount: a freshly opened sheet picks up the last branch
// chosen for its repo).
watch(
  () => props.repo,
  (repo) => {
    if (!props.base && repo) {
      try {
        const remembered = localStorage.getItem(`arachne.base.${repo.trim()}`);
        if (remembered) emit("update:base", remembered);
      } catch {
        /* storage unavailable */
      }
    }
  },
  { immediate: true },
);

// The combobox input doubles as the fuzzy search field: focusing it opens
// the menu with the full list (the query starts empty). A click counts too
// — focus fires first on a fresh field, but re-clicking an already-focused
// input fires click only, and the menu should reopen for another look.
function openMenu() {
  open.value = true;
  query.value = "";
  highlighted.value = 0;
  if (!branches.value.length && !branchesLoading.value && !branchesError.value) {
    void loadBranches();
  }
}
const onInputFocus = openMenu;
function onInputClick() {
  if (!open.value) openMenu();
}

function onInputBlur() {
  if (query.value.trim()) setBase(query.value.trim());
  // A click on a menu item fires before the blur if we let it; the menu's
  // buttons use @mousedown.prevent, so blur only fires on a real exit.
  open.value = false;
  query.value = "";
}

function onKeydown(event: KeyboardEvent) {
  if (!open.value) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      openMenu();
    } else if (event.key === "Enter") {
      // Menu closed: Enter launches, the sheet convention the Repository
      // field follows. (Clicking or focusing the field reopens the menu
      // for browsing; ArrowDown reopens it from the keyboard.)
      event.preventDefault();
      emit("submit");
    }
    return;
  }
  const list = matching.value;
  if (event.key === "Escape") {
    // The menu owns Esc first — stop it reaching the sheet's document
    // listener, which would close the whole composer.
    event.preventDefault();
    event.stopPropagation();
    open.value = false;
    query.value = "";
  } else if (event.key === "ArrowDown" && list.length) {
    event.preventDefault();
    highlighted.value = (highlighted.value + 1) % list.length;
  } else if (event.key === "ArrowUp" && list.length) {
    event.preventDefault();
    highlighted.value = (highlighted.value - 1 + list.length) % list.length;
  } else if (event.key === "Enter" && list.length) {
    event.preventDefault();
    choose(list[highlighted.value] ?? list[0]);
  } else if (event.key === "Enter") {
    event.preventDefault();
    setBase(query.value.trim());
    open.value = false;
  }
}

// Keep the highlighted row in view while arrowing through a long list.
watch(highlighted, () => {
  const menu = menuEl.value;
  if (!menu) return;
  const row = menu.querySelectorAll('[role="option"]')[highlighted.value] as HTMLElement | undefined;
  row?.scrollIntoView({ block: "nearest" });
});

// Close on outside click while open.
function onDocMousedown(e: MouseEvent) {
  if (open.value && rootEl.value && !rootEl.value.contains(e.target as Node)) {
    if (query.value.trim()) setBase(query.value.trim());
    open.value = false;
    query.value = "";
  }
}
onMounted(() => document.addEventListener("mousedown", onDocMousedown));
onUnmounted(() => document.removeEventListener("mousedown", onDocMousedown));
</script>

<template>
  <div class="nts-repo-base-row">
    <label class="nts-field nts-repo-field">
      <span class="nts-field-name">Repository</span>
      <input
        :value="props.repo"
        :list="`${pickerId}-repos`"
        :disabled="disabled"
        autocomplete="off"
        placeholder="owner/name"
        spellcheck="false"
        @input="editRepo"
        @change="rememberRepo"
        @keydown.enter.prevent="emit('submit')"
      />
      <datalist :id="`${pickerId}-repos`">
        <option v-for="suggestion in repoSuggestions" :key="suggestion" :value="suggestion" />
      </datalist>
      <span class="nts-hint">{{ showBase === false ? 'Repository holding these project resources.' : 'Choose a repository or enter a checkout path.' }}</span>
    </label>

    <div v-if="showBase !== false" ref="rootEl" class="nts-field nts-base-field">
      <span class="nts-field-name">Base <em class="nts-opt">optional</em></span>
      <div class="nts-base-wrap">
        <input
          ref="inputEl"
          :disabled="disabled"
          :aria-controls="`${pickerId}-branches`"
          :aria-activedescendant="open && matching.length ? `${pickerId}-branch-${highlighted}` : undefined"
          :value="open ? query : props.base"
          placeholder="Branch to fork from"
          spellcheck="false"
          role="combobox"
          :aria-expanded="open"
          aria-autocomplete="list"
          aria-label="Base branch"
          @focus="onInputFocus"
          @click="onInputClick"
          @blur="onInputBlur"
          @input="query = ($event.target as HTMLInputElement).value; highlighted = 0; open = true"
          @keydown="onKeydown"
        />
        <div
          v-if="open"
          :id="`${pickerId}-branches`"
          ref="menuEl"
          class="nts-base-menu mention-menu"
          role="listbox"
          aria-label="Base branches"
        >
          <div v-if="branchesLoading" class="mention-hint">Loading branches…</div>
          <div v-else-if="branchesError" class="mention-hint">{{ branchesError }}</div>
          <div v-else-if="!matching.length" class="mention-hint">{{ query.trim() ? 'Press Enter to use this branch or ref' : 'No branches found; enter a branch or ref' }}</div>
          <button type="button" @mousedown.prevent="setBase(''); open = false; query = ''; inputEl?.blur()">Use repository default</button>
          <button
            v-for="(entry, index) in matching"
            :key="entry.branch.name"
            :id="`${pickerId}-branch-${index}`"
            type="button"
            role="option"
            :aria-selected="index === highlighted"
            :class="{ selected: index === highlighted, current: entry.branch.current }"
            :title="entry.branch.worktree ?? entry.branch.name"
            @mousedown.prevent="choose(entry)"
            @mousemove="highlighted = index"
          >
            <strong><template v-for="(part, i) in highlightLabel(entry.branch.name, entry.hits)" :key="i"><mark v-if="part.hit">{{ part.text }}</mark><template v-else>{{ part.text }}</template></template></strong>
            <small v-if="entry.branch.current">current · </small><small v-if="entry.branch.worktree">has worktree</small>
          </button>
        </div>
      </div>
      <span class="nts-hint">Empty forks the repo's default branch.</span>
    </div>
  </div>
</template>
