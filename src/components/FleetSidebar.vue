<script setup lang="ts">
import { ref, computed } from "vue";
import type { SessionSummary, SessionLayout } from "../App.vue";

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
  (e: "new-thread"): void;
  (e: "new-topic"): void;
  (
    e: "reparent",
    sessionId: string,
    parentId: string | null,
    laneId?: string,
  ): void;
  (e: "delete-lane", laneId: string): void;
  (e: "archive", id: string): void;
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
  if (s.status === "orphaned") return "orphaned";
  if (s.status === "running") {
    const loud = loudTag(s);
    if (loud?.level === "blocked") return "error";
    if (loud?.level === "attention") return "attention";
    return "running";
  }
  return "done";
}

function statusLabel(s: SessionSummary): string {
  if (s.status === "orphaned") return "orphan";
  const loud = loudTag(s);
  if (loud) return loud.level;
  if (s.status === "running") return "run";
  return s.status;
}

function isIdle(s: SessionSummary): boolean {
  return s.branch.tags.some((t) => t.key === "idle");
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
    <!-- New threads and topics are composed in the main panel (the thread
         home), not here: these buttons open the sheets. -->
    <div class="new-launchers">
      <button
        class="new-thread-btn"
        :class="{ active: props.showNewThread }"
        title="Start a conversation — goal + repo"
        @click="emit('new-thread')"
      >
        + New thread
      </button>
      <button
        class="new-thread-btn"
        :class="{ active: props.showNewTopic }"
        title="Start a durable topic — title, description, goal, repo"
        @click="emit('new-topic')"
      >
        + New topic
      </button>
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
            topic: row.depth === 0 && (isTopic(row.session) || row.childCount > 0),
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
            <span class="badge" :class="statusClass(row.session)">{{
              row.session.status === "archived" ? "done" : statusLabel(row.session)
            }}</span>
            <span
              v-if="row.childCount > 0"
              class="badge dim"
              :title="`${row.childCount} delegated children`"
            >
              {{ row.childCount }}
            </span>
            <span v-if="isIdle(row.session)" class="badge idle">idle</span>
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
  </aside>
</template>
