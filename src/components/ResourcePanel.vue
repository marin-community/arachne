<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-shell";
import MarkdownIt from "markdown-it";
import DOMPurify from "dompurify";
import type { SessionSummary } from "../App.vue";
import { liveRows, prLight, prNumberFromUrl, type AttachedPrStatus, type PanelIssue, type PanelPr } from "../resourcePanel";

interface TopicResource {
  id: string;
  kind: string;
  title: string;
  repository: string;
  reference: string | null;
  path: string | null;
  url: string | null;
}
interface TopicResourcesView {
  resources: TopicResource[];
  revision: number;
}
// `topic_resources` reply: the durable manifest plus the topic's live slice
// (issues its subtree works, PRs of every thread in the subtree).
interface TopicResourcesPanel extends TopicResourcesView {
  issues: PanelIssue[];
  prs: PanelPr[];
}
interface TopicResourceContent {
  resource: TopicResource;
  content: string;
}

const props = defineProps<{ topic: SessionSummary; embedded?: boolean }>();
const emit = defineEmits<{ (e: "close"): void; (e: "error", message: string): void }>();
const snapshot = ref<TopicResourcesView>({ resources: [], revision: 0 });
const selectedId = ref<string | null>(null);

// --- Live topic resources --------------------------------------------
// The spec's resource strip invariant applies to the inspector too: the
// topic's repo, PRs, issues, and checkout are its most relevant resources,
// and they should be reachable immediately — not only attached files. The
// PRs cover the whole subtree (a worker's PR is the topic's result too);
// the issues are those the subtree claims or sourced.
const panelIssues = ref<PanelIssue[]>([]);
const panelPrs = ref<PanelPr[]>([]);
const rows = computed(() =>
  liveRows(props.topic, props.topic.id, panelPrs.value, panelIssues.value),
);

async function openExternal(url: string) {
  try {
    await open(url);
  } catch (error: any) {
    emit("error", error?.message ?? String(error));
  }
}

/** A row glyph per kind — files and URL-backed GitHub rows differ at a glance. */
function iconFor(kind: string) {
  switch (kind) {
    case "design_document": return "◇";
    case "pull_request": return "⑂";
    case "issue": return "◉";
    case "artifact": return "◍";
    default: return "▤";
  }
}

/** The status light's one-line explanation, same order as the light rules. */
function prLightLabel(light: "green" | "yellow" | "red" | "purple" | null) {
  switch (light) {
    case "green": return "Mergeable and CI passing";
    case "yellow": return "CI in progress (or unknown yet)";
    case "red": return "Failing or not mergeable";
    case "purple": return "Merged";
    case null: return "Closed without merging";
  }
}

/** Full hover text for a PR row: title, the light's one-line meaning, and
 * the reasons behind it. */
function prTooltip(pr: PanelPr) {
  const parts = [pr.pr_title, prLightLabel(prLight(pr))];
  if (pr.pr_state !== "OPEN") parts.push(`state ${pr.pr_state}`);
  if (pr.mergeable && pr.mergeable !== "MERGEABLE") parts.push(`merge ${pr.mergeable.toLowerCase()}`);
  if (pr.checks) parts.push(`CI ${pr.checks}`);
  return parts.join(" · ");
}

// The topic's own checkout: open in Zed when present, otherwise recover
// it via repos.worktrees.ensure (never resurrecting the agent) and open.
const topicWorktree = ref(props.topic.worktree_present ? props.topic.work_dir : "");
const recoveringWorktree = ref(false);

async function openTopicCheckout() {
  try {
    if (topicWorktree.value) {
      await invoke("open_in_zed", { id: props.topic.id });
      return;
    }
    recoveringWorktree.value = true;
    const wt = await invoke<{ path: string; created: boolean }>("recover_worktree", {
      repoRoot: props.topic.branch.repo_root,
      branch: props.topic.branch.branch,
    });
    topicWorktree.value = wt.path;
    await invoke("open_in_zed", { id: props.topic.id, workDir: wt.path });
  } catch (error: any) {
    emit("error", error?.message ?? String(error));
  } finally {
    recoveringWorktree.value = false;
  }
}

watch(() => props.topic.id, () => {
  topicWorktree.value = props.topic.worktree_present ? props.topic.work_dir : "";
});
const content = ref("");
const markdown = new MarkdownIt({ html: false, linkify: true, breaks: false });
const renderedContent = computed(() => DOMPurify.sanitize(markdown.render(content.value)));
const loading = ref(false);
const reading = ref(false);
const saving = ref(false);
const showAttach = ref(false);
const formKind = ref<"design_document" | "file" | "pull_request" | "issue">("design_document");
const formTitle = ref("Design document");
const formPath = ref("docs/design.md");
const formUrl = ref("");
const message = ref("");

// --- GitHub attach picker ----------------------------------------------
// Typing a PR/issue number or words searches GitHub via `gh` (gh_search):
// the repo defaults to the topic's own and is a dropdown so switching is one
// click. Results are click-to-fill; the visible title stays editable.
interface GhSearchRow {
  number: number;
  title: string;
  state: string;
  url: string;
}
const ghQuery = ref("");
const ghResults = ref<GhSearchRow[]>([]);
const ghSearching = ref(false);
const ghError = ref("");
let ghSearchSeq = 0;

/** The repos the picker offers: the topic's repo first, then any other repo
 * its subtree's issues touch. */
const ghRepos = computed(() => {
  const repos: string[] = [];
  if (props.topic.github_repo) repos.push(props.topic.github_repo);
  for (const issue of panelIssues.value) {
    if (issue.github_repo && !repos.includes(issue.github_repo)) repos.push(issue.github_repo);
  }
  // Fallback when nothing carries a slug: derive one from the repo root.
  if (!repos.length && props.topic.branch.repo_root) {
    const parts = props.topic.branch.repo_root.split("/").filter(Boolean);
    if (parts.length >= 2) repos.push(parts.slice(-2).join("/"));
  }
  return repos;
});
const ghRepo = ref("");
watch(ghRepos, (repos) => {
  if (!ghRepo.value || !repos.includes(ghRepo.value)) ghRepo.value = repos[0] ?? "";
}, { immediate: true });

async function runGhSearch() {
  const repo = ghRepo.value;
  const query = ghQuery.value.trim();
  if (!repo || !query) {
    ghResults.value = [];
    return;
  }
  const seq = ++ghSearchSeq;
  ghSearching.value = true;
  ghError.value = "";
  try {
    const rows = await invoke<GhSearchRow[]>("gh_search", { repo, kind: formKind.value, query });
    if (seq === ghSearchSeq) ghResults.value = rows;
  } catch (error: any) {
    if (seq === ghSearchSeq) {
      ghResults.value = [];
      ghError.value = error?.message ?? String(error);
    }
  } finally {
    if (seq === ghSearchSeq) ghSearching.value = false;
  }
}

/** Fill the form from a picked search result. */
function pickGhResult(row: GhSearchRow) {
  formUrl.value = row.url;
  formTitle.value = row.title;
  formKind.value = row.url.includes("/pull/") ? "pull_request" : "issue";
  ghResults.value = [];
  ghQuery.value = "";
}

const selected = computed(() => snapshot.value.resources.find((resource) => resource.id === selectedId.value) ?? null);
const sortedResources = computed(() => [...snapshot.value.resources].sort((a, b) =>
  Number(b.kind === "design_document") - Number(a.kind === "design_document") || a.title.localeCompare(b.title),
));

// Attached PR rows carry only a URL in the manifest, so their light is
// fetched directly from GitHub (`pr_status` shells out to the local `gh`).
// Failure leaves the row unlit, exactly like an unknown live state — it
// must never block the panel.
const attachedStatuses = ref<Record<string, AttachedPrStatus>>({});
async function refreshAttachedStatuses(resources: TopicResource[]) {
  const prUrls = resources
    .filter((resource) => resource.kind === "pull_request" && resource.url)
    .map((resource) => resource.url!);
  const replies = await Promise.all(
    prUrls.map(async (url) => {
      try {
        return [url, await invoke<AttachedPrStatus>("pr_status", { url })] as const;
      } catch {
        return null;
      }
    }),
  );
  const next: Record<string, AttachedPrStatus> = {};
  for (const reply of replies) if (reply) next[reply[0]] = reply[1];
  attachedStatuses.value = next;
}

/** An attached PR resource as a `PanelPr`, for `prLight`/`prTooltip`. */
function attachedPr(resource: TopicResource): PanelPr | null {
  const status = resource.url ? attachedStatuses.value[resource.url] : undefined;
  return status && resource.url
    ? {
        session_id: "attached",
        session_name: "attached",
        pr_number: prNumberFromUrl(resource.url) ?? 0,
        pr_url: resource.url,
        pr_state: status.state,
        pr_title: status.title ?? resource.title,
        is_draft: false,
        review_decision: null,
        checks: status.checks,
        mergeable: status.mergeable,
      }
    : null;
}

async function refresh() {
  const topicId = props.topic.id;
  loading.value = true;
  message.value = "";
  try {
    const next = await invoke<TopicResourcesPanel>("topic_resources", { topicId });
    if (!next || !Array.isArray(next.resources)) throw new Error("Loom returned an invalid resource list");
    if (topicId !== props.topic.id) return;
    snapshot.value = next;
    panelIssues.value = next.issues ?? [];
    panelPrs.value = next.prs ?? [];
    void refreshAttachedStatuses(next.resources);
    if (!next.resources.some((resource) => resource.id === selectedId.value)) {
      selectedId.value = next.resources.find((resource) => resource.kind === "design_document")?.id
        ?? next.resources[0]?.id ?? null;
    } else if (selectedId.value) {
      void selectResource(selectedId.value);
    }
  } catch (error: any) {
    message.value = error?.message ?? String(error);
  } finally {
    loading.value = false;
  }
}

async function selectResource(id: string) {
  selectedId.value = id;
  content.value = "";
  reading.value = true;
  try {
    const view = await invoke<TopicResourceContent>("read_topic_resource", { topicId: props.topic.id, resourceId: id });
    if (selectedId.value === id) content.value = view.content;
  } catch (error: any) {
    if (selectedId.value === id) content.value = `Preview unavailable: ${error?.message ?? String(error)}`;
  } finally {
    reading.value = false;
  }
}

watch(() => props.topic.id, () => {
  snapshot.value = { resources: [], revision: 0 };
  panelIssues.value = [];
  panelPrs.value = [];
  selectedId.value = null;
  content.value = "";
  ghResults.value = [];
  ghQuery.value = "";
  ghError.value = "";
  void refresh();
}, { immediate: true });
watch(selectedId, (id) => {
  if (id) void selectResource(id);
  else content.value = "";
});

function chooseKind() {
  if (formKind.value === "design_document") {
    formTitle.value = "Design document";
    formPath.value = "docs/design.md";
    formUrl.value = "";
  } else if (formKind.value === "file") {
    formTitle.value = "";
    formPath.value = "";
    formUrl.value = "";
  } else {
    // PR / issue bindings are URL-backed.
    formTitle.value = "";
    formPath.value = "";
    formUrl.value = "";
  }
}

const isUrlKind = computed(() => formKind.value === "pull_request" || formKind.value === "issue");
const attachValid = computed(() =>
  formTitle.value.trim() && (isUrlKind.value ? formUrl.value.trim() : formPath.value.trim()),
);

async function attach() {
  const title = formTitle.value.trim();
  const url = formUrl.value.trim();
  const path = formPath.value.trim();
  if (!title || saving.value || !attachValid.value) return;
  saving.value = true;
  message.value = "";
  try {
    const next = await invoke<TopicResourcesView>("attach_topic_resource", {
      topicId: props.topic.id,
      resource: isUrlKind.value
        ? {
            kind: formKind.value,
            title,
            repository: props.topic.branch.repo_root,
            reference: null,
            path: null,
            url,
          }
        : {
            kind: formKind.value,
            title,
            repository: props.topic.branch.repo_root,
            reference: props.topic.branch.branch,
            path,
            url: null,
          },
      expectedRevision: snapshot.value.revision,
    });
    snapshot.value = next;
    const attached = next.resources.find(
      (resource) => resource.kind === formKind.value && (isUrlKind.value ? resource.url === url : resource.path === path),
    );
    selectedId.value = attached?.id ?? selectedId.value;
    showAttach.value = false;
    message.value = "Resource attached to this topic.";
  } catch (error: any) {
    await refresh();
    message.value = error?.message ?? String(error);
  } finally {
    saving.value = false;
  }
}

async function detach(resource: TopicResource) {
  if (saving.value) return;
  saving.value = true;
  message.value = "";
  try {
    snapshot.value = await invoke<TopicResourcesView>("detach_topic_resource", {
      topicId: props.topic.id,
      resourceId: resource.id,
      expectedRevision: snapshot.value.revision,
    });
    if (selectedId.value === resource.id) selectedId.value = null;
    message.value = "Attachment removed; the file was not changed.";
  } catch (error: any) {
    await refresh();
    message.value = error?.message ?? String(error);
  } finally {
    saving.value = false;
  }
}

async function openInZed(resource: TopicResource) {
  try {
    await invoke("open_topic_resource_in_zed", { topicId: props.topic.id, resourceId: resource.id });
  } catch (error: any) {
    emit("error", error?.message ?? String(error));
  }
}

async function onPreviewClick(event: MouseEvent) {
  const link = (event.target as HTMLElement).closest("a[href]");
  if (!link) return;
  event.preventDefault();
  const href = link.getAttribute("href") ?? "";
  if (!/^https?:\/\//i.test(href)) {
    message.value = "Open related repository files in Zed.";
    return;
  }
  try {
    await open(href);
  } catch (error: any) {
    emit("error", error?.message ?? String(error));
  }
}
</script>

<template>
  <aside class="resource-panel" :class="{ embedded }" :aria-label="embedded ? 'Topic resources tab' : 'Topic resources'">
    <header v-if="!embedded" class="resource-panel-head">
      <div>
        <strong>Resources</strong>
        <div class="resource-panel-topic">{{ topic.branch.title || topic.branch.name }}</div>
      </div>
      <button v-if="!embedded" title="Refresh resources" :disabled="loading" @click="refresh">↻</button>
      <button v-if="!embedded" title="Close resources" aria-label="Close resources" @click="emit('close')">×</button>
    </header>
    <div class="resource-panel-list">
      <!-- Live topic resources (design.md "Resource slice"): the repo, the
           PRs of every thread in the topic's subtree, the issues its subtree
           works, and the checkout — always first, always actionable. -->
      <section class="resource-panel-live" aria-label="Topic repository, PRs, issues, and checkout">
        <template v-for="row in rows" :key="row.key">
          <button v-if="row.kind === 'repository' && row.url" class="resource-panel-item" title="Open the repository on GitHub" @click="openExternal(row.url)">
            <span class="resource-panel-icon">⌂</span>
            <span class="resource-panel-item-text">
              <strong>{{ row.label }}</strong>
              <small>repository</small>
            </span>
          </button>
          <div v-else-if="row.kind === 'repository'" class="resource-panel-item static">
            <span class="resource-panel-icon">⌂</span>
            <span class="resource-panel-item-text">
              <strong>{{ row.label }}</strong>
              <small>repository</small>
            </span>
          </div>
          <button v-else-if="row.kind === 'pr'" class="resource-panel-item" :title="prTooltip(row.pr)" @click="openExternal(row.pr.pr_url)">
            <span class="resource-panel-icon" :class="row.light ? `pr-light-${row.light}` : 'pr-light-none'">⑂</span>
            <span class="resource-panel-item-text">
              <strong>PR #{{ row.pr.pr_number }}</strong>
              <small>
                <template v-if="!row.ownedByTopic">{{ row.pr.session_name }} · </template>{{ row.pr.is_draft ? "draft" : row.pr.pr_state }}<template v-if="row.pr.review_decision"> · {{ row.pr.review_decision }}</template><template v-if="row.pr.checks"> · CI {{ row.pr.checks }}</template>
              </small>
            </span>
          </button>
          <button v-else-if="row.kind === 'issue'" class="resource-panel-item" :title="row.issue.title" @click="row.url && openExternal(row.url)">
            <span class="resource-panel-icon">◉</span>
            <span class="resource-panel-item-text">
              <strong>#{{ row.issue.github_issue ?? row.issue.id }} {{ row.issue.title }}</strong>
              <small>
                {{ row.open ? "issue" : "closed" }}<template v-if="row.issue.claimed_branch"> · {{ row.issue.claimed_branch }}</template>
              </small>
            </span>
          </button>
        </template>
        <!-- The checkout is a resource; recovery is its action. Present →
             open in Zed. Gone (archive keeps the branch, not the directory)
             → the same row recovers it via repos.worktrees.ensure. -->
        <button class="resource-panel-item" :disabled="recoveringWorktree" :title="topicWorktree || 'The worktree is gone; materialize a checkout for this branch'" @click="openTopicCheckout">
          <span class="resource-panel-icon">▣</span>
          <span class="resource-panel-item-text">
            <strong>Checkout</strong>
            <small>{{ recoveringWorktree ? "recovering…" : topicWorktree || `recover from ${props.topic.branch.branch}` }}</small>
          </span>
        </button>
      </section>
      <div class="resource-panel-subhead">Attached</div>
      <div v-if="loading && !snapshot.resources.length" class="resource-panel-empty">Loading…</div>
      <div v-else-if="!snapshot.resources.length" class="resource-panel-empty">
        No resources attached yet. Attach a design document or file so it stays with this topic.
      </div>
      <button v-for="resource in sortedResources" :key="resource.id" class="resource-panel-item"
        :class="{ selected: resource.id === selectedId }"
        :title="resource.kind === 'pull_request' && attachedPr(resource) ? prTooltip(attachedPr(resource)!) : (resource.url ?? resource.path ?? '')"
        @click="resource.url ? openExternal(resource.url) : (selectedId = resource.id)">
        <span class="resource-panel-icon"
          :class="resource.kind === 'pull_request' && attachedPr(resource) ? (prLight(attachedPr(resource)!) ? `pr-light-${prLight(attachedPr(resource)!)}` : 'pr-light-none') : ''">{{ iconFor(resource.kind) }}</span>
        <span class="resource-panel-item-text">
          <strong>{{ resource.title }}</strong>
          <small>{{ resource.path || resource.url || resource.reference }}</small>
        </span>
      </button>
    </div>
    <button class="resource-panel-add" @click="showAttach = !showAttach">{{ showAttach ? 'Cancel attachment' : '+ Attach resource' }}</button>
    <div v-if="showAttach" class="resource-panel-form">
      <label>Kind
        <select v-model="formKind" @change="chooseKind">
          <option value="design_document">Design document</option>
          <option value="file">File</option>
          <option value="pull_request">Pull request</option>
          <option value="issue">Issue</option>
        </select>
      </label>
      <label>Title <input v-model="formTitle" placeholder="Design document" /></label>
      <label v-if="isUrlKind">Repo
        <select v-model="ghRepo">
          <option v-for="repo in ghRepos" :key="repo" :value="repo">{{ repo }}</option>
        </select>
      </label>
      <label v-if="isUrlKind">Find on GitHub
        <input v-model="ghQuery" placeholder="#27 or words from the title" spellcheck="false"
          @input="runGhSearch" @keydown.enter.prevent="runGhSearch" />
      </label>
      <div v-if="isUrlKind && ghSearching" class="resource-panel-empty">Searching…</div>
      <div v-else-if="isUrlKind && ghError" class="resource-panel-message">{{ ghError }}</div>
      <div v-else-if="isUrlKind && ghResults.length" class="gh-results">
        <button v-for="row in ghResults" :key="row.url" type="button" class="gh-result"
          :title="row.title" @click="pickGhResult(row)">
          <strong>#{{ row.number }}</strong>
          <span class="gh-result-title">{{ row.title }}</span>
          <small>{{ row.state.toLowerCase() }}</small>
        </button>
      </div>
      <label v-if="isUrlKind">Title <input v-model="formTitle" placeholder="Pull request" /></label>
      <label v-if="isUrlKind">GitHub URL <input v-model="formUrl" placeholder="https://github.com/OWNER/REPO/pull/13" spellcheck="false" /></label>
      <label v-else>Path in topic branch <input v-model="formPath" placeholder="docs/design.md" spellcheck="false" /></label>
      <button class="primary" :disabled="saving || !attachValid" @click="attach">Attach</button>
    </div>
    <div v-if="message" class="resource-panel-message">{{ message }}</div>
    <div v-if="selected" class="resource-panel-preview">
      <div class="resource-panel-preview-head">
        <strong>{{ selected.title }}</strong>
        <button v-if="selected.url" title="Open in your browser" @click="openExternal(selected.url)">Open on GitHub</button>
        <button v-else :disabled="!selected.path" @click="openInZed(selected)">Open in Zed</button>
        <button class="danger" :disabled="saving" title="Remove attachment; keep the file" @click="detach(selected)">Remove</button>
      </div>
      <div v-if="selected.url" class="resource-panel-location" :title="selected.url">
        {{ selected.url }}
      </div>
      <div v-else class="resource-panel-location" :title="`${selected.repository} · ${selected.reference} · ${selected.path}`">
        {{ selected.reference }} · {{ selected.path }}
      </div>
      <div v-if="reading" class="resource-panel-empty">Loading preview…</div>
      <div v-else-if="selected.url" class="resource-panel-empty">This resource lives on GitHub — open it there.</div>
      <div v-else class="resource-panel-content" @click="onPreviewClick" v-html="renderedContent"></div>
    </div>
  </aside>
</template>

<style scoped>
.resource-panel { grid-area: resources; display: flex; flex-direction: column; min-width: 0; overflow: hidden; border-left: 1px solid var(--border); background: var(--bg-raised); }
/* Embedded inside the inspector's Resources tab: the inspector owns the
   column, the header, and the border-left; only the panel body renders. */
.resource-panel.embedded { grid-area: auto; flex: 1; border-left: 0; }
.resource-panel-head { display: flex; align-items: center; gap: 6px; padding: 10px 12px; border-bottom: 1px solid var(--border); }
.resource-panel-head > div { flex: 1; min-width: 0; }
.resource-panel-head strong { font-size: 13px; }
.resource-panel-topic { color: var(--text-dim); font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.resource-panel-list { overflow-y: auto; max-height: 35%; padding: 7px; }
.resource-panel.embedded .resource-panel-list { max-height: none; flex: 1; }
.resource-panel-live { display: flex; flex-direction: column; gap: 2px; }
.resource-panel-subhead { margin: 6px 2px 4px; font-size: 10px; font-weight: 600; letter-spacing: .04em; text-transform: uppercase; color: var(--text-dim); }
.resource-panel-item.static { cursor: default; }
.resource-panel-item { width: 100%; display: flex; align-items: center; gap: 8px; text-align: left; border: 0; background: transparent; padding: 8px; }
.resource-panel-item:hover, .resource-panel-item.selected { background: var(--bg-hover); }
.resource-panel-icon { font-size: 17px; color: var(--accent); }
/* PR status light: green = mergeable + CI passing, yellow = CI in progress
   (or not yet known), red = failing or not mergeable. The dot rides the
   row's right edge; the glyph shares the light so the state is readable
   either way. */
.resource-panel-icon.pr-light-green { color: var(--ok); }
.resource-panel-icon.pr-light-yellow { color: var(--attention); }
.resource-panel-icon.pr-light-red { color: var(--blocked); }
.resource-panel-icon.pr-light-purple { color: var(--merged); }
.resource-panel-icon.pr-light-none { color: var(--text-dim); }
.resource-panel-item-text { display: flex; flex-direction: column; min-width: 0; }
.resource-panel-item-text strong, .resource-panel-item-text small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.resource-panel-item-text small, .resource-panel-empty, .resource-panel-location { color: var(--text-dim); font-size: 11px; }
.resource-panel-empty { padding: 12px; }
.resource-panel-add { margin: 6px 10px; }
.resource-panel-form { display: grid; gap: 8px; padding: 10px; border-bottom: 1px solid var(--border); }
.resource-panel-form label { display: grid; gap: 3px; color: var(--text-dim); font-size: 11px; }
.resource-panel-form input, .resource-panel-form select { width: 100%; min-width: 0; background: var(--bg); color: var(--text); border: 1px solid var(--border); border-radius: 5px; padding: 6px; }
.gh-results { display: grid; gap: 2px; max-height: 180px; overflow-y: auto; }
.gh-result { display: flex; align-items: baseline; gap: 6px; width: 100%; text-align: left; border: 0; background: transparent; padding: 5px 6px; border-radius: 5px; color: var(--text); }
.gh-result:hover { background: var(--bg-hover); }
.gh-result strong { flex: none; }
.gh-result-title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.gh-result small { color: var(--text-dim); flex: none; }
.resource-panel-message { padding: 6px 12px; color: var(--accent); font-size: 11px; }
.resource-panel-preview { display: flex; flex-direction: column; min-height: 0; flex: 1; border-top: 1px solid var(--border); }
.resource-panel-preview-head { display: flex; align-items: center; gap: 5px; padding: 10px; }
.resource-panel-preview-head strong { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.resource-panel-preview-head button { white-space: nowrap; }
.resource-panel-location { padding: 0 10px 7px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.resource-panel-content { margin: 0; padding: 12px 16px; overflow: auto; flex: 1; user-select: text; overflow-wrap: anywhere; font: 12px/1.6 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; background: var(--bg); }
.resource-panel-content :deep(h1), .resource-panel-content :deep(h2), .resource-panel-content :deep(h3) { line-height: 1.25; margin: 1.2em 0 .5em; }
.resource-panel-content :deep(h1) { font-size: 20px; margin-top: .2em; }
.resource-panel-content :deep(h2) { font-size: 16px; border-bottom: 1px solid var(--border); padding-bottom: .3em; }
.resource-panel-content :deep(h3) { font-size: 13px; }
.resource-panel-content :deep(p), .resource-panel-content :deep(ul), .resource-panel-content :deep(ol) { margin: .6em 0; }
.resource-panel-content :deep(ul), .resource-panel-content :deep(ol) { padding-left: 1.6em; }
.resource-panel-content :deep(code) { font: 11px/1.4 var(--mono); background: var(--bg-hover); padding: 1px 3px; border-radius: 3px; }
.resource-panel-content :deep(pre) { overflow-x: auto; padding: 10px; background: var(--bg-hover); border-radius: 5px; }
.resource-panel-content :deep(pre code) { background: transparent; padding: 0; }
.resource-panel-content :deep(a) { color: var(--accent); }
</style>
