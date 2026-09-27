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
  launching?: boolean;
}>();

const emit = defineEmits<{
  (e: "select", id: string): void;
  (
    e: "launch",
    task: string,
    repo: string,
    meta?: { title?: string; description?: string },
  ): void;
  (
    e: "update-topic",
    id: string,
    fields: { title?: string; description?: string },
    expected?: { title: string; provenance: string },
  ): void;
  (e: "new-thread"): void;
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

// New-topic form state. Title is the short card label; description is the
// longer what-this-is-for text (also the branch description shown in the
// inbox rows); the goal sent to the agent falls back to the title.
const newTitle = ref("");
const newDesc = ref("");
const repo = ref("marin-community/arachne");

function submitTopic() {
  const title = newTitle.value.trim();
  if (!title) return;
  const desc = newDesc.value.trim();
  emit("launch", desc || title, repo.value.trim(), {
    title,
    description: desc,
  });
  newTitle.value = "";
  newDesc.value = "";
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
    <!-- Inbox tab: the filing lanes + delegation tree. -->
    <template v-if="tab === 'inbox'">
      <button class="new-thread-btn" :class="{ active: props.showNewThread }" @click="emit('new-thread')">
        + New thread
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
          No topics yet — start one above.
        </div>
      </div>
    </template>

    <!-- Topics tab: one card per topic (leader chat) with title, description,
         and a config placeholder. -->
    <template v-else>
      <div class="new-topic-card">
        <input
          v-model="newTitle"
          placeholder="New topic title…"
          @keydown.enter.prevent="submitTopic"
        />
        <textarea
          v-model="newDesc"
          rows="2"
          placeholder="What is this topic for? (description)"
        ></textarea>
        <div class="new-topic-foot">
          <input
            v-model="repo"
            placeholder="owner/name"
            spellcheck="false"
            style="font-family: var(--mono); font-size: 11px"
          />
          <button
            class="primary"
            :disabled="!newTitle.trim() || props.launching"
            @click="submitTopic"
          >
            {{ props.launching ? "…" : "Create topic" }}
          </button>
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
