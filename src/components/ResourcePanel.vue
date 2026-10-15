<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from "vue";
import { useBackendInvoke } from "../backendInvoke";

import { backendOf } from "../backendIdentity";
import { open } from "@tauri-apps/plugin-shell";
import MarkdownIt from "markdown-it";
import DOMPurify from "dompurify";
import CheckoutEditor from "./CheckoutEditor.vue";
import AttachResourceMenu, { type AttachDraft } from "./AttachResourceMenu.vue";
import type { ArtifactTarget } from "../artifactLinks";
import { createArtifactPreview, type ArtifactContent } from "../artifactPreview";
import type { SessionSummary } from "../App.vue";
import { liveRows, mentionableRows, originLabel, prLight, prNumberFromUrl, type AttachedPrStatus, type EffectiveRow, type PanelIssue, type PanelPr } from "../resourcePanel";

const invoke = useBackendInvoke(computed(() => backendOf(props.topic.id)));

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

const props = defineProps<{ topic: SessionSummary; embedded?: boolean; canEdit?: boolean; artifactRequest?: ArtifactTarget | null }>();
const emit = defineEmits<{ (e: "close"): void; (e: "error", message: string): void; (e: "checkout-saved", sessionId: string): void }>();
const snapshot = ref<TopicResourcesPanel>({
  resources: [], hidden_resources: [], revision: 0, issues: [], prs: [],
});
const selectedId = ref<string | null>(null);
const artifact = createArtifactPreview(target => invoke<ArtifactContent>("read_linked_artifact", {
  key: target.key, name: target.name,
}));
const { target: artifactTarget, view: artifactView, loading: artifactLoading, error: artifactError } = artifact;
watch(() => props.artifactRequest, target => {
  if (target) void artifact.open(target);
  else artifact.close();
}, { immediate: true });
onUnmounted(artifact.close);
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
  const id = props.topic.id;
  try {
    if (topicWorktree.value) {
      await invoke("open_in_zed", { id });
      return;
    }
    recoveringWorktree.value = true;
    const wt = await invoke<{ path: string; created: boolean }>("recover_worktree", {
      repoRoot: props.topic.branch.repo_root,
      branch: props.topic.branch.branch,
    });
    if (id !== props.topic.id) return;
    topicWorktree.value = wt.path;
    await invoke("open_in_zed", { id, workDir: wt.path });
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
const renderedArtifact = computed(() => DOMPurify.sanitize(markdown.render(artifactView.value?.content ?? "")));
const loading = ref(false);
const reading = ref(false);
const saving = ref(false);
const message = ref("");

/** The repos the GitHub picker offers: the topic's repo first, then any
 * other repo its subtree's issues touch. */
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
let statusRequest = 0;
async function refreshAttachedStatuses(resources: TopicResource[]) {
  const topicId = props.topic.id;
  const request = ++statusRequest;
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
  if (topicId === props.topic.id && request === statusRequest) attachedStatuses.value = next;
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

function selectAttachedResource(id: string) {
  artifact.close();
  selectedId.value = id;
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
  statusRequest++;
  attachedStatuses.value = {};
  void refresh();
}, { immediate: true });
watch(selectedId, (id) => {
  if (id) void selectResource(id);
  else content.value = "";
});

function onAttachDraft(draft: AttachDraft) {
  // The menu hands back a fully-shaped draft; the only piece it cannot
  // know is the topic's own repo/branch, which stamps file references
  // (validation pins files to the topic repo) and GitHub rows' repository.
  if (draft.kind === "github") {
    void attachGithub(draft.title, draft.url!);
  } else if (draft.kind === "repository") {
    void attachRepository(draft.repository);
  } else {
    void attachFile(draft.kind, draft.title, draft.path!);
  }
}

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

let managedReposRequest = 0;
async function loadManagedRepos() {
  const request = ++managedReposRequest;
  const baseUrl = backendOf(props.topic.id);
  const current = () => request === managedReposRequest && baseUrl === backendOf(props.topic.id);
  managedRepos.value = [];
  try {
    const repos = await invoke<{ slug: string }[] | null>("managed_repos", {}, baseUrl);
    if (!current()) return;
    managedRepos.value = Array.isArray(repos) ? repos.map((r) => r.slug) : [];
  } catch {
    if (current()) managedRepos.value = [];
  }
}
watch(() => backendOf(props.topic.id), loadManagedRepos, { immediate: true });

// --- Attach paths (the menu's chosen drafts) ----------------------------

async function attachGithub(title: string, url: string) {
  if (saving.value) return;
  saving.value = true;
  message.value = "";
  const before = new Set(snapshot.value.resources.map((resource) => resource.id));
  const topicId = props.topic.id;
  try {
    const kind = url.includes("/pull/") ? "pull_request" : "issue";
    const next = await invoke<TopicEffectiveView>("attach_topic_resource", {
      topicId,
      resource: {
        kind,
        title,
        repository: props.topic.branch.repo_root,
        reference: null,
        path: null,
        url,
      },
      expectedRevision: snapshot.value.revision,
    });
    if (topicId !== props.topic.id) return;
    applyEffective(next);
    const attached = next.resources.find((resource: EffectiveRow) => resource.url === url)
      ?? next.resources.find((resource) => !before.has(resource.id));
    selectedId.value = attached?.id ?? selectedId.value;
    message.value = "Resource attached to this track.";
  } catch (error: any) {
    if (topicId !== props.topic.id) return;
    await refresh();
    message.value = error?.message ?? String(error);
  } finally {
    saving.value = false;
  }
}

async function attachFile(kind: "file" | "design_document", title: string, path: string) {
  if (saving.value) return;
  saving.value = true;
  message.value = "";
  const before = new Set(snapshot.value.resources.map((resource) => resource.id));
  const topicId = props.topic.id;
  try {
    const next = await invoke<TopicEffectiveView>("attach_topic_resource", {
      topicId,
      resource: {
        kind,
        title,
        // Files are pinned to the topic's repo and its accepted branch —
        // the branch the launch created is where the work lands.
        repository: props.topic.branch.repo_root,
        reference: props.topic.branch.branch,
        path,
        url: null,
      },
      expectedRevision: snapshot.value.revision,
    });
    if (topicId !== props.topic.id) return;
    applyEffective(next);
    const attached = next.resources.find((resource: EffectiveRow) => resource.path === path)
      ?? next.resources.find((resource) => !before.has(resource.id));
    selectedId.value = attached?.id ?? selectedId.value;
    message.value = "Resource attached to this track.";
  } catch (error: any) {
    if (topicId !== props.topic.id) return;
    await refresh();
    message.value = error?.message ?? String(error);
  } finally {
    saving.value = false;
  }
}

async function attachRepository(repo: string) {
  if (saving.value || !repo.trim()) return;
  saving.value = true;
  message.value = "";
  const before = new Set(snapshot.value.resources.map((resource) => resource.id));
  const topicId = props.topic.id;
  try {
    // The slug doubles as the title — the row is its repo. The command
    // resolves it to the canonical repo root (identity matches a project
    // binding for the same repo).
    const next = await invoke<TopicEffectiveView>("attach_topic_resource", {
      topicId,
      resource: {
        kind: "repository",
        title: repo,
        repository: repo,
        reference: null,
        path: null,
        url: null,
      },
      expectedRevision: snapshot.value.revision,
    });
    if (topicId !== props.topic.id) return;
    applyEffective(next);
    const attached = next.resources.find((resource: EffectiveRow) => resource.kind === "repository" && resource.repository === repo)
      ?? next.resources.find((resource) => !before.has(resource.id));
    selectedId.value = attached?.id ?? selectedId.value;
    message.value = "Resource attached to this track.";
  } catch (error: any) {
    if (topicId !== props.topic.id) return;
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
  const topicId = props.topic.id;
  try {
    const next = await invoke<TopicEffectiveView>("detach_topic_resource", {
      topicId,
      resourceId: resource.id,
      expectedRevision: snapshot.value.revision,
    });
    if (topicId !== props.topic.id) return;
    applyEffective(next);
    if (selectedId.value === resource.id) selectedId.value = null;
    message.value = "Attachment removed; the file was not changed.";
  } catch (error: any) {
    if (topicId !== props.topic.id) return;
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
  const topicId = props.topic.id;
  try {
    const next = await invoke<TopicEffectiveView>("hide_topic_resource", {
      topicId,
      resourceId: resource.id,
      expectedRevision: snapshot.value.revision,
    });
    if (topicId !== props.topic.id) return;
    applyEffective(next);
    if (selectedId.value === resource.id) selectedId.value = null;
    message.value = "Hidden for this track; the project binding is unchanged.";
  } catch (error: any) {
    if (topicId !== props.topic.id) return;
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
  const topicId = props.topic.id;
  try {
    const next = await invoke<TopicEffectiveView>("unhide_topic_resource", {
      topicId,
      resourceId: resource.id,
      expectedRevision: snapshot.value.revision,
    });
    if (topicId !== props.topic.id) return;
    applyEffective(next);
    message.value = "Restored from the project binding.";
  } catch (error: any) {
    if (topicId !== props.topic.id) return;
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
        @click="resource.url ? openExternal(resource.url) : selectAttachedResource(resource.id)">
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
      <!-- One attach affordance (design.md "Resource attachment"): a single
           button whose menu names what can be attached — a GitHub PR/issue
           found by search, a repository file found by path autocomplete,
           or another repository. The menu's chosen drafts arrive via
           onAttachDraft. -->
      <AttachResourceMenu
        :gh-repos="ghRepos"
        :repo-options="repoOptions"
        :files-repo="props.topic.github_repo || props.topic.branch.repo_root"
        :disabled="saving"
        label="Attach…"
        @attach="onAttachDraft"
      />
    </div>
    <div v-if="message" class="resource-panel-message">{{ message }}</div>
    <div v-if="artifactTarget" class="resource-panel-preview">
      <div class="resource-panel-preview-head">
        <strong>{{ artifactView?.title || artifactTarget.name }}</strong>
        <button aria-label="Close artifact preview" @click="artifact.close">×</button>
      </div>
      <div class="resource-panel-location">{{ artifactTarget.key }} · {{ artifactTarget.name }}</div>
      <div v-if="artifactLoading" class="resource-panel-empty">Loading artifact…</div>
      <div v-else-if="artifactError" class="resource-panel-empty" role="alert">{{ artifactError }}</div>
      <div v-else class="resource-panel-content" @click="onPreviewClick" v-html="renderedArtifact"></div>
    </div>
    <div v-else-if="selected" class="resource-panel-preview">
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
