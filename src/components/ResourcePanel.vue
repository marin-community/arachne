<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-shell";
import MarkdownIt from "markdown-it";
import DOMPurify from "dompurify";
import CheckoutEditor from "./CheckoutEditor.vue";
import type { SessionSummary } from "../App.vue";
import { liveRows, mentionableRows, originLabel, prLight, prNumberFromUrl, type AttachedPrStatus, type EffectiveRow, type PanelIssue, type PanelPr } from "../resourcePanel";

interface TopicResource {
  id: string;
  kind: string;
  title: string;
  repository: string;
  reference: string | null;
  path: string | null;
  url: string | null;
}
// `topic_resources` reply: the merged effective view (project bindings
// inherited, topic overrides winning, hides flagged — design.md "Project
// defaults and resource inheritance") plus the topic's live slice (issues
// its subtree works, PRs of every thread in the subtree).
interface TopicEffectiveView {
  resources: EffectiveRow[];
  /** Inherited bindings this track hid (kept for the restore affordance). */
  hidden_resources: EffectiveRow[];
  revision: number;
  /** The topic's home project, when it has one. */
  project?: { id: string; name: string; revision: number } | null;
}
interface TopicResourcesPanel extends TopicEffectiveView {
  issues: PanelIssue[];
  prs: PanelPr[];
}
interface TopicResourceContent {
  resource: TopicResource;
  content: string;
}

const props = defineProps<{ topic: SessionSummary; embedded?: boolean; canEdit?: boolean }>();
const emit = defineEmits<{ (e: "close"): void; (e: "error", message: string): void; (e: "checkout-saved", sessionId: string): void }>();
const snapshot = ref<TopicResourcesPanel>({
  resources: [], hidden_resources: [], revision: 0, issues: [], prs: [],
});
const selectedId = ref<string | null>(null);
const editPath = ref<string | null>(null);

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
    case "repository": return "⌂";
    default: return "▤";
  }
}

/** A repository binding's GitHub URL, when its root carries a slug-shaped
 *  tail (`owner/name`) — unmanaged roots simply have no link. */
function bindingRepoUrl(resource: EffectiveRow): string | null {
  const root = resource.repository || "";
  const slug = root.split("/").filter(Boolean).slice(-2).join("/");
  return /^[\w.-]+\/[\w.-]+$/.test(slug) ? `https://github.com/${slug}` : null;
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
const attachMenuOpen = ref(false);
function chooseAttachKind(kind: "github" | "design_document" | "file" | "repository") {
  formKind.value = kind;
  attachMenuOpen.value = false;
  if (kind === "github" || kind === "repository") {
    formTitle.value = "";
    formPath.value = "";
  } else {
    chooseKind();
  }
  showAttach.value = true;
}
const formKind = ref<"github" | "design_document" | "file" | "repository">("github");
const formTitle = ref("Design document");
const formPath = ref("docs/design.md");
const message = ref("");

// --- GitHub attach picker ----------------------------------------------
// Typing a PR/issue number or words searches GitHub via `gh` (gh_search);
// results attach directly on click — no URL/title plumbing. The repo
// defaults to the topic's own and sits as a dropdown next to the box.
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
    // Unified search: GitHub treats PRs and issues as searchable either way,
    // so one fetch covers both. The kind rides each result's URL.
    const rows = await invoke<GhSearchRow[]>("gh_search", { repo, kind: "pull_request", query });
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

/** Attach a search result straight away: the URL is the identity, the
 * title rides the row, and the kind (PR vs issue) comes from the URL. */
async function attachGhResult(row: GhSearchRow) {
  if (saving.value) return;
  saving.value = true;
  message.value = "";
  try {
    const kind = row.url.includes("/pull/") ? "pull_request" : "issue";
    const next = await invoke<TopicEffectiveView>("attach_topic_resource", {
      topicId: props.topic.id,
      resource: {
        kind,
        title: row.title,
        repository: props.topic.branch.repo_root,
        reference: null,
        path: null,
        url: row.url,
      },
      expectedRevision: snapshot.value.revision,
    });
    applyEffective(next);
    const attached = next.resources.find((resource: EffectiveRow) => resource.url === row.url);
    selectedId.value = attached?.id ?? selectedId.value;
    showAttach.value = false;
    ghResults.value = [];
    ghQuery.value = "";
    message.value = "Resource attached to this track.";
  } catch (error: any) {
    message.value = error?.message ?? String(error);
  } finally {
    saving.value = false;
  }
}

const selected = computed(() => snapshot.value.resources.find((resource) => resource.id === selectedId.value) ?? null);
const sortedResources = computed(() => [...snapshot.value.resources].sort((a, b) =>
  Number(b.kind === "design_document") - Number(a.kind === "design_document") || a.title.localeCompare(b.title),
));
const hidden_resources = computed(() => [...snapshot.value.hidden_resources].sort((a, b) => a.title.localeCompare(b.title)));

/** Apply a mutation reply (the effective view) without dropping the live
 *  slice the same snapshot ref carries — mutations return no issues/prs. */
function applyEffective(next: TopicEffectiveView) {
  snapshot.value = { ...snapshot.value, ...next };
}

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
    const ids = new Set([...next.resources, ...(next.hidden_resources ?? [])].map((resource) => resource.id));
    if (!ids.has(selectedId.value ?? "")) {
      selectedId.value = next.resources.find((resource) => resource.kind === "design_document")?.id
        ?? next.resources[0]?.id ?? null;
    } else if (selectedId.value && !next.hidden_resources?.some((resource) => resource.id === selectedId.value)) {
      void selectResource(selectedId.value);
    }
  } catch (error: any) {
    message.value = error?.message ?? String(error);
  } finally {
    loading.value = false;
  }
}

function onCheckoutSaved() {
  emit("checkout-saved", props.topic.id);
  if (selectedId.value) void selectResource(selectedId.value);
}

async function selectResource(id: string) {
  selectedId.value = id;
  content.value = "";
  // Repository bindings carry no text to preview — the row's GitHub link
  // is the affordance; selecting it just shows its metadata head.
  const row = snapshot.value.resources.find((resource) => resource.id === id);
  if (row?.kind === "repository") return;
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
  snapshot.value = { resources: [], hidden_resources: [], revision: 0, issues: [], prs: [] };
  panelIssues.value = [];
  panelPrs.value = [];
  selectedId.value = null;
  editPath.value = null;
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
  } else {
    formTitle.value = "";
    formPath.value = "";
  }
  // Switching away from GitHub clears any pending search.
  if (formKind.value !== "github") {
    ghResults.value = [];
    ghError.value = "";
  }
}

const attachValid = computed(() =>
  formKind.value === "repository"
    ? Boolean(formRepo.value.trim())
    : Boolean(formTitle.value.trim() && formPath.value.trim()),
);

// --- Repository attachments (design.md: projects bind multiple repos) ---
// The picker offers the managed repositories, plus the topic's own first.

const managedRepos = ref<string[]>([]);
const formRepo = ref("");
const repoOptions = computed(() => {
  const own = props.topic.github_repo;
  const list = [...managedRepos.value];
  if (own && !list.includes(own)) list.unshift(own);
  return list;
});
watch(repoOptions, (options) => {
  if (!formRepo.value || !options.includes(formRepo.value)) formRepo.value = options[0] ?? "";
}, { immediate: true });

async function loadManagedRepos() {
  try {
    const repos = await invoke<{ slug: string }[] | null>("managed_repos");
    managedRepos.value = Array.isArray(repos) ? repos.map((r) => r.slug) : [];
  } catch {
    managedRepos.value = [];
  }
}
loadManagedRepos();

async function attach() {
  // The GitHub picker attaches straight from a search result (attachGhResult);
  // this path serves the picked kinds: design document, file, repository.
  const title = formTitle.value.trim();
  const path = formPath.value.trim();
  const repo = formRepo.value.trim();
  if (saving.value || !attachValid.value || (formKind.value !== "repository" && !title)) return;
  saving.value = true;
  message.value = "";
  const before = new Set(snapshot.value.resources.map((resource) => resource.id));
  try {
    // The draft's repository field carries the binding's own target: the
    // topic repo for files (pinned by validation) or the picked repo for
    // repository bindings (resolved to its canonical root command-side).
    const next = await invoke<TopicEffectiveView>("attach_topic_resource", {
      topicId: props.topic.id,
      resource: formKind.value === "repository"
        ? {
            // The slug doubles as the title — the row is its repo.
            kind: formKind.value,
            title: repo,
            repository: repo,
            reference: null,
            path: null,
            url: null,
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
    applyEffective(next);
    // The attached row is the new one (upsert by id; a repository binding's
    // canonical root is resolved command-side, so diff, don't guess).
    const attached = next.resources.find((resource) => !before.has(resource.id) && resource.kind === formKind.value)
      ?? next.resources.find((resource) => resource.kind === formKind.value);
    selectedId.value = attached?.id ?? selectedId.value;
    showAttach.value = false;
    message.value = "Resource attached to this track.";
  } catch (error: any) {
    await refresh();
    message.value = error?.message ?? String(error);
  } finally {
    saving.value = false;
  }
}

async function detach(resource: EffectiveRow) {
  if (saving.value) return;
  saving.value = true;
  message.value = "";
  try {
    applyEffective(await invoke<TopicEffectiveView>("detach_topic_resource", {
      topicId: props.topic.id,
      resourceId: resource.id,
      expectedRevision: snapshot.value.revision,
    }));
    if (selectedId.value === resource.id) selectedId.value = null;
    message.value = "Attachment removed; the file was not changed.";
  } catch (error: any) {
    await refresh();
    message.value = error?.message ?? String(error);
  } finally {
    saving.value = false;
  }
}

/** Hide an inherited binding for this track (design.md: hide, not delete
 *  from the project — the project keeps supplying other topics). */
async function hide(resource: EffectiveRow) {
  if (saving.value) return;
  saving.value = true;
  message.value = "";
  try {
    applyEffective(await invoke<TopicEffectiveView>("hide_topic_resource", {
      topicId: props.topic.id,
      resourceId: resource.id,
      expectedRevision: snapshot.value.revision,
    }));
    if (selectedId.value === resource.id) selectedId.value = null;
    message.value = "Hidden for this track; the project binding is unchanged.";
  } catch (error: any) {
    await refresh();
    message.value = error?.message ?? String(error);
  } finally {
    saving.value = false;
  }
}

/** Restore a hidden inherited binding. */
async function unhide(resource: EffectiveRow) {
  if (saving.value) return;
  saving.value = true;
  message.value = "";
  try {
    applyEffective(await invoke<TopicEffectiveView>("unhide_topic_resource", {
      topicId: props.topic.id,
      resourceId: resource.id,
      expectedRevision: snapshot.value.revision,
    }));
    message.value = "Restored from the project binding.";
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
  <aside class="resource-panel" :class="{ embedded }" :aria-label="embedded ? 'Track resources tab' : 'Track resources'">
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
      <section class="resource-panel-live" aria-label="Track repository, PRs, issues, and checkout">
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
        No resources attached yet. Attach a design document or file so it stays with this track.
      </div>
      <button v-for="resource in sortedResources" :key="resource.id" class="resource-panel-item"
        :class="{ selected: resource.id === selectedId }"
        :title="resource.kind === 'pull_request' && attachedPr(resource) ? prTooltip(attachedPr(resource)!) : (resource.url ?? resource.path ?? '')"
        @click="resource.url ? openExternal(resource.url) : (selectedId = resource.id)">
        <span class="resource-panel-icon"
          :class="resource.kind === 'pull_request' && attachedPr(resource) ? (prLight(attachedPr(resource)!) ? `pr-light-${prLight(attachedPr(resource)!)}` : 'pr-light-none') : ''">{{ iconFor(resource.kind) }}</span>
        <span class="resource-panel-item-text">
          <strong>{{ resource.title }}</strong>
          <small>{{ resource.kind === 'repository' ? resource.repository : (resource.path || resource.url || resource.reference) }}</small>
        </span>
        <!-- Origin label (design.md: show each binding's origin): a chip the
             row's own actions key off too — only project-origin rows hide,
             only topic rows detach. -->
        <span class="resource-origin" :class="resource.origin" :title="resource.origin === 'project' ? `Inherited from project ${snapshot.project?.name ?? ''}` : 'Attached to this track'">{{ originLabel(resource.origin) }}</span>
      </button>
      <!-- Hidden inherited bindings (design.md: a Topic can hide an inherited
           resource without deleting it from the Project): kept visible with a
           restore affordance so the opt-out is discoverable and reversible. -->
      <template v-if="hidden_resources.length">
        <div class="resource-panel-subhead">Hidden from this track</div>
        <div v-for="resource in hidden_resources" :key="`hidden:${resource.id}`" class="resource-panel-item static">
          <span class="resource-panel-icon dimmed">{{ iconFor(resource.kind) }}</span>
          <span class="resource-panel-item-text">
            <strong class="dimmed">{{ resource.title }}</strong>
            <small>{{ resource.kind === 'repository' ? resource.repository : (resource.path || resource.url || resource.reference) }}</small>
          </span>
          <span class="resource-origin project" title="Hidden inherited binding">project</span>
          <button class="link" :disabled="saving" title="Restore this inherited resource" @click="unhide(resource)">Restore</button>
        </div>
      </template>
    </div>
    <div class="resource-panel-attach">
      <button class="resource-panel-add" @click="showAttach = !showAttach">{{ showAttach ? 'Cancel' : (formKind === 'github' ? 'Find on GitHub…' : formKind === 'design_document' ? 'Design document…' : formKind === 'repository' ? 'Repository…' : 'File…') }}</button>
      <button v-if="showAttach" class="resource-panel-add-caret" title="Choose kind" @click="attachMenuOpen = !attachMenuOpen">▾</button>
      <div v-if="attachMenuOpen" class="resource-panel-attach-menu">
        <button type="button" :class="{ current: formKind === 'github' }" @click="chooseAttachKind('github')">GitHub PR or issue</button>
        <button type="button" :class="{ current: formKind === 'design_document' }" @click="chooseAttachKind('design_document')">Design document</button>
        <button type="button" :class="{ current: formKind === 'file' }" @click="chooseAttachKind('file')">File</button>
        <!-- Repository bindings (design.md: a topic can bring in another
             repo; a project can bind several) — picked, not typed. -->
        <button type="button" :class="{ current: formKind === 'repository' }" @click="chooseAttachKind('repository')">Repository</button>
      </div>
    </div>
    <div v-if="showAttach" class="resource-panel-form">
      <template v-if="formKind === 'github'">
        <div class="gh-search-bar">
          <select v-model="ghRepo" class="gh-repo" title="Search this repo">
            <option v-for="repo in ghRepos" :key="repo" :value="repo">{{ repo }}</option>
          </select>
          <input v-model="ghQuery" placeholder="#27 or words from the title" spellcheck="false"
            @input="runGhSearch" @keydown.enter.prevent="runGhSearch" />
        </div>
        <div v-if="ghSearching" class="resource-panel-empty">Searching…</div>
        <div v-else-if="ghError" class="resource-panel-message">{{ ghError }}</div>
        <div v-else-if="ghResults.length" class="gh-results">
          <button v-for="row in ghResults" :key="row.url" type="button" class="gh-result"
            :title="row.title" :disabled="saving" @click="attachGhResult(row)">
            <strong>#{{ row.number }}</strong>
            <span class="gh-result-title">{{ row.title }}</span>
            <small>{{ row.state.toLowerCase() }}</small>
          </button>
        </div>
        <div v-else-if="ghQuery.trim()" class="resource-panel-empty">Nothing found.</div>
      </template>
      <!-- Repository bindings pick their own repo (design.md: a project
           binds multiple repositories); managed list + the topic's own.
           The slug doubles as the title — the row is its repo. -->
      <template v-else-if="formKind === 'repository'">
        <label>Repository
          <select v-model="formRepo">
            <option v-for="repo in repoOptions" :key="repo" :value="repo">{{ repo }}</option>
          </select>
        </label>
        <button class="primary" :disabled="saving || !attachValid" @click="attach">Attach</button>
      </template>
      <template v-else>
        <label>Title <input v-model="formTitle" placeholder="Design document" /></label>
        <label>Path in track branch <input v-model="formPath" placeholder="docs/design.md" spellcheck="false" /></label>
        <button class="primary" :disabled="saving || !attachValid" @click="attach">Attach</button>
      </template>
    </div>
    <div v-if="message" class="resource-panel-message">{{ message }}</div>
    <div v-if="selected" class="resource-panel-preview">
      <div class="resource-panel-preview-head">
        <strong>{{ selected.title }}</strong>
        <button v-if="canEdit && selected.path && ['file', 'design_document'].includes(selected.kind) && selected.repository === topic.branch.repo_root && selected.reference === topic.branch.branch" @click="editPath = selected.path">Edit</button>
        <button v-if="selected.url" title="Open in your browser" @click="openExternal(selected.url)">Open on GitHub</button>
        <button v-else-if="selected.kind === 'repository' && bindingRepoUrl(selected)" title="Open in your browser" @click="openExternal(bindingRepoUrl(selected)!)">Open on GitHub</button>
        <button v-else :disabled="!selected.path" @click="openInZed(selected)">Open in Zed</button>
        <!-- Topic rows detach (their own addition); inherited rows hide
             (design.md: hide, not delete from the project). -->
        <button v-if="selected.origin === 'project'" class="danger" :disabled="saving" title="Hide this inherited resource for this track; the project binding stays" @click="hide(selected)">Hide</button>
        <button v-else class="danger" :disabled="saving" title="Remove attachment; keep the file" @click="detach(selected)">Remove</button>
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
    <CheckoutEditor v-if="editPath" :key="`${topic.id}:${editPath}`" :session-id="topic.id" :path="editPath" @close="editPath = null" @saved="onCheckoutSaved" />
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
.resource-panel-icon.dimmed { color: var(--text-dim); }
.dimmed { color: var(--text-dim); }
/* Origin chip (design.md: show each binding's origin): project-inherited
   rows carry the project's tint; the topic's own additions stay plain. */
.resource-origin {
  margin-left: auto; flex: none; align-self: center;
  font-size: 9px; font-weight: 600; letter-spacing: .05em; text-transform: uppercase;
  border-radius: 4px; padding: 2px 5px;
  background: var(--bg-hover); color: var(--text-dim);
}
.resource-origin.project { color: var(--accent); background: color-mix(in srgb, var(--accent) 12%, transparent); }
/* PR status light: green = mergeable + CI passing, yellow = CI in progress
   (or not yet known), red = failing or not mergeable, purple = merged,
   dim = closed without merging. The glyph itself carries the color; the
   row's hover tooltip names the light. */
.resource-panel-icon.pr-light-green { color: var(--ok); }
.resource-panel-icon.pr-light-yellow { color: var(--attention); }
.resource-panel-icon.pr-light-red { color: var(--blocked); }
.resource-panel-icon.pr-light-purple { color: var(--merged); }
.resource-panel-icon.pr-light-none { color: var(--text-dim); }
.resource-panel-item-text { display: flex; flex-direction: column; min-width: 0; }
.resource-panel-item-text strong, .resource-panel-item-text small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.resource-panel-item-text small, .resource-panel-empty, .resource-panel-location { color: var(--text-dim); font-size: 11px; }
.resource-panel-empty { padding: 12px; }
.resource-panel-attach { position: relative; margin: 6px 10px; display: flex; }
.resource-panel-add { flex: 1; min-width: 0; }
.resource-panel-add-caret { padding: 0 8px; }
.resource-panel-attach-menu { position: absolute; top: calc(100% + 4px); right: 0; min-width: 190px; background: var(--bg-raised); border: 1px solid var(--border); border-radius: 6px; box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4); z-index: 30; overflow: hidden; display: grid; }
.resource-panel-attach-menu button { display: block; width: 100%; text-align: left; padding: 7px 10px; font-size: 12px; border: 0; background: transparent; color: var(--text); }
.resource-panel-attach-menu button:hover { background: var(--bg-hover); }
.resource-panel-attach-menu button.current { color: var(--accent); }
.gh-search-bar { display: flex; gap: 5px; }
.gh-search-bar .gh-repo { width: auto; flex: none; max-width: 40%; }
.gh-search-bar input { flex: 1; min-width: 0; }
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
