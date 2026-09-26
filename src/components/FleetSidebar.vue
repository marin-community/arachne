<script setup lang="ts">
import { ref, computed } from "vue";
import type { SessionSummary, SessionLayout } from "../App.vue";

const props = defineProps<{
  fleet: SessionSummary[];
  layout: SessionLayout | null;
  selectedId: string | null;
  launching?: boolean;
}>();

const emit = defineEmits<{
  (e: "select", id: string): void;
  (e: "launch", task: string, repo: string): void;
  (e: "create-workstream", name: string): void;
  (
    e: "reparent",
    sessionId: string,
    parentId: string | null,
    laneId?: string,
  ): void;
}>();

const task = ref("");
const repo = ref("marin-community/arachne");
const newStreamName = ref("");
const showNewStream = ref(false);
// Session being dragged into a workstream (by id), or the delete-workstream
// action (handled through reparent with a sentinel).
const dragging = ref<string | null>(null);

function submit() {
  const t = task.value.trim();
  if (!t) return;
  emit("launch", t, repo.value.trim());
  task.value = "";
}

function submitNewStream() {
  const n = newStreamName.value.trim();
  if (!n) return;
  emit("create-workstream", n);
  newStreamName.value = "";
  showNewStream.value = false;
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

// --- Workstream tree --------------------------------------------------------
//
// A workstream IS a top-level chat: sessions with no parent. Children nest
// under their parent (parent_session_id, or parent_id = parent branch id as
// a fallback). The layout group is only the lane a tree lives in — Inbox
// by default, or a named group the operator filed the tree under. So:
//   sidebar = [lane headers] → [top-level sessions] → [nested children]
// and "move a chat into a workstream" = reparent it under that top-level
// session (which also follows into the parent's lane, server-side).

interface TreeNode {
  session: SessionSummary;
  children: TreeNode[];
}

interface Lane {
  id: string;
  name: string;
  system: string | null;
  trees: TreeNode[];
  count: number;
}

function isChild(s: SessionSummary): boolean {
  return !!(s.parent_session_id || s.parent_id);
}

function buildLanes(): Lane[] {
  const live = props.fleet.filter((s) => s.status !== "archived");
  const byId = new Map(live.map((s) => [s.id, s]));
  const nodes = new Map<string, TreeNode>();
  for (const s of live) nodes.set(s.id, { session: s, children: [] });

  const roots: TreeNode[] = [];
  for (const s of live) {
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

  // File trees into lanes by their ROOT session's placement.
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
    // System lanes (per-space Inboxes) hide when empty; user-created
    // workstream lanes always render so an empty one is visible — otherwise
    // creating a workstream looks like a silent failure.
    if (trees.length === 0 && meta.system) continue;
    const count = trees.reduce((acc, t) => acc + 1 + countTree(t), 0);
    lanes.push({
      id: gid,
      name: meta.name,
      system: meta.system,
      trees,
      count,
    });
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

function onDragStart(id: string, e: DragEvent) {
  dragging.value = id;
  if (e.dataTransfer) {
    e.dataTransfer.setData("text/plain", id);
    e.dataTransfer.effectAllowed = "move";
  }
}

const dropTarget = ref<string | null>(null);
function onDragOver(key: string, e: DragEvent) {
  if (!dragging.value) return;
  e.preventDefault();
  e.dataTransfer && (e.dataTransfer.dropEffect = "move");
  dropTarget.value = key;
}
function onDragLeave(key: string) {
  if (dropTarget.value === key) dropTarget.value = null;
}
function onDropRoot(parentId: string | null, key: string, e: DragEvent) {
  e.preventDefault();
  dropTarget.value = null;
  if (dragging.value && dragging.value !== parentId) {
    emit("reparent", dragging.value, parentId);
    dragging.value = null;
  }
}

// Drop on a lane header: file the chat into that lane as a top-level
// session (detach from any tree, move its placement there).
function onDropLane(laneId: string, key: string, e: DragEvent) {
  e.preventDefault();
  dropTarget.value = null;
  if (dragging.value) {
    emit(
      "reparent",
      dragging.value,
      null,
      laneId === "unfiled" ? undefined : laneId,
    );
    dragging.value = null;
  }
}
</script>

<template>
  <aside class="sidebar">
    <div class="new-task">
      <input
        v-model="task"
        placeholder="New task…"
        @keydown.enter.prevent="submit"
      />
      <button
        class="primary"
        :disabled="!task.trim() || props.launching"
        @click="submit"
      >
        {{ props.launching ? "…" : "Launch" }}
      </button>
    </div>
    <div class="new-task" style="margin-top: -4px">
      <input
        v-model="repo"
        placeholder="owner/name"
        spellcheck="false"
        style="font-family: var(--mono); font-size: 11px"
      />
    </div>

    <div class="session-list">
      <template v-for="lane in lanes" :key="lane.id">
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
        </div>
        <template v-for="node in lane.trees" :key="node.session.id">
          <!-- A workstream: its root chat + nested children -->
          <div
            class="session-item workstream"
            :class="{
              selected: node.session.id === selectedId,
              'drop-hint': dropTarget === `ws-${node.session.id}`,
            }"
            draggable="true"
            @dragstart="onDragStart(node.session.id, $event)"
            @dragover="onDragOver(`ws-${node.session.id}`, $event)"
            @dragleave="onDragLeave(`ws-${node.session.id}`)"
            @drop.stop="
              onDropRoot(node.session.id, `ws-${node.session.id}`, $event)
            "
            @click="emit('select', node.session.id)"
            :title="dragging ? 'drop here to join this workstream' : undefined"
          >
            <div class="row1">
              <span class="name">{{
                node.session.branch.name || node.session.id
              }}</span>
              <span class="badge" :class="statusClass(node.session)">{{
                statusLabel(node.session)
              }}</span>
              <span v-if="node.children.length" class="badge dim"
                >{{ node.children.length
                }}<template v-if="node.children.some((c) => c.children.length)"
                  >+</template
                ></span
              >
              <span
                v-if="node.session.branch.tags.some((t) => t.key === 'idle')"
                class="badge idle"
                >idle</span
              >
            </div>
            <div class="title">{{ subtitle(node.session) }}</div>
          </div>
          <template v-for="child in node.children" :key="child.session.id">
            <div
              class="session-item child"
              :class="{ selected: child.session.id === selectedId }"
              draggable="true"
              @dragstart="onDragStart(child.session.id, $event)"
              @click="emit('select', child.session.id)"
            >
              <div class="row1">
                <span class="name">{{
                  child.session.branch.name || child.session.id
                }}</span>
                <span class="badge" :class="statusClass(child.session)">{{
                  statusLabel(child.session)
                }}</span>
              </div>
              <div class="title">{{ subtitle(child.session) }}</div>
            </div>
            <div
              v-for="gc in child.children"
              :key="gc.session.id"
              class="session-item child grandchild"
              :class="{ selected: gc.session.id === selectedId }"
              @click="emit('select', gc.session.id)"
            >
              <div class="row1">
                <span class="name">{{
                  gc.session.branch.name || gc.session.id
                }}</span>
                <span class="badge" :class="statusClass(gc.session)">{{
                  statusLabel(gc.session)
                }}</span>
              </div>
            </div>
          </template>
        </template>
      </template>
      <div
        v-if="lanes.length === 0"
        class="session-item"
        style="color: var(--text-dim)"
      >
        No sessions yet — launch one above.
      </div>
    </div>

    <div class="new-stream" v-if="showNewStream">
      <input
        v-model="newStreamName"
        placeholder="workstream name…"
        @keydown.enter.prevent="submitNewStream"
        @keydown.esc="showNewStream = false"
      />
    </div>
    <button v-else class="link new-stream-btn" @click="showNewStream = true">
      + workstream
    </button>
  </aside>
</template>
