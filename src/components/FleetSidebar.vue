<script setup lang="ts">
import { ref, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { nextTick } from "vue";
import type { SessionSummary, SessionLayout, LaunchOptions, ResourceMention } from "../App.vue";
import { addAttachments, MAX_LAUNCH_TOTAL_BYTES, type FileAttachment } from "../attachments";

// MODEL: a topic is a chat with a leader agent. The leader is the
// top-level session (launched from the input above); children it delegates
// (loom sessions launch / the Delegate button) nest under it. Lanes below
// (layout groups) are only filing — they are NOT topics.

const props = defineProps<{
  fleet: SessionSummary[];
  layout: SessionLayout | null;
  selectedId: string | null;
  launching?: boolean;
  launchOptions: LaunchOptions | null;
}>();

const emit = defineEmits<{
  (e: "select", id: string): void;
  (
    e: "launch",
    task: string,
    repo: string,
    meta?: { title?: string; description?: string; oneOff?: boolean; mentions?: ResourceMention[]; attachments?: FileAttachment[]; profile?: string; agent?: string; model?: string; effort?: string },
    completed?: (success: boolean) => void,
  ): void;
  (
    e: "update-topic",
    id: string,
    fields: { title?: string; description?: string },
    expected?: { title: string; provenance: string },
  ): void;
  (
    e: "reparent",
    sessionId: string,
    parentId: string | null,
    laneId?: string,
  ): void;
  (e: "delete-lane", laneId: string): void;
  (e: "archive", id: string): void;
}>();

const task = ref("");
const quickAttachments = ref<FileAttachment[]>([]);
const quickAttachmentError = ref("");
const quickAttachmentLoading = ref(false);
const quickInputEl = ref<HTMLInputElement | null>(null);
const quickMentionRange = ref<{ start: number; end: number; query: string } | null>(null);
const quickMentionIndex = ref(0);
const quickMentions = ref<{ token: string; topicId: string; resourceId: string }[]>([]);
const repo = ref("marin-community/arachne");
const profile = ref("default");
const agent = ref("");
const model = ref("");
const effort = ref("");
const profiles = computed(() => props.launchOptions?.profiles.filter((p) => p.class === "interactive") ?? []);
const selectedProfile = computed(() => profiles.value.find((p) => p.name === profile.value));
const selectedAgent = computed(() => props.launchOptions?.agents.find((a) => a.kind === (agent.value || selectedProfile.value?.agent_kind || props.launchOptions?.default_agent)));
const modelChoices = computed(() => selectedAgent.value?.models ?? []);
const effortChoices = computed(() => selectedAgent.value?.efforts ?? []);
const launchConfig = () => ({
  profile: profile.value === "default" ? undefined : profile.value,
  agent: agent.value || undefined,
  model: model.value.trim() || undefined,
  effort: effort.value || undefined,
});
function onProfileChange() {
  agent.value = "";
  model.value = "";
  effort.value = "";
}
function onAgentChange() {
  model.value = "";
  effort.value = "";
}
const collapsed = ref(new Set<string>());
const dragging = ref<string | null>(null);
const dropTarget = ref<string | null>(null);

function submit() {
  if (quickMentionRange.value && matchingQuickResources.value.length) {
    chooseQuickMention(matchingQuickResources.value[quickMentionIndex.value] || matchingQuickResources.value[0]);
  }
  const t = task.value.trim();
  if ((!t && !quickAttachments.value.length) || props.launching || quickAttachmentLoading.value) return;
  emit("launch", t || `Review ${quickAttachments.value[0].name}`, repo.value.trim(), {
    ...launchConfig(), oneOff: true,
    mentions: quickMentions.value.filter((mention) => t.includes(mention.token))
      .map(({ topicId, resourceId }) => ({ topicId, resourceId })),
    attachments: quickAttachments.value,
  }, (success) => {
    if (!success) return;
    task.value = "";
    quickAttachments.value = [];
    quickMentions.value = [];
    quickMentionRange.value = null;
  });
}

// Loom tag semantics (weaver-core/src/tags.rs): the loud keys `attention`
// (agent self-report) and `triage` (outside assessment) carry values
// `attention` | `blocked`; absence is the calm/default state. `idle` is a
// quiet resting mark. Prose status lives on branch.description.
function loudTag(s: SessionSummary): { level: "attention" | "blocked" } | null {
  for (const key of ["attention", "triage"]) {
    const tag = s.branch.tags.find((t) => t.key === key);
    if (tag && (tag.value === "attention" || tag.value === "blocked")) {
      return { level: tag.value };
    }
  }
  return null;
}

function subtitle(s: SessionSummary): string {
  return s.branch.description || s.branch.title || "—";
}

function statusClass(s: SessionSummary): string {
  if (s.status === "orphaned") return "orphaned";
  const loud = loudTag(s);
  if (loud?.level === "blocked") return "error";
  if (loud?.level === "attention") return "attention";
  return "done";
}

// The text badge for a row, or null when it should stay quiet: a working
// thread spins instead, a resting one (running + idle mark) shows nothing.
function badgeLabel(s: SessionSummary): string | null {
  if (s.status === "archived") return "done";
  if (s.status === "orphaned") return "orphan";
  const loud = loudTag(s);
  if (loud) return loud.level;
  if (s.status === "running") return null;
  return s.status;
}

// --- Topic tree --------------------------------------------------------------

interface TreeNode {
  session: SessionSummary;
  children: TreeNode[];
}

// The durable marker: a leader chat stamped with the quiet `topic` tag
// (by arachne, at launch/delegate/reparent). Topic-ness never depended on
// live children — the tag survives archive and restarts. The childCount
// fallback keeps pre-marker leaders rendering as topics.
function isTopic(s: SessionSummary): boolean {
  return s.branch.tags.some((t) => t.key === "topic");
}

// The quiet `idle` mark loom stamps when an agent finishes its turn: the
// process stays `running` between turns, so "running" alone means "alive",
// not "working". (Mirrors loom's own idleTag: a resting agent is idle, a
// working one has no idle mark.)
function isIdle(s: SessionSummary): boolean {
  return s.branch.tags.some((t) => t.key === "idle");
}

interface Lane {
  id: string;
  name: string;
  system: string | null;
  trees: TreeNode[];
  count: number;
}

function buildLanes(): Lane[] {
  // Archived children stay in the tree (dimmed) — a topic's finished
  // work is part of its shape; only archived ROOTS are dropped from the
  // lanes. Without this, archiving every child makes a leader look like a
  // plain chat.
  const live = props.fleet.filter((s) => s.status !== "archived");
  const byId = new Map<string, SessionSummary>();
  for (const s of props.fleet) {
    if (s.status !== "archived") byId.set(s.id, s);
    // An archived child is kept only if its parent is visible.
  }
  const childIdsWithVisibleParent = new Set<string>();
  for (const s of props.fleet) {
    if (s.status === "archived") {
      const parent = s.parent_session_id
        ? byId.get(s.parent_session_id)
        : s.parent_id
          ? [...byId.values()].find((c) => c.branch.id === s.parent_id)
          : undefined;
      if (parent && parent.id !== s.id) childIdsWithVisibleParent.add(s.id);
    }
  }
  const nodes = new Map<string, TreeNode>();
  for (const s of live) nodes.set(s.id, { session: s, children: [] });
  for (const id of childIdsWithVisibleParent) {
    const s = props.fleet.find((x) => x.id === id)!;
    nodes.set(id, { session: s, children: [] });
  }

  const roots: TreeNode[] = [];
  const seen = new Set<string>();
  for (const s of live) {
    seen.add(s.id);
    const node = nodes.get(s.id)!;
    const parentKey = s.parent_session_id
      ? byId.get(s.parent_session_id)
      : s.parent_id
        ? [...byId.values()].find((c) => c.branch.id === s.parent_id)
        : undefined;
    if (parentKey && parentKey.id !== s.id) {
      nodes.get(parentKey.id)!.children.push(node);
    } else {
      roots.push(node);
    }
  }
  for (const id of childIdsWithVisibleParent) {
    if (seen.has(id)) continue;
    seen.add(id);
    const node = nodes.get(id)!;
    const s = node.session;
    const parentKey = s.parent_session_id
      ? byId.get(s.parent_session_id)
      : s.parent_id
        ? [...byId.values()].find((c) => c.branch.id === s.parent_id)
          : undefined;
    // parentId was pre-checked when building childIdsWithVisibleParent.
    if (parentKey) nodes.get(parentKey.id)!.children.push(node);
  }
  const byActivity = (a: TreeNode, b: TreeNode) =>
    a.session.last_activity_at < b.session.last_activity_at
      ? 1
      : a.session.last_activity_at === b.session.last_activity_at
        ? a.session.id < b.session.id
          ? 1
          : -1
        : -1;
  const sortTree = (n: TreeNode) => {
    n.children.sort(byActivity);
    n.children.forEach(sortTree);
  };
  roots.sort(byActivity);
  roots.forEach(sortTree);

  // Trees file into lanes (placement groups) by their LEADER's placement.
  const laneFor = new Map<string, TreeNode[]>();
  const laneNames = new Map<
    string,
    { name: string; order: number; system: string | null }
  >();
  if (props.layout) {
    for (const space of props.layout.spaces) {
      for (const g of space.groups) {
        laneFor.set(g.id, []);
        laneNames.set(g.id, {
          name: g.name,
          order: space.rank * 10000 + g.rank,
          system: g.system_key ?? null,
        });
      }
    }
  }
  const unfiled: TreeNode[] = [];
  for (const root of roots) {
    const gid = root.session.placement?.group_id ?? null;
    if (gid && laneFor.has(gid)) laneFor.get(gid)!.push(root);
    else unfiled.push(root);
  }

  const lanes: Lane[] = [];
  const order = (gid: string) => laneNames.get(gid)?.order ?? 999999;
  for (const gid of [...laneFor.keys()].sort((a, b) => order(a) - order(b))) {
    const trees = laneFor.get(gid)!;
    const meta = laneNames.get(gid)!;
    // System lanes (per-space Inboxes) hide when empty; user lanes always
    // render so an empty one stays visible.
    if (trees.length === 0 && meta.system) continue;
    const count = trees.reduce((acc, t) => acc + 1 + countTree(t), 0);
    lanes.push({ id: gid, name: meta.name, system: meta.system, trees, count });
  }
  if (unfiled.length > 0) {
    lanes.push({
      id: "unfiled",
      name: "Inbox",
      system: null,
      trees: unfiled,
      count: unfiled.length,
    });
  }
  return lanes;
}

function countTree(n: TreeNode): number {
  return n.children.reduce((acc, c) => acc + 1 + countTree(c), 0);
}

const lanes = computed(() => buildLanes());

// Flatten each lane's trees into render rows, skipping subtrees under a
// collapsed leader. Collapsing a leader hides everything it delegated.
interface Row {
  session: SessionSummary;
  depth: number;
  childCount: number;
  isCollapsed: boolean;
}

const laneRows = computed<{ lane: Lane; rows: Row[] }[]>(() =>
  lanes.value.map((lane) => {
    const out: Row[] = [];
    const walk = (node: TreeNode, depth: number) => {
      const isCollapsed = collapsed.value.has(node.session.id);
      out.push({
        session: node.session,
        depth,
        childCount: node.children.length,
        isCollapsed,
      });
      if (!isCollapsed) node.children.forEach((c) => walk(c, depth + 1));
    };
    lane.trees.forEach((t) => walk(t, 0));
    return { lane, rows: out };
  }),
);

function toggle(id: string) {
  if (collapsed.value.has(id)) collapsed.value.delete(id);
  else collapsed.value.add(id);
}

// The user-space Inbox: where "unfile" drops and lane deletes send chats.
const userInboxId = computed(() => {
  for (const sp of props.layout?.spaces ?? []) {
    if (sp.system_key === "user" || sp.name === "User") {
      const g = sp.groups.find((g) => g.system_key === "inbox");
      if (g) return g.id;
    }
  }
  return undefined;
});

// --- Tabs --------------------------------------------------------------------

// The sidebar's two surfaces: the Inbox (the filing lanes + delegation tree)
// and Topics (the per-topic card list with title/description/config).
const tab = ref<"inbox" | "topics">("inbox");

// --- Topics tab --------------------------------------------------------------

// A topic card's list entry: the leader session plus its delegated count.
interface TopicEntry {
  session: SessionSummary;
  childCount: number;
}

// Top-level leaders (durable `topic` marker, or delegated children as the
// pre-marker fallback), newest activity first. Archived leaders stay listed
// (dimmed) — topic-ness survives archive by design; children file under their
// leader, so they never appear here.
const topics = computed<TopicEntry[]>(() => {
  const byId = new Map(props.fleet.map((s) => [s.id, s]));
  const parentOf = (s: SessionSummary) =>
    s.parent_session_id
      ? byId.get(s.parent_session_id)
      : s.parent_id
        ? props.fleet.find((c) => c.branch.id === s.parent_id)
        : undefined;
  const childCount = new Map<string, number>();
  for (const s of props.fleet) {
    const p = parentOf(s);
    if (p && p.id !== s.id)
      childCount.set(p.id, (childCount.get(p.id) ?? 0) + 1);
  }
  return props.fleet
    .filter((s) => {
      const p = parentOf(s);
      const topLevel = !p || p.id === s.id;
      return topLevel && (isTopic(s) || (childCount.get(s.id) ?? 0) > 0);
    })
    .map((s) => ({ session: s, childCount: childCount.get(s.id) ?? 0 }))
    .sort((a, b) =>
      a.session.last_activity_at < b.session.last_activity_at ? 1 : -1,
    );
});

// A topic has a short title and a substantive body. The body is both the
// agent's initial goal and the durable branch description.
const newTitle = ref("");
const newBody = ref("");
const topicAttachments = ref<FileAttachment[]>([]);
const topicAttachmentError = ref("");
const topicAttachmentLoading = ref(false);

async function addQuickFiles(files: FileList | File[]) {
  if (quickAttachmentLoading.value) return;
  quickAttachmentLoading.value = true;
  try { quickAttachments.value = await addAttachments(quickAttachments.value, files, MAX_LAUNCH_TOTAL_BYTES); quickAttachmentError.value = ""; }
  catch (error: any) { quickAttachmentError.value = error?.message ?? String(error); }
  finally { quickAttachmentLoading.value = false; }
}
async function addTopicFiles(files: FileList | File[]) {
  if (topicAttachmentLoading.value) return;
  topicAttachmentLoading.value = true;
  try { topicAttachments.value = await addAttachments(topicAttachments.value, files, MAX_LAUNCH_TOTAL_BYTES); topicAttachmentError.value = ""; }
  catch (error: any) { topicAttachmentError.value = error?.message ?? String(error); }
  finally { topicAttachmentLoading.value = false; }
}
function onQuickFileInput(event: Event) {
  const input = event.target as HTMLInputElement;
  if (input.files) void addQuickFiles(input.files);
  input.value = "";
}
function onTopicFileInput(event: Event) {
  const input = event.target as HTMLInputElement;
  if (input.files) void addTopicFiles(input.files);
  input.value = "";
}
function onQuickPaste(event: ClipboardEvent) {
  if (!event.clipboardData?.files.length) return;
  event.preventDefault(); void addQuickFiles(event.clipboardData.files);
}
function onTopicPaste(event: ClipboardEvent) {
  if (!event.clipboardData?.files.length) return;
  event.preventDefault(); void addTopicFiles(event.clipboardData.files);
}
interface TopicMentionResource {
  topicId: string;
  id: string;
  title: string;
  kind: string;
  path: string | null;
  url: string | null;
  repository: string;
}
const topicBodyEl = ref<HTMLTextAreaElement | null>(null);
const topicMentionRange = ref<{ start: number; end: number; query: string } | null>(null);
const topicMentionIndex = ref(0);
const topicMentionResources = ref<TopicMentionResource[]>([]);
const topicMentionLoading = ref(false);
const topicMentionError = ref("");
const topicMentions = ref<{ token: string; topicId: string; resourceId: string }[]>([]);
const matchingTopicResources = computed(() => {
  const query = topicMentionRange.value?.query.trim().toLowerCase() ?? "";
  return topicMentionResources.value.filter((resource) =>
    !query || [resource.title, resource.kind, resource.path, resource.url, resource.repository]
      .some((value) => value?.toLowerCase().includes(query)),
  ).slice(0, 8);
});
const matchingQuickResources = computed(() => {
  const query = quickMentionRange.value?.query.trim().toLowerCase() ?? "";
  return topicMentionResources.value.filter((resource) =>
    !query || [resource.title, resource.kind, resource.path, resource.url, resource.repository]
      .some((value) => value?.toLowerCase().includes(query)),
  ).slice(0, 8);
});

async function loadTopicMentionResources() {
  topicMentionLoading.value = true;
  topicMentionError.value = "";
  try {
    const views = await Promise.allSettled(topics.value.slice(0, 24).map(async ({ session }) => {
      const view = await invoke<{ resources: Omit<TopicMentionResource, "topicId">[] }>("topic_resources", { topicId: session.id });
      return (view.resources ?? []).map((resource) => ({ ...resource, topicId: session.id }));
    }));
    topicMentionResources.value = views.flatMap((result) => result.status === "fulfilled" ? result.value : []);
    if (views.length && views.every((result) => result.status === "rejected")) {
      topicMentionError.value = "Could not load topic resources";
    }
  } finally {
    topicMentionLoading.value = false;
  }
}

function updateTopicMention() {
  const caret = topicBodyEl.value?.selectionStart ?? newBody.value.length;
  const match = /(?:^|\s)@([^@{}\n]{0,64})$/.exec(newBody.value.slice(0, caret));
  const wasOpen = !!topicMentionRange.value;
  topicMentionRange.value = match ? { start: caret - match[1].length - 1, end: caret, query: match[1] } : null;
  topicMentionIndex.value = 0;
  if (topicMentionRange.value && !wasOpen) void loadTopicMentionResources();
}

function updateQuickMention() {
  const caret = quickInputEl.value?.selectionStart ?? task.value.length;
  const match = /(?:^|\s)@([^@{}\n]{0,64})$/.exec(task.value.slice(0, caret));
  const wasOpen = !!quickMentionRange.value;
  quickMentionRange.value = match ? { start: caret - match[1].length - 1, end: caret, query: match[1] } : null;
  quickMentionIndex.value = 0;
  if (quickMentionRange.value && !wasOpen) void loadTopicMentionResources();
}

function chooseQuickMention(resource: TopicMentionResource) {
  const range = quickMentionRange.value;
  if (!range) return;
  const duplicate = topicMentionResources.value.some((other) => other.id !== resource.id && other.title === resource.title);
  const label = duplicate ? `${resource.title} (${resource.path || resource.url || resource.repository})` : resource.title;
  const token = `@{${label}}`;
  task.value = task.value.slice(0, range.start) + token + " " + task.value.slice(range.end);
  quickMentions.value.push({ token, topicId: resource.topicId, resourceId: resource.id });
  quickMentionRange.value = null;
  nextTick(() => {
    const caret = range.start + token.length + 1;
    quickInputEl.value?.focus();
    quickInputEl.value?.setSelectionRange(caret, caret);
  });
}

function onQuickKeydown(event: KeyboardEvent) {
  if (event.key === "Enter") {
    event.preventDefault();
    if (quickMentionRange.value && matchingQuickResources.value.length) chooseQuickMention(matchingQuickResources.value[quickMentionIndex.value] || matchingQuickResources.value[0]);
    else submit();
    return;
  }
  if (!quickMentionRange.value) return;
  if (event.key === "Escape") { event.preventDefault(); quickMentionRange.value = null; }
  else if (event.key === "ArrowDown" && matchingQuickResources.value.length) {
    event.preventDefault(); quickMentionIndex.value = (quickMentionIndex.value + 1) % matchingQuickResources.value.length;
  } else if (event.key === "ArrowUp" && matchingQuickResources.value.length) {
    event.preventDefault(); quickMentionIndex.value = (quickMentionIndex.value - 1 + matchingQuickResources.value.length) % matchingQuickResources.value.length;
  }
}

function chooseTopicMention(resource: TopicMentionResource) {
  const range = topicMentionRange.value;
  if (!range) return;
  const duplicate = topicMentionResources.value.some((other) => other.id !== resource.id && other.title === resource.title);
  const label = duplicate ? `${resource.title} (${resource.path || resource.url || resource.repository})` : resource.title;
  const token = `@{${label}}`;
  newBody.value = newBody.value.slice(0, range.start) + token + " " + newBody.value.slice(range.end);
  topicMentions.value.push({ token, topicId: resource.topicId, resourceId: resource.id });
  topicMentionRange.value = null;
  nextTick(() => {
    const caret = range.start + token.length + 1;
    topicBodyEl.value?.focus();
    topicBodyEl.value?.setSelectionRange(caret, caret);
  });
}

function onTopicBodyKeydown(event: KeyboardEvent) {
  if (!topicMentionRange.value) return;
  if (event.key === "Escape") { event.preventDefault(); topicMentionRange.value = null; }
  else if (event.key === "ArrowDown" && matchingTopicResources.value.length) {
    event.preventDefault(); topicMentionIndex.value = (topicMentionIndex.value + 1) % matchingTopicResources.value.length;
  } else if (event.key === "ArrowUp" && matchingTopicResources.value.length) {
    event.preventDefault(); topicMentionIndex.value = (topicMentionIndex.value - 1 + matchingTopicResources.value.length) % matchingTopicResources.value.length;
  } else if (event.key === "Enter" && matchingTopicResources.value.length && !event.shiftKey) {
    event.preventDefault(); chooseTopicMention(matchingTopicResources.value[topicMentionIndex.value] || matchingTopicResources.value[0]);
  }
}

function submitTopic() {
  const title = newTitle.value.trim();
  const body = newBody.value.trim();
  if ((!title && !body && !topicAttachments.value.length) || props.launching || topicAttachmentLoading.value) return;
  emit("launch", body || title || `Review ${topicAttachments.value[0].name}`, repo.value.trim(), {
    title: title || undefined,
    description: body || undefined,
    mentions: topicMentions.value.filter((mention) => body.includes(mention.token))
      .map(({ topicId, resourceId }) => ({ topicId, resourceId })),
    attachments: topicAttachments.value,
    ...launchConfig(),
  }, (success) => {
    if (!success) return;
    newTitle.value = "";
    newBody.value = "";
    topicAttachments.value = [];
    topicMentions.value = [];
  });
}

// Inline card editing: title edits are compare-and-swap fenced server-side,
// so the save passes the values the card last rendered as `expected`.
const editingId = ref<string | null>(null);
const editTitle = ref("");
const editDesc = ref("");

function startEdit(s: SessionSummary) {
  editingId.value = s.id;
  editTitle.value = s.branch.title;
  editDesc.value = s.branch.description;
}

function cancelEdit() {
  editingId.value = null;
}

function saveEdit(s: SessionSummary) {
  const fields: { title?: string; description?: string } = {};
  const title = editTitle.value.trim();
  // An emptied title is ignored (loom rejects empty labels); only changed
  // fields ride the update so the CAS fence is never tripped needlessly.
  if (title && title !== s.branch.title) fields.title = title;
  if (editDesc.value !== s.branch.description)
    fields.description = editDesc.value;
  if (fields.title)
    emit(
      "update-topic",
      s.id,
      fields,
      {
        title: s.branch.title,
        provenance: s.branch.title_provenance || "user",
      },
    );
  else if (fields.description) emit("update-topic", s.id, fields);
  editingId.value = null;
}

// --- Drag & drop ------------------------------------------------------------
//
// Drag a chat onto a LEADER row → it joins that leader's topic
// (reparent under it; the server also follows its lane).
// Drag onto a lane header → file as a top-level chat in that lane.

function onDragStart(id: string, e: DragEvent) {
  dragging.value = id;
  if (e.dataTransfer) {
    e.dataTransfer.setData("text/plain", id);
    e.dataTransfer.effectAllowed = "move";
  }
}

function onDragOver(key: string, e: DragEvent) {
  if (!dragging.value) return;
  e.preventDefault();
  if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
  dropTarget.value = key;
}

function onDragLeave(key: string) {
  if (dropTarget.value === key) dropTarget.value = null;
}

function onDropLeader(parentId: string, key: string, e: DragEvent) {
  e.preventDefault();
  dropTarget.value = null;
  if (dragging.value && dragging.value !== parentId) {
    emit("reparent", dragging.value, parentId);
    dragging.value = null;
  }
}

function onDropLane(laneId: string, key: string, e: DragEvent) {
  e.preventDefault();
  dropTarget.value = null;
  if (dragging.value) {
    const target = laneId === "unfiled" ? userInboxId.value : laneId;
    emit("reparent", dragging.value, null, target);
    dragging.value = null;
  }
}

// Row hover archive: reuse the app-wide onArchived state update. Confirm
// first because the row disappears from the lanes when archived —
// a misclick on a topic you meant to select shouldn't vanish it.
const confirmId = ref<string | null>(null);

async function archiveRow(id: string) {
  if (confirmId.value === id) {
    emit("archive", id);
    confirmId.value = null;
    return;
  }
  confirmId.value = id;
  // Reset the confirm state if the user doesn't click again.
  setTimeout(() => {
    if (confirmId.value === id) confirmId.value = null;
  }, 2500);
}
</script>

<template>
  <aside class="sidebar">
    <div class="tab-bar" role="tablist">
      <button
        class="tab"
        :class="{ active: tab === 'inbox' }"
        role="tab"
        :aria-selected="tab === 'inbox'"
        @click="tab = 'inbox'"
      >
        Inbox
      </button>
      <button
        class="tab"
        :class="{ active: tab === 'topics' }"
        role="tab"
        :aria-selected="tab === 'topics'"
        @click="tab = 'topics'"
      >
        Topics
      </button>
    </div>

    <!-- Inbox tab: filing lanes, delegation tree, and a fast one-off launch. -->
    <template v-if="tab === 'inbox'">
      <div class="new-task quick-task-wrap">
        <input
          ref="quickInputEl"
          v-model="task"
          placeholder="Quick one-off task… Use @ for resources"
          aria-label="Quick one-off task"
          @input="updateQuickMention"
          @click="updateQuickMention"
          @keydown="onQuickKeydown"
          @paste="onQuickPaste"
        />
        <button
          class="primary"
          :disabled="(!task.trim() && !quickAttachments.length) || props.launching || quickAttachmentLoading"
          @click="submit"
        >
          {{ props.launching ? "…" : "Run" }}
        </button>
        <div v-if="quickMentionRange" class="mention-menu quick-mention-menu" role="listbox" aria-label="Existing resources for quick task">
          <div v-if="topicMentionLoading" class="mention-hint">Loading resources…</div>
          <div v-else-if="topicMentionError" class="mention-hint">{{ topicMentionError }}</div>
          <div v-else-if="!matchingQuickResources.length" class="mention-hint">No matching attached resources</div>
          <button v-for="(resource, index) in matchingQuickResources" :key="`${resource.topicId}:${resource.id}`"
            role="option" :aria-selected="index === quickMentionIndex" :class="{ selected: index === quickMentionIndex }"
            @mousedown.prevent="chooseQuickMention(resource)">
            <strong>{{ resource.title }}</strong>
            <small>{{ resource.repository }} · {{ resource.path || resource.url || resource.kind }}</small>
          </button>
        </div>
      </div>
      <div class="attachment-row quick-attachment-row" @dragover.prevent @drop.prevent="($event) => $event.dataTransfer?.files && addQuickFiles($event.dataTransfer.files)">
        <label class="attachment-pick">+ Attach files<input type="file" multiple :disabled="quickAttachmentLoading" aria-label="Attach files to quick task" @change="onQuickFileInput" /></label>
        <span v-for="(file, index) in quickAttachments" :key="file.name" class="attachment-chip">
          {{ file.name }} <button type="button" :aria-label="`Remove ${file.name}`" @click="quickAttachments.splice(index, 1)">×</button>
        </span>
        <span v-if="quickAttachmentError" class="attachment-error">{{ quickAttachmentError }}</span>
        <span v-if="quickAttachmentLoading" class="attachment-hint">Reading files…</span>
      </div>
      <div class="new-task" style="margin-top: -4px">
        <input
          v-model="repo"
          placeholder="owner/name"
          spellcheck="false"
          style="font-family: var(--mono); font-size: 11px"
        />
      </div>
      <div class="new-task launch-controls" style="margin-top: -4px">
        <select v-model="profile" aria-label="Inference profile" @change="onProfileChange">
          <option v-if="!profiles.some((p) => p.name === 'default')" value="default">Default route</option>
          <option v-for="p in profiles" :key="p.name" :value="p.name">
            {{ p.name }} · {{ p.agent_kind }}
          </option>
        </select>
        <select v-model="agent" aria-label="Agent runtime" @change="onAgentChange">
          <option value="">{{ selectedProfile?.agent_kind || launchOptions?.default_agent || 'Default agent' }}</option>
          <option v-for="choice in launchOptions?.agents ?? []" :key="choice.kind" :value="choice.kind">{{ choice.label }}</option>
        </select>
        <select v-if="modelChoices.length && !selectedAgent?.accepts_raw_model" v-model="model" aria-label="Model">
          <option value="">{{ agent ? 'Runtime default model' : (selectedProfile?.model || 'Runtime default model') }}</option>
          <option v-for="choice in modelChoices" :key="choice.id" :value="choice.id">{{ choice.label }}</option>
        </select>
        <input v-else
          v-model="model"
          list="launch-models"
          :placeholder="agent ? 'Model · runtime default' : (selectedProfile?.model || 'Model · runtime default')"
          aria-label="Model override"
          spellcheck="false"
          style="font-family: var(--mono)"
        />
        <datalist id="launch-models">
          <option v-for="choice in modelChoices" :key="choice.id" :value="choice.id">{{ choice.label }}</option>
        </datalist>
        <select v-model="effort" aria-label="Reasoning effort">
          <option value="">{{ agent ? 'Default effort' : (selectedProfile?.effort || 'Default effort') }}</option>
          <option v-for="choice in effortChoices" :key="choice.id" :value="choice.id">{{ choice.label }}</option>
        </select>
      </div>

      <div class="session-list">
        <template v-for="{ lane, rows } in laneRows" :key="lane.id">
          <div
            class="group-header"
            :class="{ 'drop-hint': dropTarget === `lane-${lane.id}` }"
            @dragover="onDragOver(`lane-${lane.id}`, $event)"
            @dragleave="onDragLeave(`lane-${lane.id}`)"
            @drop="onDropLane(lane.id, `lane-${lane.id}`, $event)"
            :title="
              dragging ? 'drop here to file as a top-level chat' : lane.name
            "
          >
            <span class="group-name">{{ lane.name }}</span>
            <span class="group-count">{{ lane.count }}</span>
            <button
              v-if="!lane.system && lane.id !== 'unfiled'"
              class="link stream-delete"
              title="delete lane (chats move to Inbox)"
              @click.stop="emit('delete-lane', lane.id)"
            >
              ✕
            </button>
          </div>
          <div
            v-for="row in rows"
            :key="row.session.id"
            class="session-item"
            :class="{
              selected: row.session.id === selectedId,
              topic:
                row.depth === 0 &&
                (isTopic(row.session) || row.childCount > 0),
              child: row.depth > 0,
              archived: row.session.status === 'archived',
              'drop-hint':
                row.depth === 0 && dropTarget === `ws-${row.session.id}`,
            }"
            :style="
              row.depth > 0 ? { marginLeft: `${12 + row.depth * 14}px` } : {}
            "
            draggable="true"
            @dragstart="onDragStart(row.session.id, $event)"
            @dragover="
              row.depth === 0 && onDragOver(`ws-${row.session.id}`, $event)
            "
            @dragleave="onDragLeave(`ws-${row.session.id}`)"
            @drop.stop="
              row.depth === 0 &&
              onDropLeader(row.session.id, `ws-${row.session.id}`, $event)
            "
            @click="emit('select', row.session.id)"
            :title="
              row.depth === 0 && dragging
                ? 'drop here to join this topic'
                : undefined
            "
          >
            <div class="row1">
              <span
                v-if="row.childCount > 0"
                class="chevron"
                @click.stop="toggle(row.session.id)"
              >
                {{ row.isCollapsed ? "▸" : "▾" }}
              </span>
              <span class="name">{{
                row.session.branch.name || row.session.id
              }}</span>
              <!-- A genuinely-working thread (running, mid-turn) shows a
                   spinner; a resting one (finished turn → quiet idle mark) and
                   terminal rows stay quiet. Loud-tag threads keep their text
                   badge so attention/blocked never reads as ordinary progress. -->
              <span
                v-if="
                  row.session.status === 'running' &&
                  !isIdle(row.session) &&
                  !loudTag(row.session)
                "
                class="spinner mini"
                title="working"
                aria-hidden="true"
              ></span>
              <span
                v-else-if="badgeLabel(row.session)"
                class="badge"
                :class="statusClass(row.session)"
                >{{ badgeLabel(row.session) }}</span
              >
              <span
                v-if="row.childCount > 0"
                class="badge dim"
                :title="`${row.childCount} delegated children`"
              >
                {{ row.childCount }}
              </span>
              <button
                v-if="row.session.status !== 'archived'"
                class="row-archive"
                :class="{ confirm: confirmId === row.session.id }"
                :title="
                  confirmId === row.session.id
                    ? 'click again to archive — tears down worktree, keeps branch'
                    : 'archive this session'
                "
                @click.stop="archiveRow(row.session.id)"
              >
                {{ confirmId === row.session.id ? "archive?" : "✕" }}
              </button>
            </div>
            <div class="title">{{ subtitle(row.session) }}</div>
          </div>
        </template>
        <div
          v-if="lanes.length === 0"
          class="session-item"
          style="color: var(--text-dim)"
        >
          No topics yet — launch one above.
        </div>
      </div>
    </template>

    <!-- Topics tab: one card per topic (leader chat) with title, description,
         and a config placeholder. -->
    <template v-else>
      <div class="new-topic-card">
        <div class="new-topic-heading">New topic</div>
        <label for="new-topic-title">Title <span class="field-optional">optional</span></label>
        <input
          id="new-topic-title"
          v-model="newTitle"
          placeholder="What is this work about?"
        />
        <label for="new-topic-body">Body <span class="field-optional">optional</span></label>
        <div class="topic-body-wrap">
          <textarea
            id="new-topic-body"
            ref="topicBodyEl"
            v-model="newBody"
            rows="6"
            placeholder="Describe the goal, context, and what a good result looks like… Use @ to mention an existing resource."
            @input="updateTopicMention"
            @click="updateTopicMention"
            @keydown="onTopicBodyKeydown"
            @paste="onTopicPaste"
          ></textarea>
          <div v-if="topicMentionRange" class="mention-menu topic-body-mention-menu" role="listbox" aria-label="Existing topic resources">
            <div v-if="topicMentionLoading" class="mention-hint">Loading resources…</div>
            <div v-else-if="topicMentionError" class="mention-hint">{{ topicMentionError }}</div>
            <div v-else-if="!matchingTopicResources.length" class="mention-hint">No matching attached resources</div>
            <button v-for="(resource, index) in matchingTopicResources" :key="`${resource.topicId}:${resource.id}`"
              role="option" :aria-selected="index === topicMentionIndex" :class="{ selected: index === topicMentionIndex }"
              @mousedown.prevent="chooseTopicMention(resource)">
              <strong>{{ resource.title }}</strong>
              <small>{{ resource.repository }} · {{ resource.path || resource.url || resource.kind }}</small>
            </button>
          </div>
        </div>
        <div class="attachment-row" @dragover.prevent @drop.prevent="($event) => $event.dataTransfer?.files && addTopicFiles($event.dataTransfer.files)">
          <label class="attachment-pick">+ Attach files<input type="file" multiple :disabled="topicAttachmentLoading" aria-label="Attach files to new topic" @change="onTopicFileInput" /></label>
          <span v-for="(file, index) in topicAttachments" :key="file.name" class="attachment-chip">
            {{ file.name }} <button type="button" :aria-label="`Remove ${file.name}`" @click="topicAttachments.splice(index, 1)">×</button>
          </span>
          <span v-if="topicAttachmentError" class="attachment-error">{{ topicAttachmentError }}</span>
          <span v-if="topicAttachmentLoading" class="attachment-hint">Reading files…</span>
        </div>
        <div class="new-topic-foot">
          <input
            v-model="repo"
            placeholder="owner/name"
            spellcheck="false"
            style="font-family: var(--mono); font-size: 11px"
          />
          <button
            class="primary"
            :disabled="(!newTitle.trim() && !newBody.trim() && !topicAttachments.length) || props.launching || topicAttachmentLoading"
            @click="submitTopic"
          >
            {{ props.launching ? "…" : "Create topic" }}
          </button>
        </div>
        <div class="launch-controls" style="margin-top: 8px">
          <select v-model="profile" aria-label="Inference profile" @change="onProfileChange">
            <option v-if="!profiles.some((p) => p.name === 'default')" value="default">Default route</option>
            <option v-for="p in profiles" :key="p.name" :value="p.name">{{ p.name }} · {{ p.agent_kind }}</option>
          </select>
          <select v-model="agent" aria-label="Agent runtime" @change="onAgentChange">
            <option value="">{{ selectedProfile?.agent_kind || launchOptions?.default_agent || 'Default agent' }}</option>
            <option v-for="choice in launchOptions?.agents ?? []" :key="choice.kind" :value="choice.kind">{{ choice.label }}</option>
          </select>
          <select v-if="modelChoices.length && !selectedAgent?.accepts_raw_model" v-model="model" aria-label="Model">
            <option value="">{{ agent ? 'Runtime default model' : (selectedProfile?.model || 'Runtime default model') }}</option>
            <option v-for="choice in modelChoices" :key="choice.id" :value="choice.id">{{ choice.label }}</option>
          </select>
          <input v-else v-model="model" list="launch-models-topic" :placeholder="agent ? 'Model · runtime default' : (selectedProfile?.model || 'Model · runtime default')" aria-label="Model override" spellcheck="false" />
          <datalist id="launch-models-topic">
            <option v-for="choice in modelChoices" :key="choice.id" :value="choice.id">{{ choice.label }}</option>
          </datalist>
          <select v-model="effort" aria-label="Reasoning effort">
            <option value="">{{ agent ? 'Default effort' : (selectedProfile?.effort || 'Default effort') }}</option>
            <option v-for="choice in effortChoices" :key="choice.id" :value="choice.id">{{ choice.label }}</option>
          </select>
        </div>
      </div>

      <div class="topic-list">
        <div
          v-for="t in topics"
          :key="t.session.id"
          class="topic-card"
          :class="{
            selected: t.session.id === selectedId,
            archived: t.session.status === 'archived',
          }"
          @click="emit('select', t.session.id)"
        >
          <template v-if="editingId === t.session.id">
            <input
              v-model="editTitle"
              class="topic-edit-title"
              placeholder="title"
              @keydown.enter.prevent="saveEdit(t.session)"
              @keydown.esc.stop="cancelEdit"
              @click.stop
            />
            <textarea
              v-model="editDesc"
              class="topic-edit-desc"
              rows="3"
              placeholder="description"
              @keydown.esc.stop="cancelEdit"
              @click.stop
            ></textarea>
            <div class="topic-edit-foot">
              <button
                class="primary"
                @click.stop="saveEdit(t.session)"
              >
                Save
              </button>
              <button @click.stop="cancelEdit">Cancel</button>
            </div>
          </template>
          <template v-else>
            <div class="topic-head">
              <span class="topic-title">{{
                t.session.branch.title || t.session.branch.name
              }}</span>
              <!-- Same quiet-badge rules as inbox rows: working topic
                   spins, resting/terminal stay quiet, loud tags stay text. -->
              <span
                v-if="
                  t.session.status === 'running' &&
                  !isIdle(t.session) &&
                  !loudTag(t.session)
                "
                class="spinner mini"
                title="working"
                aria-hidden="true"
              ></span>
              <span
                v-else-if="badgeLabel(t.session)"
                class="badge"
                :class="statusClass(t.session)"
                >{{ badgeLabel(t.session) }}</span
              >
            </div>
            <div class="topic-desc">{{
              t.session.branch.description ||
              t.session.branch.goal ||
              "no description yet"
            }}</div>
            <div class="topic-foot">
              <span class="badge dim" v-if="t.childCount > 0" :title="`${t.childCount} delegated children`">{{
                t.childCount
              }}</span>
              <span class="topic-branch">{{
                t.session.branch.name || t.session.branch.branch
              }}</span>
              <button
                class="topic-edit-btn"
                title="edit title and description"
                @click.stop="startEdit(t.session)"
              >
                edit
              </button>
              <!-- TODO: topic config (agent, model, repo defaults, automation
                   triggers) — the settings surface this stub grows into. -->
              <span class="topic-config-soon" title="topic config — coming soon"
                >config ⚙</span
              >
            </div>
          </template>
        </div>
        <div v-if="topics.length === 0" class="topic-empty">
          No topics yet — create one above.
        </div>
      </div>
    </template>
  </aside>
</template>

<style scoped>
.launch-controls {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.launch-controls select,
.launch-controls input {
  box-sizing: border-box;
  flex: 1 1 118px;
  min-width: 0;
  max-width: 100%;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg);
  color: var(--text);
  padding: 6px 8px;
  font: 11px var(--mono);
}
.launch-controls select:focus,
.launch-controls input:focus {
  outline: 1px solid var(--accent);
}
</style>
