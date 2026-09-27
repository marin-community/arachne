<script setup lang="ts">
import { ref, computed, nextTick } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { SessionSummary, SessionLayout } from "../App.vue";
import { buildProjectSections, layoutProjects, topicProjectId, type ProjectRef } from "../projects";

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
  // Topics [+] on a project heading: open NewThreadSheet preselected to
  // that project (null = the Ungrouped section).
  (e: "new-thread-in-project", project: ProjectRef | null): void;
  (
    e: "reparent",
    sessionId: string,
    parentId: string | null,
    laneId?: string,
  ): void;
  (e: "delete-lane", laneId: string): void;
  (e: "archive", id: string): void;
  // Selecting a project heading filters the main-pane home to that
  // project's topics (null = the unfiltered Topics home).
  (e: "select-project", project: ProjectRef | null): void;
}>();

const collapsed = ref(new Set<string>());
const dragging = ref<string | null>(null);
const dropTarget = ref<string | null>(null);

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

// --- Topic tree --------------------------------------------------------------

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

function selectRow(row: Row) {
  if (row.depth === 0) emit("select-topic", row.session.id);
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
const tab = ref<"inbox" | "topics">("topics");

// --- Topics tab --------------------------------------------------------------

// The Topics tab groups the same topic cards by Project: a non-system
// layout group (projects.ts). The project heading carries the Topics [+]
// affordance; clicking it opens NewThreadSheet preselected to that project.

// Which project's heading is selected (filters the main-pane home). `null`
// means nothing selected — the aggregate Topics home.
const selectedProjectId = ref<string | null | undefined>(undefined);
function selectProject(project: ProjectRef): void {
  // Clicking an already-selected project clears the filter (a toggle, like
  // the Topics home link).
  selectedProjectId.value = selectedProjectId.value === project.id ? undefined : project.id;
  emit("select-project", selectedProjectId.value === undefined ? null : project);
}

// A topic card's list entry: the leader session plus its delegated count.
interface TopicEntry {
  session: SessionSummary;
  childCount: number;
}

// Every top-level thread is a topic, including legacy single-prompt launches
// that predate the marker. Archived leaders stay listed with their descendants.
const topics = computed<TopicEntry[]>(() => {
  const byId = new Map(props.fleet.map((s) => [s.id, s]));
  const byBranch = new Map(props.fleet.map((s) => [s.branch.id, s]));
  const parentOf = (s: SessionSummary) =>
    (s.parent_session_id ? byId.get(s.parent_session_id) : undefined) ??
    (s.parent_id ? byBranch.get(s.parent_id) : undefined);
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
      return topLevel;
    })
    .map((s) => ({ session: s, childCount: childCount.get(s.id) ?? 0 }))
    .sort((a, b) =>
      a.session.last_activity_at < b.session.last_activity_at ? 1 : -1,
    );
});

// Topics filed under their project (a non-system layout group, per the
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
  childCount: number;
}

const topicChildren = computed(() => {
  const byId = new Map(props.fleet.map((session) => [session.id, session]));
  const byBranch = new Map(props.fleet.map((session) => [session.branch.id, session]));
  const children = new Map<string, SessionSummary[]>();
  for (const session of props.fleet) {
    const parent =
      (session.parent_session_id ? byId.get(session.parent_session_id) : undefined) ??
      (session.parent_id ? byBranch.get(session.parent_id) : undefined);
    if (!parent || parent.id === session.id) continue;
    const siblings = children.get(parent.id) ?? [];
    siblings.push(session);
    children.set(parent.id, siblings);
  }
  for (const siblings of children.values()) {
    siblings.sort((a, b) => b.last_activity_at.localeCompare(a.last_activity_at));
  }
  return children;
});

function visibleTopicChildren(rootId: string): TopicThreadRow[] {
  const rows: TopicThreadRow[] = [];
  const seen = new Set<string>([rootId]);
  const walk = (parentId: string, depth: number) => {
    if (collapsed.value.has(parentId)) return;
    for (const session of topicChildren.value.get(parentId) ?? []) {
      if (seen.has(session.id)) continue;
      seen.add(session.id);
      rows.push({ session, depth, childCount: topicChildren.value.get(session.id)?.length ?? 0 });
      walk(session.id, depth + 1);
    }
  };
  walk(rootId, 1);
  return rows;
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
// reparenting. The ungrouped section files to the user's Inbox group.
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
        :class="{ active: tab === 'topics' }"
        role="tab"
        :aria-selected="tab === 'topics'"
        @click="tab = 'topics'"
      >
        Topics
      </button>
      <button
        class="tab"
        :class="{ active: tab === 'inbox' }"
        role="tab"
        :aria-selected="tab === 'inbox'"
        @click="tab = 'inbox'"
      >
        Inbox
      </button>
    </div>
    <!-- Inbox tab: the filing lanes + delegation tree. -->
    <template v-if="tab === 'inbox'">
      <button class="new-thread-btn" :class="{ active: props.showNewThread }" @click="emit('new-thread')">
        + New thread
      </button>
      <button class="new-thread-btn" :class="{ active: props.showNewTopic }" @click="emit('new-topic')">
        + New topic
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
          No topics yet — start one above.
        </div>
      </div>
    </template>

    <!-- Topics tab: one card per topic (leader chat) with title, description,
         and a config placeholder. -->
    <template v-else>
      <div class="topics-toolbar">
        <span>Topics</span>
        <div class="topics-toolbar-actions">
          <button
            type="button"
            class="toolbar-new-project"
            :title="'New project (a place to file topics)'"
            :aria-expanded="showNewProject"
            aria-controls="new-project-card"
            @click="showNewProject ? (showNewProject = false) : openNewProject()"
          >
            + Project
          </button>
          <!-- Composing a new topic takes over the main panel (like the
               new-thread sheet), not a floating overlay here. -->
          <button type="button" aria-label="New topic" title="New topic" :class="{ active: props.showNewTopic }" @click="emit('new-topic')">+</button>
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
          <span class="new-project-hint">A place to file related topics.</span>
          <button type="button" class="primary" :disabled="!newProjectName.trim()" @click="submitNewProject">Create</button>
        </div>
      </div>

      <div class="topic-list">
        <template v-for="section in projectSections" :key="section.id ?? 'ungrouped'">
          <!-- Project heading: select filters the main-pane home to this
               project's topics; the + opens NewThreadSheet preselected to
               it; the chevron collapses the section. -->
          <div
            class="project-header"
            :class="{
              selected: selectedProjectId === section.id,
              empty: !section.topics.length,
              'drop-hint': dropTarget === `project-${section.id ?? 'ungrouped'}`,
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
            :aria-label="`Project ${section.name}${section.topics.length ? `, ${section.topics.length} topics` : ', no topics'}`"
            @dragover="onDragOver(`project-${section.id ?? 'ungrouped'}`, $event)"
            @dragleave="onDragLeave(`project-${section.id ?? 'ungrouped'}`)"
            @drop="onDropProject(section.id, `project-${section.id ?? 'ungrouped'}`, $event)"
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
            <span class="project-count" :title="`${section.topics.length} topics`">{{ section.topics.length }}</span>
            <button
              class="project-new-topic"
              type="button"
              :title="`New topic in ${section.name}`"
              :aria-label="`New topic in ${section.name}`"
              @click.stop="emit('new-thread-in-project', { id: section.id, name: section.name })"
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
              <button v-if="t.childCount" class="topic-chevron" type="button"
                :aria-label="`${collapsed.has(t.session.id) ? 'Expand' : 'Collapse'} threads in ${t.session.branch.title || t.session.branch.name}`"
                :aria-expanded="!collapsed.has(t.session.id)"
                @click.stop="toggle(t.session.id)">{{ collapsed.has(t.session.id) ? '▸' : '▾' }}</button>
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
        <div v-for="child in visibleTopicChildren(t.session.id)" :key="child.session.id"
          class="topic-thread" :class="{ selected: child.session.id === selectedId, archived: child.session.status === 'archived' }"
          :style="{ marginLeft: `${8 + child.depth * 15}px` }"
          role="button" tabindex="0"
          @click="emit('select', child.session.id)"
          @keydown.enter.self="emit('select', child.session.id)"
          @keydown.space.self.prevent="emit('select', child.session.id)">
          <button v-if="child.childCount" class="topic-chevron" type="button"
            :aria-label="`${collapsed.has(child.session.id) ? 'Expand' : 'Collapse'} subthreads in ${child.session.branch.title || child.session.branch.name}`"
            :aria-expanded="!collapsed.has(child.session.id)"
            @click.stop="toggle(child.session.id)">{{ collapsed.has(child.session.id) ? '▸' : '▾' }}</button>
          <span v-else class="topic-thread-spacer" aria-hidden="true"></span>
          <span class="topic-thread-title">{{ child.session.branch.title || child.session.branch.name }}</span>
          <span v-if="child.childCount" class="badge dim">{{ child.childCount }}</span>
          <span v-else-if="badgeLabel(child.session)" class="badge" :class="statusClass(child.session)">{{ badgeLabel(child.session) }}</span>
        </div>
        </template>
          <div v-if="!section.topics.length" class="project-empty">No topics in this project yet.</div>
          </template>
        </template>
        <div v-if="topics.length === 0" class="topic-empty">
          No topics yet — use + to create one.
        </div>
      </div>
    </template>
  </aside>
</template>
