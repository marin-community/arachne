<script setup lang="ts">
import { ref, computed, nextTick, watch } from "vue";
import { isStandalone } from "../threadKind";
import { invoke } from "@tauri-apps/api/core";
import type { SessionSummary, SessionLayout } from "../App.vue";
import { buildProjectSections, layoutProjects, topicProjectId, type ProjectRef } from "../projects";
import {
  archivedTopicCount as countArchivedTopics,
  buildTopicList,
  childrenMap as buildChildrenMap,
  visibleTopics,
} from "../topicList";
import { byTopicRecency, topicRecencyMap } from "../topicOrder";
import { dismissibleAttention, pendingPermissionSummary } from "../topicInspector";
import { topicRootOf, threadAncestors } from "../topic-view";
// MODEL: a topic is a chat with a leader agent. The leader is the
// top-level session (launched from the input above); children it delegates
// (loom sessions launch / the Delegate button) nest under it. Lanes below
// (layout groups) are only filing — they are NOT topics.

const props = defineProps<{
  fleet: SessionSummary[];
  layout: SessionLayout | null;
  selectedId: string | null;
  showNewThread?: boolean;
  showNewTopic?: boolean;
}>();

const emit = defineEmits<{
  (e: "select", id: string): void;
  (e: "select-topic", id: string): void;
  (
    e: "update-topic",
    id: string,
    fields: { title?: string; description?: string },
    expected?: { title: string; provenance: string },
  ): void;
  (e: "new-thread"): void;
  (e: "new-topic"): void;
  // Tracks [+] on a project heading: open the new-track chat preselected
  // to that project (null = the Ungrouped section).
  (e: "new-topic-in-project", project: ProjectRef | null): void;
  // The heading's resources button: manage the project's resource
  // bindings directly (ProjectSheet) — where the inheritance defaults
  // are curated, not just overridden at topic creation.
  (e: "manage-project-resources", project: ProjectRef): void;
  (
    e: "reparent",
    sessionId: string,
    parentId: string | null,
    laneId?: string,
  ): void;
  (e: "delete-lane", laneId: string): void;
  (e: "archive", id: string): void;
  // Selecting a project heading filters the main-pane home to that
  // project's topics (null = the unfiltered Tracks home).
  (e: "select-project", project: ProjectRef | null): void;
}>();

// Track folders are collapsed by default: a track's subthreads stay
// hidden until the chevron opens them. Nested subthreads inside an
// open track keep the inverse default (expanded), so opening a track
// shows its full shape; only the root chevron collapses the folder.
const collapsed = ref(new Set<string>());
const expandedTopics = ref(new Set<string>());
const dragging = ref<string | null>(null);
const dropTarget = ref<string | null>(null);

// The Archived filter (topics toolbar): archived leaders disappear from
// the Tracks list by default; this toggle shows them again. See the topic
// list block below for the model.
const showArchived = ref(false);

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
  if (s.pending_permissions?.length && s.status !== "archived") return { level: "attention" };
  return null;
}

function subtitle(s: SessionSummary): string {
  const permission = pendingPermissionSummary(s);
  if (permission) return permission;
  return s.branch.description || s.branch.title || "—";
}

function statusClass(s: SessionSummary): string {
  const loud = loudTag(s);
  if (loud?.level === "blocked") return "error";
  if (loud?.level === "attention") return "attention";
  return "done";
}

// The text badge for a row, or null when it should stay quiet: a working
// thread spins instead, a resting one (running + idle mark) shows nothing.
function badgeLabel(s: SessionSummary): string | null {
  if (s.status === "archived") return "done";
  if (s.status === "orphaned") return null;
  const loud = loudTag(s);
  if (loud) return loud.level;
  if (s.status === "running") return null;
  return s.status;
}

// --- Track tree --------------------------------------------------------------

interface TreeNode {
  session: SessionSummary;
  children: TreeNode[];
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
  // Roots are topics: order by when a person last steered them, not by
  // agent busyness (the same rule as the Tracks tab, src/topicOrder.ts).
  // Workers nested under a leader keep the activity sort — sibling order
  // within a topic is thread-level, and a busy worker bubbling up among
  // its own siblings is informative, not disruptive.
  const byTopic = byTopicRecency(topicRecencyMap(props.fleet));
  const sortTree = (n: TreeNode) => {
    n.children.sort(byActivity);
    n.children.forEach(sortTree);
  };
  roots.sort((a, b) => byTopic(a.session, b.session));
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

function selectRow(row: Row) {
  if (row.depth === 0 && !isStandalone(row.session)) emit("select-topic", row.session.id);
  else emit("select", row.session.id);
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
    lane.trees.filter(t => isStandalone(t.session)).forEach((t) => walk(t, 0));
    return { lane: { ...lane, count: out.length }, rows: out };
  }).filter(entry => entry.rows.length > 0),
);

function toggle(id: string) {
  if (collapsed.value.has(id)) collapsed.value.delete(id);
  else collapsed.value.add(id);
}

// The track card's chevron: opening a track folder shows its subthreads
// under the coordinator; closing hides them again.
function toggleTopicExpanded(id: string) {
  const next = new Set(expandedTopics.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  expandedTopics.value = next;
}

// Selecting a nested worker expands the track folders along the way so
// the selected row is visible: a worker buried under a collapsed folder
// would otherwise highlight nothing the eye can see. Selecting the
// coordinator itself leaves its folder as the user set it — collapsed by
// default, per the folder model above.
watch(() => [props.selectedId, props.fleet] as const, ([id]) => {
  const ancestors = threadAncestors(props.fleet, id);
  if (!ancestors.length) return;
  const next = new Set(expandedTopics.value);
  let grew = false;
  for (const ancestor of ancestors) {
    if (!next.has(ancestor.id)) {
      next.add(ancestor.id);
      grew = true;
    }
  }
  if (grew) expandedTopics.value = next;
});

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
// and Tracks (the per-topic card list with title/description/config).
const tab = ref<"inbox" | "topics">("topics");
watch(() => [props.selectedId, props.fleet] as const, ([id]) => {
  const root = topicRootOf(props.fleet, id);
  if (root) tab.value = isStandalone(root) ? "inbox" : "topics";
});

// --- Tracks tab --------------------------------------------------------------

// The Tracks tab groups the same topic cards by Project: a non-system
// layout group (projects.ts). The project heading carries the Tracks [+]
// affordance; clicking it opens NewThreadSheet preselected to that project.

// Which project's heading is selected (filters the main-pane home). `null`
// means nothing selected — the aggregate Tracks home.
const selectedProjectId = ref<string | null | undefined>(undefined);
function selectProject(project: ProjectRef): void {
  // Clicking an already-selected project clears the filter (a toggle, like
  // the Tracks home link).
  selectedProjectId.value = selectedProjectId.value === project.id ? undefined : project.id;
  emit("select-project", selectedProjectId.value === undefined ? null : project);
}

// Every top-level thread is a topic, including legacy single-prompt launches
// that predate the marker. Archived leaders disappear by default (loom's
// archive tears down the checkout — a done topic is history, not fleet); the
// toolbar's Archived toggle shows them again, dimmed, with descendants.
// Order is by when a person last steered the topic — the newest user
// message anywhere in its subtree (src/topicOrder.ts) — not agent busyness:
// `last_activity_at` restamps on every streamed frame, which made the cards
// jump around while workers ran. The list logic lives in src/topicList.ts
// (tests/topicList.test.mjs).
const topicRecency = computed(() => topicRecencyMap(props.fleet));
const allTopics = computed(() =>
  buildTopicList(props.fleet, byTopicRecency(topicRecency.value)).filter(t => !isStandalone(t.session)),
);
// Archived leaders are dropped unless the Archived toggle is on.
const topics = computed(() => visibleTopics(allTopics.value, showArchived.value));
// Archived leaders hidden by the default filter — surfaced in the empty
// state so "everything is archived" never reads as "no tracks exist".
const archivedCount = computed(() => countArchivedTopics(allTopics.value));

// Tracks filed under their project (a non-system layout group, per the
// design's Project model). Sections keep layout order and empty projects
// still render — a project is a place to file, not a topic count.
// Ungrouped (and any empty section) starts collapsed by default; the user's
// toggles override that default until the next visit.
const projectOverrides = ref(new Map<string, boolean>());
const projectSections = computed(() => buildProjectSections(topics.value, props.layout));
function isProjectCollapsed(id: string | null): boolean {
  const key = id ?? "ungrouped";
  const override = projectOverrides.value.get(key);
  if (override !== undefined) return override;
  const section = projectSections.value.find((s) => s.id === id);
  return !section || section.topics.length === 0;
}
function toggleProject(id: string | null) {
  const key = id ?? "ungrouped";
  const now = isProjectCollapsed(id);
  projectOverrides.value.set(key, !now);
}

interface TopicThreadRow {
  session: SessionSummary;
  depth: number;
}

const topicChildren = computed(() => buildChildrenMap(props.fleet));

// The badge count matches what's visible: archived children are part of a
// topic's shape but the default filter hides them, so the card's count (and
// its expander) reflects the live rows unless the Archived toggle is on.
function filteredChildCount(sessionId: string): number {
  const kids = topicChildren.value.get(sessionId) ?? [];
  return showArchived.value
    ? kids.length
    : kids.filter((s) => s.status !== "archived").length;
}

function visibleTopicChildren(rootId: string): TopicThreadRow[] {
  const rows: TopicThreadRow[] = [];
  const seen = new Set<string>([rootId]);
  const walk = (parentId: string, depth: number) => {
    if (collapsed.value.has(parentId)) return;
    for (const session of topicChildren.value.get(parentId) ?? []) {
      if (seen.has(session.id)) continue;
      seen.add(session.id);
      rows.push({ session, depth });
      walk(session.id, depth + 1);
    }
  };
  // The same Archived filter as the leaders: archived child threads
  // disappear from the Tracks list by default (they return, dimmed, with
  // the toggle).
  walk(rootId, 1);
  return showArchived.value
    ? rows
    : rows.filter((r) => r.session.status !== "archived");
}

// A topic has a short title and a substantive body. The body is both the
// agent's initial goal and the durable branch description.

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

// --- Projects: create, and move a topic into one --------------------------

// A Project is a loom layout group (filing only — no execution state).
// The home for creating and filing into groups: the user's own space when
// it exists (where loom's Inbox lives), else the first space, else the
// first space at all — mirrors the userInboxId fallback the Inbox tab uses.
function projectSpaceId(): string | undefined {
  const spaces = props.layout?.spaces ?? [];
  const user = spaces.find((s) => s.system_key === "user" || s.name === "User");
  const home = user ?? spaces.find((s) => s.groups.some((g) => !g.system_key)) ?? spaces[0];
  return home?.id;
}

const showNewProject = ref(false);
const newProjectName = ref("");
const newProjectError = ref("");
const newProjectEl = ref<HTMLInputElement | null>(null);
function openNewProject() {
  showNewProject.value = true;
  newProjectError.value = "";
  nextTick(() => newProjectEl.value?.focus());
}
async function submitNewProject() {
  const name = newProjectName.value.trim();
  const spaceId = projectSpaceId();
  if (!name) return;
  if (!spaceId) {
    newProjectError.value = "no space available to create the project in";
    return;
  }
  try {
    await invoke("create_group", { spaceId, name });
    newProjectName.value = "";
    showNewProject.value = false;
  } catch (e: any) {
    newProjectError.value = e?.message ?? String(e);
  }
}

// Move to Project: a small menu of the available projects (plus the user
// Inbox, which files the topic back to unfiled).
const moveMenuId = ref<string | null>(null);
function toggleMoveMenu(id: string) {
  moveMenuId.value = moveMenuId.value === id ? null : id;
}
const moveTargets = computed(() => {
  const projects = layoutProjects(props.layout).map((p) => ({ id: p.id, name: p.name }));
  if (userInboxId.value)
    projects.push({ id: userInboxId.value, name: "Unfiled (Inbox)" });
  return projects;
});
const projectIds = computed(() => new Set(moveTargets.value.map((t) => t.id)));
async function moveToProject(topicId: string, groupId: string) {
  moveMenuId.value = null;
  try {
    await invoke("move_to_group", { sessionIds: [topicId], groupId });
  } catch (e: any) {
    console.error("move_to_group failed", e);
  }
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

// Fires whether the drop completed or the drag was cancelled, so it's the
// reliable place to clear both the drag ghost and any lingering drop hint.
function onDragEnd() {
  dragging.value = null;
  dropTarget.value = null;
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

// Drop a dragged topic here: moving between project headings (and
// ungrouped) is filing, so it goes to the layout group rather than
// reparenting. The whole project SECTION is the drop target — cards and
// empty space under a heading file into that heading's project, not just
// the thin header strip. The ungrouped section files to the user's Inbox.
function onDropProject(sectionId: string | null, key: string, e: DragEvent) {
  e.preventDefault();
  dropTarget.value = null;
  if (!dragging.value) return;
  const groupId = sectionId ?? userInboxId.value;
  if (groupId) {
    const id = dragging.value;
    dragging.value = null;
    invoke("move_to_group", { sessionIds: [id], groupId }).catch((err) =>
      console.error("move_to_group failed", err),
    );
  } else {
    dragging.value = null;
  }
}

// The section wrapper is the drop target, so moving between its header and
// its cards fires dragleave on the wrapper (dragenter on the child). Only
// clear the hint when the drag truly leaves the section — otherwise the
// hint flickers off the moment the pointer crosses into a card.
function onDragLeaveProject(key: string, e: DragEvent) {
  const to = e.relatedTarget;
  if (
    to instanceof Node &&
    e.currentTarget instanceof Node &&
    e.currentTarget.contains(to)
  )
    return;
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

// Dismiss a loud row without opening its thread — same clear_attention
// command as the home's Needs You rows. Only TAG-raised attention offers
// the dismiss; permission-raised (unanswered ACP approvals) must be
// answered in the thread, not dismissed.
const dismissingId = ref<string | null>(null);
async function dismissAttention(id: string) {
  if (dismissingId.value) return;
  dismissingId.value = id;
  try {
    await invoke("clear_attention", { session: id });
  } catch (e: any) {
    console.error("clear_attention failed", e);
  } finally {
    dismissingId.value = null;
  }
}

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
        :class="{ active: tab === 'topics' }"
        role="tab"
        :aria-selected="tab === 'topics'"
        @click="tab = 'topics'"
      >
        Tracks
      </button>
      <button
        class="tab"
        :class="{ active: tab === 'inbox' }"
        role="tab"
        :aria-selected="tab === 'inbox'"
        @click="tab = 'inbox'"
      >
        Threads
      </button>
    </div>
    <!-- Inbox tab: the filing lanes + delegation tree. -->
    <template v-if="tab === 'inbox'">
      <button class="new-thread-btn" :class="{ active: props.showNewThread }" @click="emit('new-thread')">
        + New thread
      </button>
      <button class="new-thread-btn" :class="{ active: props.showNewTopic }" @click="emit('new-topic')">
        + New track
      </button>

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
              topic: row.depth === 0,
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
            @click="selectRow(row)"
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
                v-if="dismissibleAttention(row.session)"
                class="row-dismiss"
                type="button"
                :title="`Dismiss attention — clears the flag without opening the thread`"
                :aria-label="`Dismiss attention on ${row.session.branch.title || row.session.branch.name}`"
                :disabled="dismissingId === row.session.id"
                @click.stop="dismissAttention(row.session.id)"
              >{{ dismissingId === row.session.id ? "…" : "●" }}</button>
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
          v-if="laneRows.length === 0"
          class="session-item"
          style="color: var(--text-dim)"
        >
          No one-off threads yet — start one above.
        </div>
      </div>
    </template>

    <!-- Tracks tab: one card per topic (leader chat) with title, description,
         and a config placeholder. -->
    <template v-else>
      <div class="topics-toolbar">
        <span>Tracks</span>
        <div class="topics-toolbar-actions">
          <button
            type="button"
            class="toolbar-filter"
            :class="{ on: showArchived }"
            :aria-pressed="showArchived"
            title="Show archived tracks (they stay hidden by default — archiving tears down the checkout)"
            aria-label="Show archived tracks"
            @click="showArchived = !showArchived"
          >
            Archived
          </button>
          <button
            type="button"
            class="toolbar-new-project"
            :title="'New project (a place to file tracks)'"
            :aria-expanded="showNewProject"
            aria-controls="new-project-card"
            @click="showNewProject ? (showNewProject = false) : openNewProject()"
          >
            + Project
          </button>
          <!-- Composing a new track takes over the main panel (like the
               new-thread sheet), not a floating overlay here. -->
          <button type="button" aria-label="New track" title="New track" :class="{ active: props.showNewTopic }" @click="emit('new-topic')">+</button>
        </div>
      </div>
      <div v-if="showNewProject" class="new-project-card" id="new-project-card" role="dialog" aria-modal="false" aria-label="New project">
        <div class="new-project-head">
          <span>New project</span>
          <button type="button" aria-label="Close new project" @click="showNewProject = false">×</button>
        </div>
        <input
          ref="newProjectEl"
          v-model="newProjectName"
          placeholder="Project name"
          aria-label="Project name"
          @keydown.enter.prevent="submitNewProject"
          @keydown.esc.stop="showNewProject = false"
        />
        <div v-if="newProjectError" class="new-project-error">{{ newProjectError }}</div>
        <div class="new-project-foot">
          <span class="new-project-hint">A place to file related tracks.</span>
          <button type="button" class="primary" :disabled="!newProjectName.trim()" @click="submitNewProject">Create</button>
        </div>
      </div>

      <div class="topic-list">
        <div
          v-for="section in projectSections"
          :key="section.id ?? 'ungrouped'"
          class="project-section"
          :class="{ 'drop-hint': dropTarget === `project-${section.id ?? 'ungrouped'}` }"
          @dragover="onDragOver(`project-${section.id ?? 'ungrouped'}`, $event)"
          @dragleave="onDragLeaveProject(`project-${section.id ?? 'ungrouped'}`, $event)"
          @drop="onDropProject(section.id, `project-${section.id ?? 'ungrouped'}`, $event)"
        >
          <!-- Project heading: select filters the main-pane home to this
               project's topics; the + opens the new-track chat preselected to
               it; the chevron collapses the section. The whole section
               (this heading, its cards, and its empty space) is the drop
               target for filing, not just the thin header strip. -->
          <div
            class="project-header"
            :class="{
              selected: selectedProjectId === section.id,
              empty: !section.topics.length,
            }"
            :title="
              dragging
                ? `drop here to file under ${section.name}`
                : selectedProjectId === section.id
                  ? 'selected — click to clear the project filter'
                  : 'filter home to this project'
            "
            role="button"
            tabindex="0"
            :aria-expanded="!isProjectCollapsed(section.id)"
            :aria-label="`Project ${section.name}${section.topics.length ? `, ${section.topics.length} tracks` : ', no tracks'}`"
            @click="selectProject({ id: section.id, name: section.name })"
            @keydown.enter.self="selectProject({ id: section.id, name: section.name })"
            @keydown.space.self.prevent="selectProject({ id: section.id, name: section.name })"
          >
            <button
              class="project-chevron"
              type="button"
              :title="isProjectCollapsed(section.id) ? 'Expand project' : 'Collapse project'"
              :aria-label="`${isProjectCollapsed(section.id) ? 'Expand' : 'Collapse'} ${section.name}`"
              @click.stop="toggleProject(section.id)"
            >{{ isProjectCollapsed(section.id) ? "▸" : "▾" }}</button>
            <span class="project-name">{{ section.name }}</span>
            <span class="project-count" :title="`${section.topics.length} tracks`">{{ section.topics.length }}</span>
            <button
              class="project-manage-resources"
              type="button"
              :title="`Manage resources inherited by new tracks in ${section.name}`"
              :aria-label="`Manage resources for project ${section.name}`"
              @click.stop="emit('manage-project-resources', { id: section.id, name: section.name })"
            >⚙</button>
            <button
              class="project-new-topic"
              type="button"
              :title="`New track in ${section.name}`"
              :aria-label="`New track in ${section.name}`"
              @click.stop="emit('new-topic-in-project', { id: section.id, name: section.name })"
            >+</button>
          </div>
          <template v-if="!isProjectCollapsed(section.id)">
        <template v-for="t in section.topics" :key="t.session.id">
        <div
          class="topic-card"
          :class="{
            selected: t.session.id === selectedId,
            archived: t.session.status === 'archived',
            dragging: dragging === t.session.id,
          }"
          role="button"
          tabindex="0"
          :draggable="editingId !== t.session.id"
          @dragstart="onDragStart(t.session.id, $event)"
          @dragend="onDragEnd"
          @click="emit('select-topic', t.session.id)"
          @keydown.enter.self="emit('select-topic', t.session.id)"
          @keydown.space.self.prevent="emit('select-topic', t.session.id)"
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
              <button v-if="filteredChildCount(t.session.id)" class="topic-chevron" type="button"
                :aria-label="`${expandedTopics.has(t.session.id) ? 'Expand' : 'Collapse'} threads in ${t.session.branch.title || t.session.branch.name}`"
                :aria-expanded="expandedTopics.has(t.session.id)"
                @click.stop="toggleTopicExpanded(t.session.id)">{{ expandedTopics.has(t.session.id) ? '▾' : '▸' }}</button>
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
              <button
                v-if="dismissibleAttention(t.session)"
                class="topic-dismiss"
                type="button"
                title="Dismiss attention — clears the flag without opening the thread"
                :aria-label="`Dismiss attention on ${t.session.branch.title || t.session.branch.name}`"
                :disabled="dismissingId === t.session.id"
                @click.stop="dismissAttention(t.session.id)"
              >{{ dismissingId === t.session.id ? "…" : "✕" }}</button>
            </div>
            <div class="topic-desc">{{
              t.session.branch.description ||
              t.session.branch.goal ||
              "no description yet"
            }}</div>
            <div class="topic-foot">
              <span class="badge dim" v-if="filteredChildCount(t.session.id) > 0" :title="`${filteredChildCount(t.session.id)} delegated children`">{{
                filteredChildCount(t.session.id)
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
              <div class="topic-move">
                <button
                  class="topic-move-btn"
                  type="button"
                  title="Move to project"
                  :aria-label="`Move ${t.session.branch.title || t.session.branch.name} to another project`"
                  :aria-expanded="moveMenuId === t.session.id"
                  @click.stop="toggleMoveMenu(t.session.id)"
                >move</button>
                <div v-if="moveMenuId === t.session.id" class="topic-move-menu" role="menu" :aria-label="`Move ${t.session.branch.title || t.session.branch.name} to project`">
                  <button v-for="target in moveTargets" :key="target.id" type="button" role="menuitem"
                    :class="{ current: topicProjectId(t.session, projectIds) === target.id }"
                    @click.stop="moveToProject(t.session.id, target.id)">
                    {{ target.name }}
                  </button>
                </div>
              </div>
            </div>
          </template>
        </div>
        <div v-for="child in expandedTopics.has(t.session.id) ? visibleTopicChildren(t.session.id) : []" :key="child.session.id"
          class="topic-thread" :class="{ selected: child.session.id === selectedId, archived: child.session.status === 'archived' }"
          :style="{ marginLeft: `${8 + child.depth * 15}px` }"
          role="button" tabindex="0"
          @click="emit('select', child.session.id)"
          @keydown.enter.self="emit('select', child.session.id)"
          @keydown.space.self.prevent="emit('select', child.session.id)">
          <button v-if="filteredChildCount(child.session.id)" class="topic-chevron" type="button"
            :aria-label="`${collapsed.has(child.session.id) ? 'Expand' : 'Collapse'} subthreads in ${child.session.branch.title || child.session.branch.name}`"
            :aria-expanded="!collapsed.has(child.session.id)"
            @click.stop="toggle(child.session.id)">{{ collapsed.has(child.session.id) ? '▸' : '▾' }}</button>
          <span v-else class="topic-thread-spacer" aria-hidden="true"></span>
          <span class="topic-thread-title">{{ child.session.branch.title || child.session.branch.name }}</span>
          <span v-if="filteredChildCount(child.session.id)" class="badge dim">{{ filteredChildCount(child.session.id) }}</span>
          <span v-else-if="badgeLabel(child.session)" class="badge" :class="statusClass(child.session)">{{ badgeLabel(child.session) }}</span>
        </div>
        </template>
          <div v-if="!section.topics.length" class="project-empty">No tracks in this project yet.</div>
          </template>
        </div>
        <div v-if="topics.length === 0" class="topic-empty">
          <template v-if="archivedCount > 0">
            No active tracks — {{ archivedCount }} archived. Turn on
            <button type="button" class="topic-empty-toggle" @click="showArchived = true">Archived</button>
            to see them.
          </template>
          <template v-else>No tracks yet — use + to create one.</template>
        </div>
      </div>
    </template>
  </aside>
</template>
